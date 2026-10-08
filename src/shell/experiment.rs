//! Opt-in comparison adapter. MIGHTTY_PTY_BACKEND=portable selects 0.9.0;
//! native is the default. Windows default-terminal handoffs stay native.

use std::{
    cell::{Cell, RefCell},
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::windows as native;
use super::{PtyError, PtyRead, PtySize};
use crate::profile::LaunchSpec;

pub struct PtyParts {
    pub input: PtyInput,
    pub output: PtyOutput,
    pub control: PtyControl,
}
pub enum PtyInput {
    Native(native::PtyInput),
    Portable(Box<dyn Write + Send>),
}
pub enum PtyOutput {
    Native(native::PtyOutput),
    Portable(Box<dyn Read + Send>),
}
pub enum PtyControl {
    Native(native::PtyControl),
    Portable {
        master: Option<Box<dyn portable_pty::MasterPty + Send>>,
        child: RefCell<Box<dyn portable_pty::Child + Send + Sync>>,
        exit: Cell<Option<u32>>,
    },
}

fn error(operation: &'static str, source: impl std::fmt::Display) -> PtyError {
    PtyError::Io {
        operation,
        source: io::Error::other(source.to_string()),
    }
}

impl PtyParts {
    pub fn spawn(launch: &LaunchSpec, size: PtySize) -> Result<Self, PtyError> {
        eprintln!(
            "PTY experiment backend: {}",
            std::env::var("MIGHTTY_PTY_BACKEND").unwrap_or_else(|_| "native".into())
        );
        match std::env::var("MIGHTTY_PTY_BACKEND").as_deref() {
            Ok("portable" | "portable-system") => Self::spawn_portable(launch, size),
            // Same flags as portable-pty: isolate flag effects from library effects.
            Ok("native-flags") => {
                native::PtyParts::spawn_with_flags(launch, size, 0x7).map(Self::from_native)
            }
            Ok("native-passthrough") => {
                native::PtyParts::spawn_with_flags(launch, size, 0xf).map(Self::from_native)
            }
            Ok("native") | Err(_) => native::PtyParts::spawn(launch, size).map(Self::from_native),
            Ok(_) => Err(error(
                "select PTY backend",
                "use native, native-flags, native-passthrough, portable or portable-system",
            )),
        }
    }

    fn from_native(parts: native::PtyParts) -> Self {
        Self {
            input: PtyInput::Native(parts.input),
            output: PtyOutput::Native(parts.output),
            control: PtyControl::Native(parts.control),
        }
    }

    fn spawn_portable(launch: &LaunchSpec, size: PtySize) -> Result<Self, PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }
        let pair = portable_pty::native_pty_system()
            .openpty(portable_size(size))
            .map_err(|e| error("portable openpty", e))?;
        let mut command = portable_pty::CommandBuilder::new(&launch.executable);
        command.args(&launch.arguments);
        if !launch.inherit_environment {
            command.env_clear();
        }
        for key in &launch.unset_environment {
            command.env_remove(key);
        }
        for (key, value) in &launch.environment {
            command.env(key, value);
        }
        if let Some(directory) = &launch.working_directory {
            command.cwd(directory);
        }
        // Acquire streams before spawning, so an allocation/clone failure
        // cannot leave an unowned shell running.
        let output = pair
            .master
            .try_clone_reader()
            .map_err(|e| error("portable reader", e))?;
        let input = pair
            .master
            .take_writer()
            .map_err(|e| error("portable writer", e))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| error("portable spawn", e))?;
        drop(pair.slave);
        Ok(Self {
            input: PtyInput::Portable(input),
            output: PtyOutput::Portable(output),
            control: PtyControl::Portable {
                master: Some(pair.master),
                child: RefCell::new(child),
                exit: Cell::new(None),
            },
        })
    }

    #[cfg(windows)]
    pub(crate) fn from_handoff(
        signal: std::os::windows::io::OwnedHandle,
        reference: std::os::windows::io::OwnedHandle,
        server: std::os::windows::io::OwnedHandle,
        client: std::os::windows::io::OwnedHandle,
    ) -> Result<
        (
            Self,
            std::os::windows::io::OwnedHandle,
            std::os::windows::io::OwnedHandle,
        ),
        PtyError,
    > {
        let (parts, input, output) =
            native::PtyParts::from_handoff(signal, reference, server, client)?;
        Ok((Self::from_native(parts), input, output))
    }
}

fn portable_size(size: PtySize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl PtyInput {
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.write_all_interruptible(data, &AtomicBool::new(false))
    }
    pub fn write_all_interruptible(
        &mut self,
        data: &[u8],
        stopping: &AtomicBool,
    ) -> Result<(), PtyError> {
        self.write_with_progress(data, stopping, &mut 0)
    }
    pub fn write_with_progress(
        &mut self,
        data: &[u8],
        stopping: &AtomicBool,
        written: &mut usize,
    ) -> Result<(), PtyError> {
        match self {
            Self::Native(input) => input.write_with_progress(data, stopping, written),
            Self::Portable(input) => {
                *written = 0;
                while *written < data.len() {
                    if stopping.load(Ordering::Acquire) {
                        return Err(error("portable write", "session closing"));
                    }
                    // Match native's chunking and acknowledge partial writes.
                    let end = data.len().min(*written + 32 * 1024);
                    match input.write(&data[*written..end]) {
                        Ok(0) => return Err(PtyError::ZeroLengthWrite),
                        Ok(count) => *written += count,
                        Err(e) => return Err(error("portable write", e)),
                    }
                }
                Ok(())
            }
        }
    }
}

impl PtyOutput {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        self.read_interruptible(buf, &AtomicBool::new(false))
    }
    pub fn read_interruptible(
        &mut self,
        buf: &mut [u8],
        stopping: &AtomicBool,
    ) -> Result<PtyRead, PtyError> {
        match self {
            Self::Native(output) => output.read_interruptible(buf, stopping),
            Self::Portable(output) => {
                if stopping.load(Ordering::Acquire) {
                    return Ok(PtyRead::Eof);
                }
                if buf.is_empty() {
                    return Ok(PtyRead::Data(0));
                }
                match output.read(buf) {
                    Ok(0) => Ok(PtyRead::Eof),
                    Ok(count) => Ok(PtyRead::Data(count)),
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(PtyRead::Eof),
                    Err(e) => Err(error("portable read", e)),
                }
            }
        }
    }
}

impl PtyControl {
    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }
        match self {
            Self::Native(control) => control.resize(size),
            Self::Portable { master, .. } => master
                .as_ref()
                .ok_or_else(|| error("portable resize", "session closed"))?
                .resize(portable_size(size))
                .map_err(|e| error("portable resize", e)),
        }
    }
    pub fn has_exited(&self) -> Result<bool, PtyError> {
        match self {
            Self::Native(control) => control.has_exited(),
            Self::Portable { child, exit, .. } => {
                if exit.get().is_some() {
                    return Ok(true);
                }
                let status = child
                    .borrow_mut()
                    .try_wait()
                    .map_err(|e| error("portable try_wait", e))?;
                if let Some(ref status) = status {
                    exit.set(Some(status.exit_code()));
                }
                Ok(status.is_some())
            }
        }
    }
    pub fn exit_code(&self) -> Result<Option<u32>, PtyError> {
        match self {
            Self::Native(control) => control.exit_code(),
            Self::Portable { exit, .. } => {
                self.has_exited()?;
                Ok(exit.get())
            }
        }
    }
    #[cfg(windows)]
    pub fn process_watch(&self) -> Option<crate::diagnostics::RootProcess> {
        match self {
            Self::Native(control) => control.process_watch(),
            Self::Portable { child, .. } => {
                let child = child.borrow();
                let handle = child.as_raw_handle()?;
                crate::diagnostics::watch_process(unsafe {
                    std::os::windows::io::BorrowedHandle::borrow_raw(handle)
                })
                .ok()
            }
        }
    }
    pub fn shutdown(&mut self) -> Result<(), PtyError> {
        match self {
            Self::Native(control) => control.shutdown(),
            Self::Portable {
                master,
                child,
                exit,
            } => {
                if master.is_none() {
                    return Ok(());
                }
                // Like native, close with the output reader still running.
                drop(master.take());
                let deadline = Instant::now() + Duration::from_millis(250);
                loop {
                    let status = child
                        .borrow_mut()
                        .try_wait()
                        .map_err(|e| error("portable shutdown wait", e))?;
                    if let Some(status) = status {
                        exit.set(Some(status.exit_code()));
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                child
                    .borrow_mut()
                    .kill()
                    .map_err(|e| error("portable shutdown kill", e))
            }
        }
    }
}
impl Drop for PtyControl {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
