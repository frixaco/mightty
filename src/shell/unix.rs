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

pub struct PtyParts {
    pub input: PtyInput,
    pub output: PtyOutput,
    pub control: PtyControl,
}

pub struct PtyInput {
    master_fd: RawFd,
}

pub struct PtyOutput {
    master_fd: RawFd,
}

pub struct PtyControl {
    master_fd: RawFd,
    child_pid: libc::pid_t,
    child_reaped: Cell<bool>,
    exit_code: Cell<Option<u32>>,
    shutdown_called: bool,
}

impl PtyParts {
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

        let input_fd = duplicate_fd(master_fd, "duplicate PTY input fd")?;
        let output_fd = match duplicate_fd(master_fd, "duplicate PTY output fd") {
            Ok(fd) => fd,
            Err(err) => {
                close_fd(input_fd);
                close_fd(master_fd);
                unsafe {
                    libc::kill(child_pid, libc::SIGKILL);
                }
                return Err(err);
            }
        };

        Ok(Self {
            input: PtyInput {
                master_fd: input_fd,
            },
            output: PtyOutput {
                master_fd: output_fd,
            },
            control: PtyControl {
                master_fd,
                child_pid,
                child_reaped: Cell::new(false),
                exit_code: Cell::new(None),
                shutdown_called: false,
            },
        })
    }
}

impl PtyInput {
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), PtyError> {
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
                Some(libc::EAGAIN) => thread::sleep(SHUTDOWN_POLL),
                #[cfg(any(target_os = "linux", target_os = "android"))]
                Some(libc::EWOULDBLOCK) => thread::sleep(SHUTDOWN_POLL),
                _ => return Err(PtyError::from_io("write to PTY master", error)),
            }
        }

        Ok(())
    }
}

impl Drop for PtyInput {
    fn drop(&mut self) {
        close_fd(self.master_fd);
        self.master_fd = INVALID_FD;
    }
}

impl PtyOutput {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        if buf.is_empty() {
            return Ok(PtyRead::Data(0));
        }

        loop {
            let bytes_read =
                unsafe { libc::read(self.master_fd, buf.as_mut_ptr().cast(), buf.len()) };

            if bytes_read > 0 {
                return Ok(PtyRead::Data(bytes_read as usize));
            }

            if bytes_read == 0 {
                return Ok(PtyRead::Eof);
            }

            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => continue,
                #[cfg(any(target_os = "linux", target_os = "android"))]
                Some(libc::EWOULDBLOCK) => continue,
                Some(libc::EIO) => return Ok(PtyRead::Eof),
                _ => return Err(PtyError::from_io("read from PTY master", error)),
            }
        }
    }
}

impl Drop for PtyOutput {
    fn drop(&mut self) {
        close_fd(self.master_fd);
        self.master_fd = INVALID_FD;
    }
}

impl PtyControl {
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

    pub fn exit_code(&self) -> Result<Option<u32>, PtyError> {
        if !self.has_exited()? {
            return Ok(None);
        }

        Ok(self.exit_code.get())
    }

    pub fn shutdown(&mut self) -> Result<(), PtyError> {
        self.shutdown_called = true;
        self.close_handles(true)
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
                let status = unsafe { status.assume_init() };
                self.exit_code.set(exit_code_from_status(status));
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

impl Drop for PtyControl {
    fn drop(&mut self) {
        if self.shutdown_called {
            return;
        }

        let _ = self.close_handles(false);
    }
}

pub fn is_conpty_available() -> bool {
    true
}

fn duplicate_fd(fd: RawFd, operation: &'static str) -> Result<RawFd, PtyError> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        Err(PtyError::io(operation))
    } else {
        Ok(duplicated)
    }
}

fn close_fd(fd: RawFd) {
    if fd != INVALID_FD {
        unsafe {
            libc::close(fd);
        }
    }
}

fn exit_code_from_status(status: libc::c_int) -> Option<u32> {
    if libc::WIFEXITED(status) {
        Some(libc::WEXITSTATUS(status) as u32)
    } else if libc::WIFSIGNALED(status) {
        Some((128 + libc::WTERMSIG(status)) as u32)
    } else {
        None
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
    use std::sync::mpsc;

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);

    fn spawn_test_shell() -> PtyParts {
        PtyParts::spawn("/bin/sh", PtySize::new(24, 80)).expect("spawn /bin/sh")
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

    #[test]
    fn spawn_shell() {
        let shell = PtyParts::spawn("/bin/sh", PtySize::new(24, 80));
        assert!(shell.is_ok(), "failed to spawn /bin/sh: {:?}", shell.err());
    }

    #[test]
    fn invalid_dimensions() {
        let result = PtyParts::spawn("/bin/sh", PtySize::new(0, 80));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));

        let result = PtyParts::spawn("/bin/sh", PtySize::new(24, 0));
        assert!(matches!(result, Err(PtyError::InvalidDimensions)));
    }

    #[test]
    fn reads_command_output() {
        let PtyParts {
            mut input,
            output,
            mut control,
        } = spawn_test_shell();
        let output_rx = read_until(output, "mightty-ready");

        input
            .write_all(b"printf 'mightty-ready\\n'\n")
            .expect("write command");
        assert_marker(output_rx, "mightty-ready");

        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn reports_process_exit() {
        let PtyParts {
            mut input,
            output: _output,
            mut control,
        } = spawn_test_shell();
        input.write_all(b"exit\n").expect("write exit");

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
        } = spawn_test_shell();
        control.resize(PtySize::new(40, 120)).expect("resize pty");
        control.shutdown().expect("shutdown shell");
    }

    #[test]
    fn writes_paste_sized_input() {
        let PtyParts {
            mut input,
            output: _output,
            mut control,
        } = spawn_test_shell();
        let command = format!(": {}\n", "x".repeat(8192));
        input
            .write_all(command.as_bytes())
            .expect("write large input");
        control.shutdown().expect("shutdown shell");
    }
}
