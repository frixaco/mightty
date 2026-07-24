use crate::ghostty::ffi;

/// Errors returned by the local Ghostty integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    OutOfMemory,
    InvalidValue,
    OutOfSpace { required: usize },
    UnexpectedResult(i32),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfMemory => formatter.write_str("Ghostty ran out of memory"),
            Self::InvalidValue => formatter.write_str("Ghostty rejected an invalid value"),
            Self::OutOfSpace { required } => {
                write!(formatter, "Ghostty needs a {required}-byte output buffer")
            }
            Self::UnexpectedResult(code) => {
                write!(formatter, "Ghostty returned unknown result code {code}")
            }
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn from_result(code: ffi::Result::Type) -> Result<()> {
    match code {
        ffi::Result::SUCCESS => Ok(()),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::INVALID_VALUE | ffi::Result::NO_VALUE => Err(Error::InvalidValue),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: 0 }),
        other => Err(Error::UnexpectedResult(other)),
    }
}

pub(crate) fn from_result_with_len(code: ffi::Result::Type, len: usize) -> Result<usize> {
    match code {
        ffi::Result::SUCCESS => Ok(len),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::INVALID_VALUE | ffi::Result::NO_VALUE => Err(Error::InvalidValue),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: len }),
        other => Err(Error::UnexpectedResult(other)),
    }
}
