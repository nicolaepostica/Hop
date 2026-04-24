# M13 — GUI ↔ Backend Wiring

## Goal

Turn the `hop` desktop app from a styled shell into a working controller for the server and client daemons. Today the Start/Connect buttons only flip an in-memory `bool`; after M13 they actually launch `hop_server::run` / `hop_client::run`, the user sees a live peer list, server admins can add a client's fingerprint through the UI, and the window defaults auto-populate with the real hostname and LAN IP.

Covers five pending session-todo items:

- **#11** — default display name = real hostname, not the literal `"this-computer"`.
- **#12** — show the server's LAN-reachable IP in the Server view.
- **#13** — Start button actually runs `hop_server::run`.
- **#14** — restore the `[+ Add]` button in "Connected peers" to add a client's fingerprint.
- **#15** — Connect button actually runs `hop_client::run`.

## Prerequisites

- [M11](M11-coordinator.md) — coordinator (done).
- [M12](M12-release-packaging.md) — packaging (done).

## Scope

**In scope:**
- An embedded `tokio` runtime inside the eframe app (no child-process spawning in MVP).
- A `BackendController` that owns the server or the client task, plus the `CancellationToken` used to stop it.
- A bidirectional channel between the egui thread and backend tasks — egui sends commands (Start, Stop, AddPeer), the backend emits `StatusEvent`s (PeerConnected, PeerDisconnected, Error).
- Lock the Server/Client segmented toggle while a backend is running.
- Auto-add the server's fingerprint into the local trust DB on first Connect click if the user pasted one in the form.
- `[+ Add]` modal on the Server view → append to `FingerprintDb`, live-update the peers card.
- Read the system hostname at startup (`gethostname` crate).
- Enumerate LAN IPv4 addresses (`local-ip-address` crate); display the first non-loopback one prominently.
- Toast-based error reporting for every async failure.

**Out of scope:**
- Child-process model (spawn `hops` / `hopc` as subprocesses and talk over the existing `hop-ipc` socket). Might land in M14 for headless/sandboxed deployments.
- First-run wizard. Defaults already make the first-run experience acceptable; a wizard is M14+ polish.
- Visual layout editor (drag-n-drop of screen rects). Separate milestone.
- Reload-on-config-change, `reload_layout` via IPC. Separate PR.
- QR scanning via camera. The fingerprint input stays paste-only for MVP.
- Windows service mode toggle from the GUI.

## Architecture

### The big decision: embedded runtime vs subprocess

| Option | Pros | Cons |
|---|---|---|
| **A. Embed `tokio::Runtime` inside `HopApp`** | One process, one binary to debug. `tokio::spawn` directly from egui-thread callbacks (through a handle). No IPC needed. Shares cert/fingerprint DB by pointer. | A panic in a backend task kills the GUI (mitigated by `JoinSet` + supervisor, but still one address space). Headless CI must use `hops`/`hopc` CLI, not `hop`. |
| **B. Spawn `hops`/`hopc` as child processes, talk via `hop-ipc`** | Hard isolation — backend crash doesn't take the GUI down. Matches the same IPC a future headless service would use. | Two extra binaries must be findable on `$PATH`; IPC semantics must cover Start/Stop/AddPeer/status; file-handle handover for the fingerprint DB gets fiddly. |

**Decision: Option A for MVP.** It is much simpler to get working, and the backend crates were designed to be called as libraries from the start. Option B is the right long-term choice for enterprise deployments and Windows service mode; leave it for M14 once M13's UX has been validated.

### Runtime topology

```
┌──────────────── GUI thread (eframe / winit) ────────────────┐
│                                                             │
│   HopApp                                                    │
│   ├─ AppMode (Server | Client)                              │
│   ├─ ServerState / ClientState (forms, flags)               │
│   ├─ BackendController  ◄────────┐                          │
│   │  ├─ handle: tokio::Runtime   │ commands (mpsc)          │
│   │  ├─ shutdown: Cancellation…  │ (Start, Stop, AddPeer)   │
│   │  └─ status_rx: mpsc<Status>  │                          │
│   └─ Toasts                      │                          │
│                                  │                          │
└──────────────────────────────────┼──────────────────────────┘
                                   │
┌────────── tokio runtime (multi-thread, owned by app) ───────┐
│                                   ▼                         │
│  Backend actor task                                         │
│  ├─ match current mode:                                     │
│  │    Server → hop_server::run(cfg, screen, shutdown)       │
│  │    Client → hop_client::run(cfg, screen, shutdown)       │
│  ├─ wrap server's Coordinator to intercept                  │
│  │  ClientConnected / ClientDisconnected → status_tx        │
│  └─ on error / exit: status_tx.send(Status::Stopped {err})  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

egui thread never blocks on I/O. Channels are `tokio::sync::mpsc` on the backend side, read via a non-blocking `try_recv` loop in `HopApp::update`. A repaint is requested on every status event arrival.

### New `BackendController`

Lives in `crates/hop-ui/src/runtime/controller.rs`.

```rust
pub struct BackendController {
    /// Multi-thread tokio runtime built once in `HopApp::new`.
    /// Dropped together with the app.
    runtime: tokio::runtime::Runtime,

    /// Slot for the currently running backend task, if any.
    /// `None` while stopped.
    active: Option<Running>,

    /// Inbound status events from the backend task.
    /// Polled in `HopApp::update` via `try_recv`.
    status_rx: mpsc::Receiver<StatusEvent>,
    status_tx: mpsc::Sender<StatusEvent>,
}

struct Running {
    mode: AppMode,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

pub enum StatusEvent {
    Started { listen_addr: SocketAddr },        // server only
    Connected { peer: String },                 // client only, or server peer joined
    Disconnected { peer: String },              // server peer left
    Error(String),                              // non-fatal
    Stopped { exit: Result<(), String> },       // task finished
}

impl BackendController {
    pub fn new() -> Result<Self, ControllerError>;

    pub fn start_server(&mut self, cfg: ServerConfig, screen: Arc<dyn PlatformScreen>)
        -> Result<(), ControllerError>;

    pub fn start_client(&mut self, cfg: ClientConfig, screen: Arc<dyn PlatformScreen>)
        -> Result<(), ControllerError>;

    pub fn stop(&mut self);

    pub fn is_running(&self) -> bool;

    pub fn current_mode(&self) -> Option<AppMode>;

    /// Drain status events for UI consumption.
    pub fn drain_events(&mut self) -> Vec<StatusEvent>;
}
```

**Why `Arc<dyn PlatformScreen>`:** the current `PlatformScreen` trait uses AFIT; the dyn-friendly wrapper mentioned in `specs/architecture.md §Key decisions` is required here. That is an existing TODO in the main spec — M13 surfaces it as concrete work.

**Why `current_mode() -> Option<AppMode>`:** the GUI uses it to keep the segmented toggle locked while a backend is running (`disabled = controller.is_running()`).

### Hooking into server events

The server's `Coordinator` already fires `ClientConnected` / `ClientDisconnected` events internally. To expose them to the UI we need a thin adapter.

**Approach:** the `Server::serve` accept-loop already notifies the coordinator when a peer attaches. Adding a fan-out `mpsc::Sender<ServerAudit>` to `ServerConfig` lets us observe without touching the hot path. `BackendController` creates the sender when starting the server, subscribes to the forwarder task that translates `ServerAudit` → `StatusEvent`, and plumbs it into `status_tx`.

```rust
// Additions to hop-server public API:
#[derive(Debug, Clone)]
pub enum ServerAudit {
    PeerConnected { name: String, fingerprint: Fingerprint },
    PeerDisconnected { name: String, reason: DisconnectReason },
}

pub struct ServerConfig {
    // … existing fields …
    /// Optional audit channel. If set, the server mirrors peer-
    /// connect/disconnect events into it. Intended for GUI live views.
    pub audit: Option<mpsc::Sender<ServerAudit>>,
}
```

Non-breaking: defaulted to `None` via `Option`. Client side uses the same pattern with `ClientAudit` (fewer events — just `Connected`/`Disconnected`/`ReconnectAttempt`).

### Where `PlatformScreen` comes from

Before M13, the GUI never needed a real screen backend — the view was cosmetic only. Now that Start spawns a real server, we need an `Arc<dyn PlatformScreen>`:

- On Linux: try `X11Screen::open(None)` → if that fails, `EiScreen::try_open()` → fall back to `MockScreen` + toast "no platform backend found, running in noop mode".
- On macOS: `MacOsScreen::try_open()` → MockScreen fallback.
- On Windows: `WindowsScreen::try_open()` → MockScreen fallback.

This cascade already lives in `bins/hops/src/main.rs::backend::run_server`. Extract it into a library function `hop-ui::platform::select()` that both the CLI binary and the GUI call.

## Task details

### Task #11 — real hostname

**Files:** `crates/hop-ui/src/views/server.rs`, `client.rs` (`hostname_fallback` helper).

Today:
```rust
fn hostname_fallback() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "this-computer".into())
}
```

`HOSTNAME` isn't exported to GUI apps on macOS (no login-shell inheritance); `COMPUTERNAME` is only set on Windows. On Linux X11 sessions it is usually present but not guaranteed. Result: almost everyone sees the literal `"this-computer"`.

**Fix:** use the `gethostname` crate (30 LOC, zero deps beyond libc):
```rust
fn system_hostname() -> String {
    gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "hop-host".into())
}
```

Delete the two existing `hostname_fallback` copies — replace with a single helper in `crates/hop-ui/src/util.rs` (new file).

### Task #12 — LAN IP in Server view

**Files:** `crates/hop-ui/src/views/server.rs`, new `crates/hop-ui/src/util.rs::local_ip()`.

Use the `local-ip-address` crate:
```rust
pub fn lan_ipv4() -> Option<IpAddr> {
    local_ip_address::local_ip().ok().filter(|ip| ip.is_ipv4())
}
```

UI: add a second subtle line under the Listening-on input, muted colour:
```
Listening on: 0.0.0.0:25900
Reachable at: 192.168.1.10:25900           (click to copy)
```

Port is taken from the state's `listen_addr`. If `lan_ipv4()` returns `None` (e.g. CI sandboxes, no network), omit the line silently.

### Task #13 — Start button runs the server

**Files:** `crates/hop-ui/src/app.rs`, `runtime/controller.rs` (new), `views/server.rs`.

Flow when the user clicks Start:
1. Parse `state.listen_addr` → `SocketAddr`. On parse error: toast red, do nothing.
2. Build `ServerConfig`:
   - `display_name = state.name`
   - `identity = shared.identity` (already loaded at app startup)
   - `trusted_peers = shared.trust_db`
   - `layout = shared.layout` (from `LayoutStore::load` on app init)
   - `audit = Some(tx)` where `tx` feeds status events to the UI
3. `controller.start_server(cfg, platform_screen)` spawns the task.
4. On success: `state.running = true`, segmented toggle becomes disabled.
5. Status events drained every frame; update `connected_peers: Vec<PeerInfo>` in `ServerState`.
6. On Stop click: `controller.stop()`, wait for `StatusEvent::Stopped`, clear peers.

**Segmented toggle lock:** `widgets::segmented(ui, &mut mode, options)` takes a new optional `disabled: bool` parameter (or wrap in `ui.add_enabled_ui`). When disabled, clicks are ignored and the pill is rendered at 50% opacity with a tooltip "Stop the server to switch modes."

**Error surfacing:** every `ControllerError` / `StatusEvent::Error` becomes a toast. On hard crash (task panicked) → toast red "Server crashed: <msg>", state returns to Stopped.

### Task #14 — restore [+ Add] in Connected peers

**Files:** `crates/hop-ui/src/views/server.rs`, new `widgets/add_peer_modal.rs`, `shared.rs` for `FingerprintDb` access.

Why it matters: right now the server trusts no clients by default. The only way to add a peer is `hops fingerprint add name fp` in the terminal — exactly the pattern the GUI is supposed to replace.

UI: `[+ Add peer]` button in the Connected peers card header (top-right via `Layout::right_to_left`). Click opens a modal:

```
╭── Add a trusted peer ─────────────────╮
│                                        │
│  Name:        [ laptop________________]│
│  Fingerprint: [ sha256:abc…  paste?   ]│
│                                        │
│  (paste the fingerprint exactly as     │
│   the client shows it)                 │
│                                        │
│                     [Cancel]  [Add]    │
╰────────────────────────────────────────╯
```

Validation:
- `name`: non-empty, trim whitespace, **must not already exist** in `FingerprintDb` (exact match, case-sensitive). On clash — inline red hint "A peer named '<name>' already exists" and disable [Add]. To replace an existing peer, the user must remove it first.
- `fingerprint`: `Fingerprint::from_str` must succeed (checks `sha256:<64-hex>`).
- If either fails → inline red hint under the field, disable [Add].

On confirm:
- Append to `FingerprintDb`, call `db.save(path)` atomically (tempfile + rename).
- Toast green "Peer <name> added to trust DB".
- If the server is running, no restart needed — the custom verifier re-reads from the same `Arc<FingerprintDb>` (confirm it's live-reload capable; if not, see Open questions).

### Task #15 — Connect button runs the client

**Files:** `crates/hop-ui/src/views/client.rs`, `runtime/controller.rs`.

Flow on Connect click:
1. Parse `state.server_addr` → `SocketAddr`. Error → toast.
2. Parse `state.server_fingerprint` → `Fingerprint`. Error → toast.
3. If the fingerprint isn't in the local DB yet, auto-add it as `PeerEntry { name: state.server_addr.to_string(), fingerprint, added: now() }` and save. Toast "Trusting server <addr>".
4. Build `ClientConfig`:
   - `server_addr`, `display_name = state.name`, `identity = shared.identity`, `trusted_peers = shared.trust_db`, `audit = Some(tx)`.
5. `controller.start_client(cfg, screen)`.
6. Status: show "Connecting…" while the handshake runs, flip to "Connected to <server-name>" on `ClientAudit::Connected`.
7. On Disconnect click: cancel + wait for Stopped.
8. If the server drops us (`StatusEvent::Error` or `Stopped { exit: Err }`): auto-reconnect per existing `backoff` policy — show "Reconnecting in Ns" countdown instead of Offline.

Segmented toggle: locked while connected (same logic as task #13).

## Implementation order

Recommended sequence — two coherent commits, each leaves the tree green:

### Commit 1 — "fix: cosmetic UI defaults" (~2h)

1. Task **#11**: `gethostname` crate, extract `util::system_hostname`.
2. Task **#12**: `local-ip-address` crate, `util::lan_ipv4`, second line in identity card.
3. Task **#14**: `[+ Add]` button + modal + FingerprintDb write.

These are self-contained — no backend-runtime changes, no new traits, no `async`.

### Commit 2 — "feat(hop-ui): wire Start/Connect to real runtime" (~1d)

1. Extract `platform::select()` from `bins/hops/main.rs::backend` into a library.
2. Add the `audit: Option<mpsc::Sender<ServerAudit>>` / `ClientAudit` channels to `hop-server` and `hop-client` (non-breaking, `Option`-defaulted).
3. Introduce `BackendController` in `crates/hop-ui/src/runtime/`.
4. Build a dyn-friendly `PlatformScreen` wrapper trait (mentioned in architecture.md §Key decisions) — needed here because the controller must hold the screen behind an `Arc<dyn>`.
5. Wire Start (task #13) and Connect (task #15).
6. Lock the segmented toggle when `controller.is_running()`.

## Tests

| Level | What | How |
|---|---|---|
| Unit | `util::system_hostname` returns non-empty, different from `"hop-host"` on a normal host | `#[cfg(test)] fn test_hostname_is_resolved()` |
| Unit | `util::lan_ipv4` returns an `Ipv4Addr` on a host that has one, `None` otherwise | use `local-ip-address` directly; may skip in sandboxed CI |
| Unit | Fingerprint modal validation: bad sha256 → [Add] disabled | Manual egui test with a headless harness (`egui_kittest` if we pull it in) |
| Integration | `BackendController::start_server` → `ServerAudit::PeerConnected` arrives in `status_rx` when a mock client handshakes | Use existing `tests/coordinator_e2e.rs` pattern + a fresh audit channel |
| Integration | `start_client` → `ClientAudit::Connected` on successful handshake | Same harness — two controllers in one process |
| Integration | Stop cancels cleanly — `shutdown.cancelled()` reaches the server task within 500 ms | Timer assertion |
| Manual | Launch two builds on LAN, GUI-only flow: hostname auto-filled, LAN IP shown, Add peer modal, Start server → Connect client → cursor crossing works | Covered by human |

## Edge cases

- **Start with an already-running backend** — impossible via UI (toggle locked) but defend with `ControllerError::AlreadyRunning`.
- **Platform screen cannot be opened** (no X display, ei portal denied) — `platform::select()` returns `MockScreen`; UI shows a warning banner "Running with MockScreen; input will not be injected" instead of silently accepting.
- **Port already in use** on Start — `Server::bind` returns `ServerError::Bind`. Convert to a red toast "Port 25900 already in use. Stop the other process or change the port."
- **Fingerprint format garbage** in Connect's paste box — validation fails before we touch `hop_client::run`.
- **Server goes down while client is connected** — keep-alive times out, backend emits `StatusEvent::Stopped { exit: Err(KeepAliveTimeout) }`, UI flips to "Reconnecting" with exponential backoff (`backoff` crate, already on the dep tree).
- **User closes the app while backend running** — `HopApp::Drop` cancels the shutdown token; `runtime.shutdown_timeout(Duration::from_secs(2))` lets tasks cleanup; clipboard state written to disk if applicable.
- **Two GUI instances on one machine** — port-bind conflict if both Start; the second one's toast covers the case. Optional: add a per-user lockfile in `$XDG_RUNTIME_DIR/hop/lock` to refuse the second GUI altogether.
- **`[+ Add]` while a peer with the same name already exists** — overwrite with confirmation dialog ("Replace existing peer 'laptop'?"), or reject and force unique name. Recommend overwrite with confirmation.

## Estimate

- Commit 1 (#11 + #12 + #14): **~3 hours**. Mostly egui widgets + one new dep each for hostname / LAN IP.
- Commit 2 (#13 + #15): **~1–1.5 days**. Bulk of the time goes to the `BackendController` abstraction, the `dyn PlatformScreen` wrapper, and the `audit` channels in `hop-server`/`hop-client`.

**Total:** ~2 days of focused work.

## Risks

- **Audit-channel contention.** On a healthy server with high peer churn we do not want a slow GUI to block the coordinator. Use `try_send` with drop-on-full on the `audit` side and a `warn_once!` on the server when the UI cannot keep up. Bounded channel size: 256.
- **`dyn PlatformScreen` is trickier than it sounds.** The current AFIT trait is not object-safe. The wrapper needs `Box<dyn Future>` or `async_trait`. Concretely, `hop-ui` gets a new `mod platform_dyn` with the dyn-friendly shape and thin adapters for each concrete backend. Expect 1-2 hours of boilerplate.
- **Tokio runtime lifetime.** Owning a `Runtime` inside eframe's app means `Drop` must `shutdown_timeout` so running tasks flush. Forgetting this leaves zombie sockets.
- **Trust-DB live updates.** Existing `FingerprintDb` is loaded once at start. Task #14 adds entries at runtime, task #15's auto-add too. The custom `rustls` verifier must see the new entries. Currently the verifier holds an `Arc<FingerprintDb>` by pointer — if the DB is stored behind `Arc<ArcSwap<FingerprintDb>>`, additions become visible after the next accept. Confirm via a test that spans Add → handshake-from-new-peer succeeds without GUI restart.
- **Windows XDG-runtime-dir absence.** `local-ip-address` and `gethostname` work on all three OSes, but the `lan_ipv4()` discovery on VPN-heavy setups picks a loopback tunnel. Document the behaviour; a future milestone could add an interface picker.

## Resolved / deferred decisions

1. **Embedded runtime vs subprocess spawning.** Resolved: embedded for MVP (Option A). Subprocess model kept for M14 / enterprise mode.
2. **Connection events (server side).** Resolved: non-breaking `audit: Option<mpsc::Sender<ServerAudit>>` field on `ServerConfig`. Internal coordinator code already has the events to mirror.
3. **Platform backend selection inside `hop`.** Resolved: extract the existing CLI cascade into `hop-ui::platform::select()` so both the GUI and `hops`/`hopc` share one implementation.
4. **Segmented-toggle lock UX.** Resolved: toggle renders disabled + tooltip "Stop the server to switch modes." — do not silently ignore clicks.
5. **Auto-add server fingerprint on Connect.** Resolved: yes, with a toast, because the whole point of the GUI is to avoid a CLI dance for common flows.
6. **Peer name conflict on Add.** Resolved: names must be unique. A clash shows an inline error and keeps the [Add] button disabled. To replace an existing peer, remove it first — no auto-overwrite, no confirmation dance.
7. **First-run wizard.** Deferred to M14. M13 assumes defaults are sensible after tasks #11 + #12 land.

## Readiness after M13

After M13:
- The GUI is a complete controller for a two-computer Hop setup.
- All five pending UX items resolved.
- One binary, one process — easy to debug and deploy.
- A clean boundary (`BackendController` + audit channels) unblocks the child-process model in a future M14 without touching the views.
