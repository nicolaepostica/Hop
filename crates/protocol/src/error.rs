//! Errors produced by the codec layer.

use std::io;

use thiserror::Error;

use crate::version::MAX_FRAME_BYTES;

/// Errors produced while encoding or decoding a [`Message`](crate::Message).
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Underlying I/O error from the transport.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// CBOR deserialization failed (peer sent a malformed or unsupported
    /// payload).
    #[error("CBOR decode failed: {0}")]
    Decode(String),

    /// CBOR serialization failed (programmer error: a value produced by
    /// this process could not be serialized).
    #[error("CBOR encode failed: {0}")]
    Encode(String),

    /// A frame exceeded the configured [`MAX_FRAME_BYTES`] limit.
    #[error("frame exceeds max size: {size} > {MAX_FRAME_BYTES}")]
    FrameTooLarge {
        /// Size of the rejected frame.
        size: usize,
    },
}

impl From<ciborium::de::Error<io::Error>> for ProtocolError {
    fn from(err: ciborium::de::Error<io::Error>) -> Self {
        Self::Decode(err.to_string())
    }
}

impl From<ciborium::ser::Error<io::Error>> for ProtocolError {
    fn from(err: ciborium::ser::Error<io::Error>) -> Self {
        Self::Encode(err.to_string())
    }
}
