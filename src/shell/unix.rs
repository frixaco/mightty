//! Unix PTY shell bridge.
//!
//! Manages pseudo-terminal connection between UI and shell processes on Unix.

use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use super::{PtyRead, PtySize};

const SHUTDOWN_WAIT: Duration = Duration::from_millis(250);
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);
const INVALID_FD: RawFd = -1;

#[derive(Debug)]
pub enum PtyError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    InvalidDimensions,
    EmptyCommand,
    CommandContainsNul,
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
            Self::InvalidDimensions => write!(f, "invalid terminal dimensions"),
            Self::EmptyCommand => write!(f, "shell command is empty"),
            Self::CommandContainsNul => write!(f, "shell command contains a NUL byte"),
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
    master_fd: RawFd,
    child_pid: libc::pid_t,
    child_reaped: Cell<bool>,
    shutdown_called: bool,
}

impl PtySession {
    pub fn spawn(command: &str, size: PtySize) -> Result<Self, PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        let command = command.trim();
        if command.is_empty() {
            return Err(PtyError::EmptyCommand);
        }

        let shell = CString::new("/bin/sh").expect("static shell path has no NUL");
        let shell_name = CString::new("sh").expect("static shell name has no NUL");
        let shell_arg = CString::new("-lc").expect("static shell arg has no NUL");
        let command =
            CString::new(format!("exec {command}")).map_err(|_| PtyError::CommandContainsNul)?;
        let mut winsize = winsize_from_size(size);

        let mut master_fd = INVALID_FD;
        let child_pid = unsafe {
            libc::forkpty(
                &mut master_fd,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut winsize,
            )
        };

        if child_pid < 0 {
            return Err(PtyError::io("fork PTY"));
        }

        if child_pid == 0 {
            unsafe {
                libc::execl(
                    shell.as_ptr(),
                    shell_name.as_ptr(),
                    shell_arg.as_ptr(),
                    command.as_ptr(),
                    ptr::null::<libc::c_char>(),
                );
                libc::_exit(127);
            }
        }

        let mut session = Self {
            master_fd,
            child_pid,
            child_reaped: Cell::new(false),
            shutdown_called: false,
        };

        if let Err(err) = session.set_nonblocking() {
            let _ = session.close_handles(false);
            return Err(err);
        }

        Ok(session)
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        if buf.is_empty() {
            return Ok(PtyRead::WouldBlock);
        }

        loop {
            let bytes_read =
                unsafe { libc::read(self.master_fd, buf.as_mut_ptr().cast(), buf.len()) };

            if bytes_read > 0 {
                return Ok(PtyRead::Data(bytes_read as usize));
            }

            if bytes_read == 0 {
                let _ = self.reap_child();
                return Ok(PtyRead::Eof);
            }

            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    return if self.has_exited()? {
                        Ok(PtyRead::Eof)
                    } else {
                        Ok(PtyRead::WouldBlock)
                    };
                }
                #[cfg(any(target_os = "linux", target_os = "android"))]
                Some(libc::EWOULDBLOCK) => {
                    return if self.has_exited()? {
                        Ok(PtyRead::Eof)
                    } else {
                        Ok(PtyRead::WouldBlock)
                    };
                }
                Some(libc::EIO) => {
                    let _ = self.reap_child();
                    return Ok(PtyRead::Eof);
                }
                _ => return Err(PtyError::from_io("read from PTY master", error)),
            }
        }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        let mut written_total = 0usize;

        while written_total < data.len() {
            let remaining = &data[written_total..];
            let bytes_written =
                unsafe { libc::write(self.master_fd, remaining.as_ptr().cast(), remaining.len()) };

            if bytes_written > 0 {
                written_total += bytes_written as usize;
                continue;
            }

            if bytes_written == 0 {
                return Err(PtyError::ZeroLengthWrite);
            }

            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => {
                    thread::sleep(SHUTDOWN_POLL);
                }
                #[cfg(any(target_os = "linux", target_os = "android"))]
                Some(libc::EWOULDBLOCK) => {
                    thread::sleep(SHUTDOWN_POLL);
                }
                _ => return Err(PtyError::from_io("write to PTY master", error)),
            }
        }

        Ok(())
    }

    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        let winsize = winsize_from_size(size);
        let result = unsafe { libc::ioctl(self.master_fd, libc::TIOCSWINSZ, &winsize) };
        if result < 0 {
            return Err(PtyError::io("resize PTY"));
        }

        unsafe {
            libc::kill(-self.child_pid, libc::SIGWINCH);
        }

        Ok(())
    }

    pub fn has_exited(&self) -> Result<bool, PtyError> {
        self.reap_child()
    }

    pub fn shutdown(mut self) -> Result<(), PtyError> {
        self.shutdown_called = true;
        self.close_handles(true)
    }

    fn set_nonblocking(&self) -> Result<(), PtyError> {
        let flags = unsafe { libc::fcntl(self.master_fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(PtyError::io("get PTY flags"));
        }

        let result =
            unsafe { libc::fcntl(self.master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if result < 0 {
            return Err(PtyError::io("set PTY nonblocking"));
        }

        Ok(())
    }

    fn reap_child(&self) -> Result<bool, PtyError> {
        if self.child_reaped.get() {
            return Ok(true);
        }

        let mut status = MaybeUninit::<libc::c_int>::uninit();
        loop {
            let result =
                unsafe { libc::waitpid(self.child_pid, status.as_mut_ptr(), libc::WNOHANG) };
            if result == self.child_pid {
                self.child_reaped.set(true);
                return Ok(true);
            }

            if result == 0 {
                return Ok(false);
            }

            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ECHILD) => {
                    self.child_reaped.set(true);
                    return Ok(true);
                }
                _ => return Err(PtyError::from_io("wait for child process", error)),
            }
        }
    }

    fn close_handles(&mut self, allow_graceful_wait: bool) -> Result<(), PtyError> {
        let mut first_error = None;

        if self.master_fd != INVALID_FD {
            unsafe {
                if libc::close(self.master_fd) < 0 {
                    first_error.get_or_insert_with(|| PtyError::io("close PTY master"));
                }
            }
            self.master_fd = INVALID_FD;
        }

        if !self.child_reaped.get() {
            if allow_graceful_wait {
                self.signal_child(libc::SIGHUP);
                if !self.wait_until_exit(SHUTDOWN_WAIT)? {
                    self.signal_child(libc::SIGTERM);
                }
                if !self.wait_until_exit(SHUTDOWN_WAIT)? {
                    self.signal_child(libc::SIGKILL);
                }
                let _ = self.wait_until_exit(SHUTDOWN_WAIT);
            } else {
                self.signal_child(libc::SIGKILL);
                let _ = self.reap_child();
            }
        }

        if let Some(err) = first_error {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn signal_child(&self, signal: libc::c_int) {
        unsafe {
            libc::kill(-self.child_pid, signal);
            libc::kill(self.child_pid, signal);
        }
    }

    fn wait_until_exit(&mut self, timeout: Duration) -> Result<bool, PtyError> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.reap_child()? {
                return Ok(true);
            }

            if Instant::now() >= deadline {
                return Ok(false);
            }

            thread::sleep(SHUTDOWN_POLL);
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

fn winsize_from_size(size: PtySize) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);

    fn spawn_test_shell() -> PtySession {
        PtySession::spawn("/bin/sh", PtySize::new(24, 80)).expect("spawn /bin/sh")
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
                PtyRead::WouldBlock => thread::sleep(Duration::from_millis(10)),
                PtyRead::Eof => break,
            }
        }

        panic!(
            "timed out waiting for marker {marker:?}; output was {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn spawn_shell() {
        let shell = PtySession::spawn("/bin/sh", PtySize::new(24, 80));
        assert!(shell.is_ok(), "failed to spawn /bin/sh: {:?}", shell.err());
    }

    #[test]
    fn invalid_dimensions() {
        let result = PtySession::spawn("/bin/sh", PtySize::new(0, 80));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));

        let result = PtySession::spawn("/bin/sh", PtySize::new(24, 0));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));
    }

    #[test]
    fn reads_command_output() {
        let mut shell = spawn_test_shell();
        shell
            .write(b"printf 'mightty-ready\\n'\n")
            .expect("write command");

        let output = wait_for_output(&mut shell, "mightty-ready");
        assert!(output.contains("mightty-ready"));

        shell.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reports_process_exit() {
        let mut shell = spawn_test_shell();
        shell.write(b"exit\n").expect("write exit");

        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut buf = [0u8; 1024];
        while Instant::now() < deadline {
            if shell.has_exited().expect("check process exit") {
                return;
            }

            match shell.try_read(&mut buf).expect("read from pty") {
                PtyRead::Data(_) | PtyRead::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                PtyRead::Eof => return,
            }
        }

        panic!("timed out waiting for process exit");
    }

    #[test]
    fn resizes_session() {
        let mut shell = spawn_test_shell();
        shell.resize(PtySize::new(40, 120)).expect("resize pty");
        shell.shutdown().expect("shutdown shell");
    }

    #[test]
    fn writes_paste_sized_input() {
        let mut shell = spawn_test_shell();
        let command = format!(": {}\n", "x".repeat(8192));
        shell.write(command.as_bytes()).expect("write large input");
        shell.shutdown().expect("shutdown shell");
    }
}
