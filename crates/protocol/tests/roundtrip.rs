//! Round-trip property tests: encode any `Message`, decode it, assert equal.
//!
//! Covers both raw CBOR (ciborium only) and the full framed pipeline
//! (`MessageCodec` over `BytesMut`).

mod fixtures;

use bytes::BytesMut;
use input_leap_protocol::{Message, MessageCodec};
use proptest::prelude::*;
use tokio_util::codec::{Decoder, Encoder};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Pure CBOR round-trip: serialize, deserialize, compare.
    #[test]
    fn cbor_roundtrip(message in fixtures::arb_message()) {
        let mut buf = Vec::<u8>::new();
        ciborium::into_writer(&message, &mut buf).expect("encode");
        let decoded: Message =
            ciborium::from_reader(std::io::Cursor::new(&buf)).expect("decode");
        prop_assert_eq!(message, decoded);
    }

    /// Full framed round-trip through `MessageCodec`.
    #[test]
    fn codec_roundtrip(message in fixtures::arb_message()) {
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::new();
        codec.encode(message.clone(), &mut buf).expect("encode");
        let decoded = codec
            .decode(&mut buf)
            .expect("decode")
            .expect("one complete frame");
        prop_assert!(
            buf.is_empty(),
            "codec must consume exactly one frame (leftover: {} bytes)",
            buf.len()
        );
        prop_assert_eq!(message, decoded);
    }

    /// Two messages encoded into a shared buffer must decode back to both.
    #[test]
    fn codec_pipelined(a in fixtures::arb_message(), b in fixtures::arb_message()) {
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::new();
        codec.encode(a.clone(), &mut buf).unwrap();
        codec.encode(b.clone(), &mut buf).unwrap();

        let out_a = codec.decode(&mut buf).unwrap().expect("first frame");
        let out_b = codec.decode(&mut buf).unwrap().expect("second frame");
        prop_assert!(buf.is_empty());
        prop_assert_eq!(a, out_a);
        prop_assert_eq!(b, out_b);
    }
}
