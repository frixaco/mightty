//! ConPTY Shell Bridge
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
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, S_OK,
};

// External declarations for Windows file I/O functions
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

use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
    TerminateProcess, UpdateProcThreadAttribute,
};

/// Error type for ConPTY operations
#[derive(Debug)]
pub enum ConPtyError {
    Io(io::Error),
    ConPtyNotAvailable,
    ProcessCreationFailed(u32),
    InvalidDimensions,
}

impl std::fmt::Display for ConPtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConPtyError::Io(e) => write!(f, "IO error: {}", e),
            ConPtyError::ConPtyNotAvailable => {
                write!(f, "ConPTY not available (requires Windows 10 1809+)")
            }
            ConPtyError::ProcessCreationFailed(code) => {
                write!(f, "Process creation failed with code {}", code)
            }
            ConPtyError::InvalidDimensions => write!(f, "Invalid terminal dimensions"),
        }
    }
}

impl std::error::Error for ConPtyError {}

impl From<io::Error> for ConPtyError {
    fn from(e: io::Error) -> Self {
        ConPtyError::Io(e)
    }
}

/// Safe wrapper around Windows ConPTY shell session
pub struct ConPtyShell {
    pty_handle: HPCON,
    process_handle: HANDLE,
    input_pipe: HANDLE,  // Write end (to shell stdin)
    output_pipe: HANDLE, // Read end (from shell stdout)
    shutdown_called: bool,
}

impl ConPtyShell {
    /// Spawn a new shell process with the specified dimensions
    ///
    /// # Arguments
    /// * `command` - Command to execute (e.g., "cmd.exe", "powershell.exe")
    /// * `rows` - Terminal height in rows (must be > 0)
    /// * `cols` - Terminal width in columns (must be > 0)
    ///
    /// # Example
    /// ```no_run
    /// use mightty::shell::ConPtyShell;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let _shell = ConPtyShell::spawn("cmd.exe", 24, 80)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn spawn(command: &str, rows: u16, cols: u16) -> Result<Self, ConPtyError> {
        if rows == 0 || cols == 0 {
            return Err(ConPtyError::InvalidDimensions);
        }

        // Check ConPTY availability
        if !Self::is_conpty_available() {
            return Err(ConPtyError::ConPtyNotAvailable);
        }

        unsafe {
            // Create pipes for PTY communication
            let (pty_input_write, pty_input_read) = Self::create_pipe()?;
            let (pty_output_write, pty_output_read) = match Self::create_pipe() {
                Ok(pipe) => pipe,
                Err(err) => {
                    CloseHandle(pty_input_write);
                    CloseHandle(pty_input_read);
                    return Err(err);
                }
            };

            // Create pseudo console
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };

            let mut pty_handle: HPCON = 0;
            let result =
                CreatePseudoConsole(size, pty_input_read, pty_output_write, 0, &mut pty_handle);

            if result != S_OK {
                CloseHandle(pty_input_write);
                CloseHandle(pty_input_read);
                CloseHandle(pty_output_write);
                CloseHandle(pty_output_read);
                return Err(ConPtyError::Io(io::Error::last_os_error()));
            }

            // Close handles we no longer need (ConPTY owns the read/write ends now)
            CloseHandle(pty_input_read);
            CloseHandle(pty_output_write);

            // Create process attached to PTY
            let process_handle = match Self::create_process_with_pty(command, pty_handle) {
                Ok(handle) => handle,
                Err(err) => {
                    ClosePseudoConsole(pty_handle);
                    CloseHandle(pty_input_write);
                    CloseHandle(pty_output_read);
                    return Err(err);
                }
            };

            Ok(ConPtyShell {
                pty_handle,
                process_handle,
                input_pipe: pty_input_write,
                output_pipe: pty_output_read,
                shutdown_called: false,
            })
        }
    }

    /// Read output from the shell process
    ///
    /// This is a blocking read. Returns number of bytes read (0 if process exited).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, ConPtyError> {
        unsafe {
            let mut bytes_read = 0u32;
            let result = ReadFile(
                self.output_pipe,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut bytes_read,
                null_mut(),
            );

            if result == 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(0); // Process exited
                }
                return Err(ConPtyError::Io(error));
            }

            Ok(bytes_read as usize)
        }
    }

    /// Write input to the shell process
    pub fn write(&mut self, data: &[u8]) -> Result<(), ConPtyError> {
        unsafe {
            let mut bytes_written = 0u32;
            let result = WriteFile(
                self.input_pipe,
                data.as_ptr(),
                data.len() as u32,
                &mut bytes_written,
                null_mut(),
            );

            if result == 0 {
                return Err(ConPtyError::Io(io::Error::last_os_error()));
            }

            Ok(())
        }
    }

    /// Check if there's data available to read (non-blocking)
    pub fn peek(&mut self) -> Result<bool, ConPtyError> {
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
                    return Ok(false);
                }
                return Err(ConPtyError::Io(error));
            }

            Ok(bytes_available > 0)
        }
    }

    /// Resize the terminal dimensions
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ConPtyError> {
        if rows == 0 || cols == 0 {
            return Err(ConPtyError::InvalidDimensions);
        }

        unsafe {
            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };

            let result = ResizePseudoConsole(self.pty_handle, size);
            if result != S_OK {
                return Err(ConPtyError::Io(io::Error::last_os_error()));
            }

            Ok(())
        }
    }

    /// Shutdown the shell and cleanup resources
    pub fn shutdown(mut self) -> Result<(), ConPtyError> {
        self.shutdown_called = true;
        unsafe {
            // Terminate the process
            if self.process_handle != INVALID_HANDLE_VALUE {
                TerminateProcess(self.process_handle, 0);
                CloseHandle(self.process_handle);
            }

            // Close ConPTY
            if self.pty_handle != 0 {
                ClosePseudoConsole(self.pty_handle);
            }

            // Close pipes
            CloseHandle(self.input_pipe);
            CloseHandle(self.output_pipe);

            // Prevent drop from running again
            self.process_handle = INVALID_HANDLE_VALUE;
            self.pty_handle = 0;
            self.input_pipe = INVALID_HANDLE_VALUE;
            self.output_pipe = INVALID_HANDLE_VALUE;

            Ok(())
        }
    }

    /// Check if ConPTY is available on this system
    pub fn is_conpty_available() -> bool {
        unsafe {
            // ConPTY was introduced in Windows 10 1809 (build 17763)
            // We check availability by trying to create a minimal ConPTY
            // If this succeeds, ConPTY is available
            let size = COORD { X: 2, Y: 2 };
            let mut test_handle: HPCON = 0;

            // Create minimal pipes for test
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
                0,
                &mut test_handle,
            );

            // Cleanup test resources
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

    /// Create an anonymous pipe
    fn create_pipe() -> Result<(HANDLE, HANDLE), ConPtyError> {
        let mut read_handle: HANDLE = INVALID_HANDLE_VALUE;
        let mut write_handle: HANDLE = INVALID_HANDLE_VALUE;

        let security_attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1, // TRUE
        };

        let result = unsafe { CreatePipe(&mut read_handle, &mut write_handle, &security_attrs, 0) };

        if result == 0 {
            return Err(ConPtyError::Io(io::Error::last_os_error()));
        }

        Ok((write_handle, read_handle))
    }

    /// Create a process attached to the pseudo console
    fn create_process_with_pty(command: &str, pty_handle: HPCON) -> Result<HANDLE, ConPtyError> {
        let cmd_wide: Vec<u16> = OsStr::new(command).encode_wide().chain(Some(0)).collect();

        let mut startup_info: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;

        let mut attr_list_size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_list_size);
        }

        let attr_list_layout = Layout::from_size_align(attr_list_size, 8)
            .map_err(|_| ConPtyError::Io(io::Error::other("invalid attribute list layout")))?;
        let attr_list: LPPROC_THREAD_ATTRIBUTE_LIST =
            unsafe { alloc(attr_list_layout) as LPPROC_THREAD_ATTRIBUTE_LIST };

        if attr_list.is_null() {
            return Err(ConPtyError::Io(io::Error::other(
                "failed to allocate attribute list",
            )));
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
            return Err(ConPtyError::Io(io::Error::last_os_error()));
        }

        let result = unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                pty_handle as *mut c_void,
                std::mem::size_of::<HPCON>(),
                null_mut(),
                null_mut(),
            )
        };

        if result == 0 {
            cleanup_attr_list(true);
            return Err(ConPtyError::Io(io::Error::last_os_error()));
        }

        startup_info.lpAttributeList = attr_list;

        let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let result = unsafe {
            CreateProcessW(
                null_mut(),
                cmd_wide.as_ptr() as *mut _,
                null_mut(),
                null_mut(),
                1,
                EXTENDED_STARTUPINFO_PRESENT,
                null_mut(),
                null_mut(),
                &startup_info.StartupInfo as *const _ as *mut STARTUPINFOW,
                &mut process_info,
            )
        };
        let process_error = (result == 0).then(|| unsafe { GetLastError() });
        cleanup_attr_list(true);

        if let Some(error_code) = process_error {
            return Err(ConPtyError::ProcessCreationFailed(error_code));
        }

        unsafe {
            CloseHandle(process_info.hThread);
        }

        Ok(process_info.hProcess)
    }
}

impl Drop for ConPtyShell {
    fn drop(&mut self) {
        if self.shutdown_called {
            return;
        }

        unsafe {
            if self.process_handle != INVALID_HANDLE_VALUE {
                TerminateProcess(self.process_handle, 0);
                CloseHandle(self.process_handle);
            }

            if self.pty_handle != 0 {
                ClosePseudoConsole(self.pty_handle);
            }

            if self.input_pipe != INVALID_HANDLE_VALUE {
                CloseHandle(self.input_pipe);
            }

            if self.output_pipe != INVALID_HANDLE_VALUE {
                CloseHandle(self.output_pipe);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_cmd() {
        let shell = ConPtyShell::spawn("cmd.exe", 24, 80);
        assert!(shell.is_ok(), "Failed to spawn cmd.exe: {:?}", shell.err());
    }

    #[test]
    fn test_invalid_dimensions() {
        let result = ConPtyShell::spawn("cmd.exe", 0, 80);
        assert!(matches!(result, Err(ConPtyError::InvalidDimensions)));

        let result = ConPtyShell::spawn("cmd.exe", 24, 0);
        assert!(matches!(result, Err(ConPtyError::InvalidDimensions)));
    }
}
