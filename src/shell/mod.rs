//! Shell Bridge
//!
//! Manages pseudo-terminal connection between UI and shell processes.
//! On Windows: Uses ConPTY API (Windows 10 1809+)
//! On Unix: Stub implementation (PTY support not yet implemented)

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::ConPtyShell;
#[cfg(windows)]
pub use windows::ConPtyError;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::ConPtyShell;
#[cfg(not(windows))]
pub use unix::ConPtyError;
