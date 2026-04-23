//! Application-level handshake that runs on top of the established TLS
//! connection.
//!
//! The flow is symmetric — no "server-only" state — but sequenced so
//! that a peer speaking a protocol version we cannot understand is
//! rejected before we reveal anything beyond our own `Hello`:
//!
//!   1. Send our `Hello`.
//!   2. Read and verify the peer's `Hello` (protocol version match).
//!   3. Send our `DeviceInfo`.
//!   4. Read the peer's `DeviceInfo`.
//!
//! An earlier draft fired steps 1 and 3 back-to-back to save one RTT;
//! that leaked the local screen geometry to peers we were about to
//! reject. The extra round-trip (still well under 10 ms on a LAN) is
//! worth the tighter information boundary.
//!
//! Any deviation — wrong version, wrong message type, silence past the
//! per-step timeout — aborts the handshake and leaves the caller to
//! close the connection.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use input_leap_protocol::{
    Capability, DeviceInfoPayload, HelloPayload, Message, MessageCodec, ProtocolError,
    PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsStream;
use tokio_util::codec::Framed;
use tracing::{debug, warn};

/// How long each send or receive step of the handshake may take.
///
/// Multiple seconds is ample for a local connection; the outer
/// accept/connect path already caps total TLS time, so this is just a
/// sanity net against a stuck peer at application level.
pub const HANDSHAKE_STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// Successful outcome of the handshake.
///
/// All fields describe the peer. The `peer_` prefix is intentional —
/// dropping it makes the call sites read ambiguously (`outcome.name`
/// could be confused with the local name).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_field_names, reason = "see struct doc comment")]
pub struct HandshakeOutcome {
    /// Human-readable name the peer sent in its `Hello`.
    pub peer_name: String,
    /// Capabilities the peer advertised.
    pub peer_capabilities: Vec<Capability>,
    /// The peer's screen geometry.
    pub peer_device_info: DeviceInfoPayload,
}

/// Errors that can abort the handshake.
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// The peer sent a protocol version we do not speak.
    #[error("protocol version mismatch: peer={peer}, expected={expected}")]
    VersionMismatch {
        /// Version the peer announced.
        peer: u16,
        /// Version we expected.
        expected: u16,
    },

    /// The peer sent a message we did not expect at this stage.
    #[error("unexpected message during handshake: {got:?} (expected {expected})")]
    UnexpectedMessage {
        /// What we got.
        got: Message,
        /// Short description of what we expected.
        expected: &'static str,
    },

    /// The peer hung up mid-handshake.
    #[error("peer closed the connection before the handshake completed")]
    Disconnected,

    /// A handshake step exceeded [`HANDSHAKE_STEP_TIMEOUT`].
    #[error("handshake step {step:?} timed out after {:?}", HANDSHAKE_STEP_TIMEOUT)]
    StepTimeout {
        /// Which step timed out.
        step: HandshakeStep,
    },

    /// Codec error from the underlying framed stream.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

/// Identifies the handshake step at which a failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeStep {
    /// Sending our own `Hello`.
    SendHello,
    /// Awaiting the peer's `Hello`.
    RecvHello,
    /// Sending our own `DeviceInfo`.
    SendDeviceInfo,
    /// Awaiting the peer's `DeviceInfo`.
    RecvDeviceInfo,
}

/// Convenience alias for the framed stream type we operate on.
pub type HandshakeStream = Framed<TlsStream<TcpStream>, MessageCodec>;

/// Run the handshake. Symmetric: identical on both sides.
///
/// The caller decides whether a stream belongs to a server or client
/// based on how the underlying TLS was set up; at the application
/// level both peers speak the same script.
pub async fn run_handshake(
    stream: &mut HandshakeStream,
    our_hello: HelloPayload,
    our_device_info: DeviceInfoPayload,
) -> Result<HandshakeOutcome, HandshakeError> {
    // Step 1: announce ourselves.
    send_with_timeout(stream, Message::Hello(our_hello), HandshakeStep::SendHello).await?;

    // Step 2: verify the peer's Hello *before* sending anything else.
    // This is the version gate — if the peer speaks a protocol we
    // cannot understand, we drop the connection having revealed only
    // the Hello we already sent (same information leaked by any TCP
    // probe). Nothing about the local screen goes out until the peer
    // has passed this check.
    let peer_hello = recv_hello(stream).await?;
    debug!(
        peer_name = %peer_hello.display_name,
        peer_version = peer_hello.protocol_version,
        "received peer Hello"
    );

    // Step 3: version matched — now it is safe to send DeviceInfo.
    send_with_timeout(
        stream,
        Message::DeviceInfo(our_device_info),
        HandshakeStep::SendDeviceInfo,
    )
    .await?;

    // Step 4: collect the peer's matching DeviceInfo.
    let peer_device_info = recv_device_info(stream).await?;

    Ok(HandshakeOutcome {
        peer_name: peer_hello.display_name,
        peer_capabilities: peer_hello.capabilities,
        peer_device_info,
    })
}

/// Alias: server-side handshake. At the moment identical to [`run_handshake`].
///
/// Kept separate so future asymmetric logic can land in one call site
/// without rippling through callers.
pub async fn server_handshake(
    stream: &mut HandshakeStream,
    our_hello: HelloPayload,
    our_device_info: DeviceInfoPayload,
) -> Result<HandshakeOutcome, HandshakeError> {
    run_handshake(stream, our_hello, our_device_info).await
}

/// Alias: client-side handshake. At the moment identical to [`run_handshake`].
pub async fn client_handshake(
    stream: &mut HandshakeStream,
    our_hello: HelloPayload,
    our_device_info: DeviceInfoPayload,
) -> Result<HandshakeOutcome, HandshakeError> {
    run_handshake(stream, our_hello, our_device_info).await
}

async fn send_with_timeout(
    stream: &mut HandshakeStream,
    message: Message,
    step: HandshakeStep,
) -> Result<(), HandshakeError> {
    match timeout(HANDSHAKE_STEP_TIMEOUT, stream.send(message)).await {
        Err(_) => Err(HandshakeError::StepTimeout { step }),
        Ok(Err(err)) => Err(HandshakeError::Protocol(err)),
        Ok(Ok(())) => Ok(()),
    }
}

async fn recv_with_timeout(
    stream: &mut HandshakeStream,
    step: HandshakeStep,
) -> Result<Message, HandshakeError> {
    match timeout(HANDSHAKE_STEP_TIMEOUT, stream.next()).await {
        Err(_) => Err(HandshakeError::StepTimeout { step }),
        Ok(None) => Err(HandshakeError::Disconnected),
        Ok(Some(Err(err))) => Err(HandshakeError::Protocol(err)),
        Ok(Some(Ok(msg))) => Ok(msg),
    }
}

async fn recv_hello(stream: &mut HandshakeStream) -> Result<HelloPayload, HandshakeError> {
    match recv_with_timeout(stream, HandshakeStep::RecvHello).await? {
        Message::Hello(payload) => {
            if payload.protocol_version != PROTOCOL_VERSION {
                warn!(
                    peer = payload.protocol_version,
                    expected = PROTOCOL_VERSION,
                    "rejecting peer due to protocol version mismatch"
                );
                return Err(HandshakeError::VersionMismatch {
                    peer: payload.protocol_version,
                    expected: PROTOCOL_VERSION,
                });
            }
            Ok(payload)
        }
        other => Err(HandshakeError::UnexpectedMessage {
            got: other,
            expected: "Hello",
        }),
    }
}

async fn recv_device_info(
    stream: &mut HandshakeStream,
) -> Result<DeviceInfoPayload, HandshakeError> {
    match recv_with_timeout(stream, HandshakeStep::RecvDeviceInfo).await? {
        Message::DeviceInfo(payload) => Ok(payload),
        other => Err(HandshakeError::UnexpectedMessage {
            got: other,
            expected: "DeviceInfo",
        }),
    }
}
