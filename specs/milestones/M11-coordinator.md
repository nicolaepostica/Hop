# M11 — Server Coordinator

## Goal

Land the central server actor that turns local input (on the primary) and network messages from clients into a correct stream of `Message`s. Today `Server::serve` does accept + keep-alive only — there is no "local keyboard → client" path, no active-screen switching, no clipboard-protocol handling. M11 closes that gap.

## Scope

**In scope:**
- The `Coordinator` struct — a pure state machine owning the screen layout, the active screen, the cursor, held keys/buttons/modifiers, clipboard-grab state, and a registry of connected clients.
- Edge-crossing mechanics (rect-based virtual layout) with correct release/re-press of modifiers.
- Routing `InputEvent`s from the local `PlatformScreen::event_stream()` to the right `ClientProxy`.
- Receiving inbound `Message`s from clients (`CoordinatorEvent::PeerMessage`) and handling them (clipboard protocol, disconnect).
- Integrating with the existing `Server::serve` / `ClientProxy` via channels.
- Shutdown propagation: `CancellationToken` → every client task.

**Out of scope:**
- Lazy clipboard (platform hands data out on demand at Ctrl+V on the local side). Requires a new `PlatformScreen` API, scheduled for M11.1.
- The "Ctrl+V on primary after a secondary grabbed the buffer" scenario. Works only with lazy clipboard. Documented limitation for now.
- Drag-across-edge (the mouse dragging a button while crossing). Mouse crossing is blocked while any button is held — standard Barrier/Synergy semantics.
- GUI `reload_config`: `arc_swap` is chosen exactly to enable a future swap, but the reload mechanism itself is a separate PR.
- File-clipboard (M9) through the Coordinator: the transfer engine already exists; routing `FileTransferStart/Chunk/End/Cancel` through the Coordinator is a trivial add-on, not claimed as a feature of this milestone.

## Architecture

### Component diagram

```
┌──────────────────┐        InputEvent           ┌───────────────────┐
│  PlatformScreen  │ ──────────────────────────▶ │                   │
│  (event_stream)  │                             │                   │
└──────────────────┘                             │                   │
                                                 │                   │
┌──────────────────┐  ClientEvent::Connected /   │                   │
│  Server accept   │  ::Disconnected /           │   Coordinator     │
│  loop            │ ─────PeerMessage──────────▶ │   task            │
└──────────────────┘                             │                   │
                                                 │                   │
┌──────────────────┐   CancellationToken         │                   │
│  SIGINT handler  │ ──────────────────────────▶ │                   │
└──────────────────┘                             └────────┬──────────┘
                                                          │
                            ┌─────────────────────────────┤
                            │ per-client mpsc<Message>    │
                            ▼                             ▼
                    ┌───────────────┐            ┌───────────────┐
                    │ ClientProxy   │            │ ClientProxy   │
                    │ "laptop"      │            │ "desktop"     │
                    └───────┬───────┘            └───────┬───────┘
                            │ framed TLS                 │
                            ▼                            ▼
                        laptop peer                 desktop peer
```

### Layers and files

```
crates/server/src/
├── lib.rs               # re-exports, Server::bind/serve (wires it all together)
├── error.rs             # ServerError (unchanged from M10.5 fix #1)
├── coordinator/
│   ├── mod.rs           # Coordinator struct + Event / Output enums
│   ├── layout.rs        # ScreenLayout (rect-based virtual space) + crossing math
│   ├── held.rs          # HeldState (keys/buttons/mods) + transition helpers
│   ├── clipboard.rs     # ClipboardGrabState (owner/seq tracking, pending requests)
│   └── task.rs          # The tokio task that drives Coordinator + channel plumbing
└── proxy.rs             # ClientProxy: framed<->mpsc adapter + keep-alive
```

## Data shapes

### ScreenLayout (rect-based)

Every screen is a rectangle in a virtual coordinate space. The cursor lives in global coordinates; the active screen is whichever rect currently contains it.

```rust
pub struct ScreenLayout {
    screens: Vec<ScreenEntry>,
    primary: ScreenName,
}

pub struct ScreenEntry {
    pub name: ScreenName,
    /// Top-left corner of this screen in the virtual coordinate space.
    pub origin_x: i32,
    pub origin_y: i32,
    /// Physical resolution in pixels.
    pub width: u32,
    pub height: u32,
}

pub type ScreenName = String;  // matches display_name from Hello
```

Example config (TOML in the future; the struct is fixed now):
```toml
primary = "desk"

[[screen]]
name = "desk"
origin_x = 0
origin_y = 0
width = 1920
height = 1080

[[screen]]
name = "laptop"
origin_x = -1440   # to the left of desk
origin_y = 90      # vertical centering
width = 1440
height = 900

[[screen]]
name = "monitor"
origin_x = 1920    # to the right of desk
origin_y = -180    # top alignment differs
width = 2560
height = 1440
```

Query: `layout.screen_at(vx, vy) -> Option<&ScreenEntry>` — O(n), where n is the number of screens (usually ≤ 5).

**Fork decision:**
- Layout is held in `Arc<ArcSwap<ScreenLayout>>`. The Coordinator loads a snapshot once per `on_input` iteration via `.load()`. Lock-free read path.

### LayoutStore and where layout lives on disk

Layout lives in a **separate file** `layout.toml`, not inside the main `config.toml`. Reasons:

1. **Different owners.** `config.toml` is admin-level (`listen_addr`, `cert_dir`), rarely edited; `layout.toml` is user-level, rewritten by the GUI every time the user rearranges monitors. Separate files → the GUI can do atomic writes (`tempfile` + rename) without clobbering admin settings and comments.
2. **Precedent.** `FingerprintDb` already lives in its own file (`fingerprints.toml`); architecturally we've already chosen "what changes often from outside — in a separate file".
3. **Live reload.** `ArcSwap` was picked for hot-swap. A separate file lets the reload path avoid dragging the figment layers (env, CLI) that don't concern layout.

**Structure:**

```toml
# ~/.config/hop/config.toml

listen_addr = "0.0.0.0:25900"
display_name = "desk"

[tls]
cert_dir       = "./config/tls"
fingerprint_db = "./config/fingerprints.toml"

[layout]
# Path to the layout file. Reloaded live via IPC reload_layout().
path = "./config/layout.toml"
```

```toml
# ~/.config/hop/layout.toml

primary = "desk"

[[screen]]
name     = "desk"
origin_x = 0
origin_y = 0
width    = 1920
height   = 1080

[[screen]]
name     = "laptop"
origin_x = -1440
origin_y = 90
width    = 1440
height   = 900
```

**Only `ServerSettings` gains `LayoutSettings { path: PathBuf }`.** The client never reads a layout — it only ever has one screen.

**Default path:** `<project_config_dir>/layout.toml` via `directories::ProjectDirs` — next to `config.toml`.

**Missing file:** `tracing::warn!("layout file not found at {}; add at least one client screen to route input", path)` + empty layout (primary only). The server starts in degraded mode (routes nothing) but does not crash — important for first-run UX, when the user hasn't configured the layout yet.

**`LayoutStore` — a thin wrapper:**

```rust
pub struct LayoutStore {
    path: PathBuf,
    inner: Arc<ArcSwap<ScreenLayout>>,
}

impl LayoutStore {
    /// Load from disk. Missing file → empty layout + warn.
    pub fn load(path: PathBuf) -> Result<Self, ConfigError>;
    /// Cheap: Arc pointer-bump.
    pub fn snapshot(&self) -> Arc<ScreenLayout>;
    /// Re-read from disk and atomically swap. Coordinator sees the
    /// new layout on its next on_event iteration.
    pub fn reload(&self) -> Result<(), ConfigError>;
}
```

The GUI calls `reload_layout` via IPC (a new `IpcHandler` method, added during M11 wire-up) → the server calls `store.reload()` → the `Coordinator` sees the new layout on its next `on_event`.

### HeldState

```rust
pub struct HeldState {
    keys: BTreeSet<KeyId>,           // non-modifier keys held on active side
    buttons: BTreeSet<ButtonId>,     // mouse buttons held on active side
    mods: ModifierMask,              // modifiers (Shift/Ctrl/Alt/Meta/Locks/AltGr)
}

impl HeldState {
    /// Apply one event to the held state. Returns `true` if modifier
    /// mask changed as a side-effect (helps callers decide whether to
    /// re-emit mods on a screen transition).
    pub fn apply(&mut self, event: &InputEvent) -> bool;

    /// Messages needed to "unstick" the currently-held state on the
    /// active screen before we leave it.
    pub fn leave_messages(&self) -> Vec<Message>;

    /// Messages needed to "restore" held modifiers on the new active
    /// screen after we entered it. Non-modifier keys and buttons are
    /// intentionally NOT re-pressed (see design rationale).
    pub fn enter_messages(&self) -> Vec<Message>;

    pub fn any_button_held(&self) -> bool;
}
```

**Fork decision 3 (where held-state lives):** inside the `Coordinator`, driven by the `InputEvent` stream. The platform layer emits raw KeyDown/KeyUp/MouseButton; the Coordinator aggregates. This lets the Coordinator correctly "unwind" the state on a transition.

**Re-press policy:**
- On `leave_messages()`: emit `KeyUp` for every element of `keys`, and `ButtonUp` (as `MouseButton { down: false }`) for every element of `buttons`. Then zero the modifiers by emitting a separate `KeyUp` per flag in `mods`.
- On `enter_messages()`: emit `KeyDown` **only** for modifiers in `mods`. Non-modifier keys and buttons are **not** carried over (rare case, safer to release and forget).

**Drag-across-edge:** if `any_button_held() == true`, then `Coordinator::on_input(MouseMove)` **does not cross** a boundary; the cursor is clamped to the current screen until all buttons are released. Matches Barrier/Synergy behaviour.

### ClipboardGrabState

```rust
pub struct ClipboardGrabState {
    /// Current owner per clipboard id (Clipboard / Primary).
    owner: HashMap<ClipboardId, GrabRecord>,
}

pub struct GrabRecord {
    pub owner: ScreenName,
    /// Monotonic seq bumped on screen-transitions; peers use it to
    /// discard stale Grab/Request messages that arrived after the
    /// active screen moved on.
    pub seq: u32,
}

impl ClipboardGrabState {
    pub fn current_seq(&self, id: ClipboardId) -> u32;
    pub fn bump_seq(&mut self, id: ClipboardId);
    pub fn on_grab(&mut self, from: ScreenName, id: ClipboardId, seq: u32) -> bool;  // true if accepted
    pub fn owner_of(&self, id: ClipboardId) -> Option<&ScreenName>;
}
```

The Coordinator owns a single instance; the module is tested independently.

### Coordinator

```rust
pub struct Coordinator {
    layout: Arc<ArcSwap<ScreenLayout>>,
    active: ScreenName,                          // whose inputs are being captured / forwarded
    cursor: (i32, i32),                          // virtual coords
    held: HeldState,
    grabs: ClipboardGrabState,
    clients: HashMap<ScreenName, ClientHandle>,  // connected + known-in-layout
    orphans: HashMap<ScreenName, ClientHandle>,  // connected but not in layout
    seq: u32,
}

pub struct ClientHandle {
    pub tx: mpsc::Sender<Message>,
    pub capabilities: Vec<Capability>,
}

pub enum CoordinatorEvent {
    /// Local platform input (primary side only).
    LocalInput(InputEvent),
    /// A peer connected and finished handshake.
    ClientConnected {
        name: ScreenName,
        tx: mpsc::Sender<Message>,
        capabilities: Vec<Capability>,
    },
    /// A peer disconnected for any reason.
    ClientDisconnected { name: ScreenName },
    /// A peer sent us a wire message.
    PeerMessage { from: ScreenName, msg: Message },
    /// Layout has been swapped (future: reload_config).
    LayoutReloaded,
}

pub enum CoordinatorOutput {
    /// Send a message to a specific client.
    Send { to: ScreenName, msg: Message },
    /// Inject a message locally (only meaningful when `active == primary`).
    InjectLocal(Message),
    /// Log + metrics hook.
    Warn(String),
}

impl Coordinator {
    pub fn new(
        layout: Arc<ArcSwap<ScreenLayout>>,
        local: ScreenName,  // the primary's own name
    ) -> Self;

    /// Single entry point. `buf` is reused by callers to avoid per-event allocation.
    pub fn on_event(&mut self, event: CoordinatorEvent, buf: &mut Vec<CoordinatorOutput>);
}
```

**Fork decision 5 (pure vs side-effectful):** pure + `Vec<Output>`. Callers reuse one `Vec<CoordinatorOutput>` buffer across calls. Tests feed events and assert on outputs — no tokio runtime needed for 95% of the matrix.

## Key invariants + operation order

### On a MouseMove from the local event stream

Sequence inside the Coordinator:
1. Update `cursor += (dx, dy)` (for RelMove) or `cursor = (x, y)` (for absolute).
2. `if self.held.any_button_held() { clamp cursor to current active screen rect; emit forward-as-usual; return; }`
3. `layout.screen_at(cursor)` — find which screen we're on now.
4. If it's the same as `self.active` → just forward MouseMove to the matching sink (local inject or network).
5. If different → **atomic crossing transaction**:
   - `for msg in held.leave_messages()` → `Send` to the old active (if remote) / `InjectLocal` (if primary).
   - `Send ScreenLeave` to the old active (if remote).
   - `self.active = new_name`
   - `self.seq += 1` (global seq for ScreenEnter)
   - `Send ScreenEnter { x, y, seq, mask = self.held.mods }` to the new active (if remote).
   - `for msg in held.enter_messages()` → `Send` to the new active.
6. After the transition — forward the original MouseMove to the new active (in local coordinates inside the target screen).

### On Key/Button events

1. `self.held.apply(event)` — update sets.
2. If `active == local_primary` — `InjectLocal` (noop on the server; the OS already handled it locally). Effectively nothing to do.
3. If `active` is a remote client — `Send { to: active, msg: Message::Key/Button(...) }`.

### On ClipboardGrab from a client

`Coordinator::on_event(PeerMessage { from, ClipboardGrab { id, seq }})`:
1. `self.grabs.on_grab(from, id, seq)` — if `seq < current_seq`, drop (stale).
2. Broadcast `ClipboardGrab { id, seq }` to every client **except** `from`.
3. No platform actions (lazy-clipboard — out of scope).

### On ClipboardRequest from a client

`Coordinator::on_event(PeerMessage { from, ClipboardRequest { id, seq }})`:
1. Look up the owner. If it's the primary — platform via a separate path (see "Local path to the platform" below). For now: `Warn("clipboard request for primary not supported yet")`.
2. If another client — `Send { to: owner, msg: ClipboardRequest { id, seq }}`.

### On ClipboardData from a client

`Coordinator::on_event(PeerMessage { from, ClipboardData { id, format, data }})`:
1. If there's an outstanding request for this `(id, seq)` — forward `ClipboardData` to the requester.
2. Otherwise warn + drop.

### On ClientConnected / ClientDisconnected

Connected:
1. If `name` is in the `layout` → `clients.insert(name, handle)`.
2. Otherwise → `orphans.insert(name, handle)` + `Warn("client 'X' connected but not in layout; inputs won't be routed to it")`.
3. Bootstrap: `Send ScreenEnter` with `seq=0` if this client becomes active immediately (rare; normally the primary starts active).

Disconnected:
1. `clients.remove(name)` / `orphans.remove(name)`.
2. If `active == name` → switch `active` back to the primary, `self.seq += 1`, and emit `held.enter_messages()` as a local inject (technically a noop, but the seq-bump is correct for any in-flight clipboard grabs).
3. If `name` was the owner of any clipboard → clear the matching `grabs.owner.remove(...)`.

## ClientProxy

```rust
pub struct ClientProxy {
    name: ScreenName,
    framed: HandshakeStream,
    inbound_tx: mpsc::Sender<CoordinatorEvent>,  // wraps PeerMessage
    outbound_rx: mpsc::Receiver<Message>,
    shutdown: CancellationToken,
}

impl ClientProxy {
    pub async fn run(self) -> Result<(), ServerError>;
}
```

Loop:
```rust
loop {
    select! {
        biased;
        () = shutdown.cancelled() => {
            framed.send(Message::Disconnect { reason: UserInitiated }).await.ok();
            break;
        }
        Some(msg) = outbound_rx.recv() => {
            framed.send(msg).await?;
        }
        incoming = framed.next() => {
            match incoming {
                Some(Ok(msg)) => {
                    keepalive.mark_seen();
                    match msg {
                        Message::Disconnect { .. } => break,
                        msg => {
                            inbound_tx.send(CoordinatorEvent::PeerMessage { from: name.clone(), msg })
                                .await
                                .map_err(|_| ServerError::CoordinatorGone)?;
                        }
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
        _ = keepalive.tick() => {
            if keepalive.is_timed_out() {
                framed.send(Message::Disconnect { reason: KeepAliveTimeout }).await.ok();
                break;
            }
            framed.send(Message::KeepAlive).await?;
        }
    }
}
// on exit: send Disconnected event to coordinator
inbound_tx.send(CoordinatorEvent::ClientDisconnected { name }).await.ok();
```

**Fork decision 4 (outbound backpressure):**
- `mpsc::channel(1024)` bounded.
- The `Coordinator` in `task.rs` uses `tx.try_send(msg)`:
  - `Ok(())` — normal.
  - `Err(TrySendError::Full(_))` — slow client → close the connection: send `ClientDisconnected` over a loopback channel; the Coordinator drops the client; the proxy task notices the outbound channel closing and exits. `Warn("client X dropped due to outbound backpressure")`.
  - `Err(TrySendError::Closed(_))` — the proxy is already gone, silently drop from `clients`.

## Local path to the platform

The `Coordinator` does not call `PlatformScreen` directly (that would make it unreasonably hard to test). Instead, the `task.rs` that drives the Coordinator has a **second outbound channel** `mpsc<Message>` → a `PlatformDispatcher` task. That task:
- Takes `Message::Key/Mouse/Clipboard*` → calls `screen.inject_key(...)` / `screen.set_clipboard(...)`.
- For primary-side `InjectLocal` — it's what happens when `active == local`, but is normally unnecessary (the OS already handled it). Useful for clipboard writes (when we want to put data into the local buffer on an incoming `ClipboardData` from another peer).

## Shutdown propagation

```
CancellationToken (SIGINT)
    ├─▶ Server::serve loop exits
    ├─▶ Coordinator task: drains its inbound channel, does a final `on_event(ClientDisconnected)` for every remaining client, sends CoordinatorEvent::Shutdown internally so the task exits.
    ├─▶ Each ClientProxy: a biased select catches cancelled() first, sends Disconnect to the peer, closes.
    └─▶ PlatformDispatcher: drains, exits.
```

A `JoinSet` covers every ClientProxy + the Coordinator task + the PlatformDispatcher; it's awaited in the `Server::serve` epilogue.

## Implementation order

1. ✅ **`coordinator/layout.rs`** (commit `54d7f6b`) — `ScreenLayout`, `ScreenEntry`, `screen_at()`, `clamp()`, `LayoutStore` with `ArcSwap` + live reload. 11 unit tests.

2. ✅ **`coordinator/held.rs`** (commit `54d7f6b`) — `HeldState::{apply, leave_messages, enter_messages, any_button_held}`. 10 unit tests (Shift replay, drag block, modifier fixed-order).

3. ✅ **`coordinator/clipboard.rs`** (commit `54d7f6b`) — `ClipboardGrabState` with seq-based stale detection. 6 unit tests.

4. ✅ **`coordinator/state.rs`** (commit `63c0b0b`) — `Coordinator`, `CoordinatorEvent`, `CoordinatorOutput`. Pure state machine. 11 unit tests (crossing, drag-block, orphan, active-disconnect, clipboard broadcast/request/stale).

5. ✅ **`coordinator/proxy.rs`** — `ClientProxy` with outbound mpsc + keep-alive + inbound forward. 5 integration tests on `tokio::io::duplex` (PeerMessage forwarding, outbound writes, keep-alive filter, peer-disconnect, coordinator-drop).

6. ✅ **`coordinator/task.rs`** — tokio driver task + platform dispatcher. `try_send` backpressure drop-on-full. 3 unit tests (crossing emits ScreenEnter, backpressure tolerance, InjectLocal reaches the dispatcher).

7. ✅ **`Server::serve`** — rewritten in `crates/server/src/lib.rs`: `spawn_coordinator` + input-stream forwarder + per-peer `ClientProxy`. `ServerConfig` gains a required `layout: SharedLayout` field; the binary (`bins/hops/src/main.rs`) temporarily substitutes `ScreenLayout::single_primary(display_name)` until the `layout.toml` loader lands.

8. ✅ **E2E test:** `crates/server/tests/coordinator_e2e.rs` — 3-screen layout, two mock clients, MouseMove walking desk → monitor → laptop. We assert that monitor receives ScreenEnter + MouseMove + ScreenLeave, while laptop receives ScreenEnter without ScreenLeave.

**Current status:** all 8 steps are done. 49 server tests + 1 handshake E2E + 1 coordinator E2E, clippy clean.

## Test plan

| Level | What | With |
|---|---|---|
| Unit | ScreenLayout: `screen_at`, rect arithmetic | `proptest` — random rects, random points |
| Unit | HeldState leave/enter symmetry | `rstest` — parameterised presses |
| Unit | ClipboardGrabState state machine | hand-written unit tests |
| Unit | `Coordinator::on_event` — full event matrix | hand-written, one per scenario |
| Integration | ClientProxy inbound/outbound over mock TCP | `tokio::net::duplex` |
| E2E | Server with two mock clients + a simulated event_stream | `MockScreen` + `Server::bind/serve` |

Planned coverage: ≥ 85% for the `coordinator/` module (pure logic code). ClientProxy is `tokio`-dependent, ~70% is enough.

## Estimate and risks

- **Development:** ~3 days. Roughly 1 day for layout + held + clipboard modules, 1 day for the Coordinator + tests, 1 day for `task.rs` + ClientProxy + E2E.
- **Risks:**
  - Edge-crossing arithmetic easily picks up off-by-ones at boundary points (cursor exactly on an edge, leaving via a corner). Covered by property tests.
  - Backpressure drop-on-full may be too aggressive on slow networks. If E2E tests become flaky, switch to bounded(8192) to buffer short spikes.
  - Lazy clipboard is deferred — which means Ctrl+V on the primary after a remote grab doesn't work. Document in the README.

## Resolved / deferred decisions

1. ~~**Layout storage.**~~ **Resolved:** separate `layout.toml`, path configured via `ServerSettings.layout.path`. Reasons are in the "LayoutStore and where layout lives on disk" section above.

2. **Inter-screen scale-factor:** DPI 100% on laptop vs 150% on desk. Should the y-coordinate scale on a crossing? The current rect-based layout operates in physical pixels. **Deferred:** ignore heterogeneous DPI until a second real client shows up; then add `logical_height` / coordinate transforms. For M11 we operate in physical pixels across all screens.

3. ~~**Broadcast vs per-client seq.**~~ **Resolved:** a single **global** `self.seq` for the whole Coordinator. The per-client seq alternative was considered — rejected because the active-screen switch is an event that concerns every client at once (a clipboard grab during a cross must have one seq, visible identically to every peer). Not planned to change.
