# M0 — Workspace skeleton, CI, tooling

## Goal

Prepare an empty but fully-configured Cargo workspace with CI, lints, and dev tooling. After M0 every new line of code lands in a correctly-configured environment — we won't have to come back and retrofit CI/tooling later.

## Prerequisites

None.

## Scope

**In scope:**
- Cargo workspace with empty crates matching the structure in the main spec
- Pinned versions for all core dependencies in `[workspace.dependencies]`
- GitHub Actions CI: Linux + macOS + Windows
- Lints, formatting, deny checks
- `xtask` for dev commands
- Pinned MSRV

**Out of scope:**
- Any product code in the crates (only `//! TODO` modules)
- Real tests (only infrastructure checks)
- GUI changes

## Tasks

### Workspace

- [ ] Create root `Cargo.toml` with `[workspace]` + `resolver = "2"`
- [ ] `rust-toolchain.toml` with `channel = "stable"`, pin MSRV (>= 1.75 for AFIT)
- [ ] Create empty library crates:
  - `crates/common/`
  - `crates/protocol/`
  - `crates/net/`
  - `crates/ipc/`
  - `crates/config/`
  - `crates/server/`
  - `crates/client/`
  - `crates/platform/core/`
  - `crates/platform/x11/` (with crate-level `#[cfg(target_os = "linux")]` gate)
  - `crates/platform/macos/` (with `#[cfg(target_os = "macos")]`)
  - `crates/platform/windows/` (with `#[cfg(windows)]`)
  - `crates/platform/ei/` (with `#[cfg(target_os = "linux")]`)
- [ ] Create empty binary crates:
  - `bins/hops/` with a `fn main()` that prints the version
  - `bins/hopc/` with a `fn main()` that prints the version
  - `bins/hop-migrate/` (behind a feature flag, not built by default)
- [ ] Create `xtask/` with `cargo xtask ci`, `cargo xtask fmt` stubs
- [ ] Each crate: `lib.rs` with `#![deny(warnings, unsafe_code)]` (snap-level — `unsafe` is only allowed inside `platform/*/ffi.rs` via a local `#[allow(unsafe_code)]`)

### Dependencies (populate `[workspace.dependencies]`)

Preliminary list — concrete versions picked as the latest stable at M0 time:

- `tokio` (multi-thread, full features disabled by default, per-crate opt-in)
- `tokio-util` (codec)
- `tokio-rustls`, `rustls`, `rustls-pemfile`, `rcgen`
- `bytes`
- `serde`, `serde_json`
- `ciborium`
- `thiserror`, `anyhow`
- `tracing`, `tracing-subscriber`
- `clap` (derive)
- `figment` (toml + env)
- `directories`
- `interprocess`
- `backoff`
- `arc-swap`
- `x11rb` (only in `platform/x11`)
- `reis` (only in `platform/ei`)
- `windows` (only in `platform/windows`)
- `objc2`, `core-graphics` (only in `platform/macos`)
- Dev: `proptest`, `insta`, `tokio-test`, `rstest`

### Tooling configs

- [ ] `rustfmt.toml`:
  ```toml
  edition = "2021"
  max_width = 100
  imports_granularity = "Module"
  group_imports = "StdExternalCrate"
  ```
- [ ] `clippy.toml`:
  ```toml
  msrv = "1.75.0"
  avoid-breaking-exported-api = false
  ```
- [ ] `deny.toml` for `cargo-deny`:
  - advisories: deny vulnerabilities
  - licenses: allow MIT/Apache-2.0/BSD-3-Clause/ISC/Unicode-DFS-2016; deny GPL
  - bans: deny multiple versions of `syn`, `tokio`, `rustls`
- [ ] `.gitignore`: `/target`, `.DS_Store`, `*.swp`
- [ ] `.editorconfig` (4 spaces, LF, UTF-8 per CLAUDE.md coding conventions)

### CI (GitHub Actions)

- [ ] `.github/workflows/ci.yml`:
  - Matrix: `{ ubuntu-latest, macos-latest, windows-latest }` × stable toolchain
  - Steps:
    - `cargo fmt --all --check`
    - `cargo clippy --workspace --all-targets -- -D warnings`
    - `cargo build --workspace --all-targets`
    - `cargo nextest run --workspace`
    - `cargo-deny check` (Linux only)
  - Caching via `Swatinem/rust-cache@v2`
- [ ] `.github/workflows/release.yml` — stub with a manual trigger (fleshed out in M10)

### xtask

- [ ] `cargo xtask ci` — runs the same suite as CI, locally
- [ ] `cargo xtask fmt` — `cargo fmt` + TOML formatting (`taplo fmt`)
- [ ] `cargo xtask udeps` — `cargo +nightly udeps` to surface unused dependencies (optional command)

### Documentation

- [ ] `README.md` at the root:
  - Short project description
  - Build instructions (`cargo build --workspace`)
  - Link to `specs/`
- [ ] `CONTRIBUTING.md`:
  - How to run CI locally (`cargo xtask ci`)
  - Coding conventions (`snake_case` per Rust, 100-char lines)
  - Where tests live

## Acceptance criteria

- [ ] `cargo build --workspace` passes on Linux/macOS/Windows
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings
- [ ] `cargo fmt --all --check` — clean
- [ ] `cargo nextest run --workspace` — 0 tests, 0 failures (OK for M0)
- [ ] `cargo deny check` — green
- [ ] CI workflow triggers on push/PR and passes on all three OSes
- [ ] `./target/release/hops --version` and `./target/release/hopc --version` print the correct version from `Cargo.toml`
- [ ] Every crate shows up in `cargo tree --workspace`

## Tests

M0 has no real code, but the infrastructure must be in place:
- [ ] Dummy `#[test] fn smoke() { assert_eq!(2 + 2, 4); }` in `crates/common/tests/` — proves `cargo nextest` actually runs something
- [ ] A one-shot CI job that fails on a deliberate `cargo clippy` warning — manual check for one iteration, then remove

## Risks / open questions

1. **Tokio feature flags in workspace deps:** set `default-features = false` at the workspace root and opt into specific features per-crate? Or the opposite — `full` at the workspace root and disable per-crate? Recommend the first (explicit features → less compile time).
2. **`reis` version is unstable.** Before M0, check that the current release builds. If not — defer to M6 and add `reis` to `[workspace.dependencies]` as `optional = true` without pulling it in.
3. **`cargo-deny` on Windows:** historically flakes on licenses — run it on Linux only; that's fine.
4. **MSRV 1.75:** the minimum for AFIT-in-traits. If a newer stable ships before M0 starts — bump (never drop below 1.75).
5. **Naming:** workspace-package = `hop`? Or keep `input-leap` and rename after removing C++? Recommend `hop` straight away so build artefacts don't collide with the C++ tree on CI during the transition (even without wire-compat, artefact paths can overlap).
