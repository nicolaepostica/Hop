//! End-to-end IPC test: bind a server on a tempdir socket, connect a
//! client, exercise the full request/response vocabulary.
//!
//! Uses a stub `IpcHandler` that tracks its own peer list in-memory so
//! the test doesn't touch the real fingerprint DB.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hop_ipc::{
    protocol::IpcError, IpcClient, IpcClientError, IpcHandler, IpcServer, StatusReply,
};
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct StubHandler {
    peers: Mutex<Vec<(String, String)>>,
    status_label: Mutex<String>,
}

#[async_trait]
impl IpcHandler for StubHandler {
    async fn status(&self) -> StatusReply {
        let label = self.status_label.lock().await.clone();
        let peers = self.peers.lock().await.len();
        StatusReply {
            listen_addr: "127.0.0.1:24800".into(),
            display_name: label,
            local_fingerprint: "sha256:stub".into(),
            trusted_peer_count: peers,
        }
    }

    async fn add_peer(
        &self,
        name: String,
        fingerprint: String,
    ) -> Result<bool, (IpcError, String)> {
        if !fingerprint.starts_with("sha256:") {
            return Err((
                IpcError::InvalidArgument,
                format!("bad fingerprint: {fingerprint}"),
            ));
        }
        let mut peers = self.peers.lock().await;
        let existed = peers.iter().any(|(n, _)| n == &name);
        peers.retain(|(n, _)| n != &name);
        peers.push((name, fingerprint));
        Ok(!existed)
    }

    async fn remove_peer(&self, name: String) -> Result<bool, (IpcError, String)> {
        let mut peers = self.peers.lock().await;
        let before = peers.len();
        peers.retain(|(n, _)| n != &name);
        Ok(peers.len() != before)
    }
}

async fn spin_server(
    handler: Arc<StubHandler>,
) -> (CancellationToken, PathBuf, tokio::task::JoinHandle<()>) {
    let dir = TempDir::new().unwrap();
    // Keep the tempdir alive for the whole test by leaking it — the
    // socket file needs the directory to exist for the duration.
    let dir = dir.keep();
    let path = dir.join("daemon.sock");

    let server = IpcServer::bind(&path).expect("bind ipc server");
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let handle = tokio::spawn(async move {
        let _ = server.serve(handler, shutdown_clone).await;
    });
    // Tiny yield to let the listener actually start accepting.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (shutdown, path, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_status_round_trips() {
    let handler = Arc::new(StubHandler::default());
    *handler.status_label.lock().await = "probe".into();
    let (shutdown, path, join) = spin_server(Arc::clone(&handler)).await;

    let mut client = IpcClient::connect(&path).await.expect("connect");
    let status = client.get_status().await.expect("get_status");
    assert_eq!(status.display_name, "probe");
    assert_eq!(status.trusted_peer_count, 0);

    shutdown.cancel();
    let _ = join.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_then_remove_peer() {
    let handler = Arc::new(StubHandler::default());
    let (shutdown, path, join) = spin_server(Arc::clone(&handler)).await;

    let mut client = IpcClient::connect(&path).await.expect("connect");

    let first = client
        .add_peer_fingerprint("laptop", "sha256:deadbeef")
        .await
        .expect("add laptop");
    assert!(first, "first add should report ok=true");

    let second = client
        .add_peer_fingerprint("laptop", "sha256:cafebabe")
        .await
        .expect("replace laptop");
    assert!(
        !second,
        "second add on same name should report ok=false (replace)"
    );

    let removed = client.remove_peer("laptop").await.expect("remove");
    assert!(removed);
    let removed_again = client.remove_peer("laptop").await.expect("remove again");
    assert!(!removed_again);

    shutdown.cancel();
    let _ = join.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_fingerprint_yields_daemon_error() {
    let handler = Arc::new(StubHandler::default());
    let (shutdown, path, join) = spin_server(handler).await;

    let mut client = IpcClient::connect(&path).await.expect("connect");
    let err = client
        .add_peer_fingerprint("laptop", "not-a-fingerprint")
        .await
        .expect_err("validation should reject");
    match err {
        IpcClientError::DaemonError(payload) => {
            assert_eq!(payload.code, IpcError::InvalidArgument.code());
            assert!(payload.message.contains("not-a-fingerprint"));
        }
        other => panic!("expected DaemonError, got {other:?}"),
    }

    shutdown.cancel();
    let _ = join.await;
}
