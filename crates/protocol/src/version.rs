//! Protocol versioning and size limits.

/// Current wire protocol version.
///
/// Bumped only when we need to break the CBOR schema in a way that
/// cannot be expressed by adding fields with defaults or new variants.
/// Peers compare this value during the handshake and refuse to proceed
/// on mismatch.
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum size of a single framed message in bytes.
///
/// The length-delimited codec rejects frames larger than this before
/// allocating a buffer, protecting against memory-exhaustion attacks
/// on malformed or malicious peers. 16 MiB comfortably covers
/// clipboard payloads; file transfers use a separate chunked protocol.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
