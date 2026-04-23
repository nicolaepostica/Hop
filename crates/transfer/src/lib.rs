// Intentionally allow a few pedantic lints — the engine has several
// state-machine match arms where "single_match_else" and
// "manual_let_else" rewrites hurt readability.
#![allow(
    clippy::single_match_else,
    clippy::manual_let_else,
    clippy::cast_possible_truncation
)]

//! File-clipboard transfer engine.
//!
//! Splits cleanly into:
//!
//! - [`TransferSender`] — walks a directory tree, emits
//!   [`Message::FileTransferStart`] / [`Message::FileChunk`] /
//!   [`Message::FileTransferEnd`].
//! - [`TransferReceiver`] — accepts those messages, writes into a
//!   private staging directory, validates every relative path against
//!   traversal, and finalises atomically into the drop directory.
//!
//! Both sides work over any async channel that carries
//! [`Message`](hop_protocol::Message) — in production that's
//! the framed TLS connection, in tests it's a `tokio::sync::mpsc`.

pub mod error;
pub mod path_guard;
pub mod receiver;
pub mod sender;

pub use self::error::TransferError;
pub use self::path_guard::validate_rel_path;
pub use self::receiver::{ReceivedFile, TransferReceiver};
pub use self::sender::{TransferSender, DEFAULT_CHUNK_BYTES};
