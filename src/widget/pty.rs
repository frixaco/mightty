use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;

use crate::profile::LaunchSpec;
use crate::shell::PtySize;
#[cfg(any(windows, unix))]
use crate::shell::{PtyParts, PtyRead};

pub(super) const OUTPUT_DRAIN_BUDGET: usize = 256 * 1024;

const READ_BUFFER_SIZE: usize = 32 * 1024;
const OUTPUT_QUEUE_CAPACITY: usize = 64;

pub(super) enum PtyCommand {
    Write(Vec<u8>),
    WriteAck(Vec<u8>, flume::Sender<crate::control::Acknowledgement>),
    ResizeAck(PtySize, flume::Sender<crate::control::Acknowledgement>),
    Shutdown,
}

const INPUT_QUEUE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct PtySender {
    sender: flume::Sender<QueuedCommand>,
    bytes: Arc<AtomicUsize>,
    size: Arc<AtomicU32>,
}
struct QueuedCommand {
    command: Option<PtyCommand>,
    bytes: usize,
    budget: Arc<AtomicUsize>,
}
impl Drop for QueuedCommand {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl PtySender {
    pub(super) fn acknowledged_size(&self) -> Option<PtySize> {
        let size = self.size.load(Ordering::Acquire);
        (size != 0).then(|| PtySize::new(size as u16, (size >> 16) as u16))
    }
    pub(super) fn send(&self, command: PtyCommand) -> Result<(), &'static str> {
        let bytes = match &command {
            PtyCommand::Write(data) | PtyCommand::WriteAck(data, _) => data.len(),
            _ => 0,
        };
        self.bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= INPUT_QUEUE_BYTES)
            })
            .map_err(|_| "PTY input byte budget exhausted")?;
        let queued = QueuedCommand {
            command: Some(command),
            bytes,
            budget: Arc::clone(&self.bytes),
        };
        self.sender
            .try_send(queued)
            .map_err(|_| "PTY command queue full or closed")
    }
}

#[cfg(test)]
pub(super) fn test_channel() -> (PtySender, TestReceiver) {
    let (sender, receiver) = flume::bounded(256);
    (
        PtySender {
            sender,
            bytes: Arc::new(AtomicUsize::new(0)),
            size: Arc::new(AtomicU32::new(0)),
        },
        TestReceiver(receiver),
    )
}
#[cfg(test)]
pub(super) struct TestReceiver(flume::Receiver<QueuedCommand>);
#[cfg(test)]
impl TestReceiver {
    pub(super) fn try_recv(&self) -> Result<PtyCommand, flume::TryRecvError> {
        self.0
            .try_recv()
            .map(|mut queued| queued.command.take().unwrap())
    }
    pub(super) fn try_iter(&self) -> impl Iterator<Item = PtyCommand> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
}

pub(super) enum PtyEvent {
    Output(Vec<u8>),
    OutputEnded,
    IoFailed(String),
}

impl PtyEvent {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Output(data) => data.len(),
            Self::OutputEnded | Self::IoFailed(_) => 0,
        }
    }
}

#[cfg(any(windows, unix))]
pub(super) struct PtyWorker {
    root_process: Option<crate::diagnostics::RootProcess>,
    command_tx: PtySender,
    control_thread: Option<JoinHandle<()>>,
    reader_thread: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    stop_reader: Arc<AtomicBool>,
}

#[cfg(not(any(windows, unix)))]
pub(super) struct PtyWorker;

#[cfg(any(windows, unix))]
impl PtyWorker {
    pub(super) fn spawn(
        launch: LaunchSpec,
        rows: u16,
        cols: u16,
        exit_flag: Arc<AtomicBool>,
    ) -> Result<(Self, flume::Receiver<PtyEvent>), crate::shell::PtyError> {
        let parts = PtyParts::spawn(&launch, PtySize::new(rows, cols))?;
        let result = Self::from_parts(parts, exit_flag);
        result
            .0
            .command_tx
            .size
            .store((u32::from(cols) << 16) | u32::from(rows), Ordering::Release);
        Ok(result)
    }

    pub(super) fn from_parts(
        parts: PtyParts,
        exit_flag: Arc<AtomicBool>,
    ) -> (Self, flume::Receiver<PtyEvent>) {
        #[cfg(windows)]
        let root_process = parts.control.process_watch();
        #[cfg(not(windows))]
        let root_process = None;
        let (sender, command_rx) = flume::bounded::<QueuedCommand>(256);
        let command_tx = PtySender {
            sender,
            bytes: Arc::new(AtomicUsize::new(0)),
            size: Arc::new(AtomicU32::new(0)),
        };
        let acknowledged_size = Arc::clone(&command_tx.size);
        let (event_tx, event_rx) = flume::bounded::<PtyEvent>(OUTPUT_QUEUE_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop_reader = Arc::new(AtomicBool::new(false));
        let control_stopping = stopping.clone();

        let mut input = parts.input;
        let mut control = parts.control;
        let control_exit_flag = Arc::clone(&exit_flag);
        let control_event_tx = event_tx.clone();
        let control_thread = std::thread::spawn(move || {
            while let Ok(mut queued) = command_rx.recv() {
                if control_stopping.load(Ordering::Acquire) {
                    break;
                }
                let result = match queued.command.take().expect("queued command") {
                    PtyCommand::Write(data) => {
                        input.write_all_interruptible(&data, &control_stopping)
                    }
                    PtyCommand::WriteAck(data, reply) => {
                        let mut written_bytes = 0;
                        let result =
                            input.write_with_progress(&data, &control_stopping, &mut written_bytes);
                        let _ = reply.try_send(crate::control::Acknowledgement {
                            written_bytes,
                            error: result.as_ref().err().map(ToString::to_string),
                        });
                        result
                    }
                    PtyCommand::ResizeAck(size, reply) => {
                        let result = control.resize(size);
                        if result.is_ok() {
                            acknowledged_size.store(
                                (u32::from(size.cols) << 16) | u32::from(size.rows),
                                Ordering::Release,
                            );
                        }
                        let _ = reply.try_send(crate::control::Acknowledgement {
                            written_bytes: 0,
                            error: result.as_ref().err().map(ToString::to_string),
                        });
                        result
                    }
                    PtyCommand::Shutdown => break,
                };

                if let Err(err) = result {
                    if control_stopping.load(Ordering::Acquire) {
                        break;
                    }
                    eprintln!("ConPTY command failed: {err}");
                    control_exit_flag.store(true, Ordering::Relaxed);
                    let _ = control_event_tx.try_send(PtyEvent::IoFailed(err.to_string()));
                    break;
                }
            }

            let _ = control.shutdown();
        });

        let mut output = parts.output;
        let reader_exit_flag = Arc::clone(&exit_flag);
        let reader_stopping = stopping.clone();
        let reader_cancel = stop_reader.clone();
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; READ_BUFFER_SIZE];
            let mut failure = None;

            loop {
                match output.read_interruptible(&mut buf, &reader_cancel) {
                    Ok(PtyRead::Data(0)) => {}
                    Ok(PtyRead::Data(n)) => {
                        let mut event = PtyEvent::Output(buf[..n].to_vec());
                        // Keep draining ConPTY during close, even when the UI no longer
                        // consumes output. ClosePseudoConsole needs its output reader.
                        while !reader_stopping.load(Ordering::Acquire) {
                            match event_tx.send_timeout(event, std::time::Duration::from_millis(20))
                            {
                                Ok(()) => break,
                                Err(flume::SendTimeoutError::Timeout(returned)) => event = returned,
                                Err(flume::SendTimeoutError::Disconnected(_)) => break,
                            }
                        }
                    }
                    Ok(PtyRead::Eof) => break,
                    Err(err) => {
                        if !reader_cancel.load(Ordering::Acquire) {
                            eprintln!("PTY output read failed: {err}");
                            failure = Some(err.to_string());
                        }
                        break;
                    }
                }
            }

            reader_exit_flag.store(true, Ordering::Relaxed);
            let event = failure.map_or(PtyEvent::OutputEnded, PtyEvent::IoFailed);
            let _ = event_tx.send_timeout(event, std::time::Duration::from_millis(100));
        });

        (
            Self {
                root_process,
                command_tx,
                control_thread: Some(control_thread),
                reader_thread: Some(reader_thread),
                stopping,
                stop_reader,
            },
            event_rx,
        )
    }

    pub(super) fn command_tx(&self) -> PtySender {
        self.command_tx.clone()
    }
    pub(super) fn root_process(&self) -> Option<crate::diagnostics::RootProcess> {
        self.root_process.clone()
    }

    pub(super) fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.command_tx.send(PtyCommand::Shutdown);

        if let Some(handle) = self.control_thread.take() {
            // Retry cancellation to cover shutdown racing with entry into a
            // synchronous OS write. The flag prevents any subsequent writes.
            while !handle.is_finished() {
                crate::shell::cancel_io(&handle);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = handle.join();
        }

        self.stop_reader.store(true, Ordering::Release);
        if let Some(handle) = self.reader_thread.take() {
            while !handle.is_finished() {
                crate::shell::cancel_io(&handle);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = handle.join();
        }
    }
}

#[cfg(not(any(windows, unix)))]
impl PtyWorker {
    pub(super) fn shutdown(&mut self) {}
}

impl Drop for PtyWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    fn process_handle() -> OwnedHandle {
        unsafe {
            let process = GetCurrentProcess();
            let mut handle = std::ptr::null_mut();
            assert_ne!(
                DuplicateHandle(
                    process,
                    process,
                    process,
                    &mut handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS
                ),
                0
            );
            OwnedHandle::from_raw_handle(handle)
        }
    }

    #[test]
    fn shutdown_interrupts_stalled_handoff_input_and_output() {
        let (parts, _unread_input, _open_output) = PtyParts::from_handoff(
            process_handle(),
            process_handle(),
            process_handle(),
            process_handle(),
        )
        .unwrap();
        let (mut worker, receiver) = PtyWorker::from_parts(parts, Arc::new(AtomicBool::new(false)));
        worker
            .command_tx()
            .send(PtyCommand::Write(vec![b'a'; 1024 * 1024]))
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(receiver);
        let start = Instant::now();
        worker.shutdown();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "shutdown waited for the pipe peer"
        );
        // The test process handle is observed, never terminated by handoff close.
    }

    #[test]
    fn shutdown_discards_queued_input_even_if_receiver_stays_connected() {
        let (parts, _unread_input, _open_output) = PtyParts::from_handoff(
            process_handle(),
            process_handle(),
            process_handle(),
            process_handle(),
        )
        .unwrap();
        let (mut worker, _receiver) =
            PtyWorker::from_parts(parts, Arc::new(AtomicBool::new(false)));
        for _ in 0..16 {
            worker
                .command_tx()
                .send(PtyCommand::Write(vec![b'a'; 128 * 1024]))
                .unwrap();
        }
        let start = Instant::now();
        worker.shutdown();
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.shutdown(); // Also safe when Drop calls it again.
    }
}
