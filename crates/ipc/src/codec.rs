//! Newline-delimited JSON codec for `IpcMessage`.
//!
//! Keeps the wire format debuggable with `nc` / `socat` so an operator
//! can poke at a live daemon without the GUI.

use std::io;

use bytes::BytesMut;
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};

use crate::protocol::IpcMessage;

/// Maximum bytes we'll buffer before declaring the peer abusive.
///
/// 1 MiB is orders of magnitude more than any legitimate JSON-RPC
/// request we expect — status replies are sub-kilobyte — but leaves
/// room for future structured log notifications.
pub const MAX_LINE_LEN: usize = 1024 * 1024;

/// Encoder/decoder pair for newline-delimited JSON IPC frames.
#[derive(Debug, Default)]
pub struct LineJsonCodec {
    _private: (),
}

impl LineJsonCodec {
    /// Construct a fresh codec.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Decoder for LineJsonCodec {
    type Item = IpcMessage;
    type Error = LineJsonError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(newline_pos) = src.iter().position(|b| *b == b'\n') else {
            if src.len() > MAX_LINE_LEN {
                return Err(LineJsonError::LineTooLong { size: src.len() });
            }
            return Ok(None);
        };

        let line = src.split_to(newline_pos + 1);
        // Drop the newline (and an optional \r before it) from the
        // slice we hand to serde.
        let trimmed = strip_line_ending(&line);
        if trimmed.is_empty() {
            // Blank keep-alive line; skip.
            return self.decode(src);
        }
        let msg: IpcMessage = serde_json::from_slice(trimmed)?;
        Ok(Some(msg))
    }
}

impl Encoder<IpcMessage> for LineJsonCodec {
    type Error = LineJsonError;

    fn encode(&mut self, item: IpcMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = serde_json::to_vec(&item)?;
        if bytes.len() + 1 > MAX_LINE_LEN {
            return Err(LineJsonError::LineTooLong { size: bytes.len() });
        }
        dst.reserve(bytes.len() + 1);
        dst.extend_from_slice(&bytes);
        dst.extend_from_slice(b"\n");
        Ok(())
    }
}

fn strip_line_ending(line: &[u8]) -> &[u8] {
    let end = line.len();
    let without_lf = if end > 0 && line[end - 1] == b'\n' {
        end - 1
    } else {
        end
    };
    let without_cr = if without_lf > 0 && line[without_lf - 1] == b'\r' {
        without_lf - 1
    } else {
        without_lf
    };
    &line[..without_cr]
}

/// Codec errors.
#[derive(Debug, Error)]
pub enum LineJsonError {
    /// Underlying transport I/O error.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// Peer sent malformed JSON.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A single line exceeded [`MAX_LINE_LEN`].
    #[error("line too long: {size} bytes")]
    LineTooLong {
        /// Size observed when the check tripped.
        size: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{IpcRequest, RequestPayload};

    #[test]
    fn encode_decode_single_message() {
        let mut codec = LineJsonCodec::new();
        let req = IpcMessage::Request(IpcRequest {
            jsonrpc: crate::protocol::JsonRpcVersion,
            id: 1,
            payload: RequestPayload::GetStatus,
        });
        let mut buf = BytesMut::new();
        codec.encode(req.clone(), &mut buf).unwrap();
        assert!(buf.ends_with(b"\n"));
        let decoded = codec.decode(&mut buf).unwrap().expect("frame");
        assert_eq!(req, decoded);
    }

    #[test]
    fn two_messages_in_one_buffer() {
        let mut codec = LineJsonCodec::new();
        let mut buf = BytesMut::new();
        codec
            .encode(
                IpcMessage::Request(IpcRequest {
                    jsonrpc: crate::protocol::JsonRpcVersion,
                    id: 1,
                    payload: RequestPayload::GetStatus,
                }),
                &mut buf,
            )
            .unwrap();
        codec
            .encode(
                IpcMessage::Request(IpcRequest {
                    jsonrpc: crate::protocol::JsonRpcVersion,
                    id: 2,
                    payload: RequestPayload::RemovePeer {
                        name: "laptop".into(),
                    },
                }),
                &mut buf,
            )
            .unwrap();
        let a = codec.decode(&mut buf).unwrap().unwrap();
        let b = codec.decode(&mut buf).unwrap().unwrap();
        let IpcMessage::Request(r) = a else {
            panic!("expected request")
        };
        assert_eq!(r.id, 1);
        let IpcMessage::Request(r) = b else {
            panic!("expected request")
        };
        assert_eq!(r.id, 2);
    }

    #[test]
    fn partial_line_yields_none() {
        let mut codec = LineJsonCodec::new();
        let mut buf = BytesMut::from(&b"{\"id\":1"[..]);
        assert!(matches!(codec.decode(&mut buf), Ok(None)));
    }

    #[test]
    fn malformed_json_errors() {
        let mut codec = LineJsonCodec::new();
        let mut buf = BytesMut::from(&b"not json\n"[..]);
        match codec.decode(&mut buf) {
            Err(LineJsonError::Json(_)) => {}
            other => panic!("expected Json error, got {other:?}"),
        }
    }

    #[test]
    fn tolerates_crlf_endings() {
        let mut codec = LineJsonCodec::new();
        let mut buf = BytesMut::from(
            &br#"{"id":1,"method":"get_status","params":null}
"#[..],
        );
        // Replace the \n with \r\n.
        let mut with_crlf = BytesMut::new();
        for byte in &buf {
            if *byte == b'\n' {
                with_crlf.extend_from_slice(b"\r\n");
            } else {
                with_crlf.extend_from_slice(&[*byte]);
            }
        }
        buf = with_crlf;
        let decoded = codec.decode(&mut buf).unwrap().expect("frame");
        let IpcMessage::Request(r) = decoded else {
            panic!("expected request")
        };
        assert_eq!(r.id, 1);
    }
}
