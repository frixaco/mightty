//! One-instance ownership and authenticated local activation transport.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_CONNECTED,
    GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
};

use crate::application::{
    ACTIVATION_FRAME_HEADER_BYTES, ActivationRequest, decode_frame_header, decode_request_payload,
    encode_frame,
};

const INSTANCE_NAMESPACE_VERSION: &str = "v1";
const ACTIVATION_BUFFER_BYTES: u32 = 16 * 1024;
const ACTIVATION_CONNECT_TIMEOUT_MS: u32 = 2_000;

/// Result of claiming the one-instance mutex for the current user session.
pub enum InstanceClaim {
    Primary(PrimaryInstance),
    Secondary,
}

/// Keeps primary-process ownership and the activation server alive.
pub struct PrimaryInstance {
    _mutex: OwnedHandle,
    pipe_name: Arc<Vec<u16>>,
    stop: Arc<AtomicBool>,
    server_thread: Option<JoinHandle<()>>,
}

/// Claim the application instance for the current Windows user session.
pub fn claim_instance() -> io::Result<InstanceClaim> {
    let identity = ProcessIdentity::current()?;
    let mutex_name = wide_null(&format!(
        "Local\\mightty-{}-{INSTANCE_NAMESPACE_VERSION}",
        identity.sid
    ));
    let mutex = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return Err(io::Error::last_os_error());
    }
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle::new(mutex);
    if already_exists {
        return Ok(InstanceClaim::Secondary);
    }

    Ok(InstanceClaim::Primary(PrimaryInstance {
        _mutex: mutex,
        pipe_name: Arc::new(pipe_name(&identity)),
        stop: Arc::new(AtomicBool::new(false)),
        server_thread: None,
    }))
}

impl PrimaryInstance {
    /// Start accepting activation requests after the application owns the mutex.
    pub fn start(&mut self, sender: flume::Sender<ActivationRequest>) -> io::Result<()> {
        if self.server_thread.is_some() {
            return Ok(());
        }

        let identity = ProcessIdentity::current()?;
        let security = PipeSecurity::for_user(&identity.sid)?;
        let first_pipe = create_server_pipe(&self.pipe_name, &security.attributes)?;
        let pipe_name = Arc::clone(&self.pipe_name);
        let stop = Arc::clone(&self.stop);
        self.server_thread = Some(
            thread::Builder::new()
                .name("mightty-activation".to_string())
                .spawn(move || {
                    serve_activation_requests(
                        first_pipe,
                        &pipe_name,
                        &identity.sid,
                        &stop,
                        &sender,
                    );
                })?,
        );
        Ok(())
    }
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = connect_pipe(&self.pipe_name);
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Send one request to the primary process.
pub fn send_activation(request: &ActivationRequest) -> io::Result<()> {
    let identity = ProcessIdentity::current()?;
    let pipe_name = pipe_name(&identity);
    let pipe = connect_pipe(&pipe_name)?;
    let frame = encode_frame(request).map_err(io::Error::other)?;
    write_all(pipe.raw(), &frame)
}

fn serve_activation_requests(
    mut pipe: OwnedHandle,
    pipe_name: &[u16],
    user_sid: &str,
    stop: &AtomicBool,
    sender: &flume::Sender<ActivationRequest>,
) {
    loop {
        let connected = unsafe { ConnectNamedPipe(pipe.raw(), null_mut()) };
        if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            if stop.load(Ordering::Acquire) {
                break;
            }
        } else if !stop.load(Ordering::Acquire)
            && let Ok(request) = read_activation(pipe.raw())
        {
            let _ = sender.send(request);
        }

        unsafe {
            DisconnectNamedPipe(pipe.raw());
        }
        if stop.load(Ordering::Acquire) {
            break;
        }

        let security = match PipeSecurity::for_user(user_sid) {
            Ok(security) => security,
            Err(_) => break,
        };
        pipe = match create_server_pipe(pipe_name, &security.attributes) {
            Ok(pipe) => pipe,
            Err(_) => break,
        };
    }
}

fn read_activation(pipe: HANDLE) -> io::Result<ActivationRequest> {
    let mut header_bytes = [0_u8; ACTIVATION_FRAME_HEADER_BYTES];
    read_exact(pipe, &mut header_bytes)?;
    let header = decode_frame_header(&header_bytes).map_err(io::Error::other)?;
    let mut payload = vec![0_u8; header.payload_len()];
    read_exact(pipe, &mut payload)?;
    decode_request_payload(header, &payload).map_err(io::Error::other)
}

fn create_server_pipe(
    pipe_name: &[u16],
    security_attributes: &SECURITY_ATTRIBUTES,
) -> io::Result<OwnedHandle> {
    let pipe = unsafe {
        CreateNamedPipeW(
            pipe_name.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            ACTIVATION_BUFFER_BYTES,
            ACTIVATION_BUFFER_BYTES,
            0,
            security_attributes,
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(OwnedHandle::new(pipe))
    }
}

fn connect_pipe(pipe_name: &[u16]) -> io::Result<OwnedHandle> {
    let deadline = Instant::now() + Duration::from_millis(ACTIVATION_CONNECT_TIMEOUT_MS.into());
    loop {
        let pipe = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if pipe != INVALID_HANDLE_VALUE {
            return Ok(OwnedHandle::new(pipe));
        }

        let error = io::Error::last_os_error();
        if Instant::now() >= deadline {
            return Err(error);
        }
        unsafe {
            WaitNamedPipeW(pipe_name.as_ptr(), 50);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_exact(pipe: HANDLE, mut bytes: &mut [u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut read = 0;
        let result = unsafe {
            ReadFile(
                pipe,
                bytes.as_mut_ptr(),
                bytes.len().min(u32::MAX as usize) as u32,
                &mut read,
                null_mut(),
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "activation pipe closed before the request was complete",
            ));
        }
        bytes = &mut bytes[read as usize..];
    }
    Ok(())
}

fn write_all(pipe: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut written = 0;
        let result = unsafe {
            WriteFile(
                pipe,
                bytes.as_ptr(),
                bytes.len().min(u32::MAX as usize) as u32,
                &mut written,
                null_mut(),
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "activation pipe accepted no request bytes",
            ));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// Separate request/reply endpoint for every GUI process, including test/COM starts.
pub struct ControlServer {
    descriptor: crate::control::Descriptor,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    threads: Vec<JoinHandle<()>>,
}
struct ConnectionCount(Arc<AtomicUsize>);
impl Drop for ConnectionCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ControlServer {
    pub fn start(sender: flume::Sender<crate::control::Dispatch>) -> io::Result<Self> {
        let identity = ProcessIdentity::current()?;
        let creation = process_creation_time(unsafe { GetCurrentProcess() })?;
        let instance_id = format!("{}-{creation}", std::process::id());
        let descriptor = crate::control::Descriptor {
            protocol_version: crate::control::PROTOCOL_VERSION,
            instance_id: instance_id.clone(),
            pid: std::process::id(),
            process_creation_time: creation,
            endpoint: control_pipe_name(&identity, &instance_id),
            started_unix_ms: crate::feedback::unix_timestamp_ms().to_string(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut server = Self {
            descriptor,
            stop,
            active: Arc::new(AtomicUsize::new(0)),
            threads: Vec::new(),
        };
        for _ in 0..crate::control::MAX_CONNECTIONS {
            let pipe_name = wide_null(&server.descriptor.endpoint);
            let security = PipeSecurity::for_user(&identity.sid)?;
            let pipe = unsafe {
                CreateNamedPipeW(
                    pipe_name.as_ptr(),
                    windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE
                        | PIPE_READMODE_BYTE
                        | windows_sys::Win32::System::Pipes::PIPE_NOWAIT
                        | PIPE_REJECT_REMOTE_CLIENTS,
                    crate::control::MAX_CONNECTIONS as u32,
                    65536,
                    65536,
                    0,
                    &security.attributes,
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let pipe = OwnedHandle::new(pipe);
            let stop = Arc::clone(&server.stop);
            let active = Arc::clone(&server.active);
            let sender = sender.clone();
            let descriptor = server.descriptor.clone();
            server.threads.push(
                thread::Builder::new()
                    .name("mightty-control".into())
                    .spawn(move || {
                        while !stop.load(Ordering::Acquire) {
                            let connected = unsafe { ConnectNamedPipe(pipe.raw(), null_mut()) };
                            let error = unsafe { GetLastError() };
                            if connected == 0 && error != ERROR_PIPE_CONNECTED {
                                thread::sleep(Duration::from_millis(5));
                                continue;
                            }
                            let deadline = Instant::now() + Duration::from_secs(2);
                            active.fetch_add(1,Ordering::AcqRel);
                            let _connection = ConnectionCount(active.clone());
                            let result =
                                read_control_frame(pipe.raw(), deadline, &stop).and_then(|bytes| {
                                    let request: crate::control::Request = match serde_json::from_slice(&bytes) {
                                        Ok(request)=>request,
                                        Err(error)=>{
                                            let value=serde_json::json!({"protocol_version":1,"request_id":null,"instance_id":descriptor.instance_id,"revision":null,"ok":false,"error":{"code":"invalid_request","message":error.to_string(),"effect":"none","details":{}}});
                                            write_control_frame(pipe.raw(), &serde_json::to_vec(&value).map_err(io::Error::other)?, deadline, &stop)?;
                                            let mut receipt=[0];
                                            return control_io(pipe.raw(),&mut receipt,false,deadline,&stop);
                                        }
                                    };
                                    let deadline = Instant::now()
                                        + Duration::from_millis(request.timeout_ms.min(60000));
                                    let response = match crate::control::validate(
                                        &request,
                                        &descriptor.instance_id,
                                    ) {
                                        Err(error) => {
                                            crate::control::reply(&request, 0, Err(error))
                                        }
                                        Ok(()) if request.op == "handshake" => {
                                            crate::control::reply(
                                                &request,
                                                0,
                                                Ok(serde_json::to_value(&descriptor).unwrap()),
                                            )
                                        }
                                        Ok(()) => {
                                            if request.op == "events" {
                                                let (reply_tx, reply_rx) = flume::bounded(32);
                                                let pending = crate::control::Dispatch {request:request.clone(),reply:reply_tx,deadline};
                                                if sender.try_send(pending).is_ok() {
                                                    let mut last_revision = serde_json::Value::Null;
                                                    loop {
                                                        let mut value = match reply_rx.recv_timeout(Duration::from_millis(10)) {
                                                            Ok(value)=>value,
                                                            Err(flume::RecvTimeoutError::Disconnected)=>return Ok(()),
                                                            Err(_) if stop.load(Ordering::Acquire)=>return Ok(()),
                                                            Err(_) if Instant::now()>=deadline=>serde_json::json!({"type":"end","revision":last_revision}),
                                                            Err(_)=>continue,
                                                        };
                                                        if value["type"] == "resync_required" {value["last_delivered_revision"]=last_revision.clone();}
                                                        let mut bytes=serde_json::to_vec(&value).map_err(io::Error::other)?;
                                                        if bytes.len()>crate::control::MAX_FRAME_BYTES {
                                                            value=serde_json::json!({"protocol_version":crate::control::PROTOCOL_VERSION,"instance_id":descriptor.instance_id,"type":"resync_required","reason":"response_limit","last_delivered_revision":last_revision,"revision":value["revision"]});
                                                            bytes=serde_json::to_vec(&value).map_err(io::Error::other)?;
                                                        }
                                                        let terminal = value["type"] == "resync_required" || value["type"] == "end" || value["ok"] == false;
                                                        let io_deadline=Instant::now()+Duration::from_secs(2);
                                                        write_control_frame(pipe.raw(), &bytes, io_deadline,&stop)?;
                                                        control_io(pipe.raw(), &mut [0u8],false,io_deadline,&stop)?;
                                                        last_revision=value["revision"].clone();
                                                        if terminal {return Ok(());}
                                                    }
                                                }
                                            }
                                            let (reply_tx, reply_rx) = flume::bounded(1);
                                            let pending = crate::control::Dispatch {
                                                request: request.clone(),
                                                reply: reply_tx,
                                                deadline,
                                            };
                                            if sender.try_send(pending).is_err() {
                                                crate::control::reply(
                                                    &request,
                                                    0,
                                                    Err(crate::control::ControlError::new(
                                                        "busy",
                                                        "control queue is full",
                                                    )),
                                                )
                                            } else {
                                                loop {
                                                    match reply_rx
                                                        .recv_timeout(Duration::from_millis(10))
                                                    {
                                                        Ok(value) => break value,
                                                        Err(
                                                            flume::RecvTimeoutError::Disconnected,
                                                        ) => {
                                                            return Err(io::Error::other(
                                                                "application stopped",
                                                            ));
                                                        }
                                                        Err(_)
                                                            if Instant::now() >= deadline + Duration::from_millis(250)
                                                                || stop.load(Ordering::Acquire) =>
                                                        {
                                                            return Err(io::Error::new(
                                                                io::ErrorKind::TimedOut,
                                                                "control outcome unknown",
                                                            ));
                                                        }
                                                        Err(_) => {}
                                                    }
                                                }
                                            }
                                        }
                                    };
                                    let mut bytes=serde_json::to_vec(&response).map_err(io::Error::other)?;
                                    if bytes.len()>crate::control::MAX_FRAME_BYTES {
                                        let mut error=crate::control::ControlError::new("response_limit","response exceeds 2 MiB; request a smaller state subtree");
                                        if !matches!(request.op.as_str(),"handshake"|"capabilities"|"profiles"|"state"|"pane.read"|"wait"|"snapshot") {error.effect="committed";}
                                        error.details=Box::new(serde_json::json!({"limit":crate::control::MAX_FRAME_BYTES,"bytes":bytes.len(),"target":request.target}));
                                        let mut failure=crate::control::reply(&request,0,Err(error));failure["revision"]=response["revision"].clone();
                                        bytes=serde_json::to_vec(&failure).map_err(io::Error::other)?;
                                    }
                                    write_control_frame(
                                        pipe.raw(),
                                        &bytes,
                                        deadline + Duration::from_millis(250),
                                        &stop,
                                    )?;
                                    // DisconnectNamedPipe discards unread bytes. Wait for the client
                                    // to confirm receipt without a blocking FlushFileBuffers call.
                                    let mut received = [0u8; 1];
                                    control_io(pipe.raw(), &mut received, false, deadline + Duration::from_millis(250), &stop)
                                });
                            let _ = result;
                            unsafe {
                                DisconnectNamedPipe(pipe.raw());
                            }
                        }
                    })?,
            );
        }
        let directory = crate::control::directory();
        std::fs::create_dir_all(&directory)?;
        let temporary = directory.join(format!("{instance_id}.tmp"));
        std::fs::write(
            &temporary,
            serde_json::to_vec(&server.descriptor).map_err(io::Error::other)?,
        )?;
        std::fs::rename(temporary, directory.join(format!("{instance_id}.json")))?;
        Ok(server)
    }

    pub fn descriptor(&self) -> &crate::control::Descriptor {
        &self.descriptor
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Let an already committed final-window close reply reach its client.
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.active.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        self.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(
            crate::control::directory().join(format!("{}.json", self.descriptor.instance_id)),
        );
    }
}

fn control_pipe_name(identity: &ProcessIdentity, instance_id: &str) -> String {
    format!(
        r"\\.\pipe\mightty-control-{}-{}-v1-{instance_id}",
        identity.session_id, identity.sid
    )
}

fn process_creation_time(process: HANDLE) -> io::Result<String> {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};
    let mut times = [FILETIME::default(); 4];
    if unsafe {
        GetProcessTimes(
            process,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(
        ((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
            .to_string(),
    )
}

pub fn discover_control_instances(
    instance: Option<&str>,
) -> io::Result<Vec<crate::control::Descriptor>> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let identity = ProcessIdentity::current()?;
    let mut descriptors = Vec::new();
    let entries = match std::fs::read_dir(crate::control::directory()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(descriptors),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten().take(256) {
        if instance.is_some_and(|id| entry.file_name() != format!("{id}.json").as_str()) {
            continue;
        }
        if entry.path().extension().is_none_or(|v| v != "json") {
            continue;
        }
        if !entry
            .metadata()
            .is_ok_and(|metadata| metadata.len() <= 4096)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(descriptor) = serde_json::from_slice::<crate::control::Descriptor>(&bytes) else {
            continue;
        };
        if descriptor.protocol_version != crate::control::PROTOCOL_VERSION
            || instance.is_some_and(|id| descriptor.instance_id != id)
            || descriptor.endpoint != control_pipe_name(&identity, &descriptor.instance_id)
        {
            continue;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, descriptor.pid) };
        if handle.is_null() {
            continue;
        }
        let handle = OwnedHandle::new(handle);
        if process_creation_time(handle.raw()).ok().as_ref()
            != Some(&descriptor.process_creation_time)
        {
            continue;
        }
        let request = crate::control::request(&descriptor, "handshake");
        if let Ok(reply) = send_control(&descriptor, &request)
            && reply["ok"] == true
            && reply["result"]["instance_id"] == descriptor.instance_id
        {
            descriptors.push(descriptor);
        }
    }
    descriptors.sort_by(|a, b| a.started_unix_ms.cmp(&b.started_unix_ms));
    Ok(descriptors)
}

pub fn send_control(
    descriptor: &crate::control::Descriptor,
    request: &crate::control::Request,
) -> io::Result<serde_json::Value> {
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        System::Pipes::{PIPE_NOWAIT, SetNamedPipeHandleState},
    };
    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
    let pipe_name = wide_null(&descriptor.endpoint);
    let pipe = loop {
        let handle = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break OwnedHandle::new(handle);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::last_os_error());
        }
        thread::sleep(Duration::from_millis(5));
    };
    let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
    if unsafe { SetNamedPipeHandleState(pipe.raw(), &mode, null(), null()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let stop = AtomicBool::new(false);
    write_control_frame(
        pipe.raw(),
        &serde_json::to_vec(request).map_err(io::Error::other)?,
        deadline,
        &stop,
    )?;
    loop {
        let bytes = read_control_frame(pipe.raw(), deadline + Duration::from_secs(2), &stop)?;
        control_io(
            pipe.raw(),
            &mut [1u8],
            true,
            deadline + Duration::from_secs(2),
            &stop,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if request.op != "events" {
            return Ok(value);
        }
        use std::io::Write;
        println!("{value}");
        std::io::stdout().flush()?;
        if value["type"] == "end" || value["type"] == "resync_required" || value["ok"] == false {
            return Ok(value);
        }
    }
}

fn read_control_frame(pipe: HANDLE, deadline: Instant, stop: &AtomicBool) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    control_io(pipe, &mut header, false, deadline, stop)?;
    let len = u32::from_le_bytes(header) as usize;
    if len > crate::control::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame exceeds limit",
        ));
    }
    let mut bytes = vec![0u8; len];
    control_io(pipe, &mut bytes, false, deadline, stop)?;
    Ok(bytes)
}

fn write_control_frame(
    pipe: HANDLE,
    bytes: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    if bytes.len() > crate::control::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame exceeds limit",
        ));
    }
    let mut header = (bytes.len() as u32).to_le_bytes();
    control_io(pipe, &mut header, true, deadline, stop)?;
    let mut bytes = bytes.to_vec();
    control_io(pipe, &mut bytes, true, deadline, stop)
}

fn control_io(
    pipe: HANDLE,
    mut bytes: &mut [u8],
    write: bool,
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control I/O deadline",
            ));
        }
        let mut count = 0;
        let length = bytes.len().min(65536) as u32;
        let ok = unsafe {
            if write {
                WriteFile(pipe, bytes.as_ptr(), length, &mut count, null_mut())
            } else {
                ReadFile(pipe, bytes.as_mut_ptr(), length, &mut count, null_mut())
            }
        };
        if count > 0 {
            bytes = &mut bytes[count as usize..];
            continue;
        }
        let error = unsafe { GetLastError() };
        if ok == 0
            && error != windows_sys::Win32::Foundation::ERROR_NO_DATA
            && error != windows_sys::Win32::Foundation::ERROR_PIPE_LISTENING
        {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

struct ProcessIdentity {
    session_id: u32,
    sid: String,
}

impl ProcessIdentity {
    fn current() -> io::Result<Self> {
        let mut session_id = 0;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id) } == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            session_id,
            sid: current_user_sid()?,
        })
    }
}

fn current_user_sid() -> io::Result<String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle::new(token);

    let mut bytes_needed = 0;
    unsafe {
        GetTokenInformation(token.raw(), TokenUser, null_mut(), 0, &mut bytes_needed);
    }
    if bytes_needed == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(io::Error::last_os_error());
    }

    let word_count = (bytes_needed as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; word_count];
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes_needed,
            &mut bytes_needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };

    let mut sid_text = null_mut();
    if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sid_text = LocalAllocation(sid_text.cast());
    let length = unsafe {
        (0..)
            .find(|&index| *sid_text.0.cast::<u16>().add(index) == 0)
            .expect("Windows SID string is null terminated")
    };
    let wide = unsafe { std::slice::from_raw_parts(sid_text.0.cast::<u16>(), length) };
    Ok(String::from_utf16_lossy(wide))
}

fn pipe_name(identity: &ProcessIdentity) -> Vec<u16> {
    wide_null(&format!(
        r"\\.\pipe\mightty-{}-{}-{INSTANCE_NAMESPACE_VERSION}",
        identity.session_id, identity.sid
    ))
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

struct PipeSecurity {
    _descriptor: LocalAllocation,
    attributes: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    fn for_user(user_sid: &str) -> io::Result<Self> {
        let sddl = wide_null(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{user_sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            _descriptor: LocalAllocation(descriptor),
            attributes: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
        })
    }
}

struct LocalAllocation(*mut std::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

struct OwnedHandle(HANDLE);

// This wrapper has unique ownership. Only its owning thread uses the handle.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE) -> Self {
        debug_assert!(!handle.is_null() && handle != INVALID_HANDLE_VALUE);
        Self(handle)
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
            self.0 = INVALID_HANDLE_VALUE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of_val;

    #[test]
    fn instance_names_include_session_user_and_protocol_version() {
        let identity = ProcessIdentity {
            session_id: 12,
            sid: "S-1-5-21-100".to_string(),
        };
        let pipe = String::from_utf16_lossy(&pipe_name(&identity));

        assert_eq!(
            pipe.trim_end_matches('\0'),
            r"\\.\pipe\mightty-12-S-1-5-21-100-v1"
        );
    }

    #[test]
    fn security_attributes_have_the_windows_layout_size() {
        assert_eq!(
            size_of_val(&SECURITY_ATTRIBUTES::default()),
            size_of::<SECURITY_ATTRIBUTES>()
        );
    }
}
