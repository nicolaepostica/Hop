//! Client-side TLS dialer.

use std::net::SocketAddr;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::debug;

use crate::fingerprint::Fingerprint;
use crate::listener::TLS_HANDSHAKE_TIMEOUT;
use crate::stream::ConnectedStream;
use crate::tls::DEFAULT_CERT_SAN;

/// Errors from [`connect`].
#[derive(Debug, Error)]
pub enum ConnectError {
    /// TCP dial failed (host unreachable, connection refused, ...).
    #[error("tcp connect to {addr} failed: {source}")]
    Tcp {
        /// Address we tried to connect to.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// TLS handshake failed (cert rejected, crypto error, ...).
    #[error("TLS handshake failed: {0}")]
    Tls(#[source] std::io::Error),

    /// Peer didn't finish the TLS handshake in time.
    #[error("TLS handshake timed out after {:?}", TLS_HANDSHAKE_TIMEOUT)]
    HandshakeTimeout,

    /// Handshake succeeded but the server did not present a cert.
    #[error("server did not present a certificate")]
    MissingPeerCert,
}

/// Connect to a server over TCP + TLS and return the established stream.
pub async fn connect(
    addr: SocketAddr,
    config: Arc<ClientConfig>,
) -> Result<ConnectedStream, ConnectError> {
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|source| ConnectError::Tcp { addr, source })?;
    debug!(%addr, "TCP connected");

    let connector = TlsConnector::from(config);
    // Self-signed certs are verified by fingerprint, not by name, so
    // the SAN we pass here only matters for the rustls machinery.
    let server_name = ServerName::try_from(DEFAULT_CERT_SAN)
        .expect("DEFAULT_CERT_SAN is a valid DNS name")
        .to_owned();

    let tls = match timeout(TLS_HANDSHAKE_TIMEOUT, connector.connect(server_name, tcp)).await {
        Err(_) => return Err(ConnectError::HandshakeTimeout),
        Ok(Err(err)) => return Err(ConnectError::Tls(err)),
        Ok(Ok(tls)) => tls,
    };

    let (_, conn) = tls.get_ref();
    let peer_cert = conn
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or(ConnectError::MissingPeerCert)?;
    let fingerprint = Fingerprint::from_cert_der(peer_cert.as_ref());

    Ok(ConnectedStream::new(
        tokio_rustls::TlsStream::Client(tls),
        fingerprint,
        addr,
    ))
}
