//! ConPTY shell bridge.
//!
//! Manages pseudo-terminal connection between UI and shell processes on Windows.
//! Uses Windows ConPTY API (available on Windows 10 1809+).

use std::alloc::{Layout, alloc, dealloc};
use std::ffi::{OsStr, c_void};
use std::io;
use std::os::raw::c_uint;
use std::os::windows::ffi::OsStrExt;
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
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{PtyRead, PtySize};

const WAIT_FAILED: u32 = u32::MAX;
const SHUTDOWN_WAIT_MS: u32 = 250;

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

    fn PeekNamedPipe(
        hNamedPipe: HANDLE,
        lpBuffer: *mut c_void,
        nBufferSize: c_uint,
        lpBytesRead: *mut c_uint,
        lpTotalBytesAvail: *mut c_uint,
        lpBytesLeftThisMessage: *mut c_uint,
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

pub struct PtySession {
    pty_handle: HPCON,
    process_handle: HANDLE,
    input_pipe: HANDLE,
    output_pipe: HANDLE,
    shutdown_called: bool,
}

impl PtySession {
    /// Spawn a new shell process with the specified dimensions.
    ///
    /// ```no_run
    /// use mightty::shell::{PtySession, PtySize};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let _shell = PtySession::spawn("cmd.exe", PtySize::new(24, 80))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn spawn(command: &str, size: PtySize) -> Result<Self, PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        if !Self::is_conpty_available() {
            return Err(PtyError::ConPtyNotAvailable);
        }

        unsafe {
            let (pty_input_write, pty_input_read) = Self::create_pipe()?;
            let (pty_output_write, pty_output_read) = match Self::create_pipe() {
                Ok(pipe) => pipe,
                Err(err) => {
                    CloseHandle(pty_input_write);
                    CloseHandle(pty_input_read);
                    return Err(err);
                }
            };

            let coord = size_to_coord(size);
            let mut pty_handle: HPCON = 0;
            let result =
                CreatePseudoConsole(coord, pty_input_read, pty_output_write, 0, &mut pty_handle);

            if result != S_OK {
                CloseHandle(pty_input_write);
                CloseHandle(pty_input_read);
                CloseHandle(pty_output_write);
                CloseHandle(pty_output_read);
                return Err(PtyError::io("create pseudoconsole"));
            }

            let process_handle = match Self::create_process_with_pty(command, pty_handle) {
                Ok(handle) => handle,
                Err(err) => {
                    ClosePseudoConsole(pty_handle);
                    CloseHandle(pty_input_write);
                    CloseHandle(pty_input_read);
                    CloseHandle(pty_output_write);
                    CloseHandle(pty_output_read);
                    return Err(err);
                }
            };

            CloseHandle(pty_input_read);
            CloseHandle(pty_output_write);

            Ok(Self {
                pty_handle,
                process_handle,
                input_pipe: pty_input_write,
                output_pipe: pty_output_read,
                shutdown_called: false,
            })
        }
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        if buf.is_empty() {
            return Ok(PtyRead::WouldBlock);
        }

        let Some(bytes_available) = self.bytes_available()? else {
            return Ok(PtyRead::Eof);
        };

        if bytes_available == 0 {
            return if self.has_exited()? {
                Ok(PtyRead::Eof)
            } else {
                Ok(PtyRead::WouldBlock)
            };
        }

        let bytes_to_read = bytes_available.min(buf.len() as u32);
        unsafe {
            let mut bytes_read = 0u32;
            let result = ReadFile(
                self.output_pipe,
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

    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        let mut written_total = 0usize;

        while written_total < data.len() {
            let remaining = &data[written_total..];
            let bytes_to_write = remaining.len().min(u32::MAX as usize) as u32;

            unsafe {
                let mut bytes_written = 0u32;
                let result = WriteFile(
                    self.input_pipe,
                    remaining.as_ptr(),
                    bytes_to_write,
                    &mut bytes_written,
                    null_mut(),
                );

                if result == 0 {
                    return Err(PtyError::io("write to ConPTY input pipe"));
                }

                if bytes_written == 0 {
                    return Err(PtyError::ZeroLengthWrite);
                }

                written_total += bytes_written as usize;
            }
        }

        Ok(())
    }

    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        unsafe {
            let result = ResizePseudoConsole(self.pty_handle, size_to_coord(size));
            if result != S_OK {
                return Err(PtyError::io("resize pseudoconsole"));
            }

            Ok(())
        }
    }

    pub fn has_exited(&self) -> Result<bool, PtyError> {
        if self.process_handle == INVALID_HANDLE_VALUE {
            return Ok(true);
        }

        match unsafe { WaitForSingleObject(self.process_handle, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(PtyError::io("wait for process")),
            status => Err(PtyError::ProcessWaitFailed(status)),
        }
    }

    pub fn exit_code(&self) -> Result<Option<u32>, PtyError> {
        if !self.has_exited()? {
            return Ok(None);
        }

        let mut exit_code = 0u32;
        let result = unsafe { GetExitCodeProcess(self.process_handle, &mut exit_code) };
        if result == 0 {
            return Err(PtyError::io("get process exit code"));
        }

        Ok(Some(exit_code))
    }

    pub fn shutdown(mut self) -> Result<(), PtyError> {
        self.shutdown_called = true;
        self.close_handles(true)
    }

    pub fn is_conpty_available() -> bool {
        unsafe {
            let size = COORD { X: 2, Y: 2 };
            let mut test_handle: HPCON = 0;

            let (test_input_write, test_input_read) = match Self::create_pipe() {
                Ok(p) => p,
                Err(_) => return false,
            };
            let (test_output_write, test_output_read) = match Self::create_pipe() {
                Ok(p) => p,
                Err(_) => {
                    CloseHandle(test_input_write);
                    CloseHandle(test_input_read);
                    return false;
                }
            };

            let result = CreatePseudoConsole(
                size,
                test_input_read,
                test_output_write,
                1,
                &mut test_handle,
            );

            CloseHandle(test_input_read);
            CloseHandle(test_input_write);
            CloseHandle(test_output_read);
            CloseHandle(test_output_write);

            if result == S_OK && test_handle != 0 {
                ClosePseudoConsole(test_handle);
                return true;
            }

            false
        }
    }

    fn bytes_available(&self) -> Result<Option<u32>, PtyError> {
        unsafe {
            let mut bytes_available: u32 = 0;
            let result = PeekNamedPipe(
                self.output_pipe,
                null_mut(),
                0,
                null_mut(),
                &mut bytes_available,
                null_mut(),
            );

            if result == 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(None);
                }
                return Err(PtyError::from_io("peek ConPTY output pipe", error));
            }

            Ok(Some(bytes_available))
        }
    }

    fn create_pipe() -> Result<(HANDLE, HANDLE), PtyError> {
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

        Ok((write_handle, read_handle))
    }

    fn create_process_with_pty(command: &str, pty_handle: HPCON) -> Result<HANDLE, PtyError> {
        let mut cmd_wide: Vec<u16> = OsStr::new(command).encode_wide().chain(Some(0)).collect();
        let application = application_name(command);
        let application_wide = application.as_deref().map(|application| {
            OsStr::new(application)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<u16>>()
        });
        let application_ptr = application_wide
            .as_ref()
            .map_or(null_mut(), |application| application.as_ptr() as *mut _);

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

        let result =
            unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_list_size) };
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
        let result = unsafe {
            CreateProcessW(
                application_ptr,
                cmd_wide.as_mut_ptr(),
                null_mut(),
                null_mut(),
                0,
                EXTENDED_STARTUPINFO_PRESENT,
                null_mut(),
                null_mut(),
                (&mut startup_info as *mut STARTUPINFOEXW).cast::<STARTUPINFOW>(),
                &mut process_info,
            )
        };
        let process_error = (result == 0).then(|| unsafe { GetLastError() });
        cleanup_attr_list(true);

        if let Some(error_code) = process_error {
            return Err(PtyError::ProcessCreationFailed(error_code));
        }

        unsafe {
            CloseHandle(process_info.hThread);
        }

        Ok(process_info.hProcess)
    }

    fn close_handles(&mut self, allow_graceful_wait: bool) -> Result<(), PtyError> {
        let mut first_error = None;

        unsafe {
            if self.input_pipe != INVALID_HANDLE_VALUE {
                if CloseHandle(self.input_pipe) == 0 {
                    first_error.get_or_insert_with(|| PtyError::io("close ConPTY input pipe"));
                }
                self.input_pipe = INVALID_HANDLE_VALUE;
            }

            if self.process_handle != INVALID_HANDLE_VALUE {
                if allow_graceful_wait {
                    match WaitForSingleObject(self.process_handle, SHUTDOWN_WAIT_MS) {
                        WAIT_OBJECT_0 => {}
                        WAIT_TIMEOUT => {
                            if TerminateProcess(self.process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                        WAIT_FAILED => {
                            first_error.get_or_insert_with(|| PtyError::io("wait for process"));
                            if TerminateProcess(self.process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                        status => {
                            first_error.get_or_insert(PtyError::ProcessWaitFailed(status));
                            if TerminateProcess(self.process_handle, 0) == 0 {
                                first_error
                                    .get_or_insert_with(|| PtyError::io("terminate process"));
                            }
                        }
                    }
                } else if TerminateProcess(self.process_handle, 0) == 0 {
                    first_error.get_or_insert_with(|| PtyError::io("terminate process"));
                }

                if CloseHandle(self.process_handle) == 0 {
                    first_error.get_or_insert_with(|| PtyError::io("close process handle"));
                }
                self.process_handle = INVALID_HANDLE_VALUE;
            }

            if self.pty_handle != 0 {
                ClosePseudoConsole(self.pty_handle);
                self.pty_handle = 0;
            }

            if self.output_pipe != INVALID_HANDLE_VALUE {
                if CloseHandle(self.output_pipe) == 0 {
                    first_error.get_or_insert_with(|| PtyError::io("close ConPTY output pipe"));
                }
                self.output_pipe = INVALID_HANDLE_VALUE;
            }
        }

        if let Some(err) = first_error {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl Drop for PtySession {
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

fn application_name(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    let first = if let Some(rest) = trimmed.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        trimmed.split_whitespace().next()?
    };

    (first.contains('\\') || first.contains('/')).then(|| first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);

    fn spawn_test_cmd() -> PtySession {
        PtySession::spawn("C:\\Windows\\System32\\cmd.exe /d /q", PtySize::new(24, 80))
            .expect("spawn cmd.exe")
    }

    fn wait_for_output(shell: &mut PtySession, marker: &str) -> String {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut output = Vec::new();
        let mut buf = [0u8; 4096];

        while Instant::now() < deadline {
            match shell.try_read(&mut buf).expect("read from pty") {
                PtyRead::Data(n) => {
                    output.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&output);
                    if text.contains(marker) {
                        return text.into_owned();
                    }
                }
                PtyRead::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                PtyRead::Eof => break,
            }
        }

        panic!(
            "timed out waiting for marker {marker:?}; output was {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn spawn_cmd() {
        let shell = PtySession::spawn("cmd.exe", PtySize::new(24, 80));
        assert!(shell.is_ok(), "failed to spawn cmd.exe: {:?}", shell.err());
    }

    #[test]
    fn invalid_dimensions() {
        let result = PtySession::spawn("cmd.exe", PtySize::new(0, 80));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));

        let result = PtySession::spawn("cmd.exe", PtySize::new(24, 0));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));
    }

    #[test]
    fn reads_command_output() {
        let mut shell = spawn_test_cmd();
        shell
            .write(b"echo mightty-ready\r\n")
            .expect("write command");

        let output = wait_for_output(&mut shell, "mightty-ready");
        assert!(output.contains("mightty-ready"));

        shell.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reports_process_exit() {
        let mut shell = spawn_test_cmd();
        shell.write(b"exit\r\n").expect("write exit");

        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut buf = [0u8; 1024];
        while Instant::now() < deadline {
            if shell.has_exited().expect("check process exit") {
                return;
            }

            match shell.try_read(&mut buf).expect("read from pty") {
                PtyRead::Data(_) | PtyRead::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                PtyRead::Eof => return,
            }
        }

        panic!("timed out waiting for process exit");
    }

    #[test]
    fn resizes_session() {
        let mut shell = spawn_test_cmd();
        shell
            .resize(PtySize::new(40, 120))
            .expect("resize pseudoconsole");
        shell.shutdown().expect("shutdown shell");
    }

    #[test]
    fn writes_paste_sized_input() {
        let mut shell = spawn_test_cmd();
        let command = format!("rem {}\r\n", "x".repeat(8192));
        shell.write(command.as_bytes()).expect("write large input");
        shell.shutdown().expect("shutdown shell");
    }
}
