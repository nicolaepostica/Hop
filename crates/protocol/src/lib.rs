//! Input Leap wire protocol v1.
//!
//! Messages are serialized with CBOR (RFC 8949) and framed using a
//! length-delimited codec (4-byte big-endian length prefix).
//!
//! Implementation lands in [M1](../../../specs/milestones/M1-protocol.md).
