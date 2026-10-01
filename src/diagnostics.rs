//! Owned metadata, bounded journal and one background state writer.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

static DIRTY: AtomicBool = AtomicBool::new(true);
#[cfg(windows)]
mod processes;
#[cfg(windows)]
pub use processes::{stop_sampler, watch_process};

#[derive(Clone)]
pub struct RootProcess {
    pub identity: Value,
    pub(super) observation: Arc<Mutex<Value>>,
}
impl RootProcess {
    pub fn state(&self) -> Value {
        self.observation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
static LOGS: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());
struct Outcome {
    state: Value,
    root: Option<RootProcess>,
    retained: std::time::Instant,
}
static OUTCOMES: Mutex<VecDeque<Outcome>> = Mutex::new(VecDeque::new());
pub fn retain_outcome(mut state: Value, root: Option<RootProcess>) {
    state["removed_at"] = json!(timestamp());
    let mut outcomes = OUTCOMES.lock().unwrap_or_else(|p| p.into_inner());
    outcomes.push_back(Outcome {
        state,
        root,
        retained: std::time::Instant::now(),
    });
    prune_outcomes(&mut outcomes);
    mark_dirty();
}
fn prune_outcomes(outcomes: &mut VecDeque<Outcome>) {
    while outcomes
        .front()
        .is_some_and(|entry| entry.retained.elapsed().as_secs() >= 300)
        || outcomes.len() > 64
        || outcomes
            .iter()
            .map(|entry| entry.state.to_string().len())
            .sum::<usize>()
            > 1024 * 1024
    {
        outcomes.pop_front();
    }
}
pub fn outcomes() -> Vec<Value> {
    let mut outcomes = OUTCOMES.lock().unwrap_or_else(|p| p.into_inner());
    prune_outcomes(&mut outcomes);
    outcomes
        .iter()
        .map(|entry| {
            let mut state = entry.state.clone();
            if let Some(root) = &entry.root {
                state["processes"] = root.state();
            }
            state
        })
        .collect()
}
pub fn outcome(target: &crate::control::Target) -> Option<Value> {
    let id = target.pane_id.as_deref()?;
    outcomes().into_iter().find(|state| {
        state["pane_id"] == id
            && target
                .tab_id
                .as_ref()
                .is_none_or(|id| state["tab_id"] == *id)
            && target
                .window_id
                .as_ref()
                .is_none_or(|id| state["window_id"] == *id)
    })
}
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Release);
}
pub fn take_dirty() -> bool {
    DIRTY.swap(false, Ordering::AcqRel)
}

pub fn timestamp() -> String {
    #[cfg(windows)]
    {
        let mut t = windows_sys::Win32::Foundation::SYSTEMTIME::default();
        unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTime(&mut t) };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        crate::feedback::unix_timestamp_ms().to_string()
    }
}
pub fn record(subsystem: &str, code: &str, message: &str, context: Value) {
    let mut logs = LOGS.lock().unwrap_or_else(|p| p.into_inner());
    if logs.len() == 128 {
        logs.pop_front();
    }
    logs.push_back(json!({"time":timestamp(),"subsystem":subsystem,"severity":"error","code":code,"message":message.chars().take(1024).collect::<String>(),"context":context}));
    mark_dirty();
}
pub fn recent() -> Vec<Value> {
    LOGS.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect()
}
pub fn record_control(request: &crate::control::Request, response: &Value) {
    let mut logs = LOGS.lock().unwrap_or_else(|p| p.into_inner());
    if logs.len() == 128 {
        logs.pop_front();
    }
    logs.push_back(json!({"time":timestamp(),"subsystem":"control","severity":if response["ok"]==true{"info"}else{"error"},"code":"control_outcome","request_id":request.request_id,"instance_id":request.instance_id,"op":request.op,"target":request.target,"ok":response["ok"],"error_code":response["error"]["code"],"effect":response["error"]["effect"]}));
    mark_dirty();
}

pub fn root_directory() -> PathBuf {
    crate::settings::settings_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("diagnostics")
}

#[derive(Clone, Debug, Default)]
pub struct PersistenceStatus {
    pub revision: Option<String>,
    pub error: Option<String>,
}
pub struct Writer {
    directory: PathBuf,
    sender: flume::Sender<Value>,
    replace: flume::Receiver<Value>,
    status: Arc<Mutex<PersistenceStatus>>,
    thread: Option<JoinHandle<()>>,
}
impl Writer {
    pub fn start(instance_id: &str) -> Self {
        let directory = root_directory().join(instance_id);
        let (sender, receiver) = flume::bounded::<Value>(1);
        let replace = receiver.clone();
        let status = Arc::new(Mutex::new(PersistenceStatus::default()));
        let worker_status = Arc::clone(&status);
        let worker_directory = directory.clone();
        let thread=std::thread::Builder::new().name("mightty-diagnostics".into()).spawn(move||{
            prune_metadata(&worker_directory);
            let mut previous=Value::Null;let mut last_log:Option<Value>=None;
            while let Ok(state)=receiver.recv(){
                let result=(||->std::io::Result<()>{
                    std::fs::create_dir_all(&worker_directory)?;
                    atomic_write(&worker_directory.join("state.json"),&serde_json::to_vec_pretty(&state).map_err(std::io::Error::other)?)?;
                    let journal=worker_directory.join("events.ndjson");
                    if std::fs::metadata(&journal).is_ok_and(|m|m.len()>4*1024*1024) {
                        let old=worker_directory.join("events.previous.ndjson");
                        std::fs::rename(&journal,old)?;
                    }
                    let mut file=std::fs::OpenOptions::new().create(true).append(true).open(journal)?;
                    let mut metadata=state.clone();
                    // The journal does not continuously copy pane contents or output counters.
                    if let Some(windows)=metadata["windows"].as_array_mut(){for window in windows {if let Some(tabs)=window["tabs"].as_array_mut(){for tab in tabs {if let Some(panes)=tab["panes"].as_array_mut(){for pane in panes {if let Some(object)=pane.as_object_mut(){object.remove("output_seq");object.remove("output_cursor");object.remove("cursor");object.remove("viewport");}}}}}}}
                    if let Some(outcomes)=metadata["outcomes"].as_array_mut(){for outcome in outcomes {if let Some(object)=outcome.as_object_mut(){object.remove("final_tail");}}}
                    metadata.as_object_mut().unwrap().remove("observed_at");metadata.as_object_mut().unwrap().remove("revision");metadata.as_object_mut().unwrap().remove("persistence");metadata.as_object_mut().unwrap().remove("diagnostics");
                    if metadata!=previous {writeln!(file,"{}",json!({"schema_version":1,"time":timestamp(),"revision":state["revision"],"kind":"metadata_changed","state":metadata}))?;previous=metadata;}
                    let logs=recent();
                    let start=last_log.as_ref().and_then(|last|logs.iter().position(|log|log==last)).map_or(0,|i|i+1);
                    for log in &logs[start..]{writeln!(file,"{log}")?;}
                    last_log=logs.last().cloned();
                    Ok(())
                })();
                let mut status=worker_status.lock().unwrap_or_else(|p|p.into_inner());
                match result {Ok(())=>{status.revision=state["revision"].as_str().map(str::to_string);status.error=None;},Err(error)=>status.error=Some(error.to_string())}
            }
        }).expect("diagnostics writer thread");
        Self {
            directory,
            sender,
            replace,
            status,
            thread: Some(thread),
        }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn status(&self) -> Value {
        let status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        json!({"directory":self.directory,"persisted_revision":status.revision,"write_error":status.error})
    }
    pub fn submit(&self, mut state: Value) {
        loop {
            match self.sender.try_send(state) {
                Ok(()) => break,
                Err(flume::TrySendError::Full(value)) => {
                    state = value;
                    let _ = self.replace.try_recv();
                }
                Err(_) => break,
            }
        }
    }
    pub fn flush(&self, revision: &str, deadline: std::time::Instant) {
        while std::time::Instant::now() < deadline {
            let status = self.status.lock().unwrap_or_else(|p| p.into_inner());
            if status.revision.as_deref() == Some(revision) || status.error.is_some() {
                break;
            }
            drop(status);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        let (closed, _) = flume::bounded(0);
        self.sender = closed;
        if let Some(thread) = self.thread.take() {
            // Shutdown already attempted a bounded flush; slow storage must not hold the UI.
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, path)
}

fn prune_metadata(current: &Path) {
    let Some(root) = current.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut inactive = Vec::new();
    for entry in entries.flatten().take(1024) {
        let path = entry.path();
        if path == current || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((pid, birth)) = name
            .split_once('-')
            .and_then(|(pid, birth)| Some((pid.parse::<u32>().ok()?, birth.parse::<u64>().ok()?)))
        else {
            continue;
        };
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, FILETIME},
                System::Threading::{
                    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                },
            };
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if !handle.is_null() {
                let mut times = [FILETIME::default(); 4];
                let ok = unsafe {
                    GetProcessTimes(
                        handle,
                        &mut times[0],
                        &mut times[1],
                        &mut times[2],
                        &mut times[3],
                    )
                };
                unsafe { CloseHandle(handle) };
                if ok == 0
                    || ((u64::from(times[0].dwHighDateTime) << 32)
                        | u64::from(times[0].dwLowDateTime))
                        == birth
                {
                    continue;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (pid, birth);
            continue;
        }
        #[cfg(windows)]
        if let Ok(modified) = std::fs::metadata(path.join("state.json")).and_then(|m| m.modified())
        {
            inactive.push((modified, path));
        }
    }
    inactive.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in inactive.into_iter().skip(32) {
        // Delete only our metadata files. Captures and other files have their own lifetime.
        for name in [
            "state.json",
            "state.tmp",
            "events.ndjson",
            "events.previous.ndjson",
        ] {
            let _ = std::fs::remove_file(path.join(name));
        }
        let _ = std::fs::remove_dir(path);
    }
}

pub fn read_saved(instance: Option<&str>, directory: Option<&str>) -> Result<Value, String> {
    let root = directory.map_or_else(root_directory, |path| {
        PathBuf::from(path).join("diagnostics")
    });
    let mut latest: Option<Value> = None;
    for entry in std::fs::read_dir(&root)
        .map_err(|e| e.to_string())?
        .flatten()
        .take(256)
    {
        if instance.is_some_and(|id| entry.file_name() != id) {
            continue;
        }
        let path = entry.path().join("state.json");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 32 * 1024 * 1024) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && value["schema_version"] == 1
            && latest
                .as_ref()
                .is_none_or(|old| value["started_at"].as_str() > old["started_at"].as_str())
        {
            latest = Some(value);
        }
    }
    let mut state = latest.ok_or("no compatible saved instance state")?;
    state["source"] = json!("saved");
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_existing_state_atomically() {
        let directory = std::env::temp_dir().join(format!(
            "mightty-state-{}",
            crate::feedback::unix_timestamp_ms()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
