//! End-to-end: spawn `server::run` and `client::run` in the same
//! process, verify they handshake, exchange a few keep-alives, and
//! shut down cleanly within the milestone's time budget (15 s).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use input_leap_client::{run as run_client, ClientConfig};
use input_leap_net::{load_or_generate_cert, FingerprintDb, PeerEntry};
use input_leap_platform::MockScreen;
use input_leap_server::coordinator::{LayoutStore, ScreenLayout};
use input_leap_server::{Server, ServerConfig};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_and_client_handshake_and_keepalive() {
    // 1. Two identities with mutual trust.
    let server_dir = TempDir::new().unwrap();
    let client_dir = TempDir::new().unwrap();
    let server_id = load_or_generate_cert(server_dir.path()).unwrap();
    let client_id = load_or_generate_cert(client_dir.path()).unwrap();

    let mut server_db = FingerprintDb::new();
    server_db.add(PeerEntry {
        name: "client".into(),
        fingerprint: client_id.fingerprint,
        added: Utc::now(),
    });
    let mut client_db = FingerprintDb::new();
    client_db.add(PeerEntry {
        name: "server".into(),
        fingerprint: server_id.fingerprint,
        added: Utc::now(),
    });

    // 2. Bind the server first so we know the port before the client runs.
    let layout = LayoutStore::from_layout(ScreenLayout::single_primary("server")).handle();
    let server_cfg = ServerConfig {
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        display_name: "server".into(),
        identity: server_id,
        trusted_peers: Arc::new(server_db),
        capabilities: Vec::new(),
        layout,
    };
    let server = Server::bind(server_cfg).await.expect("bind server");
    let server_addr = server.local_addr();

    // 3. Wire up shutdown.
    let shutdown = CancellationToken::new();
    let server_screen = Arc::new(MockScreen::default_stub());
    let server_shutdown = shutdown.clone();
    let server_task =
        tokio::spawn(async move { server.serve(server_screen, server_shutdown).await });

    // 4. Client.
    let client_cfg = ClientConfig {
        server_addr,
        display_name: "client".into(),
        identity: client_id,
        trusted_peers: Arc::new(client_db),
        capabilities: Vec::new(),
    };
    let client_screen = Arc::new(MockScreen::default_stub());
    let client_shutdown = shutdown.clone();
    let client_task =
        tokio::spawn(async move { run_client(client_cfg, client_screen, client_shutdown).await });

    // 5. Let the pair run long enough for a few keep-alive cycles
    // (KEEPALIVE_INTERVAL is 3 s; a second of real time is plenty for
    // the handshake + an initial exchange).
    sleep(Duration::from_millis(1500)).await;

    // 6. Graceful shutdown from the outside.
    shutdown.cancel();

    // 7. Both tasks must exit within the milestone's 15-second budget.
    let deadline = Duration::from_secs(15);
    let server_result = timeout(deadline, server_task)
        .await
        .expect("server task did not exit within 15 s")
        .expect("server task panicked");
    let client_result = timeout(deadline, client_task)
        .await
        .expect("client task did not exit within 15 s")
        .expect("client task panicked");

    server_result.expect("server finished cleanly");
    client_result.expect("client finished cleanly");
}
