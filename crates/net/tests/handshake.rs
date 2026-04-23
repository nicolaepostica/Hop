//! Integration tests for the TCP + TLS + handshake layer.
//!
//! Boots a listener on `127.0.0.1:0` and a client in the same process,
//! verifying the happy path plus two failure modes (wrong protocol
//! version, unknown fingerprint). Uses real cryptography — each test
//! generates its own self-signed pair via rcgen into a temp dir.

use std::net::SocketAddr;
use std::sync::Arc;

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use hop_net::{
    build_client_config, build_server_config, client_handshake, connect, load_or_generate_cert,
    server_handshake, ConnectedStream, FingerprintDb, HandshakeError, Listener, LoadedIdentity,
    PeerEntry,
};
use hop_protocol::{Capability, DeviceInfoPayload, HelloPayload, Message, PROTOCOL_VERSION};
use tempfile::TempDir;

fn gen_identity() -> (LoadedIdentity, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let identity = load_or_generate_cert(dir.path()).expect("generate cert");
    (identity, dir)
}

fn trust(peers: &[&LoadedIdentity]) -> Arc<FingerprintDb> {
    let mut db = FingerprintDb::new();
    for (i, ident) in peers.iter().enumerate() {
        db.add(PeerEntry {
            name: format!("peer-{i}"),
            fingerprint: ident.fingerprint,
            added: Utc::now(),
        });
    }
    Arc::new(db)
}

async fn bind_server(
    identity: &LoadedIdentity,
    trust_db: Arc<FingerprintDb>,
) -> (Listener, SocketAddr) {
    let tls = build_server_config(identity, trust_db).expect("server tls");
    let listener = Listener::bind("127.0.0.1:0".parse().unwrap(), Arc::new(tls))
        .await
        .expect("bind");
    let addr = listener.local_addr();
    (listener, addr)
}

async fn dial(
    server_addr: SocketAddr,
    identity: &LoadedIdentity,
    trust_db: Arc<FingerprintDb>,
) -> Result<ConnectedStream, hop_net::ConnectError> {
    let tls = build_client_config(identity, trust_db).expect("client tls");
    connect(server_addr, Arc::new(tls)).await
}

fn hello(name: &str, version: u16) -> HelloPayload {
    HelloPayload {
        protocol_version: version,
        display_name: name.into(),
        capabilities: vec![Capability::UnicodeClipboard],
    }
}

fn device_info(width: u32, height: u32) -> DeviceInfoPayload {
    DeviceInfoPayload {
        width,
        height,
        cursor_x: 0,
        cursor_y: 0,
        scale_factor_pct: 100,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_happy_path() {
    let (server_id, _s_dir) = gen_identity();
    let (client_id, _c_dir) = gen_identity();

    let server_trust = trust(&[&client_id]);
    let client_trust = trust(&[&server_id]);

    let (listener, addr) = bind_server(&server_id, server_trust).await;

    let server_task = tokio::spawn(async move {
        let stream = listener.accept().await.expect("accept");
        let mut framed = stream.into_framed();
        server_handshake(
            &mut framed,
            hello("server", PROTOCOL_VERSION),
            device_info(1920, 1080),
        )
        .await
    });

    let client_task = tokio::spawn(async move {
        let stream = dial(addr, &client_id, client_trust).await.expect("dial");
        let mut framed = stream.into_framed();
        client_handshake(
            &mut framed,
            hello("client", PROTOCOL_VERSION),
            device_info(2560, 1440),
        )
        .await
    });

    let server_out = server_task.await.unwrap().expect("server handshake");
    let client_out = client_task.await.unwrap().expect("client handshake");

    assert_eq!(server_out.peer_name, "client");
    assert_eq!(server_out.peer_device_info, device_info(2560, 1440));
    assert_eq!(client_out.peer_name, "server");
    assert_eq!(client_out.peer_device_info, device_info(1920, 1080));
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_rejects_wrong_protocol_version() {
    let (server_id, _s_dir) = gen_identity();
    let (client_id, _c_dir) = gen_identity();

    let server_trust = trust(&[&client_id]);
    let client_trust = trust(&[&server_id]);

    let (listener, addr) = bind_server(&server_id, server_trust).await;

    let server_task = tokio::spawn(async move {
        let stream = listener.accept().await.expect("accept");
        let mut framed = stream.into_framed();
        server_handshake(
            &mut framed,
            hello("server", PROTOCOL_VERSION),
            device_info(1920, 1080),
        )
        .await
    });

    let client_task = tokio::spawn(async move {
        let stream = dial(addr, &client_id, client_trust).await.expect("dial");
        let mut framed = stream.into_framed();
        // Skip the library handshake — send a Hello with a bogus version directly.
        framed
            .send(Message::Hello(hello("client", PROTOCOL_VERSION + 99)))
            .await
            .expect("send bogus hello");
        // Drain whatever the server sends so we can see it shut down.
        while let Some(msg) = framed.next().await {
            if msg.is_err() {
                break;
            }
        }
    });

    let server_err = server_task.await.unwrap();
    assert!(
        matches!(server_err, Err(HandshakeError::VersionMismatch { .. })),
        "expected VersionMismatch, got {server_err:?}"
    );
    client_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_fingerprint_aborts_at_tls_layer() {
    let (server_id, _s_dir) = gen_identity();
    let (client_id, _c_dir) = gen_identity();

    // Server does NOT trust the client.
    let empty = Arc::new(FingerprintDb::new());
    // Client still trusts the server (otherwise we couldn't isolate the failure).
    let client_trust = trust(&[&server_id]);

    let (listener, addr) = bind_server(&server_id, empty).await;

    // Server task: we expect accept() to fail because the peer cert is
    // rejected by the verifier during TLS handshake.
    let server_task = tokio::spawn(async move { listener.accept().await });

    let client_task = tokio::spawn(async move { dial(addr, &client_id, client_trust).await });

    let server_result = server_task.await.unwrap();
    let client_result = client_task.await.unwrap();
    assert!(
        server_result.is_err(),
        "server accept should have failed due to untrusted client cert, got Ok"
    );
    // The client may observe success at the TLS layer (the server
    // verifies the client cert AFTER it has already sent its own
    // ServerHello, so the dial may complete before the alert arrives).
    // What matters for security is that the server rejected. If the
    // client did establish a stream, any subsequent I/O will fail —
    // we don't assert on it here to keep the test deterministic.
    drop(client_result);
}
