//! Input Leap client.
//!
//! At M2 the client connects, handshakes, and exchanges keep-alives
//! with the server. Real event injection lands in M3+.

use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use input_leap_net::{
    build_client_config, client_handshake, connect, ConnectError, FingerprintDb, HandshakeError,
    KeepAliveTracker, LoadedIdentity, TlsError,
};
use input_leap_platform::PlatformScreen;
use input_leap_protocol::{
    Capability, DeviceInfoPayload, DisconnectReason, HelloPayload, Message, ProtocolError,
    PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Errors the client can produce.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Building the rustls client config failed.
    #[error("build TLS config: {0}")]
    TlsConfig(#[from] TlsError),

    /// TCP + TLS connect failed.
    #[error("connect to server: {0}")]
    Connect(#[from] ConnectError),

    /// Application-level handshake with the server failed.
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),

    /// Protocol framing / codec error on an established session.
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
}

/// Everything the client needs to run.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Remote server address.
    pub server_addr: SocketAddr,
    /// Name advertised in the `Hello` handshake.
    pub display_name: String,
    /// Local TLS identity (cert chain + private key).
    pub identity: LoadedIdentity,
    /// Trusted peer fingerprint database (holds the server's fingerprint).
    pub trusted_peers: Arc<FingerprintDb>,
    /// Capabilities advertised to the server.
    pub capabilities: Vec<Capability>,
}

/// Connect to the server and run until the shutdown token is triggered
/// (or the connection ends).
pub async fn run<S>(
    cfg: ClientConfig,
    screen: Arc<S>,
    shutdown: CancellationToken,
) -> Result<(), ClientError>
where
    S: PlatformScreen,
{
    let tls_config = build_client_config(&cfg.identity, cfg.trusted_peers.clone())?;
    let stream = connect(cfg.server_addr, Arc::new(tls_config)).await?;
    info!(
        peer = %stream.peer_addr(),
        fingerprint = %stream.peer_fingerprint(),
        "connected to server"
    );

    let info = screen.screen_info();
    let our_hello = HelloPayload {
        protocol_version: PROTOCOL_VERSION,
        display_name: cfg.display_name.clone(),
        capabilities: cfg.capabilities.clone(),
    };
    let our_device_info = DeviceInfoPayload {
        width: info.width,
        height: info.height,
        cursor_x: info.cursor_x,
        cursor_y: info.cursor_y,
        scale_factor_pct: info.scale_factor_pct,
    };

    let mut framed = stream.into_framed();
    let outcome = client_handshake(&mut framed, our_hello, our_device_info).await?;
    info!(peer_name = %outcome.peer_name, "handshake complete");

    session_loop(&mut framed, shutdown).await
}

async fn session_loop(
    framed: &mut input_leap_net::HandshakeStream,
    shutdown: CancellationToken,
) -> Result<(), ClientError> {
    let mut keepalive = KeepAliveTracker::new();

    loop {
        select! {
            biased;

            () = shutdown.cancelled() => {
                let _ = framed
                    .send(Message::Disconnect {
                        reason: DisconnectReason::UserInitiated,
                    })
                    .await;
                return Ok(());
            }

            incoming = framed.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        keepalive.mark_seen();
                        if matches!(msg, Message::Disconnect { .. }) {
                            debug!(?msg, "server sent Disconnect");
                            return Ok(());
                        }
                        debug!(?msg, "message from server");
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => return Ok(()),
                }
            }

            _ = keepalive.tick() => {
                if keepalive.is_timed_out() {
                    warn!("server keepalive timeout");
                    let _ = framed
                        .send(Message::Disconnect {
                            reason: DisconnectReason::KeepAliveTimeout,
                        })
                        .await;
                    return Ok(());
                }
                framed.send(Message::KeepAlive).await?;
            }
        }
    }
}
