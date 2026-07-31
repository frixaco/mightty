//! Application lifecycle and process-activation models.

pub mod activation;
#[cfg(windows)]
pub mod windows;

pub use activation::{
    ACTIVATION_FRAME_HEADER_BYTES, ACTIVATION_PROTOCOL_VERSION, ActivationFrameHeader,
    ActivationProtocolError, ActivationRequest, MAX_ACTIVATION_PAYLOAD_BYTES, decode_frame,
    decode_frame_header, decode_request_payload, encode_frame,
};
