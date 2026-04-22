//! Framed CBOR codec for [`Message`](crate::Message).

use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::error::ProtocolError;
use crate::message::Message;
use crate::version::MAX_FRAME_BYTES;

/// Size of the length prefix prepended to each frame, in bytes.
const HEADER_LEN: usize = 4;

/// A [`Decoder`]/[`Encoder`] pair that turns raw bytes into [`Message`]
/// values and back.
///
/// Each frame is a 4-byte big-endian length prefix followed by the
/// CBOR-serialized body. Frames larger than [`MAX_FRAME_BYTES`] are
/// rejected up front — before allocation — so a malicious peer cannot
/// force us to reserve arbitrary memory.
#[derive(Debug, Default)]
pub struct MessageCodec {
    // Held for future use (e.g. partial-body statistics); the codec is
    // otherwise stateless.
    _private: (),
}

impl MessageCodec {
    /// Constructs a new codec.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }
        let size = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if size > MAX_FRAME_BYTES {
            // The peer is trying to make us allocate more than we're
            // willing to; drop the whole buffer so the stream can
            // resynchronize (in practice we will close the connection).
            src.advance(src.len());
            return Err(ProtocolError::FrameTooLarge { size });
        }
        if src.len() < HEADER_LEN + size {
            // Hint the underlying buffer at the allocation we will
            // eventually need, so partial reads do not keep growing it
            // in tiny increments.
            src.reserve(HEADER_LEN + size - src.len());
            return Ok(None);
        }
        src.advance(HEADER_LEN);
        let body = src.split_to(size).freeze();
        let message: Message = ciborium::from_reader(body.reader())?;
        Ok(Some(message))
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // Serialize into a scratch buffer first so we can reject
        // oversized messages before touching `dst`.
        let mut body: Vec<u8> = Vec::with_capacity(64);
        ciborium::into_writer(&item, &mut body)?;
        if body.len() > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge { size: body.len() });
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "checked against MAX_FRAME_BYTES"
        )]
        let size = body.len() as u32;
        dst.reserve(HEADER_LEN + body.len());
        dst.put_u32(size);
        dst.extend_from_slice(&body);
        Ok(())
    }
}

/// Convenience wrapper: pair any async byte stream with a fresh
/// [`MessageCodec`], producing a [`Framed`] sink/stream over [`Message`].
pub fn framed<T>(io: T) -> Framed<T, MessageCodec>
where
    T: AsyncRead + AsyncWrite,
{
    Framed::new(io, MessageCodec::new())
}
