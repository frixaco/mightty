//! Unix shell bridge placeholder.
//!
//! mightty currently implements shell I/O through Windows ConPTY only.

use std::io;

use super::{PtyRead, PtySize};

#[derive(Debug)]
pub enum PtyError {
    Unsupported,
    InvalidDimensions,
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "PTY support is not implemented on this platform"),
            Self::InvalidDimensions => write!(f, "invalid terminal dimensions"),
        }
    }
}

impl std::error::Error for PtyError {}

pub struct PtySession;

impl PtySession {
    pub fn spawn(_command: &str, size: PtySize) -> Result<Self, PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        Err(PtyError::Unsupported)
    }

    pub fn try_read(&mut self, _buf: &mut [u8]) -> Result<PtyRead, PtyError> {
        Err(PtyError::Unsupported)
    }

    pub fn write(&mut self, _data: &[u8]) -> Result<(), PtyError> {
        Err(PtyError::Unsupported)
    }

    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        if !size.is_valid() {
            return Err(PtyError::InvalidDimensions);
        }

        Err(PtyError::Unsupported)
    }

    pub fn has_exited(&self) -> Result<bool, PtyError> {
        Err(PtyError::Unsupported)
    }

    pub fn shutdown(self) -> Result<(), PtyError> {
        Ok(())
    }
}

impl From<io::Error> for PtyError {
    fn from(_error: io::Error) -> Self {
        Self::Unsupported
    }
}
