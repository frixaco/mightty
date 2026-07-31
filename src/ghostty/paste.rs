use crate::ghostty::error::from_result_with_len;
use crate::ghostty::{Error, Result, ffi};

/// Return whether Ghostty considers the bytes safe for immediate paste.
pub fn is_safe(data: &[u8]) -> bool {
    unsafe { ffi::ghostty_paste_is_safe(data.as_ptr().cast(), data.len()) }
}

/// Encode clipboard bytes for the current terminal paste mode.
pub fn encode(data: &[u8], bracketed: bool) -> Result<Vec<u8>> {
    let capacity = data.len().checked_add(12).ok_or(Error::InvalidValue)?;
    let mut input = data.to_vec();
    let mut output = vec![0_u8; capacity];

    let written = encode_into(&mut input, bracketed, &mut output)?;
    output.truncate(written);
    Ok(output)
}

fn encode_into(input: &mut [u8], bracketed: bool, output: &mut [u8]) -> Result<usize> {
    let mut written = 0;
    let result = unsafe {
        ffi::ghostty_paste_encode(
            input.as_mut_ptr().cast(),
            input.len(),
            bracketed,
            output.as_mut_ptr().cast(),
            output.len(),
            &raw mut written,
        )
    };
    let written = from_result_with_len(result, written)?;
    if written > output.len() {
        return Err(Error::InvalidValue);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_command_injection_boundaries() {
        assert!(is_safe(b"cargo test"));
        assert!(!is_safe(b"cargo test\nwhoami"));
        assert!(!is_safe(b"safe\x1b[201~unsafe"));
    }

    #[test]
    fn encodes_plain_and_bracketed_paste() {
        assert_eq!(encode(b"one\ntwo", false).unwrap(), b"one\rtwo");
        assert_eq!(
            encode(b"one\ntwo", true).unwrap(),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
    }

    #[test]
    fn strips_control_bytes_before_encoding() {
        assert_eq!(encode(b"a\0b\x1bc", false).unwrap(), b"a b c");
    }
}
