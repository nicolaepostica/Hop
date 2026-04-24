# Spec: Hop — Architecture

## Goal

Rewrite Hop entirely in Rust. Replace the hand-rolled event loop, the home-grown thread abstraction, and OpenSSL with a modern stack: `tokio`, `rustls`, `tracing`. Drop backwards compatibility with the C++ version and with Synergy/Barrier — that gives us room to design the wire protocol, the config, and the IPC from scratch, Rust-idiomatically, without legacy wire formats.

Motivations: eliminating a whole class of memory-safety bugs, native Wayland support via libei/portal without FFI hacks, simpler code thanks to standard crates, a type-safe protocol that can evolve through versioning.

## Background

The current implementation is C++14. It uses a hand-rolled `EventQueue`, a polling-thread `SocketMultiplexer`, per-platform `Arch*` facades, OpenSSL directly, XML config (inherited from Synergy), and a binary wire protocol with 4-byte ASCII message codes and big-endian encoding.

**Backwards compatibility with the C++ Hop and with Synergy/Barrier is NOT required.** There is no transition period between C++ and Rust — users upgrade server and clients simultaneously. That removes every legacy constraint on wire format, config, and IPC.

## Scope

**In scope:**

- `hops` (server) and `hopc` (client) in Rust
- A new v1 wire protocol on CBOR (RFC 8949) + length-delimited framing
- TLS via `rustls` + self-signed cert with fingerprint verification
- Platform backends: X11 (`x11rb`), macOS (`core-graphics`/`objc2`), Windows (`windows-rs`), Wayland (libei via `reis`)
- Clipboard sync (text, HTML, **files** — see §File clipboard below)
- GUI ↔ daemon IPC via Unix socket / Named pipe (`interprocess` crate)
- TOML config; one-shot migration tool from the old XML
- Tests: unit (`proptest` round-trip) + integration (network, IPC, platforms)

**Out of scope for the first iteration:**

- The Qt GUI is replaced with a new egui binary called `hop`
- Drag & drop — post-MVP
- Windows service (`hopd`) — a thin wrapper via the `windows-service` crate in M10, not a separate codebase

## Requirements

1. The Rust `hops` and `hopc` binaries replace the C++ versions; the user upgrades in a single step on every machine (server and clients).
2. All I/O is non-blocking on `tokio`. No polling thread, no hand-rolled multiplexer.
3. TLS via `rustls` (`tokio-rustls`); fingerprint DB in TOML with comments.
4. The platform layer is a `PlatformScreen` trait with AFIT (`-> impl Future`); `#[async_trait]` only where AFIT does not work (dyn-trait).
5. Wayland backend via libei (`reis` crate behind an internal `EiBackend` trait) + `xdg-desktop-portal` (`zbus`).
6. IPC via `interprocess::local_socket` (Unix socket on Linux/macOS, Named pipe on Windows); JSON-RPC-like protocol. `--ipc-tcp=<port>` flag for a remote GUI.
7. Config in TOML via `serde` + `figment` (layered: file → env → CLI). Paths via the `directories` crate (XDG-compliant on Linux).
8. Logging — `tracing` + `tracing-subscriber` with structured fields; forwarding logs to the GUI via a separate subscriber layer over IPC.
9. Errors — `thiserror` in library crates, `anyhow` only in binaries.
10. CI: `clippy -D warnings`, `rustfmt --check`, `cargo-nextest`, `cargo-deny` (licenses, advisories, duplicates).
11. `unsafe` is allowed only inside `platform/*/ffi.rs` modules with documented invariants (a `// SAFETY: ...` comment on every block).

## User / system flow

```block
[Primary machine]                            [Secondary machine]
PlatformScreen (X11/macOS/Win/EI)            PlatformScreen (X11/macOS/Win/EI)
        |                                            |
   InputEvent stream                          inject_*(key/mouse/...)
        |                                            |
    Server task  ──── TCP:25900 / TLS ────   Client task
        |                                            |
   ScreenRouter                               ServerProxy
        |
    IPC ── Unix socket / Named pipe ── GUI (egui)
```

**Handshake (v1):**

1. Server listens on `0.0.0.0:25900`.
2. Client connects, TLS handshake (`tokio-rustls`). The server verifies the client's fingerprint against the local DB.
3. Exchange `Hello` messages (CBOR) with `protocol_version: u16`, `display_name: String`, `capabilities: Vec<Capability>`.
4. Server sends `DeviceInfoRequest` → client replies with `DeviceInfo` carrying screen dimensions.
5. Connection active; `KeepAlive` every 3 s in both directions; 3 misses = `Disconnect { reason: KeepAliveTimeout }`.

## Technical approach

### Workspace layout (Cargo workspace)

```block
hop/
  Cargo.toml                   # [workspace] + [workspace.dependencies]
  rust-toolchain.toml          # pinned MSRV (stable, >= 1.75 for AFIT)
  rustfmt.toml
  clippy.toml
  deny.toml                    # cargo-deny config
  crates/
    common/                    # KeyId, ButtonId, ClipboardId, ModifierMask, etc.
    protocol/                  # CBOR message schema, codec
    net/                       # TcpListener/Stream, tokio-rustls, framing
    ipc/                       # interprocess + JSON-RPC
    config/                    # TOML via serde + figment
    server/                    # Server logic, ScreenRouter
    client/                    # Client logic, ServerProxy
    transfer/                  # File-clipboard engine (see §File clipboard)
    platform/
      core/                    # PlatformScreen trait, shared types
      x11/
      macos/
      windows/
      ei/                      # Wayland/libei
    hop-ui/                    # egui desktop UI
  bins/
    hops/                      # server
    hopc/                      # client
    hop/                       # desktop UI
    hop-migrate/               # one-shot XML → TOML migration
  xtask/                       # dev commands: xtask ci, xtask release, ...
```

### Key decisions

**Async runtime:** `tokio` multi-thread. Platform events (X11 / libei fd) are read in a dedicated `tokio::task` via `tokio::io::unix::AsyncFd`; events flow into an `mpsc` channel. The server/client core is a `tokio::select!` loop over channels and network I/O.

**Protocol (`protocol` crate):**

- Framing: `tokio_util::codec::LengthDelimitedCodec`, 4-byte BE length prefix, `max_frame_length = 16 MiB`.
- Serialisation: CBOR via `ciborium` (RFC 8949, cross-language friendly).
- Messages: `enum Message` with `#[serde(tag = "type")]`:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Hello(HelloPayload),
    DeviceInfoRequest,
    DeviceInfo(DeviceInfoPayload),
    KeyDown      { key: KeyId, mods: ModifierMask },
    KeyUp        { key: KeyId, mods: ModifierMask },
    KeyRepeat    { key: KeyId, mods: ModifierMask, count: u16 },
    MouseMove    { x: i32, y: i32 },
    MouseRelMove { dx: i32, dy: i32 },
    MouseButton  { button: ButtonId, down: bool },
    MouseWheel   { dx: i32, dy: i32 },
    ScreenEnter  { x: i32, y: i32, seq: u32, mask: ModifierMask },
    ScreenLeave,
    ClipboardGrab    { id: ClipboardId, seq: u32 },
    ClipboardRequest { id: ClipboardId, seq: u32 },
    ClipboardData    { id: ClipboardId, format: ClipboardFormat, data: Bytes },
    // File-clipboard — details in §File clipboard
    FileTransferStart  { transfer_id: TransferId, clipboard_seq: u32, manifest: FileManifest },
    FileChunk          { transfer_id: TransferId, entry_index: u32, offset: u64, data: Bytes },
    FileTransferEnd    { transfer_id: TransferId },
    FileTransferCancel { transfer_id: TransferId, reason: TransferCancelReason },
    KeepAlive,
    Disconnect { reason: DisconnectReason },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloPayload {
    pub protocol_version: u16,
    pub display_name: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Capability {
    FileClipboard,
    UnicodeClipboard,
    ClipboardHtml,
    #[serde(other)]
    Unknown,
}
```

Coordinates are `i32` (dropping Synergy's legacy `i16`). The protocol version lives only in `Hello` — negotiated at handshake; individual messages are not versioned.

**TLS:** `tokio-rustls`. On first start we generate a self-signed cert via `rcgen` and store it in `<config_dir>/tls/{cert.pem,key.pem}` with mode `0600`. Fingerprint DB — TOML:

```toml
# List of trusted peers. Added automatically after confirming the
# fingerprint via GUI/CLI, or by hand.

[[peer]]
name = "laptop"
fingerprint = "sha256:abc123..."
added = "2026-04-22"
```

**Platform trait (AFIT):**

```rust
pub trait PlatformScreen: Send + Sync {
    fn inject_key(&self, key: KeyId, mods: ModifierMask, down: bool)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_button(&self, btn: ButtonId, down: bool)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_move(&self, x: i32, y: i32)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_wheel(&self, dx: i32, dy: i32)
        -> impl Future<Output = Result<()>> + Send;
    fn get_clipboard(&self, id: ClipboardId, format: ClipboardFormat)
        -> impl Future<Output = Result<Bytes>> + Send;
    fn set_clipboard(&self, id: ClipboardId, format: ClipboardFormat, data: Bytes)
        -> impl Future<Output = Result<()>> + Send;
    fn read_file_clipboard(&self)
        -> impl Future<Output = Result<Option<Vec<PathBuf>>>> + Send;
    fn write_file_clipboard(&self, paths: &[PathBuf])
        -> impl Future<Output = Result<()>> + Send;
    fn screen_info(&self) -> ScreenInfo;
    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send;
}
```

Where `dyn PlatformScreen` is needed (e.g. runtime backend selection), we use a separate `dyn`-friendly wrapper trait built on `#[async_trait]` that wraps the AFIT version.

**Wayland/libei:** the `reis` crate behind a thin internal `EiBackend` trait (~3 methods: `create_session`, `poll_events`, `emit_*`). If `reis` stalls, we can swap to `bindgen` — only `platform/ei/backend.rs` needs rewriting.

**Server routing:** `tokio::sync::mpsc` between tasks: `PlatformReader` → `Coordinator` → `ClientProxy` (one per client). Screen layout — `Arc<arc_swap::ArcSwap<ScreenLayout>>` for a lock-free hot-path read. Details: `specs/milestones/M11-coordinator.md`.

**IPC (`ipc` crate):** `interprocess::local_socket` for Unix-domain / Named pipes. Paths:

- Linux: `$XDG_RUNTIME_DIR/hop/daemon.sock`
- macOS: `$TMPDIR/hop/daemon.sock`
- Windows: `\\.\pipe\hop-daemon`

Protocol — newline-delimited JSON in JSON-RPC 2.0 style (`{ "id", "method", "params" }` + notifications without `id`). Methods: `get_status`, `reload_config`, `add_peer_fingerprint`, `subscribe_logs`, ...

**Config (`config` crate):** `figment` layers:

1. Defaults (hard-coded in the crate)
2. `<config_dir>/config.toml`
3. `HOP_*` env vars
4. CLI arguments (`clap` derive)

Typed structs, validation via `TryFrom<RawConfig, Error = ConfigError>`.

**Logging:** `tracing` + `tracing-subscriber` with:

- `fmt` layer → stderr (human / JSON controlled by env var)
- `ipc` layer → serialises events into the IPC channel for the GUI

**Errors:** `thiserror` per-crate:

```rust
// protocol/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("CBOR decode failed at byte {offset}: {source}")]
    Decode { offset: u64, source: ciborium::de::Error<std::io::Error> },
    #[error("frame exceeds max size: {size} > {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("unsupported protocol version: {got}, expected {expected}")]
    VersionMismatch { got: u16, expected: u16 },
}
```

Binaries use `anyhow::Result<()>` in `main` + `.context(...)` at the boundaries.

**Paths:** `directories::ProjectDirs::from("com", "Hop", "hop")`.

**Reconnect:** `backoff` crate, exponential, 1 s → 30 s max, with jitter.

**Task management:** every `tokio::spawn` is wrapped in a `tokio::task::JoinSet`. A task panic → `tracing::error!` + a supervised restart attempt; the process does not die.

## File clipboard (M9)

Extends the shared clipboard so that copying files/folders in a file manager (Ctrl+C) and pasting on another computer (Ctrl+V) transfers the files over the network and lands them in the destination folder.

### Background: platform formats

At the OS level, files in a clipboard look like:

- **Windows:** `CF_HDROP` — a path list + a `DROPFILES` struct.
- **Linux/X11:** MIME type `text/uri-list` — `file:///path\r\n`-delimited list of URIs.
- **macOS:** `NSFilenamesPboardType` / `public.file-url` — array of paths on the pasteboard.
- **Wayland:** `text/uri-list` via `wl_data_device` / `ext-data-control`.

### Scope

**In scope:**

- Capability `Capability::FileClipboard` in the handshake.
- A `ClipboardFormat::Files` format with typed contents.
- Detecting a file clipboard on each platform and reading the list of paths/URIs.
- Transferring a file tree (recursively) over the network, symmetrically.
- Writing received files into a configurable destination folder (drop directory).
- Progress notifications to the GUI via IPC on both sides.
- Support for Windows, macOS, Linux/X11, Linux/Wayland.

**Out of scope:**

- Drag & drop (separate feature, post-MVP).
- Real-time sync of file changes.
- Name-conflict dialog (v1: auto-suffix `_1`, `_2`).
- Symlink transfer (v1 skips with warn).
- Preserving permissions/xattr/ACL (v1 carries only contents and names).

### Requirements

1. Ctrl+C on one or more files/folders → Ctrl+V on another machine → all files appear in the target machine's drop directory.
2. Folders are copied recursively; the nesting is preserved.
3. Transfer goes over the same TLS connection (port 25900); no separate channel.
4. Clipboard sync is **pull-on-demand**: a grab announces ownership, contents move only on an explicit paste.
5. Drop directory is configurable; default — `<user_download_dir>/Hop/` via `directories::UserDirs`.
6. For transfers > 10 MB, IPC emits progress events to the GUI (sender and receiver) every 5% or 1 s, whichever comes first.
7. If the receiver does not announce `Capability::FileClipboard` in `Hello` — fallback to text clipboard.
8. Cancellation or connection drop — `.part` files are cleaned up via a `Drop` guard.
9. Max size per transfer is capped by config (default 2 GiB).
10. Symmetry: client → server works identically.

### Flow

```block
[Machine A]                                 [Machine B]

1. User presses Ctrl+C on files
   PlatformScreen::read_file_clipboard() →
       Vec<PathBuf> (or None if clipboard isn't file-typed)
   → local state: FileClipboardSlot { paths, seq }

2. Cursor crosses over to B (screen switch)
   → Message::ClipboardGrab { id: File, seq }

3. User presses Ctrl+V on B
   → Message::ClipboardRequest { id: File, seq }

4. A receives ClipboardRequest:
   → spawn TransferSender task:
       Message::FileTransferStart { transfer_id, manifest }
       Message::FileChunk { transfer_id, entry_index, offset, data }  (many)
       Message::FileTransferEnd { transfer_id }
   → IPC: ProgressEvent { direction: Sending, percent, ... }

5. B receives the stream:
   → spawn TransferReceiver task
   → writes files under drop_directory/<transfer_id>/ as .part
   → on FileTransferEnd — atomic rename .part → final name
   → inject URI list into OS clipboard of the received files
   → IPC: ProgressEvent { direction: Receiving, ... }

6. When done: IPC → GUI: TransferComplete { count, bytes }
```

### Types (`common` crate)

```rust
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    Text,
    Html,
    Bitmap,
    Files,       // <-- new
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileManifestEntry {
    pub rel_path: PathBuf,   // path relative to the copy root
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileManifest {
    pub entries: Vec<FileManifestEntry>,
    pub total_bytes: u64,
}

pub type TransferId = u64;   // unique per connection

#[derive(Debug, Serialize, Deserialize)]
pub enum TransferCancelReason {
    UserCancelled,
    DiskFull,
    SizeMismatch,       // the file on the sender's disk changed
    PeerError,
    PathTraversal,
}
```

**Why `entry_index` + `offset` on `FileChunk`:** a defence against any state desynchronisation; the receiver can write to the right file at the right position even if "which file are we in the middle of?" state is lost (useful for a future resume-after-reconnect).

**Chunk size:** 64 KiB by default (`FILE_CHUNK_BYTES: usize = 65_536`). Lower bound — MTU-friendly, upper bound — `max_frame_length` (16 MiB).

### Capability negotiation

Both sides advertise `Capability::FileClipboard` in `HelloPayload::capabilities`. On `ClipboardGrab { id: Files }` the server checks the receiver advertised the capability. If not — it doesn't send a file-grab; the text clipboard keeps working.

### Implementation (`transfer` crate)

**Sender** — `TransferSender` task:

- Walks the tree recursively via `tokio::fs::read_dir`.
- Builds a `FileManifest`, checks `total_bytes <= max_transfer_bytes`.
- Reads files chunk-wise via `tokio::fs::File::read_buf`.
- Sends `FileChunk` frames, honouring backpressure from the `net` crate (bounded `mpsc`).

**Receiver** — `TransferReceiver` task:

- Creates `<drop_dir>/<transfer_id>/` as a staging directory.
- Writes every file as `<rel_path>.part`.
- On `FileTransferEnd` — atomic `tokio::fs::rename` into `<drop_dir>/<manifest_root_name>/`.
- Name collisions — `name_1`, `name_2`, ... without a dialog.
- Injects a URI list into the OS clipboard via `write_file_clipboard`.

**Cancellation:**

- Either side sends `FileTransferCancel`.
- A `Drop` guard on the staging directory removes every `.part` file if the task dies.

### Security

- **Path traversal:** every `rel_path` in the manifest is validated:
  - No `..` components.
  - Not absolute, doesn't start with `/` or a drive letter.
  - After `canonicalize` still inside the staging dir.
  - Violation — `FileTransferCancel { reason: PathTraversal }` + `tracing::error!` as a security event.
- **Symlinks:** the sender skips symlinks with `tracing::warn!` (v1); v2 could add a follow option.
- **Max size:** from config, default 2 GiB. Over the limit — reject at manifest time.

### Configuration

```toml
[file_transfer]
enabled = true
drop_directory = "~/Downloads/Hop"       # expanded via directories
max_transfer_bytes = 2_147_483_648       # 2 GiB
chunk_bytes = 65536
follow_symlinks = false                  # reserved for future
```

`drop_directory` resolves via `shellexpand` + `directories::UserDirs::download_dir()` as fallback.

### IPC events (to the GUI)

```rust
pub enum IpcNotification {
    TransferStarted {
        transfer_id: TransferId,
        direction: TransferDirection,
        peer_name: String,
        total_bytes: u64,
        file_count: u32,
    },
    TransferProgress {
        transfer_id: TransferId,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    TransferCompleted {
        transfer_id: TransferId,
        bytes_transferred: u64,
    },
    TransferCancelled {
        transfer_id: TransferId,
        reason: TransferCancelReason,
    },
}

pub enum TransferDirection { Sending, Receiving }
```

## Edge cases & error handling

### General

- **Protocol-version mismatch:** `Disconnect { reason: ProtocolVersionMismatch { server, client } }`, graceful close.
- **TLS handshake timeout (10 s):** drop the connection, `tracing::warn!` with the client IP.
- **Fingerprint mismatch:** `Disconnect { reason: UnknownPeer }`, log the client's fingerprint for manual addition to the DB. The GUI can raise a prompt.
- **Platform backend unavailable (libei < 1.0, no X11):** `anyhow::bail!` at start with a specific hint (`install libei >= 1.0` / `DISPLAY unset`).
- **CBOR decode error:** `Disconnect { reason: MalformedMessage }`, don't panic. Log a hex dump of the first 64 bytes of the frame for triage.
- **Frame > 16 MB:** `LengthDelimitedCodec` returns an error → `Disconnect { reason: FrameTooLarge }`.
- **Clipboard > 1 MB (text):** truncate to the limit, `tracing::warn!` with the original size.
- **Reconnect loop:** client with `backoff`, 1 s → 30 s max, jitter 0–25%.
- **Tokio task panic:** `JoinSet` with a supervisor; a task falling → log + restart that task; the process survives.
- **IPC socket already exists:** on startup the server removes a stale socket after confirming no live process owns it (via an advisory lock file next to the socket).
- **IPC security:** Unix socket with mode `0600`; Named pipe with an ACL for `current_user`.

### File-clipboard specific

- **File changed during read:** the sender compares actual bytes read against `size` from the manifest; mismatch → `FileTransferCancel { reason: SizeMismatch }`.
- **Out of disk on the receiver:** `tokio::io::Error { kind: StorageFull }` → `FileTransferCancel { reason: DiskFull }`, delete `.part` files.
- **Name collision in drop_dir:** auto-suffix `_1`, `_2`, ... without a dialog.
- **Path traversal:** immediate `FileTransferCancel { reason: PathTraversal }`, drop the connection, `tracing::error!` with the peer name and offending path.
- **Connection drop mid-transfer:** a `Drop` guard on `TransferReceiver` removes the whole staging directory.
- **Clipboard grabbed by another process (X11):** `read_file_clipboard` returns `Ok(None)`, silent fallback to text.
- **Old peer without `Capability::FileClipboard`:** we don't announce a file-grab; text clipboard keeps working.
- **Empty manifest (copying an empty folder):** legal, create an empty folder in drop_dir.
- **Manifest with zero `total_bytes` but non-zero `entries.len()`:** legal (all files are empty). Handle normally.
- **Backpressure:** if the receiver falls behind, the bounded `mpsc` inside the `net` crate creates backpressure on `TransferSender::read_file` — it blocks on `await`.
- **Multiple concurrent transfers:** supported (`transfer_id` is unique); each has its own task and staging dir.

## Implementation order

Detailed milestone plan: `specs/milestones/`. In brief:

| M   | Artefact                                                            | Status |
|-----|---------------------------------------------------------------------|--------|
| M0  | Workspace skeleton, CI, tooling                                     | ✅     |
| M1  | `protocol` crate: CBOR messages, property tests                     | ✅     |
| M2  | `net` crate: TCP + TLS + handshake + mock screen                    | ✅     |
| M3  | `platform/x11`: working server+client on Linux/X11                  | ✅     |
| M4  | Clipboard (text/HTML) + TOML config                                 | ✅     |
| M5  | `ipc` + GUI adaptation to the new IPC                               | ✅     |
| M6  | `platform/ei`: Wayland/libei (experimental)                         | ✅     |
| M7  | `platform/macos`                                                    | ✅     |
| M8  | `platform/windows`                                                  | ✅     |
| M9  | File clipboard (see §File clipboard above)                          | ✅     |
| M10 | Windows service wrapper (`hops --service`)                          | ✅     |
| M11 | Screen-crossing coordinator (see `milestones/M11-coordinator.md`)   | ✅     |
| M12 | Release packaging (see `milestones/M12-release-packaging.md`)       | ✅     |
| M13 | GUI ↔ backend wiring (see `milestones/M13-gui-backend.md`)          | planned |

## Open questions

1. **Screen layout: `arc_swap` vs `tokio::sync::RwLock`** — we picked `arc_swap` in M11 for the lock-free hot path. Revisit only if pathological write loads appear.
2. **`hop-migrate` XML → TOML** — a separate binary, not in the default install; stays as an optional tool for migrators from C++ Hop.
3. **Permissions/xattr on file-clipboard receive:** v1 ignores (umask of the current user). v2 — only on explicit request; cross-platform semantics are complicated.
4. **Compression for file trees:** a zstd stream over `FileChunk`? Deferred to v2 pending real-world measurements.
