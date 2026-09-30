//! ConPTY shell bridge.
//!
//! Manages pseudo-terminal connection between UI and shell processes on Windows.
//! Uses synchronous ConPTY pipes serviced by separate input/control and output
//! threads.

use std::alloc::{Layout, alloc, dealloc};
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::raw::c_uint;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle as WindowsOwnedHandle};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, S_OK, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{PtyRead, PtySize};
use crate::profile::LaunchSpec;

const WAIT_FAILED: u32 = u32::MAX;
const SHUTDOWN_WAIT_MS: u32 = 250;
const PTY_SIGNAL_RESIZE_WINDOW: u16 = 8;

unsafe extern "system" {
    fn ReadFile(
        hFile: HANDLE,
        lpBuffer: *mut u8,
        nNumberOfBytesToRead: c_uint,
        lpNumberOfBytesRead: *mut c_uint,
        lpOverlapped: *mut c_void,
    ) -> i32;

    fn WriteFile(
        hFile: HANDLE,
        lpBuffer: *const u8,
        nNumberOfBytesToWrite: c_uint,
        lpNumberOfBytesWritten: *mut c_uint,
        lpOverlapped: *mut c_void,
    ) -> i32;
}

#[derive(Debug)]
pub enum PtyError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    ConPtyNotAvailable,
    ProcessCreationFailed(u32),
    ProcessWaitFailed(u32),
    InvalidDimensions,
    InvalidProcessInput(&'static str),
    ZeroLengthWrite,
}

impl PtyError {
    fn io(operation: &'static str) -> Self {
        Self::Io {
            operation,
            source: io::Error::last_os_error(),
        }
    }

    fn from_io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation} failed: {source}"),
            Self::ConPtyNotAvailable => {
                write!(f, "ConPTY not available (requires Windows 10 1809+)")
            }
            Self::ProcessCreationFailed(code) => {
                write!(f, "create process failed with Windows error {code}")
            }
            Self::ProcessWaitFailed(code) => {
                write!(f, "wait for process failed with Windows status {code}")
            }
            Self::InvalidDimensions => write!(f, "invalid terminal dimensions"),
            Self::InvalidProcessInput(field) => write!(f, "invalid process {field}"),
            Self::ZeroLengthWrite => write!(f, "write made no progress"),
        }
    }
}

impl std::error::Error for PtyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for PtyError {
    fn from(source: io::Error) -> Self {
        Self::from_io("I/O operation", source)
    }
}

pub struct PtyParts {
    pub input: PtyInput,
    pub output: PtyOutput,
    pub control: PtyControl,
}

pub struct PtyInput {
    handle: OwnedHandle,
}

pub struct PtyOutput {
    handle: OwnedHandle,
}

pub struct PtyControl {
    backend: Option<PtyControlBackend>,
    process_handle: Option<OwnedHandle>,
    shutdown_called: bool,
}

enum PtyControlBackend {
    ConPty(HPCON),
    Handoff {
        signal: OwnedHandle,
        reference: OwnedHandle,
        server: OwnedHandle,
    },
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
}

struct OwnedHandle {
    handle: HANDLE,
}

// The split handle wrappers have unique ownership of their Windows handles and
// close them in Drop. Moving that ownership to a dedicated I/O thread is safe.
unsafe impl Send for PtyInput {}
unsafe impl Send for PtyOutput {}
unsafe impl Send for PtyControl {}

impl PtyParts {
    /// Spawn a new shell process with split input, output, and control handles.
    ///
    /// ```no_run
    /// use mightty::{profile::LaunchSpec, shell::{PtyParts, PtySize}};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let launch = LaunchSpec::new("cmd.exe");
    /// let _parts = PtyParts::spawn(&launch, PtySize::new(24, 80))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn spawn(launch: &LaunchSpec, size: PtySize) -> Result<Self, PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        if !is_conpty_available() {
            return Err(PtyError::ConPtyNotAvailable);
        }

        unsafe {
            let pty_input = Pipe::create()?;
            let pty_output = Pipe::create()?;

            let coord = size_to_coord(size);
            let mut pty_handle: HPCON = 0;
            let result = CreatePseudoConsole(
                coord,
                pty_input.read.raw(),
                pty_output.write.raw(),
                0,
                &mut pty_handle,
            );

            if result != S_OK {
                return Err(PtyError::io("create pseudoconsole"));
            }

            let process_handle = match create_process_with_pty(launch, pty_handle) {
                Ok(handle) => handle,
                Err(err) => {
                    ClosePseudoConsole(pty_handle);
                    return Err(err);
                }
            };

            Ok(Self {
                input: PtyInput {
                    handle: pty_input.write,
                },
                output: PtyOutput {
                    handle: pty_output.read,
                },
                control: PtyControl {
                    backend: Some(PtyControlBackend::ConPty(pty_handle)),
                    process_handle: Some(process_handle),
                    shutdown_called: false,
                },
            })
        }
    }

    /// Create terminal data pipes and adopt the handles received from a handoff.
    ///
    /// `signal`, `reference`, `server`, and `client` must be independent
    /// duplicates. This function takes their ownership. The returned input peer
    /// is the read end used by the console host. The output peer is its write end.
    pub(crate) fn from_handoff(
        signal: WindowsOwnedHandle,
        reference: WindowsOwnedHandle,
        server: WindowsOwnedHandle,
        client: WindowsOwnedHandle,
    ) -> Result<(Self, WindowsOwnedHandle, WindowsOwnedHandle), PtyError> {
        let input = Pipe::create()?;
        let output = Pipe::create()?;

        Ok((
            Self {
                input: PtyInput {
                    handle: input.write,
                },
                output: PtyOutput {
                    handle: output.read,
                },
                control: PtyControl {
                    backend: Some(PtyControlBackend::Handoff {
                        signal: OwnedHandle::from_windows(signal),
                        reference: OwnedHandle::from_windows(reference),
                        server: OwnedHandle::from_windows(server),
                    }),
                    process_handle: Some(OwnedHandle::from_windows(client)),
                    shutdown_called: false,
                },
            },
            input.read.into_windows(),
            output.write.into_windows(),
        ))
    }
}

pub fn is_conpty_available() -> bool {
    unsafe {
        let size = COORD { X: 2, Y: 2 };
        let mut test_handle: HPCON = 0;

        let test_input = match Pipe::create() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let test_output = match Pipe::create() {
            Ok(p) => p,
            Err(_) => return false,
        };

        let result = CreatePseudoConsole(
            size,
            test_input.read.raw(),
            test_output.write.raw(),
            1,
            &mut test_handle,
        );

        if result == S_OK && test_handle != 0 {
            ClosePseudoConsole(test_handle);
            return true;
        }

        false
    }
}

impl Pipe {
    fn create() -> Result<Self, PtyError> {
        let mut read_handle: HANDLE = INVALID_HANDLE_VALUE;
        let mut write_handle: HANDLE = INVALID_HANDLE_VALUE;

        let security_attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 0,
        };

        let result = unsafe { CreatePipe(&mut read_handle, &mut write_handle, &security_attrs, 0) };
        if result == 0 {
            return Err(PtyError::io("create anonymous pipe"));
        }

        Ok(Self {
            read: OwnedHandle::new(read_handle),
            write: OwnedHandle::new(write_handle),
        })
    }
}

impl OwnedHandle {
    fn new(handle: HANDLE) -> Self {
        Self { handle }
    }

    fn raw(&self) -> HANDLE {
        self.handle
    }

    fn from_windows(handle: WindowsOwnedHandle) -> Self {
        Self::new(handle.into_raw_handle())
    }

    fn into_windows(mut self) -> WindowsOwnedHandle {
        let handle = self.handle;
        self.handle = INVALID_HANDLE_VALUE;

        // The handle came from CreatePipe and remains uniquely owned.
        unsafe { WindowsOwnedHandle::from_raw_handle(handle) }
    }

    fn close(&mut self, operation: &'static str) -> Result<(), PtyError> {
        if self.handle == INVALID_HANDLE_VALUE || self.handle.is_null() {
            return Ok(());
        }

        let result = unsafe { CloseHandle(self.handle) };
        self.handle = INVALID_HANDLE_VALUE;

        if result == 0 {
            Err(PtyError::io(operation))
        } else {
            Ok(())
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = self.close("close handle");
    }
}

fn create_process_with_pty(
    launch: &LaunchSpec,
    pty_handle: HPCON,
) -> Result<OwnedHandle, PtyError> {
    let application_wide = windows_application_name(launch)?;
    let mut command_line = windows_command_line(launch)?;
    let environment = windows_environment_block(launch)?;
    let working_directory = launch
        .working_directory
        .as_ref()
        .map(|directory| nul_terminated_wide(directory.as_os_str(), "working directory"))
        .transpose()?;

    let mut startup_info: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_info.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup_info.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup_info.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

    let mut attr_list_size: usize = 0;
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_list_size);
    }

    let attr_list_layout = Layout::from_size_align(attr_list_size, 8).map_err(|_| {
        PtyError::from_io(
            "create attribute list layout",
            io::Error::other("invalid attribute list layout"),
        )
    })?;
    let attr_list: LPPROC_THREAD_ATTRIBUTE_LIST =
        unsafe { alloc(attr_list_layout) as LPPROC_THREAD_ATTRIBUTE_LIST };

    if attr_list.is_null() {
        return Err(PtyError::from_io(
            "allocate process attribute list",
            io::Error::other("allocation returned null"),
        ));
    }

    let cleanup_attr_list = |initialized: bool| unsafe {
        if initialized {
            DeleteProcThreadAttributeList(attr_list);
        }
        dealloc(attr_list as *mut u8, attr_list_layout);
    };

    let result = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_list_size) };
    if result == 0 {
        cleanup_attr_list(false);
        return Err(PtyError::io("initialize process attribute list"));
    }

    let result = unsafe {
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pty_handle as *const c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null_mut(),
        )
    };

    if result == 0 {
        cleanup_attr_list(true);
        return Err(PtyError::io("attach pseudoconsole attribute"));
    }

    startup_info.lpAttributeList = attr_list;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let creation_flags = EXTENDED_STARTUPINFO_PRESENT
        | if environment.is_some() {
            CREATE_UNICODE_ENVIRONMENT
        } else {
            0
        };
    let result = unsafe {
        CreateProcessW(
            application_wide
                .as_ref()
                .map_or(null_mut(), |application| application.as_ptr() as *mut u16),
            command_line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            0,
            creation_flags,
            environment.as_ref().map_or(null_mut(), |environment| {
                environment.as_ptr() as *mut c_void
            }),
            working_directory
                .as_ref()
                .map_or(null_mut(), |directory| directory.as_ptr() as *mut u16),
            (&mut startup_info as *mut STARTUPINFOEXW).cast::<STARTUPINFOW>(),
            &mut process_info,
        )
    };
    let process_error = (result == 0).then(|| unsafe { GetLastError() });
    cleanup_attr_list(true);

    if let Some(error_code) = process_error {
        return Err(PtyError::ProcessCreationFailed(error_code));
    }

    let process_handle = OwnedHandle::new(process_info.hProcess);
    let mut thread_handle = OwnedHandle::new(process_info.hThread);
    if let Err(err) = thread_handle.close("close process thread handle") {
        unsafe {
            TerminateProcess(process_handle.raw(), 0);
        }
        return Err(err);
    }

    Ok(process_handle)
}

impl PtyInput {
    pub fn write_all_interruptible(
        &mut self,
        data: &[u8],
        stopping: &std::sync::atomic::AtomicBool,
    ) -> Result<(), PtyError> {
        for chunk in data.chunks(32 * 1024) {
            if stopping.load(std::sync::atomic::Ordering::Acquire) {
                return Err(PtyError::from_io(
                    "write terminal input",
                    io::Error::new(io::ErrorKind::Interrupted, "session closing"),
                ));
            }
            self.write_all(chunk)?;
        }
        Ok(())
    }
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), PtyError> {
        write_all_to_handle(self.handle.raw(), data, "write to terminal input pipe")
    }
}

impl PtyOutput {
    pub fn read_interruptible(
        &mut self,
        buf: &mut [u8],
        stopping: &std::sync::atomic::AtomicBool,
    ) -> Result<PtyRead, PtyError> {
        if stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(PtyRead::Eof);
        }
        self.read(buf)
    }
    pub fn read(&mut self, buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        if buf.is_empty() {
            return Ok(PtyRead::Data(0));
        }

        let bytes_to_read = buf.len().min(u32::MAX as usize) as u32;
        unsafe {
            let mut bytes_read = 0u32;
            let result = ReadFile(
                self.handle.raw(),
                buf.as_mut_ptr(),
                bytes_to_read,
                &mut bytes_read,
                null_mut(),
            );

            if result == 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(PtyRead::Eof);
                }
                return Err(PtyError::from_io("read from ConPTY output pipe", error));
            }

            if bytes_read == 0 {
                Ok(PtyRead::Eof)
            } else {
                Ok(PtyRead::Data(bytes_read as usize))
            }
        }
    }
}

impl PtyControl {
    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        let Some(backend) = &self.backend else {
            return Err(PtyError::from_io(
                "resize pseudoconsole",
                io::Error::other("pseudoconsole handle is closed"),
            ));
        };

        match backend {
            PtyControlBackend::ConPty(pty_handle) => unsafe {
                let result = ResizePseudoConsole(*pty_handle, size_to_coord(size));
                if result != S_OK {
                    return Err(PtyError::io("resize pseudoconsole"));
                }
            },
            PtyControlBackend::Handoff { signal, .. } => {
                let mut packet = [0u8; 6];
                packet[0..2].copy_from_slice(&PTY_SIGNAL_RESIZE_WINDOW.to_le_bytes());
                packet[2..4].copy_from_slice(&size.cols.to_le_bytes());
                packet[4..6].copy_from_slice(&size.rows.to_le_bytes());
                write_all_to_handle(signal.raw(), &packet, "write terminal resize signal")?;
            }
        }

        Ok(())
    }

    pub fn has_exited(&self) -> Result<bool, PtyError> {
        let Some(process_handle) = &self.process_handle else {
            return Ok(true);
        };

        match unsafe { WaitForSingleObject(process_handle.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(PtyError::io("wait for process")),
            status => Err(PtyError::ProcessWaitFailed(status)),
        }
    }

    pub fn exit_code(&self) -> Result<Option<u32>, PtyError> {
        let Some(process_handle) = &self.process_handle else {
            return Ok(None);
        };

        if !self.has_exited()? {
            return Ok(None);
        }

        let mut exit_code = 0u32;
        let result = unsafe { GetExitCodeProcess(process_handle.raw(), &mut exit_code) };
        if result == 0 {
            return Err(PtyError::io("get process exit code"));
        }

        Ok(Some(exit_code))
    }

    pub fn shutdown(&mut self) -> Result<(), PtyError> {
        self.shutdown_called = true;
        self.close_handles(true)
    }

    fn close_handles(&mut self, allow_graceful_wait: bool) -> Result<(), PtyError> {
        let mut first_error = None;
        let terminate_process = matches!(self.backend, Some(PtyControlBackend::ConPty(_)));

        unsafe {
            match self.backend.take() {
                Some(PtyControlBackend::ConPty(pty_handle)) => ClosePseudoConsole(pty_handle),
                Some(PtyControlBackend::Handoff {
                    mut signal,
                    mut reference,
                    mut server,
                }) => {
                    for (handle, operation) in [
                        (&mut signal, "close handoff signal handle"),
                        (&mut reference, "close handoff reference handle"),
                        (&mut server, "close handoff server handle"),
                    ] {
                        if let Err(err) = handle.close(operation) {
                            first_error.get_or_insert(err);
                        }
                    }
                }
                None => {}
            }

            if let Some(mut process_handle) = self.process_handle.take() {
                let raw_process_handle = process_handle.raw();
                if !terminate_process {
                    // The console host owns a handed process. We only observe it.
                } else if allow_graceful_wait {
                    match WaitForSingleObject(raw_process_handle, SHUTDOWN_WAIT_MS) {
                        WAIT_OBJECT_0 => {}
                        WAIT_TIMEOUT => {
                            if TerminateProcess(raw_process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                        WAIT_FAILED => {
                            first_error.get_or_insert_with(|| PtyError::io("wait for process"));
                            if TerminateProcess(raw_process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                        status => {
                            first_error.get_or_insert(PtyError::ProcessWaitFailed(status));
                            if TerminateProcess(raw_process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                    }
                } else if TerminateProcess(raw_process_handle, 0) == 0 {
                    first_error.get_or_insert_with(|| PtyError::io("terminate process"));
                }

                if let Err(err) = process_handle.close("close process handle") {
                    first_error.get_or_insert(err);
                }
            }
        }

        if let Some(err) = first_error {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl Drop for PtyControl {
    fn drop(&mut self) {
        if self.shutdown_called {
            return;
        }

        let _ = self.close_handles(false);
    }
}

fn size_to_coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols as i16,
        Y: size.rows as i16,
    }
}

fn write_all_to_handle(
    handle: HANDLE,
    data: &[u8],
    operation: &'static str,
) -> Result<(), PtyError> {
    let mut written_total = 0usize;

    while written_total < data.len() {
        let remaining = &data[written_total..];
        let bytes_to_write = remaining.len().min(u32::MAX as usize) as u32;

        unsafe {
            let mut bytes_written = 0u32;
            let result = WriteFile(
                handle,
                remaining.as_ptr(),
                bytes_to_write,
                &mut bytes_written,
                null_mut(),
            );

            if result == 0 {
                return Err(PtyError::io(operation));
            }

            if bytes_written == 0 {
                return Err(PtyError::ZeroLengthWrite);
            }

            written_total += bytes_written as usize;
        }
    }

    Ok(())
}

fn windows_application_name(launch: &LaunchSpec) -> Result<Option<Vec<u16>>, PtyError> {
    let executable: Vec<u16> = launch.executable().encode_wide().collect();
    if executable.is_empty() || executable.contains(&0) {
        return Err(PtyError::InvalidProcessInput("executable"));
    }

    // A null application name makes CreateProcessW search for a bare file name.
    // A path remains explicit and avoids parsing it from the command line.
    executable
        .iter()
        .any(|value| matches!(*value, 0x2f | 0x5c))
        .then(|| nul_terminated_wide(launch.executable(), "executable"))
        .transpose()
}

fn windows_command_line(launch: &LaunchSpec) -> Result<Vec<u16>, PtyError> {
    if launch.executable().is_empty() {
        return Err(PtyError::InvalidProcessInput("executable"));
    }

    let mut command_line = Vec::new();
    push_windows_argument(&mut command_line, launch.executable())?;
    for argument in &launch.arguments {
        command_line.push(u16::from(b' '));
        push_windows_argument(&mut command_line, argument)?;
    }
    command_line.push(0);
    Ok(command_line)
}

// Windows programs usually parse CreateProcessW's command line with the CRT
// rules. Backslashes before quotes need doubling so each argument round-trips.
fn push_windows_argument(output: &mut Vec<u16>, argument: &OsStr) -> Result<(), PtyError> {
    let argument: Vec<u16> = argument.encode_wide().collect();
    if argument.contains(&0) {
        return Err(PtyError::InvalidProcessInput("argument"));
    }
    let quoted = argument.is_empty()
        || argument
            .iter()
            .any(|value| matches!(*value, 0x09 | 0x20 | 0x22));
    if !quoted {
        output.extend(argument);
        return Ok(());
    }

    output.push(u16::from(b'"'));
    let mut backslashes = 0;
    for value in argument {
        if value == u16::from(b'\\') {
            backslashes += 1;
            continue;
        }
        if value == u16::from(b'"') {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            output.push(value);
        } else {
            output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            output.push(value);
        }
        backslashes = 0;
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    output.push(u16::from(b'"'));
    Ok(())
}

fn windows_environment_block(launch: &LaunchSpec) -> Result<Option<Vec<u16>>, PtyError> {
    if launch.environment.is_empty() {
        return Ok(None);
    }

    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in &launch.environment {
        validate_environment_key(key)?;
        environment.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        environment.push((key.clone(), value.clone()));
    }
    environment.sort_by(|(left, _), (right, _)| {
        left.to_string_lossy()
            .to_ascii_lowercase()
            .cmp(&right.to_string_lossy().to_ascii_lowercase())
    });

    let mut block = Vec::new();
    for (key, value) in environment {
        append_environment_value(&mut block, &key)?;
        block.push(u16::from(b'='));
        append_environment_value(&mut block, &value)?;
        block.push(0);
    }
    block.push(0);
    Ok(Some(block))
}

fn validate_environment_key(key: &OsStr) -> Result<(), PtyError> {
    let wide: Vec<u16> = key.encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) || wide.contains(&u16::from(b'=')) {
        return Err(PtyError::InvalidProcessInput("environment key"));
    }
    Ok(())
}

fn append_environment_value(output: &mut Vec<u16>, value: &OsStr) -> Result<(), PtyError> {
    let wide: Vec<u16> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(PtyError::InvalidProcessInput("environment value"));
    }
    output.extend(wide);
    Ok(())
}

fn nul_terminated_wide(value: &OsStr, field: &'static str) -> Result<Vec<u16>, PtyError> {
    let mut wide: Vec<u16> = value.encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(PtyError::InvalidProcessInput(field));
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, GetCurrentProcess};

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);

    fn spawn_test_cmd() -> PtyParts {
        let launch = LaunchSpec::new("C:\\Windows\\System32\\cmd.exe").with_arguments(["/d", "/q"]);
        PtyParts::spawn(&launch, PtySize::new(24, 80)).expect("spawn cmd.exe")
    }

    fn read_until(mut output: PtyOutput, marker: &'static str) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut text = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match output.read(&mut buf) {
                    Ok(PtyRead::Data(0)) => {}
                    Ok(PtyRead::Data(n)) => {
                        text.extend_from_slice(&buf[..n]);
                        let decoded = String::from_utf8_lossy(&text);
                        if decoded.contains(marker) {
                            let _ = tx.send(decoded.into_owned());
                            return;
                        }
                    }
                    Ok(PtyRead::Eof) | Err(_) => {
                        let _ = tx.send(String::from_utf8_lossy(&text).into_owned());
                        return;
                    }
                }
            }
        });

        rx
    }

    fn assert_marker(rx: mpsc::Receiver<String>, marker: &str) -> String {
        let output = rx
            .recv_timeout(TEST_TIMEOUT)
            .unwrap_or_else(|_| panic!("timed out waiting for marker {marker:?}"));
        assert!(
            output.contains(marker),
            "expected marker {marker:?}; output was {output:?}"
        );
        output
    }

    fn duplicate_handle(handle: RawHandle) -> WindowsOwnedHandle {
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = INVALID_HANDLE_VALUE;
        let result = unsafe {
            DuplicateHandle(
                process,
                handle,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        assert_ne!(
            result,
            0,
            "duplicate handle: {}",
            io::Error::last_os_error()
        );

        unsafe { WindowsOwnedHandle::from_raw_handle(duplicate) }
    }

    fn disposable_handle() -> WindowsOwnedHandle {
        let pipe = Pipe::create().expect("create disposable handle");
        pipe.read.into_windows()
    }

    #[test]
    fn spawn_cmd() {
        let launch = LaunchSpec::new("cmd.exe");
        let shell = PtyParts::spawn(&launch, PtySize::new(24, 80));
        assert!(shell.is_ok(), "failed to spawn cmd.exe: {:?}", shell.err());
    }

    #[test]
    fn invalid_dimensions() {
        let launch = LaunchSpec::new("cmd.exe");
        let result = PtyParts::spawn(&launch, PtySize::new(0, 80));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));

        let result = PtyParts::spawn(&launch, PtySize::new(24, 0));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));
    }

    #[test]
    fn quotes_windows_arguments_without_shell_parsing() {
        let launch = LaunchSpec::new("C:\\Program Files\\pwsh.exe").with_arguments([
            "-NoLogo",
            "hello world",
            "C:\\path with space\\",
        ]);
        let command_line = windows_command_line(&launch).unwrap();
        let command_line = String::from_utf16(&command_line[..command_line.len() - 1]).unwrap();

        assert_eq!(
            command_line,
            r#""C:\Program Files\pwsh.exe" -NoLogo "hello world" "C:\path with space\\""#,
        );
    }

    #[test]
    fn rejects_invalid_environment_keys() {
        let launch = LaunchSpec::new("cmd.exe").with_environment([("BAD=KEY", "value")]);
        assert!(matches!(
            windows_environment_block(&launch),
            Err(PtyError::InvalidProcessInput("environment key"))
        ));
    }

    #[test]
    fn passes_profile_environment_to_the_child() {
        let launch = LaunchSpec::new("C:\\Windows\\System32\\cmd.exe")
            .with_arguments(["/d", "/q"])
            .with_environment([("MIGHTTY_PROFILE_TEST", "works")]);
        let PtyParts {
            mut input,
            output,
            mut control,
        } = PtyParts::spawn(&launch, PtySize::new(24, 80)).expect("spawn cmd.exe");
        let output_rx = read_until(output, "mightty-env-works");

        input
            .write_all(b"echo mightty-env-%MIGHTTY_PROFILE_TEST%\r\n")
            .expect("write environment command");
        assert_marker(output_rx, "mightty-env-works");
        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reads_command_output() {
        let PtyParts {
            mut input,
            output,
            mut control,
        } = spawn_test_cmd();
        let output_rx = read_until(output, "mightty-ready");

        input
            .write_all(b"echo mightty-ready\r\n")
            .expect("write command");
        assert_marker(output_rx, "mightty-ready");

        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reads_input_written_after_idle_without_polling() {
        let PtyParts {
            mut input,
            output,
            mut control,
        } = spawn_test_cmd();
        let output_rx = read_until(output, "mightty-idle-ready");

        thread::sleep(Duration::from_millis(80));
        input
            .write_all(b"echo mightty-idle-ready\r\n")
            .expect("write command after idle");
        assert_marker(output_rx, "mightty-idle-ready");

        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reads_large_output_in_order() {
        let PtyParts {
            mut input,
            output,
            mut control,
        } = spawn_test_cmd();
        let output_rx = read_until(output, "mightty-high-done");

        input
            .write_all(
                b"for /l %i in (1,1,5000) do @echo mightty-line-%i\r\necho mightty-high-done\r\n",
            )
            .expect("write high-output command");
        let output = assert_marker(output_rx, "mightty-high-done");
        let first = output.find("mightty-line-1").expect("line 1");
        let last = output.find("mightty-line-5000").expect("line 5000");
        let done = output.find("mightty-high-done").expect("done marker");
        assert!(first < last && last < done);

        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reports_process_exit() {
        let PtyParts {
            mut input,
            output: _output,
            mut control,
        } = spawn_test_cmd();
        input.write_all(b"exit\r\n").expect("write exit");

        let deadline = Instant::now() + TEST_TIMEOUT;
        while Instant::now() < deadline {
            if control.has_exited().expect("check process exit") {
                control.shutdown().expect("shutdown shell");
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }

        panic!("timed out waiting for process exit");
    }

    #[test]
    fn resizes_session() {
        let PtyParts {
            input: _input,
            output: _output,
            mut control,
        } = spawn_test_cmd();
        control
            .resize(PtySize::new(40, 120))
            .expect("resize pseudoconsole");
        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn writes_paste_sized_input() {
        let PtyParts {
            mut input,
            output: _output,
            mut control,
        } = spawn_test_cmd();
        let command = format!("rem {}\r\n", "x".repeat(8192));
        input
            .write_all(command.as_bytes())
            .expect("write large input");
        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn handed_session_uses_expected_pipe_directions_and_resize_packet() {
        let signal_pipe = Pipe::create().expect("create signal pipe");
        let signal = signal_pipe.write.into_windows();
        let mut signal_reader = PtyOutput {
            handle: signal_pipe.read,
        };
        let client = duplicate_handle(unsafe { GetCurrentProcess() });

        let (mut parts, input_peer, output_peer) =
            PtyParts::from_handoff(signal, disposable_handle(), disposable_handle(), client)
                .expect("create handed session");

        parts
            .input
            .write_all(b"terminal-input")
            .expect("write terminal input");
        let mut input_buf = [0u8; 14];
        let mut input_peer = PtyOutput {
            handle: OwnedHandle::from_windows(input_peer),
        };
        assert_eq!(
            input_peer.read(&mut input_buf).expect("read host input"),
            PtyRead::Data(input_buf.len()),
        );
        assert_eq!(&input_buf, b"terminal-input");

        write_all_to_handle(
            output_peer.as_raw_handle(),
            b"terminal-output",
            "write host output",
        )
        .expect("write host output");
        let mut output_buf = [0u8; 15];
        assert_eq!(
            parts
                .output
                .read(&mut output_buf)
                .expect("read terminal output"),
            PtyRead::Data(output_buf.len()),
        );
        assert_eq!(&output_buf, b"terminal-output");

        parts
            .control
            .resize(PtySize::new(40, 120))
            .expect("write resize packet");
        let mut resize_packet = [0u8; 6];
        assert_eq!(
            signal_reader
                .read(&mut resize_packet)
                .expect("read resize packet"),
            PtyRead::Data(resize_packet.len()),
        );
        assert_eq!(resize_packet, [8, 0, 120, 0, 40, 0]);

        parts.control.shutdown().expect("shutdown handed session");
    }

    #[test]
    fn handed_session_does_not_terminate_client_process() {
        let mut child = Command::new("C:\\Windows\\System32\\cmd.exe");
        child
            .args(["/d", "/q", "/c", "ping -n 30 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = child.spawn().expect("spawn handed client");
        let client = duplicate_handle(child.as_raw_handle());

        let signal_pipe = Pipe::create().expect("create signal pipe");
        let signal = signal_pipe.write.into_windows();
        let (mut parts, _input_peer, _output_peer) =
            PtyParts::from_handoff(signal, disposable_handle(), disposable_handle(), client)
                .expect("create handed session");

        parts.control.shutdown().expect("shutdown handed session");
        let still_running = child.try_wait().expect("query handed client").is_none();
        let _ = child.kill();
        let _ = child.wait();

        assert!(still_running, "shutdown terminated the handed client");
    }
}
