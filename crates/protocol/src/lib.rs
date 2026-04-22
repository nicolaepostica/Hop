//! Input Leap wire protocol v1.
//!
//! Messages are serialized with CBOR (RFC 8949) and framed with a
//! length-delimited codec: a 4-byte big-endian length prefix followed
//! by the serialized [`Message`] body. The maximum framed size is
//! [`MAX_FRAME_BYTES`].
//!
//! # Example
//!
//! ```no_run
//! use futures::{SinkExt, StreamExt};
//! use input_leap_protocol::{framed, Capability, HelloPayload, Message};
//!
//! # async fn demo(stream: tokio::net::TcpStream) -> Result<(), Box<dyn std::error::Error>> {
//! let mut conn = framed(stream);
//! conn.send(Message::Hello(HelloPayload {
//!     protocol_version: input_leap_protocol::PROTOCOL_VERSION,
//!     display_name: "laptop".into(),
//!     capabilities: vec![Capability::UnicodeClipboard],
//! })).await?;
//! let reply = conn.next().await.transpose()?;
//! # drop(reply);
//! # Ok(()) }
//! ```

mod codec;
mod error;
mod message;
mod version;

pub use self::codec::{framed, MessageCodec};
pub use self::error::ProtocolError;
pub use self::message::{Capability, DeviceInfoPayload, DisconnectReason, HelloPayload, Message};
pub use self::version::{MAX_FRAME_BYTES, PROTOCOL_VERSION};
