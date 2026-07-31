//! Versioned activation requests for one running mightty application.
//!
//! The transport reads and validates the fixed header before it allocates the
//! payload. This module does not own named pipes, access control, or dispatch.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::action::AppAction;
use crate::profile::ProfileId;

pub const ACTIVATION_PROTOCOL_VERSION: u16 = 1;
pub const ACTIVATION_FRAME_HEADER_BYTES: usize = 12;
pub const MAX_ACTIVATION_PAYLOAD_BYTES: usize = 16 * 1024;

const ACTIVATION_FRAME_MAGIC: [u8; 4] = *b"MTTY";
const RESERVED_HEADER_FLAGS: u16 = 0;

/// One request sent from a secondary process to the primary process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationRequest {
    /// Show and focus the normal application window.
    Activate,
    /// Open a normal terminal tab with the selected profile.
    OpenProfile { profile_id: ProfileId },
    /// Show the quick terminal, optionally with a selected profile.
    OpenQuickTerminal { profile_id: Option<ProfileId> },
    /// Dispatch the same typed action used by shortcuts, menus, and the palette.
    Dispatch { action: AppAction },
}

/// A validated frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationFrameHeader {
    payload_len: usize,
}

impl ActivationFrameHeader {
    pub const fn payload_len(self) -> usize {
        self.payload_len
    }
}

/// Encode one request as a complete protocol frame.
pub fn encode_frame(request: &ActivationRequest) -> Result<Vec<u8>, ActivationProtocolError> {
    let payload = serde_json::to_vec(request)
        .map_err(|error| ActivationProtocolError::InvalidPayload(error.to_string()))?;
    validate_payload_size(payload.len())?;

    let mut frame = Vec::with_capacity(ACTIVATION_FRAME_HEADER_BYTES + payload.len());
    frame.extend_from_slice(&ACTIVATION_FRAME_MAGIC);
    frame.extend_from_slice(&ACTIVATION_PROTOCOL_VERSION.to_le_bytes());
    frame.extend_from_slice(&RESERVED_HEADER_FLAGS.to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode one complete frame.
pub fn decode_frame(frame: &[u8]) -> Result<ActivationRequest, ActivationProtocolError> {
    if frame.len() < ACTIVATION_FRAME_HEADER_BYTES {
        return Err(ActivationProtocolError::InvalidHeaderLength {
            actual: frame.len(),
        });
    }

    let header = decode_frame_header(&frame[..ACTIVATION_FRAME_HEADER_BYTES])?;
    decode_request_payload(header, &frame[ACTIVATION_FRAME_HEADER_BYTES..])
}

/// Validate a fixed header before the transport reads its payload.
pub fn decode_frame_header(bytes: &[u8]) -> Result<ActivationFrameHeader, ActivationProtocolError> {
    if bytes.len() != ACTIVATION_FRAME_HEADER_BYTES {
        return Err(ActivationProtocolError::InvalidHeaderLength {
            actual: bytes.len(),
        });
    }
    if bytes[..4] != ACTIVATION_FRAME_MAGIC {
        return Err(ActivationProtocolError::InvalidMagic);
    }

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != ACTIVATION_PROTOCOL_VERSION {
        return Err(ActivationProtocolError::UnsupportedVersion { version });
    }

    let reserved = u16::from_le_bytes([bytes[6], bytes[7]]);
    if reserved != RESERVED_HEADER_FLAGS {
        return Err(ActivationProtocolError::UnsupportedHeaderFlags { flags: reserved });
    }

    let payload_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    validate_payload_size(payload_len)?;
    Ok(ActivationFrameHeader { payload_len })
}

/// Decode the exact payload declared by a validated header.
pub fn decode_request_payload(
    header: ActivationFrameHeader,
    payload: &[u8],
) -> Result<ActivationRequest, ActivationProtocolError> {
    if payload.len() != header.payload_len {
        return Err(ActivationProtocolError::PayloadLengthMismatch {
            declared: header.payload_len,
            actual: payload.len(),
        });
    }
    if payload.is_empty() {
        return Err(ActivationProtocolError::EmptyPayload);
    }

    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| ActivationProtocolError::InvalidPayload(error.to_string()))?;
    let request: ActivationRequest = serde_json::from_value(value.clone())
        .map_err(|error| ActivationProtocolError::InvalidPayload(error.to_string()))?;
    let canonical = serde_json::to_value(&request)
        .map_err(|error| ActivationProtocolError::InvalidPayload(error.to_string()))?;
    if value != canonical {
        return Err(ActivationProtocolError::InvalidPayload(
            "payload contains unknown or omitted fields".to_string(),
        ));
    }
    Ok(request)
}

fn validate_payload_size(size: usize) -> Result<(), ActivationProtocolError> {
    if size > MAX_ACTIVATION_PAYLOAD_BYTES {
        return Err(ActivationProtocolError::PayloadTooLarge {
            size,
            maximum: MAX_ACTIVATION_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationProtocolError {
    InvalidHeaderLength { actual: usize },
    InvalidMagic,
    UnsupportedVersion { version: u16 },
    UnsupportedHeaderFlags { flags: u16 },
    PayloadTooLarge { size: usize, maximum: usize },
    PayloadLengthMismatch { declared: usize, actual: usize },
    EmptyPayload,
    InvalidPayload(String),
}

impl fmt::Display for ActivationProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeaderLength { actual } => write!(
                formatter,
                "activation frame header must be {ACTIVATION_FRAME_HEADER_BYTES} bytes, got {actual}"
            ),
            Self::InvalidMagic => formatter.write_str("activation frame has invalid magic bytes"),
            Self::UnsupportedVersion { version } => {
                write!(
                    formatter,
                    "activation protocol version {version} is unsupported"
                )
            }
            Self::UnsupportedHeaderFlags { flags } => {
                write!(
                    formatter,
                    "activation frame header flags {flags:#06x} are unsupported"
                )
            }
            Self::PayloadTooLarge { size, maximum } => {
                write!(
                    formatter,
                    "activation payload is {size} bytes; maximum is {maximum}"
                )
            }
            Self::PayloadLengthMismatch { declared, actual } => write!(
                formatter,
                "activation payload declares {declared} bytes, got {actual}"
            ),
            Self::EmptyPayload => formatter.write_str("activation payload is empty"),
            Self::InvalidPayload(error) => {
                write!(formatter, "activation payload is invalid: {error}")
            }
        }
    }
}

impl std::error::Error for ActivationProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_typed_requests() {
        let profile_id = ProfileId::new("powershell").unwrap();
        let requests = [
            ActivationRequest::Activate,
            ActivationRequest::OpenProfile {
                profile_id: profile_id.clone(),
            },
            ActivationRequest::OpenQuickTerminal {
                profile_id: Some(profile_id.clone()),
            },
            ActivationRequest::Dispatch {
                action: AppAction::NewTab {
                    profile_id: Some(profile_id),
                },
            },
        ];

        for request in requests {
            let frame = encode_frame(&request).unwrap();
            assert_eq!(decode_frame(&frame).unwrap(), request);
        }
    }

    #[test]
    fn supports_bounded_transport_reads() {
        let frame = encode_frame(&ActivationRequest::Activate).unwrap();
        let header = decode_frame_header(&frame[..ACTIVATION_FRAME_HEADER_BYTES]).unwrap();
        let payload = &frame[ACTIVATION_FRAME_HEADER_BYTES..];

        assert_eq!(header.payload_len(), payload.len());
        assert_eq!(
            decode_request_payload(header, payload).unwrap(),
            ActivationRequest::Activate
        );
    }

    #[test]
    fn rejects_invalid_header_fields() {
        let frame = encode_frame(&ActivationRequest::Activate).unwrap();

        let mut invalid_magic = frame.clone();
        invalid_magic[0] = b'X';
        assert_eq!(
            decode_frame(&invalid_magic),
            Err(ActivationProtocolError::InvalidMagic)
        );

        let mut unsupported_version = frame.clone();
        unsupported_version[4..6].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            decode_frame(&unsupported_version),
            Err(ActivationProtocolError::UnsupportedVersion { version: 2 })
        );

        let mut unsupported_flags = frame;
        unsupported_flags[6..8].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            decode_frame(&unsupported_flags),
            Err(ActivationProtocolError::UnsupportedHeaderFlags { flags: 1 })
        );
    }

    #[test]
    fn rejects_payload_sizes_before_decoding() {
        let frame = encode_frame(&ActivationRequest::Activate).unwrap();
        let mut oversized_header = frame[..ACTIVATION_FRAME_HEADER_BYTES].to_vec();
        oversized_header[8..12]
            .copy_from_slice(&((MAX_ACTIVATION_PAYLOAD_BYTES + 1) as u32).to_le_bytes());

        assert_eq!(
            decode_frame_header(&oversized_header),
            Err(ActivationProtocolError::PayloadTooLarge {
                size: MAX_ACTIVATION_PAYLOAD_BYTES + 1,
                maximum: MAX_ACTIVATION_PAYLOAD_BYTES,
            })
        );

        let header = decode_frame_header(&frame[..ACTIVATION_FRAME_HEADER_BYTES]).unwrap();
        assert_eq!(
            decode_request_payload(header, b""),
            Err(ActivationProtocolError::PayloadLengthMismatch {
                declared: frame.len() - ACTIVATION_FRAME_HEADER_BYTES,
                actual: 0,
            })
        );
    }

    #[test]
    fn rejects_truncated_empty_or_extra_frame_data() {
        assert_eq!(
            decode_frame(&[0; ACTIVATION_FRAME_HEADER_BYTES - 1]),
            Err(ActivationProtocolError::InvalidHeaderLength {
                actual: ACTIVATION_FRAME_HEADER_BYTES - 1,
            })
        );
        assert_eq!(
            decode_frame(&raw_frame(b"")),
            Err(ActivationProtocolError::EmptyPayload)
        );

        let mut frame = encode_frame(&ActivationRequest::Activate).unwrap();
        let declared = frame.len() - ACTIVATION_FRAME_HEADER_BYTES;
        frame.push(b' ');
        assert_eq!(
            decode_frame(&frame),
            Err(ActivationProtocolError::PayloadLengthMismatch {
                declared,
                actual: declared + 1,
            })
        );
    }

    #[test]
    fn rejects_unknown_or_invalid_request_data() {
        assert_invalid_payload(br#"{"type":"activate","unexpected":true}"#);
        assert_invalid_payload(br#"{"type":"open_profile","profile_id":"PowerShell 7"}"#);
        assert_invalid_payload(
            br#"{"type":"dispatch","action":{"type":"copy","unexpected":true}}"#,
        );
        assert_invalid_payload(br#"{"type":"unknown"}"#);
    }

    fn assert_invalid_payload(payload: &[u8]) {
        let frame = raw_frame(payload);
        assert!(matches!(
            decode_frame(&frame),
            Err(ActivationProtocolError::InvalidPayload(_))
        ));
    }

    fn raw_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(ACTIVATION_FRAME_HEADER_BYTES + payload.len());
        frame.extend_from_slice(&ACTIVATION_FRAME_MAGIC);
        frame.extend_from_slice(&ACTIVATION_PROTOCOL_VERSION.to_le_bytes());
        frame.extend_from_slice(&RESERVED_HEADER_FLAGS.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }
}
