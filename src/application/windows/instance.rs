//! One-instance ownership and authenticated local activation transport.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
