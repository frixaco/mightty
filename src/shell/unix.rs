//! Unix shell bridge placeholder.
//!
//! mightty currently implements shell I/O through Windows ConPTY only.

use std::io;

#[derive(Debug)]
pub enum ConPtyError {
    Unsupported,
    InvalidDimensions,
}

impl std::fmt::Display for ConPtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "PTY support is not implemented on this platform"),
            Self::InvalidDimensions => write!(f, "invalid terminal dimensions"),
        }
    }
}

impl std::error::Error for ConPtyError {}

pub struct ConPtyShell;

impl ConPtyShell {
    pub fn spawn(_command: &str, rows: u16, cols: u16) -> Result<Self, ConPtyError> {
        if rows == 0 || cols == 0 {
            return Err(ConPtyError::InvalidDimensions);
        }

        Err(ConPtyError::Unsupported)
    }

    pub fn read(&mut self, _buf: &mut [u8]) -> Result<usize, ConPtyError> {
        Err(ConPtyError::Unsupported)
    }

    pub fn write(&mut self, _data: &[u8]) -> Result<(), ConPtyError> {
        Err(ConPtyError::Unsupported)
    }

    pub fn peek(&mut self) -> Result<bool, ConPtyError> {
        Err(ConPtyError::Unsupported)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ConPtyError> {
        if rows == 0 || cols == 0 {
            return Err(ConPtyError::InvalidDimensions);
        }

        Err(ConPtyError::Unsupported)
    }

    pub fn shutdown(self) -> Result<(), ConPtyError> {
        Ok(())
    }

    pub fn is_conpty_available() -> bool {
        false
    }
}

impl From<io::Error> for ConPtyError {
    fn from(_error: io::Error) -> Self {
        Self::Unsupported
    }
}
