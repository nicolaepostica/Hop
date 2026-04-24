# M2 — `net` crate: TCP + TLS + handshake + MockScreen

## Goal

Land a working end-to-end channel "Rust server ↔ Rust client" over TCP + TLS, with a complete handshake and keep-alive exchange — still without a real platform backend (MockScreen). After M2 you can run `hops` and `hopc` on localhost, they complete the handshake, swap `DeviceInfo`, and trade `KeepAlive` frames until `Ctrl+C`.

## Prerequisites

- [M0](M0-skeleton.md) — workspace
- [M1](M1-protocol.md) — `protocol` crate

## Scope

**In scope:**
- `net` crate: `Listener`, `ConnectedStream`, TCP+TLS abstraction
- Self-signed cert generation via `rcgen` on first start
- Load / save cert + key under `<config_dir>/tls/`
- Fingerprint DB loader (TOML), basic CRUD
- Handshake state machine: TLS handshake → `Hello` exchange → `DeviceInfo` exchange → active
- KeepAlive timer via `tokio::time::interval`, 3 misses = disconnect
- `MockScreen` in `platform/core` for testing without a real backend
- Minimal CLI for `hops` and `hopc`: `--listen`, `--connect`, `--fingerprint`, `--name`
- Integration test: two `tokio::spawn`-ed process emulators run the full handshake + 5 seconds of KeepAlive
- Graceful shutdown via `tokio::signal::ctrl_c` + `Disconnect { reason: UserInitiated }`

**Out of scope:**
- Real platform I/O (M3+)
- Clipboard (M4)
- IPC with GUI (M5)
- TOML config file (M4) — we use CLI args only for now
- Hot-reload of the certificate / fingerprint DB (future, not MVP)

## Tasks

### `net` crate — transport

- [ ] `crates/net/src/tls.rs`:
  - `pub struct TlsConfig { server_config, client_config, fingerprint_db }`
  - `fn load_or_generate_cert(dir: &Path) -> Result<(Certificate, PrivateKey)>`:
    - If `cert.pem` and `key.pem` exist in `dir` — load via `rustls-pemfile`
    - Otherwise — generate via `rcgen::generate_simple_self_signed(vec!["hop-host".into()])`, store with mode `0600` on Unix / appropriate ACL on Windows
  - Custom `rustls::server::ClientCertVerifier` and `rustls::client::ServerCertVerifier` — verify through the fingerprint DB (not a CA chain)
- [ ] `crates/net/src/fingerprint.rs`:
  - `struct FingerprintDb` — wrapper around `Vec<PeerEntry>`
  - `struct PeerEntry { name: String, fingerprint: Fingerprint, added: chrono::DateTime<Utc> }`
  - `struct Fingerprint([u8; 32])` (SHA-256) with `Display`/`FromStr` (format `sha256:hex`)
  - `fn load(path: &Path) -> Result<FingerprintDb>` — TOML via the `toml` crate
  - `fn save(&self, path: &Path) -> Result<()>`
  - `fn contains(&self, fp: &Fingerprint) -> Option<&PeerEntry>`
- [ ] `crates/net/src/listener.rs`:
  - `pub struct Listener { tcp: TcpListener, tls: Arc<ServerConfig> }`
  - `async fn accept(&self) -> Result<ConnectedStream>` — accept TCP → TLS handshake → return a ready `ConnectedStream`
  - TLS handshake runs in a spawned task with a 10 s timeout
- [ ] `crates/net/src/client.rs`:
  - `pub async fn connect(addr: SocketAddr, tls: Arc<ClientConfig>) -> Result<ConnectedStream>`
- [ ] `crates/net/src/stream.rs`:
  - `pub struct ConnectedStream` — wraps `tokio_rustls::TlsStream<TcpStream>`
  - Accessor for the peer fingerprint (pulled from `CertificateDer` via `rustls-pki-types`)
  - `into_framed(self) -> Framed<..., MessageCodec>` — bridge to `protocol`

### Handshake state machine

- [ ] `crates/net/src/handshake.rs`:
  ```rust
  pub struct HandshakeResult {
      pub peer_name: String,
      pub peer_capabilities: Vec<Capability>,
      pub peer_device_info: DeviceInfoPayload,
  }

  pub async fn server_handshake(
      conn: &mut Framed<ConnectedStream, MessageCodec>,
      our_info: &HelloPayload,
      our_device: &DeviceInfoPayload,
  ) -> Result<HandshakeResult, HandshakeError>;

  pub async fn client_handshake(...) -> Result<HandshakeResult, HandshakeError>;
  ```
- Steps (server):
  1. Wait for `Hello` from the client with a 5 s timeout
  2. Validate `protocol_version == 1`
  3. Send our `Hello`
  4. Send `DeviceInfoRequest`
  5. Wait for `DeviceInfo` with a 5 s timeout
  6. Return `HandshakeResult`
- Client — symmetric (initiates `Hello` first, answers `DeviceInfoRequest`)
- Errors: `HandshakeError` via `thiserror` with a variant per phase

### KeepAlive

- [ ] `crates/net/src/keepalive.rs`:
  - `pub struct KeepAliveTask { tx: mpsc::Sender<Message>, last_seen: Arc<AtomicU64> }`
  - `spawn_keepalive(tx, last_seen)` — `tokio::time::interval(3s)` emits `KeepAlive` + checks `last_seen`; if > 9 s, it sends `Disconnect { reason: KeepAliveTimeout }` and exits
  - Inbound `KeepAlive` simply updates `last_seen` (atomically)

### MockScreen

- [ ] `crates/platform/core/src/mock.rs`:
  - `pub struct MockScreen { events: Mutex<Vec<RecordedEvent>>, ... }`
  - Full `PlatformScreen` impl — records every `inject_*` into an in-memory log; `event_stream` returns a preloaded `Vec<InputEvent>`
  - Used in M2 tests (server/client) and up to M3

### Minimal `hops` / `hopc` mains

- [ ] `crates/server/src/lib.rs`:
  - `pub async fn run(config: ServerConfig, screen: impl PlatformScreen) -> Result<()>` — accept loop + per-client task
  - Every incoming connection → handshake → `select!` loop over `{ incoming Message | keepalive | shutdown }`
  - At M2: receives `MouseMove`/`KeyDown` — just logs via `tracing::info!`, does not inject (that's M3+)
- [ ] `crates/client/src/lib.rs`:
  - `pub async fn run(config: ClientConfig, screen: impl PlatformScreen) -> Result<()>` — connect + handshake + event loop
  - The event side (`screen.event_stream`) pushes into the void for now (MockScreen returns an empty stream)
- [ ] `bins/hops/src/main.rs`:
  - `clap` derive with `--listen 0.0.0.0:24800`, `--name`, `--cert-dir`, `--fingerprint-db`
  - `tracing_subscriber::fmt().init()`
  - Create `MockScreen`, call `server::run`
  - Clean shutdown on `SIGINT` / `Ctrl+C`
- [ ] `bins/hopc/src/main.rs`:
  - Same shape, with `--connect 127.0.0.1:24800`, `--server-fingerprint`, `--name`

### Tests

- [ ] `crates/net/tests/handshake.rs`:
  - `#[tokio::test]` spins up a `Listener` on `127.0.0.1:0` (random port) and connects via `connect`
  - Runs the full handshake, asserts on `HandshakeResult` from both sides
  - Verifies `DeviceInfo` round-trips correctly
- [ ] `crates/net/tests/handshake_failures.rs`:
  - TLS timeout: client opens TCP but never sends TLS hello — server drops after 10 s
  - Hello timeout: TLS completes, client never sends `Hello` — server drops after 5 s
  - Wrong protocol_version: client sends `Hello { protocol_version: 999 }` — server replies with `Disconnect { reason: ProtocolVersionMismatch }` and closes
  - Unknown fingerprint: client with an unknown fingerprint — verifier rejects; the connection never establishes
- [ ] `crates/net/tests/keepalive.rs`:
  - Two peers, one stops sending `KeepAlive` (simulated via a mock) — the other drops with `KeepAliveTimeout` after ~9 s (use `tokio::time::pause/advance` for determinism)
- [ ] `tests/e2e.rs` (workspace-level, not per-crate):
  - Spawns `hop_server::run` and `hop_client::run` in two `tokio::spawn` tasks on random ports
  - Via `MockScreen`, verifies the handshake completes, 3 KeepAlive cycles go back and forth, then a graceful shutdown via a cancellation token
  - Test timeout: 15 seconds

### Fingerprint DB CRUD

- [ ] CLI subcommands `hops fingerprint add <name> <fp>` and `hops fingerprint list`
- [ ] File format:
  ```toml
  # <config_dir>/fingerprints.toml
  [[peer]]
  name = "laptop"
  fingerprint = "sha256:abcdef..."
  added = "2026-04-22T10:00:00Z"
  ```

### Logging

- [ ] On handshake — `tracing::info!(peer = %name, fingerprint = %fp, "peer connected")`
- [ ] On disconnect — `tracing::info!(peer = %name, reason = ?r, "peer disconnected")`
- [ ] On errors — `tracing::warn!` or `error!` depending on severity
- [ ] `RUST_LOG=hop=debug` — env-controlled verbosity

## Acceptance criteria

- [ ] `cargo run --bin hops -- --listen 127.0.0.1:24800 --name server-a` starts, listens on the port, logs `fingerprint: sha256:...`
- [ ] `cargo run --bin hopc -- --connect 127.0.0.1:24800 --server-fingerprint sha256:... --name client-b` connects, handshake succeeds, both print `peer connected`
- [ ] On `Ctrl+C` from either side — graceful disconnect, both processes exit with code 0
- [ ] The e2e integration test passes in CI (Linux/macOS/Windows) in under 15 seconds
- [ ] All unit/integration tests are green
- [ ] Clippy `-D warnings` is green
- [ ] `CHANGELOG.md` gets a "M2: TLS handshake + KeepAlive" entry

## Tests

Everything is listed under "Tests" above. For determinism of KeepAlive tests, use `tokio::time::pause` / `advance` instead of real `sleep`. For network tests — `127.0.0.1:0` (random port) + `tokio::net::lookup_host`.

## Risks / open questions

1. **`rustls` custom-verifier API** changes across major `rustls` versions. Pin the exact version in workspace deps and README. Plan: use `rustls::server::danger::ClientCertVerifier` (with the `danger` feature) — that's the canonical way to implement self-signed + fingerprint model.
2. **Fingerprint DB race:** if GUI and daemon both write `fingerprints.toml` at once — conflict. Not solved in M2 (only the daemon writes); in M5 — via an IPC `add_peer_fingerprint` command (daemon is the sole writer).
3. **CN/SAN in the self-signed cert:** what to put there? Suggested: `hop-<random-suffix>` as SAN and DNS name. The verifier looks only at the fingerprint, not CN.
4. **Windows cert-file permissions:** the `0600` equivalent via `windows-acl` or `windows-rs`. First pass — keep the file in the user profile dir (already OS-isolated); ACL is optional hardening.
5. **Graceful shutdown of both sides:** who initiates `Disconnect`? The one that received `Ctrl+C`. The other side sees `Disconnect` → closes the stream → exits the event loop. Verify in the e2e test.
6. **`bytes::Bytes` vs `Vec<u8>` inside `ClipboardData`:** irrelevant for M2 (clipboard lands in M4); mentioned here so it isn't forgotten when designing `ClipboardData` in M1.

## Readiness for M3

After M2 we have:
- Working TCP+TLS+handshake on top of `MockScreen`
- Full connection lifecycle
- Fingerprint-based trust model

M3 adds a real `platform/x11` backend — `MockScreen` is swapped for `X11Screen`, nothing else changes.
