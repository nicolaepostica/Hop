//! JSON-RPC-ish wire types for the daemon IPC.

use serde::{Deserialize, Serialize};

/// Monotonic request identifier. Responses echo it back so a client
/// can multiplex several in-flight calls on the same connection.
pub type RequestId = u64;

/// Any message flowing over the IPC socket.
///
/// Serialized as untagged: the presence of `id`, `result`, or `error`
/// disambiguates. Notifications have neither `id` nor `result`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum IpcMessage {
    /// Client -> daemon.
    Request(IpcRequest),
    /// Daemon -> client, reply to a previous request.
    Response(IpcResponse),
    /// Daemon -> client, unsolicited (e.g. log record).
    Notify(IpcNotify),
}

/// Client request. `id` must be unique within a connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcRequest {
    /// Opaque identifier echoed back in the response.
    pub id: RequestId,
    /// Flattened method + params.
    #[serde(flatten)]
    pub payload: RequestPayload,
}

/// The specific method a client is invoking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    /// Ask the daemon to describe its current state.
    GetStatus,
    /// Add a peer's fingerprint to the trust store.
    AddPeerFingerprint {
        /// Human-readable peer name.
        name: String,
        /// SHA-256 fingerprint in `sha256:<hex>` form.
        fingerprint: String,
    },
    /// Remove a peer by name. Reply `{"removed": bool}`.
    RemovePeer {
        /// Human-readable peer name.
        name: String,
    },
}

/// Daemon response to a request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcResponse {
    /// Echo of the request's id.
    pub id: RequestId,
    /// Either `result` or `error`.
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

/// Success or failure of a single request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ResponseOutcome {
    /// Method-specific success payload.
    #[serde(rename = "result")]
    Result(ResultPayload),
    /// Failure with code + message.
    #[serde(rename = "error")]
    Error(ErrorPayload),
}

/// Per-method success payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ResultPayload {
    /// Reply to `get_status`.
    Status(StatusReply),
    /// Reply to `add_peer_fingerprint` / `remove_peer`.
    Ok {
        /// `true` if the operation actually changed state.
        ok: bool,
    },
}

/// Structured daemon status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusReply {
    /// Address the mTLS listener is bound to.
    pub listen_addr: String,
    /// Human-readable name the daemon advertises in its handshake.
    pub display_name: String,
    /// SHA-256 fingerprint of the local cert.
    pub local_fingerprint: String,
    /// How many trusted peers are in the DB right now.
    pub trusted_peer_count: usize,
}

/// Error body shared by all request failures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorPayload {
    /// Machine-readable code (stable across releases).
    pub code: i32,
    /// Human-readable description, shown verbatim in the GUI.
    pub message: String,
}

/// Semantic error kinds returned to IPC clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Handler returned an error of its own.
    HandlerFailed,
    /// Request referenced an unknown name / fingerprint.
    NotFound,
    /// Request arguments failed to validate (e.g. bad fingerprint string).
    InvalidArgument,
    /// Daemon is shutting down; no more requests accepted.
    Shutdown,
}

impl IpcError {
    /// Stable numeric code for logs and machine-readable consumers.
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::HandlerFailed => 1,
            Self::NotFound => 2,
            Self::InvalidArgument => 3,
            Self::Shutdown => 4,
        }
    }
}

/// Notifications the daemon may push without a preceding request.
/// Currently unused but reserved so the framing stays stable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum IpcNotify {
    /// Placeholder; log streaming lands here.
    Log {
        /// Log level (e.g. "info", "warn").
        level: String,
        /// Message body.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let req = IpcRequest {
            id: 7,
            payload: RequestPayload::AddPeerFingerprint {
                name: "laptop".into(),
                fingerprint: "sha256:abc".into(),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""method":"add_peer_fingerprint""#));
        let back: IpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_with_result_and_with_error_parse_back() {
        let ok = IpcResponse {
            id: 1,
            outcome: ResponseOutcome::Result(ResultPayload::Ok { ok: true }),
        };
        let err = IpcResponse {
            id: 2,
            outcome: ResponseOutcome::Error(ErrorPayload {
                code: IpcError::NotFound.code(),
                message: "no such peer".into(),
            }),
        };
        let ok_text = serde_json::to_string(&ok).unwrap();
        let err_text = serde_json::to_string(&err).unwrap();
        let ok_back: IpcResponse = serde_json::from_str(&ok_text).unwrap();
        let err_back: IpcResponse = serde_json::from_str(&err_text).unwrap();
        assert_eq!(ok, ok_back);
        assert_eq!(err, err_back);
    }
}
