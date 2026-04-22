//! Networking for Input Leap: TCP, TLS (rustls), fingerprint DB,
//! handshake.
//!
//! Implementation lands in [M2](../../../specs/milestones/M2-net-handshake.md).

pub mod client;
pub mod fingerprint;
pub mod handshake;
pub mod keepalive;
pub mod listener;
pub mod stream;
pub mod tls;

pub use self::client::{connect, ConnectError};
pub use self::fingerprint::{
    Fingerprint, FingerprintDb, FingerprintDbError, FingerprintParseError, PeerEntry,
};
pub use self::handshake::{
    client_handshake, run_handshake, server_handshake, HandshakeError, HandshakeOutcome,
    HandshakeStep, HandshakeStream, HANDSHAKE_STEP_TIMEOUT,
};
pub use self::keepalive::{
    KeepAliveTracker, KEEPALIVE_INTERVAL, KEEPALIVE_MAX_MISSES, KEEPALIVE_TIMEOUT,
};
pub use self::listener::{AcceptError, Listener, TLS_HANDSHAKE_TIMEOUT};
pub use self::stream::ConnectedStream;
pub use self::tls::{
    build_client_config, build_server_config, install_default_crypto_provider,
    load_or_generate_cert, FingerprintVerifier, LoadedIdentity, TlsError,
};
