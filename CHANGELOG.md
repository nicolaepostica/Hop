# Changelog

All notable changes to this project will be documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### M2 — TCP, mTLS, and the application handshake

- `net` crate: `Listener`, `connect`, `ConnectedStream` (TLS-wrapped
  TCP with peer fingerprint attached), application `run_handshake`
  (symmetric `Hello` + `DeviceInfo` exchange), `KeepAliveTracker`
  (3 s interval, 9 s timeout).
- Self-signed identity: `load_or_generate_cert` generates a fresh
  `cert.pem`/`key.pem` pair via `rcgen` on first run and restricts the
  key to `0600` on Unix.
- `FingerprintVerifier` implements both `ServerCertVerifier` and
  `ClientCertVerifier` — mTLS with CA chains ignored, trust anchored
  in a TOML-backed `FingerprintDb`.
- `platform/core`: `PlatformScreen` trait with async-fn-in-trait
  methods (`inject_*`, `get/set_clipboard`, `event_stream`) and a
  ready-to-use `MockScreen` for tests.
- `server` crate: `Server::bind` / `Server::serve` split so tests can
  learn the OS-assigned port before entering the accept loop; convenience
  `run` keeps the single-call API for binaries.
- `client` crate: `run` connects, handshakes, and enters a keep-alive
  `select!` loop with graceful shutdown.
- `hops` / `hopc` binaries gain `clap`-based CLIs with
  a `fingerprint {add,list,remove,show}` subcommand for peer management
  and `tracing_subscriber` logging.
- Tests (all green):
  - `net::fingerprint` — round-trip through string / TOML, misuse
    rejection.
  - `net::keepalive` — three timed property tests with
    `tokio::time::pause`.
  - `net/tests/handshake.rs` — happy path, wrong protocol version,
    unknown fingerprint rejected at the TLS layer.
  - `server/tests/e2e.rs` — spawns `server::run` + `client::run` in
    one process, verifies handshake + keep-alive, graceful shutdown
    within 15 s budget (finishes in ~1.5 s).

### M1 — CBOR wire protocol

- `protocol` crate with 17 `Message` variants (CBOR, adjacently tagged
  via `#[serde(tag = "type")]`, 16 MiB max frame, 4-byte BE length
  prefix).
- `MessageCodec` (hand-rolled framing after the `LengthDelimitedCodec`
  peek proved racy on partial frames).
- Tests: property round-trip via proptest (256 cases × 3 strategies),
  `insta` golden snapshots for all 17 variants, six framing edge cases.

### M0 — Workspace skeleton

- Cargo workspace (resolver 2) with 12 library crates, 3 binaries, and
  an `xtask` runner.
- GitHub Actions CI matrix (Linux / macOS / Windows) running
  `fmt --check`, `clippy -D warnings`, `build`, `nextest`, and
  `cargo-deny`.
- Pinned `[workspace.dependencies]` covering tokio, rustls, serde,
  ciborium, tracing, clap, and the platform crates.
