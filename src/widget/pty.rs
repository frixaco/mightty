use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
    Resize(PtySize),
    Shutdown,
}

pub(super) enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

impl PtyEvent {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Output(data) => data.len(),
            Self::Exited => 0,
        }
    }
}

#[cfg(any(windows, unix))]
pub(super) struct PtyWorker {
    command_tx: flume::Sender<PtyCommand>,
    control_thread: Option<JoinHandle<()>>,
    reader_thread: Option<JoinHandle<()>>,
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
        let (command_tx, command_rx) = flume::unbounded::<PtyCommand>();
        let (event_tx, event_rx) = flume::bounded::<PtyEvent>(OUTPUT_QUEUE_CAPACITY);

        let mut input = parts.input;
        let mut control = parts.control;
        let control_exit_flag = Arc::clone(&exit_flag);
        let control_event_tx = event_tx.clone();
        let control_thread = std::thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                let result = match command {
                    PtyCommand::Write(data) => input.write_all(&data),
                    PtyCommand::Resize(size) => control.resize(size),
                    PtyCommand::Shutdown => break,
                };

                if let Err(err) = result {
                    eprintln!("ConPTY command failed: {err}");
                    control_exit_flag.store(true, Ordering::Relaxed);
                    let _ = control_event_tx.try_send(PtyEvent::Exited);
                    break;
                }
            }

            let _ = control.shutdown();
        });

        let mut output = parts.output;
        let reader_exit_flag = Arc::clone(&exit_flag);
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; READ_BUFFER_SIZE];

            loop {
                match output.read(&mut buf) {
                    Ok(PtyRead::Data(0)) => {}
                    Ok(PtyRead::Data(n)) => {
                        if event_tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Ok(PtyRead::Eof) => break,
                    Err(err) => {
                        eprintln!("ConPTY output read failed: {err}");
                        break;
                    }
                }
            }

            reader_exit_flag.store(true, Ordering::Relaxed);
            let _ = event_tx.try_send(PtyEvent::Exited);
        });

        Ok((
            Self {
                command_tx,
                control_thread: Some(control_thread),
                reader_thread: Some(reader_thread),
            },
            event_rx,
        ))
    }

    pub(super) fn command_tx(&self) -> flume::Sender<PtyCommand> {
        self.command_tx.clone()
    }

    pub(super) fn shutdown(&mut self) {
        let _ = self.command_tx.send(PtyCommand::Shutdown);

        if let Some(handle) = self.control_thread.take() {
            let _ = handle.join();
        }

        if let Some(handle) = self.reader_thread.take() {
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
