//! Shell Bridge
//!
//! Manages pseudo-terminal connection between UI and shell processes.
//! On Windows: Uses ConPTY API (Windows 10 1809+)
//! On Unix: Uses forkpty-backed pseudo-terminal sessions.

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
    Eof,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    HandoffPty, PtyControl, PtyError, PtyInput, PtyOutput, PtyParts, is_conpty_available,
};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{PtyControl, PtyError, PtyInput, PtyOutput, PtyParts, is_conpty_available};

#[cfg(not(any(windows, unix)))]
compile_error!("mightty shell bridge supports Windows and Unix targets only");
