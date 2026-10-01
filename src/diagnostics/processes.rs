//! One sampler owns duplicate root handles, so exit remains observable after pane removal.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    sync::{OnceLock, Weak},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            GetExitCodeProcess, GetProcessId, GetProcessTimes, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

struct Watch {
    handle: OwnedHandle,
    pid: u32,
    creation: String,
    executable: Option<String>,
    observation: Weak<Mutex<Value>>,
}
struct Sampler {
    sender: flume::Sender<Watch>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
static SAMPLER: OnceLock<Sampler> = OnceLock::new();

pub fn watch_process(handle: BorrowedHandle<'_>) -> Result<RootProcess, String> {
    let handle = handle.try_clone_to_owned().map_err(|e| e.to_string())?;
    let raw = handle.as_raw_handle() as HANDLE;
    let pid = unsafe { GetProcessId(raw) };
    if pid == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let creation = creation_time(raw).map_err(|e| e.to_string())?;
    let executable = executable_path(raw);
    let identity = json!({"pid":pid,"creation_time":creation});
    let observation = Arc::new(Mutex::new(
        json!({"identity":identity,"executable":executable,"availability":"pending","lifecycle":"unknown","sampled_at":null,"descendants":[]}),
    ));
    let sampler=SAMPLER.get_or_init(|| {
        let (sender,receiver)=flume::bounded(256);let stop=Arc::new(AtomicBool::new(false));let worker_stop=Arc::clone(&stop);
        let thread=std::thread::Builder::new().name("mightty-processes".into()).spawn(move||{
            let mut watches:Vec<Watch>=Vec::new();let mut sampled=Instant::now()-Duration::from_secs(1);
            while !worker_stop.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_millis(100)) {Ok(watch)=>watches.push(watch),Err(flume::RecvTimeoutError::Disconnected)=>break,Err(_)=>{}}
                while let Ok(watch)=receiver.try_recv(){watches.push(watch);}
                watches.retain(|watch|watch.observation.strong_count()>0);
                if sampled.elapsed()<Duration::from_secs(1){continue;}
                sampled=Instant::now();let processes=enumerate();let time=timestamp();let mut child_budget=512usize;
                for watch in &watches {
                    let Some(observation)=watch.observation.upgrade()else{continue;};
                    let raw=watch.handle.as_raw_handle() as HANDLE;
                    let exited=unsafe {WaitForSingleObject(raw,0)}==WAIT_OBJECT_0;let mut exit_code=0;
                    let known_exit=exited&&unsafe {GetExitCodeProcess(raw,&mut exit_code)}!=0;
                    let mut descendants=Vec::new();let mut covered=true;
                    if let Ok(entries)=&processes && !exited {
                        let mut selected=BTreeMap::from([(watch.pid, watch.creation.parse::<u64>().unwrap_or(0))]);
                        let mut visited=BTreeSet::from([watch.pid]);
                        for _ in 0..entries.len().min(512){
                            let children=entries.iter().filter(|(pid,(parent,_))|!visited.contains(pid)&&selected.contains_key(parent)).map(|(pid,_)|*pid).collect::<Vec<_>>();
                            if children.is_empty(){break;}
                            for pid in children.into_iter().take(child_budget.min(128usize.saturating_sub(descendants.len()))) {
                                visited.insert(pid);
                                let (parent,executable)=&entries[&pid];
                                let raw=unsafe {OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid)};
                                let birth=if raw.is_null(){None}else{let handle=unsafe {OwnedHandle::from_raw_handle(raw.cast())};creation_time(handle.as_raw_handle() as HANDLE).ok()};
                                if birth.as_ref().is_some_and(|birth|birth.parse::<u64>().unwrap_or(0)<selected[parent]){continue;}
                                if let Some(birth)=&birth {selected.insert(pid,birth.parse::<u64>().unwrap_or(0));}else{covered=false;}
                                descendants.push(json!({"pid":pid,"parent_pid":parent,"executable":executable,"creation_time":birth,"identity_verified":birth.is_some()}));child_budget-=1;
                            }
                            if descendants.len()>=128||child_budget==0{covered=false;break;}
                        }
                    }else{covered=false;}
                    *observation.lock().unwrap_or_else(|p|p.into_inner())=json!({"identity":{"pid":watch.pid,"creation_time":watch.creation},"executable":watch.executable,"executable_availability":if watch.executable.is_some(){"observed"}else{"unavailable"},"availability":"observed","sampled_at":time,"lifecycle":if exited{"exited"}else{"running"},"exit_code":if known_exit{Some(exit_code)}else{None},"descendants":descendants,"coverage":{"local_windows_only":true,"complete":false,"truncated_or_unavailable":!covered,"enumeration_error":processes.as_ref().err().map(ToString::to_string)}});
                }
                mark_dirty();
            }
        }).expect("process sampler thread");
        Sampler{sender,stop,thread:Mutex::new(Some(thread))}
    });
    sampler
        .sender
        .try_send(Watch {
            handle,
            pid,
            creation,
            executable,
            observation: Arc::downgrade(&observation),
        })
        .map_err(|_| "process sampler queue full".to_string())?;
    Ok(RootProcess {
        identity,
        observation,
    })
}
pub fn stop_sampler() {
    if let Some(sampler) = SAMPLER.get() {
        sampler.stop.store(true, Ordering::Release);
        if let Some(thread) = sampler
            .thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }
}

fn executable_path(handle: HANDLE) -> Option<String> {
    let mut buffer = vec![0_u16; 4096];
    let mut length = buffer.len() as u32;
    (unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } != 0)
        .then(|| String::from_utf16_lossy(&buffer[..length as usize]))
}
fn creation_time(handle: HANDLE) -> std::io::Result<String> {
    let mut times = [FILETIME::default(); 4];
    if unsafe {
        GetProcessTimes(
            handle,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(
        ((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
            .to_string(),
    )
}
fn enumerate() -> std::io::Result<BTreeMap<u32, (u32, String)>> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut result = BTreeMap::new();
    let mut valid = unsafe { Process32FirstW(snapshot.as_raw_handle() as HANDLE, &mut entry) };
    if valid == 0 {
        return Err(std::io::Error::last_os_error());
    }
    while valid != 0 && result.len() < 8192 {
        let length = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        result.insert(
            entry.th32ProcessID,
            (
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..length]),
            ),
        );
        valid = unsafe { Process32NextW(snapshot.as_raw_handle() as HANDLE, &mut entry) };
    }
    Ok(result)
}
