//! Local IPC between the Input Leap daemon and its GUI.
//!
//! Transport: newline-delimited JSON over a Unix domain socket
//! (Linux/macOS) or a Named Pipe (Windows), wrapped by the
//! [`interprocess`] crate's tokio integration so the daemon can serve
//! many clients from one runtime.
//!
//! Schema is a trimmed JSON-RPC:
//!
//! ```text
//! request    { "id": <u64>, "method": "<name>", "params": {...} }
//! response   { "id": <u64>, "result": {...} }
//! error      { "id": <u64>, "error": { "code": <int>, "message": "..." } }
//! notify     { "method": "<name>", "params": {...} }   // no id
//! ```
//!
//! Methods for M5: `get_status`, `add_peer_fingerprint`,
//! `remove_peer`. Log streaming and `reload_config` are follow-ups.

pub mod client;
pub mod codec;
pub mod protocol;
pub mod server;

pub use self::client::{IpcClient, IpcClientError};
pub use self::codec::{LineJsonCodec, LineJsonError};
pub use self::protocol::{
    ErrorPayload, IpcError, IpcMessage, IpcRequest, IpcResponse, RequestId, StatusReply,
};
pub use self::server::{default_socket_path, IpcHandler, IpcServer, IpcServerError};
