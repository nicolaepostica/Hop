//! Input Leap server.
//!
//! At M2 the server accepts peers, performs the handshake, and
//! exchanges keep-alives. Screen routing, clipboard, and real platform
//! I/O land in M3+.

pub mod coordinator;

use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use input_leap_net::{
    build_server_config, server_handshake, AcceptError, ConnectedStream, FingerprintDb,
    HandshakeError, HandshakeStream, KeepAliveTracker, Listener, LoadedIdentity, TlsError,
};
use input_leap_platform::PlatformScreen;
use input_leap_protocol::{
    Capability, DeviceInfoPayload, DisconnectReason, HelloPayload, Message, ProtocolError,
    PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::select;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Errors the server can produce.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Binding the TCP listener failed.
    #[error("bind {addr}: {source}")]
    Bind {
        /// Address we tried to bind.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// Building the rustls server config failed.
    #[error("build TLS config: {0}")]
    TlsConfig(#[from] TlsError),

    /// Application-level handshake with a peer failed.
    #[error("handshake with {peer}: {source}")]
    Handshake {
        /// Peer whose handshake went wrong.
        peer: SocketAddr,
        /// Underlying handshake failure.
        #[source]
        source: HandshakeError,
    },

    /// Protocol framing / codec error on an established session.
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
}

/// Everything the server needs to run.
///
/// Cheap to share across tasks — [`Server`] wraps this in an [`Arc`]
/// internally so every accepted connection only pays a pointer-bump
/// instead of cloning the cert chain + private key material.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind the TCP listener to (`0.0.0.0:port`).
    pub listen_addr: SocketAddr,
    /// Name advertised in the `Hello` handshake.
    pub display_name: String,
    /// Local TLS identity (cert chain + private key).
    pub identity: LoadedIdentity,
    /// Trusted peer fingerprint database.
    pub trusted_peers: Arc<FingerprintDb>,
    /// Capabilities advertised to peers.
    pub capabilities: Vec<Capability>,
}

/// A bound-but-not-yet-serving Input Leap server.
///
/// Split out from [`run`] so tests (and `--print-address`-style CLIs)
/// can learn the OS-assigned port before entering the accept loop.
pub struct Server {
    listener: Listener,
    cfg: Arc<ServerConfig>,
}

impl Server {
    /// Bind the listener and build the TLS config. Returns before any
    /// connection is accepted.
    pub async fn bind(cfg: ServerConfig) -> Result<Self, ServerError> {
        let tls_config = build_server_config(&cfg.identity, cfg.trusted_peers.clone())?;
        let listener = Listener::bind(cfg.listen_addr, Arc::new(tls_config))
            .await
            .map_err(|source| ServerError::Bind {
                addr: cfg.listen_addr,
                source,
            })?;
        Ok(Self {
            listener,
            cfg: Arc::new(cfg),
        })
    }

    /// Address the listener is actually bound to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr()
    }

    /// Run the accept loop until the shutdown token fires.
    ///
    /// Client connections are spawned into a [`JoinSet`] so finished
    /// tasks are reaped as they complete (bounded memory even under a
    /// churny workload) and panics surface via `tracing::warn!` instead
    /// of being silently swallowed by a detached `tokio::spawn`.
    pub async fn serve<S>(
        self,
        screen: Arc<S>,
        shutdown: CancellationToken,
    ) -> Result<(), ServerError>
    where
        S: PlatformScreen,
    {
        info!(
            addr = %self.listener.local_addr(),
            fingerprint = %self.cfg.identity.fingerprint,
            "server listening"
        );

        let mut clients: JoinSet<()> = JoinSet::new();

        loop {
            select! {
                accept = self.listener.accept() => {
                    match accept {
                        Ok(stream) => {
                            let peer_addr = stream.peer_addr();
                            let peer_fp = *stream.peer_fingerprint();
                            debug!(peer = %peer_addr, fingerprint = %peer_fp, "client connected");
                            let cfg = Arc::clone(&self.cfg);
                            let screen = Arc::clone(&screen);
                            let task_shutdown = shutdown.clone();
                            clients.spawn(async move {
                                match handle_client(cfg, screen, stream, task_shutdown).await {
                                    Ok(()) => debug!(peer = %peer_addr, "client session ended"),
                                    Err(err) => warn!(
                                        peer = %peer_addr,
                                        error = %err,
                                        "client session ended with error"
                                    ),
                                }
                            });
                        }
                        Err(AcceptError::HandshakeTimeout | AcceptError::Tls(_)
                            | AcceptError::MissingPeerCert) => {
                            // Already logged inside the listener; keep accepting.
                        }
                        Err(AcceptError::Tcp(err)) => {
                            warn!(error = %err, "TCP accept failed");
                        }
                    }
                }

                // Reap finished client tasks as they complete. Without
                // this arm the JoinSet would hold onto every handle
                // until shutdown, leaking memory for long-running
                // servers with frequent reconnects.
                Some(result) = clients.join_next() => {
                    if let Err(err) = result {
                        if err.is_panic() {
                            warn!(error = %err, "client task panicked");
                        }
                    }
                }

                () = shutdown.cancelled() => {
                    info!("server shutdown requested");
                    break;
                }
            }
        }

        // Drain in-flight clients so their Disconnect frames go out
        // before we tear down the runtime.
        while let Some(result) = clients.join_next().await {
            if let Err(err) = result {
                if err.is_panic() {
                    warn!(error = %err, "client task panicked during drain");
                }
            }
        }
        Ok(())
    }
}

/// Convenience: bind the server and serve until shutdown.
pub async fn run<S>(
    cfg: ServerConfig,
    screen: Arc<S>,
    shutdown: CancellationToken,
) -> Result<(), ServerError>
where
    S: PlatformScreen,
{
    Server::bind(cfg).await?.serve(screen, shutdown).await
}

async fn handle_client<S>(
    cfg: Arc<ServerConfig>,
    screen: Arc<S>,
    stream: ConnectedStream,
    shutdown: CancellationToken,
) -> Result<(), ServerError>
where
    S: PlatformScreen,
{
    let peer_addr = stream.peer_addr();
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
    let outcome = server_handshake(&mut framed, our_hello, our_device_info)
        .await
        .map_err(|source| ServerError::Handshake {
            peer: peer_addr,
            source,
        })?;
    debug!(
        peer = %peer_addr,
        peer_name = %outcome.peer_name,
        "handshake complete"
    );

    session_loop(&mut framed, shutdown).await
}

async fn session_loop(
    framed: &mut HandshakeStream,
    shutdown: CancellationToken,
) -> Result<(), ServerError> {
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
                            debug!(?msg, "peer sent Disconnect");
                            return Ok(());
                        }
                        debug!(?msg, "message from peer");
                    }
                    Some(Err(err)) => return Err(err.into()),
                    None => return Ok(()),
                }
            }

            _ = keepalive.tick() => {
                if keepalive.is_timed_out() {
                    warn!("peer keepalive timeout");
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
