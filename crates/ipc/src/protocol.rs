//! JSON-RPC 2.0 wire types for the daemon IPC.
//!
//! Every message on the socket carries the mandatory `"jsonrpc": "2.0"`
//! tag, request/response objects use `id`, and the server reports errors
//! via the standard `{ "code", "message", "data"? }` object — so
//! off-the-shelf JSON-RPC clients (`jq`, `curl`, GUI debug panels,
//! `nc | jq`) can talk to the daemon without a bespoke parser.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Monotonic request identifier. Responses echo it back so a client
/// can multiplex several in-flight calls on the same connection.
pub type RequestId = u64;

/// The `"jsonrpc"` tag value this implementation speaks.
pub const JSONRPC_VERSION: &str = "2.0";

/// Marker type enforcing `"jsonrpc": "2.0"` on serialize and refusing
/// any other value on deserialize.
///
/// Serializes as the literal string `"2.0"`. Deserialization fails if
/// the peer sends anything else (including `"1.0"` or an integer) —
/// mixing JSON-RPC 1.0 and 2.0 peers is always a bug.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct JsonRpcVersion;

impl Serialize for JsonRpcVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(JSONRPC_VERSION)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s == JSONRPC_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported jsonrpc version: {s}; expected {JSONRPC_VERSION}"
            )))
        }
    }
}

/// Any message flowing over the IPC socket.
///
/// Serialized as untagged: the presence of `id`, `result`, or `error`
/// disambiguates between request, response, and notification. Every
/// variant carries `jsonrpc: "2.0"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum IpcMessage {
    /// Client → daemon.
    Request(IpcRequest),
    /// Daemon → client, reply to a previous request.
    Response(IpcResponse),
    /// Daemon → client, unsolicited (e.g. log record).
    Notify(IpcNotify),
}

/// Client request. `id` must be unique within a connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcRequest {
    /// JSON-RPC version tag, always `"2.0"`.
    #[serde(default)]
    pub jsonrpc: JsonRpcVersion,
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
    /// JSON-RPC version tag, always `"2.0"`.
    #[serde(default)]
    pub jsonrpc: JsonRpcVersion,
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
///
/// Matches the standard JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorPayload {
    /// Machine-readable code. Uses JSON-RPC 2.0 error-code conventions
    /// (negative for standard / server errors; see [`IpcError::code`]).
    pub code: i32,
    /// Human-readable description, shown verbatim in the GUI.
    pub message: String,
    /// Optional free-form detail. Currently unused; reserved so the
    /// server can grow richer diagnostics without breaking clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Semantic error kinds returned to IPC clients.
///
/// Each variant maps to a concrete JSON-RPC 2.0 error code via
/// [`Self::code`]. Standard codes are negative (`-326xx` for the JSON-RPC
/// predefined set; `-320xx` for implementation-defined server errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Handler returned an error of its own (internal daemon failure).
    HandlerFailed,
    /// Request referenced an unknown name / fingerprint.
    NotFound,
    /// Request arguments failed to validate (e.g. bad fingerprint string).
    InvalidArgument,
    /// Daemon is shutting down; no more requests accepted.
    Shutdown,
}

impl IpcError {
    /// Stable JSON-RPC 2.0 code for this error.
    ///
    /// Values in `-32768..=-32000` are reserved by the JSON-RPC 2.0
    /// specification; we use `-32602` for invalid-params semantics and
    /// `-32000..-32099` for our own server-defined cases.
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::HandlerFailed => -32000,
            Self::NotFound => -32001,
            Self::Shutdown => -32002,
            Self::InvalidArgument => -32602,
        }
    }
}

/// Notifications the daemon may push without a preceding request.
/// Currently unused but reserved so the framing stays stable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcNotify {
    /// JSON-RPC version tag, always `"2.0"`.
    #[serde(default)]
    pub jsonrpc: JsonRpcVersion,
    /// Flattened method + params.
    #[serde(flatten)]
    pub payload: NotifyPayload,
}

/// Notify payload kinds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum NotifyPayload {
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
    fn request_carries_jsonrpc_2_0() {
        let req = IpcRequest {
            jsonrpc: JsonRpcVersion,
            id: 7,
            payload: RequestPayload::AddPeerFingerprint {
                name: "laptop".into(),
                fingerprint: "sha256:abc".into(),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0""#), "missing version tag: {json}");
        assert!(json.contains(r#""method":"add_peer_fingerprint""#));
        let back: IpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_round_trips() {
        let ok = IpcResponse {
            jsonrpc: JsonRpcVersion,
            id: 1,
            outcome: ResponseOutcome::Result(ResultPayload::Ok { ok: true }),
        };
        let err = IpcResponse {
            jsonrpc: JsonRpcVersion,
            id: 2,
            outcome: ResponseOutcome::Error(ErrorPayload {
                code: IpcError::NotFound.code(),
                message: "no such peer".into(),
                data: None,
            }),
        };
        let ok_text = serde_json::to_string(&ok).unwrap();
        let err_text = serde_json::to_string(&err).unwrap();
        assert_eq!(ok, serde_json::from_str(&ok_text).unwrap());
        assert_eq!(err, serde_json::from_str(&err_text).unwrap());
    }

    #[test]
    fn wrong_jsonrpc_version_is_rejected() {
        let bogus = r#"{"jsonrpc":"1.0","id":1,"method":"get_status"}"#;
        let parse: Result<IpcRequest, _> = serde_json::from_str(bogus);
        assert!(parse.is_err());
    }

    #[test]
    fn request_without_jsonrpc_tag_is_accepted() {
        // Tolerance: peers that forget the tag get the default 2.0.
        let legacy = r#"{"id":1,"method":"get_status","params":null}"#;
        let req: IpcRequest = serde_json::from_str(legacy).expect("parse");
        assert_eq!(req.jsonrpc, JsonRpcVersion);
    }

    #[test]
    fn error_codes_follow_jsonrpc_conventions() {
        assert_eq!(IpcError::InvalidArgument.code(), -32602);
        assert!(IpcError::HandlerFailed.code() >= -32099);
        assert!(IpcError::HandlerFailed.code() <= -32000);
    }
}
