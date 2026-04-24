# M1 — `protocol` crate: CBOR messages v1, codec, tests

## Goal

Land the full Hop v1 wire message schema on serde+CBOR with length-delimited framing. Once M1 is done, any crate can import `hop_protocol::{Message, Codec, ProtocolError}` and work with serialised messages over an arbitrary `AsyncRead` / `AsyncWrite` — even without a real network.

## Prerequisites

- [M0](M0-skeleton.md) — workspace skeleton

## Scope

**In scope:**
- Message types `Message` (enum) with every variant listed in the main spec
- Supporting types: `HelloPayload`, `DeviceInfoPayload`, `Capability`, `DisconnectReason`, `KeyId`, `ButtonId`, `ClipboardId`, `ClipboardFormat`, `ModifierMask`
- CBOR serialise / deserialise via `ciborium`
- Framing: `tokio_util::codec::LengthDelimitedCodec`, max frame 16 MiB
- `Encoder` / `Decoder` types composed via `FramedWrite` / `FramedRead`
- `ProtocolError` via `thiserror`
- Property tests (`proptest`): round-trip every `Message` variant
- Golden snapshot tests (`insta`): hex dumps of canonical bytes for each variant (they document the wire format)
- Module-level `//!` documentation with an encode/decode example

**Out of scope:**
- File-clipboard messages (M9)
- Networking (M2) — `protocol` runs over any `AsyncRead` / `AsyncWrite`
- TLS (M2)
- Handshake state machine (M2)

## Tasks

### Message types

- [ ] `crates/common/src/ids.rs`:
  - `KeyId(u32)`, `ButtonId(u8)`, `ClipboardId(u8)` as newtype wrappers
  - `ModifierMask(u32)` as bitflags via the `bitflags` crate
  - `ClipboardFormat` enum
  - Derive `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`
- [ ] `crates/protocol/src/message.rs`:
  - `enum Message` with `#[serde(tag = "type")]` and every variant from the main spec
  - `HelloPayload { protocol_version, display_name, capabilities }`
  - `DeviceInfoPayload { width, height, scale_factor, ... }`
  - `enum Capability` with `#[serde(rename_all = "snake_case")]` and `#[serde(other)] Unknown` for forward-compat
  - `enum DisconnectReason { ProtocolVersionMismatch, KeepAliveTimeout, UnknownPeer, MalformedMessage, FrameTooLarge, UserInitiated, InternalError, ... }`
- [ ] `crates/protocol/src/version.rs`:
  - `pub const PROTOCOL_VERSION: u16 = 1;`
  - `pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;`

### Codec

- [ ] `crates/protocol/src/codec.rs`:
  - `struct MessageCodec` wrapping a `LengthDelimitedCodec`
  - `impl Encoder<Message> for MessageCodec` — ciborium into a `BytesMut`
  - `impl Decoder for MessageCodec { type Item = Message; }` — read a frame, then ciborium from the slice
  - Errors map to `ProtocolError`
- [ ] Helper: `pub fn framed<T: AsyncRead + AsyncWrite>(io: T) -> Framed<T, MessageCodec>`

### Error handling

- [ ] `crates/protocol/src/error.rs`:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum ProtocolError {
      #[error("io error: {0}")]
      Io(#[from] std::io::Error),
      #[error("CBOR decode failed: {0}")]
      Decode(#[from] ciborium::de::Error<std::io::Error>),
      #[error("CBOR encode failed: {0}")]
      Encode(#[from] ciborium::ser::Error<std::io::Error>),
      #[error("frame exceeds max size: {size} > {MAX_FRAME_BYTES}")]
      FrameTooLarge { size: usize },
  }
  ```

### Tests

- [ ] `crates/protocol/tests/roundtrip.rs`:
  - `proptest!` per `Message` variant — random instance, serialise, deserialise, compare (`assert_eq!`)
  - `Arbitrary` impls via `proptest-derive` where possible; manual for types with invariants (e.g. `ModifierMask` — only valid bits)
- [ ] `crates/protocol/tests/snapshots.rs`:
  - For each `Message` variant — canonical instance → serialise → `insta::assert_snapshot!(hex_dump)`
  - Snapshots committed to the repo — they document the wire format and catch unauthorised schema changes
- [ ] `crates/protocol/tests/framing.rs`:
  - Two `Message`s written to a `Vec<u8>` via `FramedWrite`, read back via `FramedRead` — same values
  - Truncated frame → decoder returns `Ok(None)` (need more bytes), not an error
  - Frame with length > `MAX_FRAME_BYTES` → `ProtocolError::FrameTooLarge`
  - Correct length + broken CBOR inside → `ProtocolError::Decode`
- [ ] Fuzz target (optional, if time permits): `cargo-fuzz` against the decoder — arbitrary bytes must not panic

### Documentation

- [ ] `crates/protocol/src/lib.rs` — `//!` module-level docs with an example:
  ```rust
  //! # Example
  //! ```no_run
  //! # use tokio::net::TcpStream;
  //! # use hop_protocol::{framed, Message, HelloPayload, Capability};
  //! # async fn demo(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
  //! use futures::{SinkExt, StreamExt};
  //! let mut conn = framed(stream);
  //! conn.send(Message::Hello(HelloPayload {
  //!     protocol_version: 1,
  //!     display_name: "laptop".into(),
  //!     capabilities: vec![Capability::UnicodeClipboard],
  //! })).await?;
  //! let reply = conn.next().await.transpose()?;
  //! # Ok(()) }
  //! ```
- [ ] `docs/wire-format.md` (or under `specs/`? — decide during implementation) — human-readable wire-format description for future implementations in other languages

## Acceptance criteria

- [ ] Every `Message` variant has a property-test round-trip — 0 fails on 10k iterations
- [ ] Snapshot tests: one canonical instance per variant, committed under `crates/protocol/tests/snapshots/`
- [ ] `cargo bench` (optional but desirable) shows `KeepAlive` encode/decode < 1 µs and `MouseMove` < 5 µs on a modern CPU
- [ ] The crate's public API is documented: `cargo doc --no-deps` produces no missing-docs warnings
- [ ] CI is green; crate docs will render on docs.rs without warnings
- [ ] `cargo deny check` is green

## Tests

On top of the list above:
- [ ] Every `Capability` variant serialises to a known snake_case string (not a serde auto-detected name)
- [ ] Forward-compat: a `Hello` whose capabilities list contains an unknown string still deserialises — the unknown slot becomes `Capability::Unknown`, the rest parses normally
- [ ] `DisconnectReason` with an unknown variant — graceful fallback (e.g. `DisconnectReason::Unknown(String)`? — or `Disconnect { reason: Unknown }`; decide during implementation)

## Risks / open questions

1. **CBOR map vs array encoding.** `ciborium` encodes structs as maps by default (field names in the wire format). Pro: forward-compat via `#[serde(skip)]`. Con: more bytes on the wire (for `MouseMove` the difference is noticeable: 2 fields × 3-byte key + 2-byte value vs 4-byte `[x, y]` as an array). Decision for M1: **keep map encoding** — readable wire format beats a few bytes; at 1000 events/s the overhead is < 50 KB/s. If benchmarks in M3 show a problem, switch to a manual `serde::Serializer` with array encoding on the hot path.
2. **Length-prefix endianness.** `LengthDelimitedCodec` defaults to 4-byte big-endian. Confirm and document in `docs/wire-format.md`.
3. **`#[serde(other)]` for `Capability`:** does it require `#[serde(untagged)]` or a tag-free enum? Clarify during implementation; may need `#[serde(other)] Unknown(String)` or a custom `Deserialize`.
4. **Protobuf alternative?** Consciously rejected: CBOR needs neither codegen nor IDL; derive serialisation gives the same level of cross-language support. Don't revisit without a strong reason.
5. **Nested `Bytes` inside `ClipboardData`:** `bytes::Bytes` + serde needs the `bytes` crate's `serde` feature. Check in M0 that it's enabled.

## Readiness for M2

After M1 we have:
- Types and codec for every subsequent layer
- Confidence in the wire format via snapshot tests
- Fuzz-safe decoder

That's the minimum M2 needs (handshake over TLS).
