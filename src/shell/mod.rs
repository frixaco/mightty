//! Shell Bridge
//!
//! Manages pseudo-terminal connection between UI and shell processes.
//! On Windows: Uses ConPTY API (Windows 10 1809+)
//! On Unix: Stub implementation (PTY support not yet implemented)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }

    pub const fn is_valid(self) -> bool {
        self.rows > 0 && self.cols > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyRead {
    Data(usize),
    WouldBlock,
    Eof,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::PtyError;
#[cfg(windows)]
pub use windows::PtySession;
#[cfg(windows)]
pub type ConPtyError = PtyError;
#[cfg(windows)]
pub type ConPtyShell = PtySession;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::PtyError;
#[cfg(not(windows))]
pub use unix::PtySession;
#[cfg(not(windows))]
pub type ConPtyError = PtyError;
#[cfg(not(windows))]
pub type ConPtyShell = PtySession;
