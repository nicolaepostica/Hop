//! TCP listener that performs the TLS handshake before handing off.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::ServerConfig;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

use crate::fingerprint::Fingerprint;
use crate::stream::ConnectedStream;

/// How long a peer has to finish the TLS handshake.
///
/// Chosen empirically: well above round-trip latency on a LAN, well
/// below the default TCP retransmit timers on any OS. Longer than this
/// and we risk a slow-loris style resource exhaustion; shorter and
/// flaky networks drop valid peers.
pub const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors from [`Listener::accept`].
#[derive(Debug, Error)]
pub enum AcceptError {
    /// Failed to accept a raw TCP connection.
    #[error("accept failed: {0}")]
    Tcp(#[source] std::io::Error),

    /// TLS handshake failed (peer cert rejected, crypto error, ...).
    #[error("TLS handshake failed: {0}")]
    Tls(#[source] std::io::Error),

    /// Peer didn't finish the TLS handshake in time.
    #[error("TLS handshake timed out after {:?}", TLS_HANDSHAKE_TIMEOUT)]
    HandshakeTimeout,

    /// Handshake succeeded but the peer did not present a client cert
    /// (mTLS requires it).
    #[error("peer did not present a certificate")]
    MissingPeerCert,
}

/// A TCP listener that wraps each accepted connection in TLS before
/// returning it.
///
/// Call [`accept`](Self::accept) in a loop to service peers.
pub struct Listener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    local_addr: SocketAddr,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("local_addr", &self.local_addr)
            .finish_non_exhaustive()
    }
}

impl Listener {
    /// Bind to `addr` and prepare to accept mTLS connections using
    /// `config`.
    pub async fn bind(addr: SocketAddr, config: Arc<ServerConfig>) -> std::io::Result<Self> {
        let tcp = TcpListener::bind(addr).await?;
        let local_addr = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(config);
        Ok(Self {
            tcp,
            acceptor,
            local_addr,
        })
    }

    /// The address the listener actually bound to — useful when the
    /// caller passed port 0 and wants the OS-assigned one.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept one peer and perform the TLS handshake.
    ///
    /// Connections that fail to complete the handshake within
    /// [`TLS_HANDSHAKE_TIMEOUT`] are dropped. On any error the caller
    /// may loop and call `accept` again; the listener itself stays
    /// healthy.
    pub async fn accept(&self) -> Result<ConnectedStream, AcceptError> {
        let (tcp, peer_addr) = self.tcp.accept().await.map_err(AcceptError::Tcp)?;
        debug!(peer = %peer_addr, "accepted raw TCP");

        let tls = match timeout(TLS_HANDSHAKE_TIMEOUT, self.acceptor.accept(tcp)).await {
            Err(_) => {
                warn!(peer = %peer_addr, "TLS handshake timed out");
                return Err(AcceptError::HandshakeTimeout);
            }
            Ok(Err(err)) => {
                warn!(peer = %peer_addr, error = %err, "TLS handshake failed");
                return Err(AcceptError::Tls(err));
            }
            Ok(Ok(tls)) => tls,
        };

        let (_, conn) = tls.get_ref();
        let peer_cert = conn
            .peer_certificates()
            .and_then(|c| c.first())
            .ok_or(AcceptError::MissingPeerCert)?;
        let fingerprint = Fingerprint::from_cert_der(peer_cert.as_ref());

        Ok(ConnectedStream::new(
            tokio_rustls::TlsStream::Server(tls),
            fingerprint,
            peer_addr,
        ))
    }
}
