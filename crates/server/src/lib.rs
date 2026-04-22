//! Input Leap server.
//!
//! At M2 the server accepts peers, performs the handshake, and
//! exchanges keep-alives. Screen routing, clipboard, and real platform
//! I/O land in M3+.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use input_leap_net::{
    build_server_config, server_handshake, AcceptError, ConnectedStream, FingerprintDb,
    HandshakeStream, KeepAliveTracker, Listener, LoadedIdentity,
};
use input_leap_platform::PlatformScreen;
use input_leap_protocol::{
    Capability, DeviceInfoPayload, DisconnectReason, HelloPayload, Message, PROTOCOL_VERSION,
};
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Everything the server needs to run.
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
    cfg: ServerConfig,
}

impl Server {
    /// Bind the listener and build the TLS config. Returns before any
    /// connection is accepted.
    pub async fn bind(cfg: ServerConfig) -> Result<Self> {
        let tls_config = build_server_config(&cfg.identity, cfg.trusted_peers.clone())
            .context("build server TLS config")?;
        let listener = Listener::bind(cfg.listen_addr, Arc::new(tls_config))
            .await
            .with_context(|| format!("bind {}", cfg.listen_addr))?;
        Ok(Self { listener, cfg })
    }

    /// Address the listener is actually bound to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr()
    }

    /// Run the accept loop until the shutdown token fires.
    pub async fn serve<S>(self, screen: Arc<S>, shutdown: CancellationToken) -> Result<()>
    where
        S: PlatformScreen,
    {
        info!(
            addr = %self.listener.local_addr(),
            fingerprint = %self.cfg.identity.fingerprint,
            "server listening"
        );

        let mut client_tasks = Vec::new();

        loop {
            select! {
                accept = self.listener.accept() => {
                    match accept {
                        Ok(stream) => {
                            let peer_addr = stream.peer_addr();
                            let peer_fp = *stream.peer_fingerprint();
                            info!(peer = %peer_addr, fingerprint = %peer_fp, "client connected");
                            let cfg = self.cfg.clone();
                            let screen = Arc::clone(&screen);
                            let shutdown = shutdown.clone();
                            let task = tokio::spawn(async move {
                                if let Err(err) = handle_client(&cfg, screen, stream, shutdown).await {
                                    warn!(
                                        peer = %peer_addr,
                                        error = %err,
                                        "client session ended with error"
                                    );
                                } else {
                                    info!(peer = %peer_addr, "client session ended");
                                }
                            });
                            client_tasks.push(task);
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
                () = shutdown.cancelled() => {
                    info!("server shutdown requested");
                    break;
                }
            }
        }

        for task in client_tasks {
            let _ = task.await;
        }
        Ok(())
    }
}

/// Convenience: bind the server and serve until shutdown.
pub async fn run<S>(cfg: ServerConfig, screen: Arc<S>, shutdown: CancellationToken) -> Result<()>
where
    S: PlatformScreen,
{
    Server::bind(cfg).await?.serve(screen, shutdown).await
}

async fn handle_client<S>(
    cfg: &ServerConfig,
    screen: Arc<S>,
    stream: ConnectedStream,
    shutdown: CancellationToken,
) -> Result<()>
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
        .context("server handshake")?;
    info!(
        peer = %peer_addr,
        peer_name = %outcome.peer_name,
        "handshake complete"
    );

    session_loop(&mut framed, shutdown).await
}

async fn session_loop(framed: &mut HandshakeStream, shutdown: CancellationToken) -> Result<()> {
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
