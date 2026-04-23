//! A post-handshake, peer-identified TLS connection.

use std::net::SocketAddr;

use hop_protocol::{framed, MessageCodec};
use tokio::net::TcpStream;
use tokio_rustls::TlsStream;
use tokio_util::codec::Framed;

use crate::fingerprint::Fingerprint;

/// A TLS-wrapped TCP connection with the peer's identity already
/// verified against the fingerprint database.
///
/// Carries the peer's fingerprint and remote socket address so server
/// and client loops can log them without touching rustls internals
/// again.
#[derive(Debug)]
pub struct ConnectedStream {
    inner: TlsStream<TcpStream>,
    peer_fingerprint: Fingerprint,
    peer_addr: SocketAddr,
}

impl ConnectedStream {
    pub(crate) fn new(
        inner: TlsStream<TcpStream>,
        peer_fingerprint: Fingerprint,
        peer_addr: SocketAddr,
    ) -> Self {
        Self {
            inner,
            peer_fingerprint,
            peer_addr,
        }
    }

    /// The fingerprint of the peer's certificate, used to look up the
    /// peer in the trust store.
    #[must_use]
    pub fn peer_fingerprint(&self) -> &Fingerprint {
        &self.peer_fingerprint
    }

    /// The peer's remote socket address (useful for logging).
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Wrap the stream in the Hop framed codec.
    #[must_use]
    pub fn into_framed(self) -> Framed<TlsStream<TcpStream>, MessageCodec> {
        framed(self.inner)
    }
}
