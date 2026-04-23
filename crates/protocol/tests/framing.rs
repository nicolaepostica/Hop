//! Framing edge cases for `MessageCodec`.
//!
//! Covers the interesting states of the decoder: partial frames, empty
//! buffers, oversized frames advertised in the length header, and valid
//! frames with garbage payloads. Happy-path round-trip is covered by
//! `tests/roundtrip.rs`.

use bytes::BytesMut;
use hop_protocol::{Message, MessageCodec, ProtocolError, MAX_FRAME_BYTES};
use tokio_util::codec::{Decoder, Encoder};

fn encode(message: Message) -> BytesMut {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::new();
    codec.encode(message, &mut buf).expect("encode");
    buf
}

#[test]
fn empty_buffer_yields_none() {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::new();
    assert!(matches!(codec.decode(&mut buf), Ok(None)));
}

#[test]
fn truncated_header_yields_none() {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::from(&[0x00, 0x00, 0x00][..]); // 3 of 4 header bytes
    assert!(matches!(codec.decode(&mut buf), Ok(None)));
    // Partial header is retained for the next decode call.
    assert_eq!(buf.len(), 3);
}

#[test]
fn truncated_body_yields_none_then_completes() {
    let full = encode(Message::KeepAlive);
    assert!(full.len() > 4, "keep-alive frame is non-trivial");
    // Feed everything except the final byte: header says "N bytes" but
    // only N-1 are present. The decoder must not error, just wait.
    let mut buf = BytesMut::from(&full[..full.len() - 1]);
    let mut codec = MessageCodec::new();
    assert!(matches!(codec.decode(&mut buf), Ok(None)));
    // Feed the missing byte; now the frame decodes.
    buf.extend_from_slice(&full[full.len() - 1..]);
    let msg = codec.decode(&mut buf).unwrap().expect("frame now complete");
    assert_eq!(msg, Message::KeepAlive);
    assert!(buf.is_empty());
}

#[test]
fn two_frames_in_one_buffer_decode_independently() {
    let mut buf = BytesMut::new();
    let mut codec = MessageCodec::new();
    codec.encode(Message::KeepAlive, &mut buf).unwrap();
    codec.encode(Message::ScreenLeave, &mut buf).unwrap();

    let first = codec.decode(&mut buf).unwrap().expect("first");
    assert_eq!(first, Message::KeepAlive);
    let second = codec.decode(&mut buf).unwrap().expect("second");
    assert_eq!(second, Message::ScreenLeave);
    assert!(buf.is_empty());
}

#[test]
fn oversized_frame_header_errors() {
    // Header announces a frame larger than MAX_FRAME_BYTES. Decoder
    // must reject before attempting to allocate that much.
    let oversized = u32::try_from(MAX_FRAME_BYTES + 1).expect("fits in u32");
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&oversized.to_be_bytes());
    let mut codec = MessageCodec::new();
    match codec.decode(&mut buf) {
        Err(ProtocolError::FrameTooLarge { size }) => {
            assert_eq!(size, oversized as usize);
        }
        other => panic!("expected FrameTooLarge, got {other:?}"),
    }
}

#[test]
fn malformed_cbor_body_errors() {
    // Valid frame header (length = 3) followed by CBOR bytes that do
    // not decode into a Message. 0xff is "break" which is not a valid
    // top-level item here.
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&3u32.to_be_bytes());
    buf.extend_from_slice(&[0xff, 0xff, 0xff]);
    let mut codec = MessageCodec::new();
    match codec.decode(&mut buf) {
        Err(ProtocolError::Decode(_)) => {}
        other => panic!("expected Decode, got {other:?}"),
    }
}
