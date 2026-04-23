//! Hop server.
//!
//! M11 wires the [`Coordinator`](coordinator::Coordinator) state
//! machine into the accept loop: each connected peer owns a
//! [`ClientProxy`](coordinator::ClientProxy) that forwards peer
//! messages into the coordinator and drains its outbound channel back
//! out to the wire. A single coordinator task owns the routing tables;
//! local input observed on the primary's `PlatformScreen::event_stream`
//! is pumped into the same coordinator.

pub mod coordinator;

use std::net::SocketAddr;
use std::sync::Arc;

use futures::StreamExt;
use hop_net::{
    build_server_config, server_handshake, AcceptError, ConnectedStream, FingerprintDb,
    HandshakeError, Listener, LoadedIdentity, TlsError,
};
use hop_platform::PlatformScreen;
use hop_protocol::{
    Capability, DeviceInfoPayload, HelloPayload, Message, ProtocolError, PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::select;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::coordinator::task::OUTBOUND_CHANNEL_CAPACITY;
use crate::coordinator::{
    spawn_coordinator, ClientProxy, CoordinatorEvent, CoordinatorHandle, SharedLayout,
};

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
    /// Name advertised in the `Hello` handshake. Must match the entry
    /// for the primary screen in [`Self::layout`] for input to route.
    pub display_name: String,
    /// Local TLS identity (cert chain + private key).
    pub identity: LoadedIdentity,
    /// Trusted peer fingerprint database.
    pub trusted_peers: Arc<FingerprintDb>,
    /// Capabilities advertised to peers.
    pub capabilities: Vec<Capability>,
    /// Screen layout the coordinator routes input over.
    pub layout: SharedLayout,
}

/// A bound-but-not-yet-serving Hop server.
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
    /// Spawns, for the lifetime of this call:
    ///   1. the coordinator task (state machine + routing table);
    ///   2. the platform dispatcher (consumes `InjectLocal` outputs);
    ///   3. a local-input forwarder pumping `screen.event_stream()`
    ///      into the coordinator;
    ///   4. one [`ClientProxy`] per accepted peer, joined in a
    ///      [`JoinSet`] so panics surface via `tracing::warn!`.
    ///
    /// Shutdown fans out via the shared [`CancellationToken`]: every
    /// task's `select!` watches it and exits on its own.
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

        // 1. Coordinator + platform dispatcher.
        let (handle, coord_task, dispatcher_task) = spawn_coordinator(
            Arc::clone(&self.cfg.layout),
            self.cfg.display_name.clone(),
            Arc::clone(&screen),
            &shutdown,
        );

        // 2. Local-input forwarder.
        let input_task = spawn_input_forwarder(&screen, handle.clone(), shutdown.clone());

        // 3. Accept loop.
        let mut proxies: JoinSet<()> = JoinSet::new();
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
                            let handle = handle.clone();
                            let task_shutdown = shutdown.clone();
                            proxies.spawn(async move {
                                match accept_and_proxy(cfg, screen, stream, handle, task_shutdown).await {
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

                Some(result) = proxies.join_next() => {
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

        // Drain in-flight proxies so their Disconnect frames go out
        // before we tear down the runtime.
        while let Some(result) = proxies.join_next().await {
            if let Err(err) = result {
                if err.is_panic() {
                    warn!(error = %err, "client task panicked during drain");
                }
            }
        }

        // Wait for the coordinator halo to wind down. They all watch
        // the shared shutdown token so the awaits complete promptly.
        let _ = input_task.await;
        let _ = coord_task.await;
        let _ = dispatcher_task.await;
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

/// Run a peer handshake, register the client with the coordinator, and
/// drive its [`ClientProxy`] until the session ends.
async fn accept_and_proxy<S>(
    cfg: Arc<ServerConfig>,
    screen: Arc<S>,
    stream: ConnectedStream,
    handle: CoordinatorHandle,
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

    let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(OUTBOUND_CHANNEL_CAPACITY);
    if handle
        .register_client(
            outcome.peer_name.clone(),
            outbound_tx,
            outcome.peer_capabilities.clone(),
        )
        .await
        .is_err()
    {
        // Coordinator task gone — shutdown in progress.
        return Ok(());
    }

    let proxy = ClientProxy::new(
        outcome.peer_name,
        framed,
        outbound_rx,
        handle.commands_tx.clone(),
        shutdown,
    );
    if let Err(err) = proxy.run().await {
        warn!(peer = %peer_addr, error = %err, "proxy terminated with error");
    }
    Ok(())
}

/// Background task: pump [`InputEvent`](hop_platform::InputEvent)s
/// from the local platform into the coordinator.
///
/// Exits on shutdown, when the event stream ends, or when the
/// coordinator's command channel closes.
fn spawn_input_forwarder<S>(
    screen: &Arc<S>,
    handle: CoordinatorHandle,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()>
where
    S: PlatformScreen,
{
    let mut stream = screen.event_stream();
    tokio::spawn(async move {
        loop {
            select! {
                biased;
                () = shutdown.cancelled() => {
                    stream.shutdown();
                    break;
                }
                event = stream.next() => {
                    let Some(event) = event else {
                        debug!("local input stream ended");
                        break;
                    };
                    if handle
                        .send_event(CoordinatorEvent::LocalInput(event))
                        .await
                        .is_err()
                    {
                        debug!("coordinator command channel closed");
                        break;
                    }
                }
            }
        }
    })
}
