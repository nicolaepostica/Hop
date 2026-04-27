# M14 — System Tray

## Goal

Make `hop` a real desktop-resident application: a tray icon (status-bar item on macOS, notification-area icon on Windows, AppIndicator/StatusNotifier on Linux) that survives the main window being closed, exposes the most common controls without bringing the window forward, and reflects backend state at a glance.

After M14 the user can:

- Close the main window and the daemon keeps running, with a tray icon as the visible affordance.
- See **Idle / Server running / Client connected** from the icon alone (three icon variants).
- Start, stop, or switch mode from the tray menu without ever opening the window.
- Re-open the window from the tray (`Show window`) or by left-clicking the icon.
- Quit the whole app — daemon and tray together — from the tray.

The first-run wizard mentioned alongside tray in `README.md` is a separate concern and is deferred to **M15 — First-run wizard**. M14 ships only the tray.

## Prerequisites

- [M13](M13-gui-backend.md) — embedded `BackendController` with Start/Stop and `StatusEvent` stream. Tray is a thin consumer of M13's API.
- M12 packaging is implicitly extended: tray icon assets must ship in the bundle.

## Scope

**In scope:**

- A `Tray` actor in `crates/hop-ui/src/tray/` built on the [`tray-icon`](https://crates.io/crates/tray-icon) crate (Tauri ecosystem, MIT, actively maintained, all three OSes).
- Three icon variants (Idle / Server / Client) generated from the existing `assets/hop.svg` source, plus a macOS `template` (monochrome) variant for menubar.
- A static menu with: status header (disabled label), `Show window`, mode submenu (`Server` / `Client`, radio, locked while backend is running), `Start` / `Stop`, separator, `About Hop`, `Quit`.
- Bidirectional integration with the existing `BackendController`:
  - tray menu → `ControllerCommand` (Start / Stop / SwitchMode);
  - `StatusEvent` stream → tray icon variant + status header text + enable/disable of menu items.
- **Close-to-tray** on Linux and Windows: the Close (X) button hides the window instead of exiting; explicit `Quit` from the tray menu (or `File → Quit` if/when added) terminates the process. macOS keeps standard behaviour (close hides window, Cmd-Q quits).
- A **`--no-tray`** CLI flag and a `[gui] tray = "auto" | "off"` config setting for headless/CI / unsupported environments.
- Linux runtime detection: skip tray creation with a one-line `tracing::warn!` if no D-Bus session bus / no `org.kde.StatusNotifierWatcher` is available, instead of crashing.

**Out of scope:**

- First-run wizard (M15).
- Notifications / toast bubbles from the tray (e.g. "peer joined"). The in-app `egui-notify` toast stack covers this when the window is open; a system-notification path can come later via [`notify-rust`](https://crates.io/crates/notify-rust).
- Per-peer dynamic submenu ("Connected: laptop ▸ Disconnect"). Defer to a follow-up — keeps M14 small and avoids menu-rebuild complexity.
- Custom tooltip colour/animations beyond what `tray-icon` exposes natively.
- Auto-start on login (Login Items / Startup folder / `.desktop` autostart). Separate milestone.
- Theming the icon to match light/dark system theme on Windows. macOS template image gives this for free; Linux follows the desktop's icon theme. Windows users get a single full-colour icon for now.

## Architecture

### The big decision: who owns the event loop

`tray-icon` is built on top of [`tao`](https://crates.io/crates/tao), which is a fork of `winit`. eframe also drives a `winit` event loop. Two options:

| Option | Pros | Cons |
|---|---|---|
| **A. Run the tray on a dedicated background thread with its own native event loop** | Linux-correct: tray-icon needs a running GTK loop on the same thread that owns the icon — eframe does not run one. macOS-broken: `NSStatusBar` is main-thread-only and would crash there. | Cross-thread channel needed. |
| **B. Create the tray on the main thread, pump tray events from `update`** | macOS-correct. Windows-correct (winit pumps a Win32 message loop on the main thread, which `tray-icon` piggybacks on). Direct access to `BackendController`. | Linux-broken: tray-icon's Linux backend (libappindicator + GTK) requires a real GTK main loop, which eframe never starts. Menu clicks would never fire. |

**Decision: per-OS topology — main thread on macOS / Windows, dedicated GTK thread on Linux.**

The "right shape" is dictated by `tray-icon`'s own platform backends:

- **macOS** (`NSStatusItem`): main-thread-only. eframe's `winit` loop drives `NSRunLoop`; `tray-icon` events are delivered through that loop. Use Option B.
- **Windows** (`Shell_NotifyIcon` + per-thread hidden window): any thread that pumps Win32 messages works; eframe on the main thread already does. Use Option B.
- **Linux** (`libappindicator` over D-Bus, glued through GTK): tray-icon explicitly requires `gtk::init()` + a live `gtk::main()` loop on the same thread that owns the `TrayIcon`. eframe (winit + glow) does not run a GTK loop. Use Option A: spawn a worker thread that calls `gtk::init()`, builds the tray, and runs `gtk::main()`. The worker owns the `TrayIcon` for its full lifetime; egui talks to it via two `crossbeam_channel`s (commands in, events out). Closing the app sends a `Shutdown` command and the worker calls `gtk::main_quit()`.

This matches how Tauri itself drives `tray-icon` and is exactly the topology the upstream README warns is required. Implementation lives in `crates/hop-ui/src/tray/{mod.rs, backend_main.rs, backend_gtk.rs}` with `cfg`-gated dispatch.

To keep the eframe loop responsive while the window is hidden (so the egui side of the tray events keeps draining):

- Keep eframe alive when the user clicks Close — instead of letting the viewport close, set `state.window_visible = false` and hide the viewport via `ViewportCommand::Visible(false)`. eframe keeps spinning, the OS hides the window.
- On macOS / Windows: drain `TrayIconEvent::receiver()` and `MenuEvent::receiver()` from `HopApp::update` each frame.
- On Linux: drain the worker's outbound `crossbeam_channel<TrayCommand>` from `HopApp::update`. The worker thread translates native events into `TrayCommand`s before sending.
- Request a low-rate repaint (e.g. `ctx.request_repaint_after(Duration::from_millis(250))`) so the loop keeps pumping at 4 Hz when the window is hidden. Drops to ~0.05% CPU in our prototype.

### Runtime topology

```
            macOS / Windows                            Linux
   ┌──────────── main thread ──────────┐    ┌──────── main thread ────────┐
   │                                    │    │                              │
   │  HopApp (eframe)                   │    │  HopApp (eframe)             │
   │  ├─ Tray { TrayIcon, Menu, … }     │    │  ├─ TrayHandle               │
   │  └─ update():                      │    │  │   ├─ cmd_tx: Sender<…>    │
   │       drain MenuEvent::receiver()  │    │  │   ├─ evt_rx: Receiver<…>  │
   │       drain TrayIconEvent::recv()  │    │  │   └─ join: JoinHandle<()> │
   │       reconcile()                  │    │  └─ update():                │
   │                                    │    │       drain evt_rx           │
   └────────────────────────────────────┘    │       send reconcile() cmds  │
                                              └──────────────┬───────────────┘
                                                             │
                                                  ┌──────────┴──────────────┐
                                                  │   GTK worker thread     │
                                                  │   gtk::init()           │
                                                  │   build TrayIcon + Menu │
                                                  │   gtk::main()           │
                                                  │   on idle: drain cmd_rx,│
                                                  │     handle Reconcile /  │
                                                  │     Shutdown            │
                                                  │   on menu/icon event:   │
                                                  │     evt_tx.send(cmd)    │
                                                  └─────────────────────────┘
```

### `Tray` module

The public surface is the same on every OS — only the implementation behind it differs. Lives in `crates/hop-ui/src/tray/`:

- `tray/mod.rs` — `Tray` enum-wrapper that hides the per-OS backend; `TrayState`, `TrayCommand`, `TrayError` types.
- `tray/icons.rs` — load PNGs for the three states from embedded bytes (`include_bytes!`), pre-decoded into `tray_icon::Icon` once at construction time.
- `tray/menu.rs` — build the `Menu` + `TrayMenuItems` handles. Returns strongly-typed `MenuId`s so the dispatcher can match against them. Used by both backends.
- `tray/backend_main.rs` — *macOS / Windows* backend. Owns `tray_icon::TrayIcon` directly on the eframe main thread.
- `tray/backend_gtk.rs` — *Linux* backend. Spawns a worker thread that calls `gtk::init()`, builds the tray, runs `gtk::main()`. Communicates with the main thread via two `crossbeam_channel`s.

```rust
pub struct Tray { backend: Backend }

#[cfg(any(target_os = "macos", target_os = "windows"))]
type Backend = backend_main::MainThreadTray;

#[cfg(target_os = "linux")]
type Backend = backend_gtk::GtkWorkerHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    ServerRunning { peer_count: usize },
    ClientConnected,
}

struct TrayMenuItems {
    status_header: tray_icon::menu::MenuItem, // disabled — display only
    show_window:   tray_icon::menu::MenuItem,
    mode_server:   tray_icon::menu::CheckMenuItem,
    mode_client:   tray_icon::menu::CheckMenuItem,
    start_stop:    tray_icon::menu::MenuItem,  // label flips Start/Stop
    about:         tray_icon::menu::MenuItem,
    quit:          tray_icon::menu::MenuItem,
}

struct TrayIcons {
    idle:   tray_icon::Icon,
    server: tray_icon::Icon,
    client: tray_icon::Icon,
}

impl Tray {
    /// Construct. Returns `Ok(None)` if the platform reports the tray
    /// is unavailable (e.g. no StatusNotifierWatcher on a Wayland-only
    /// session, no D-Bus, gtk::init failure).
    pub fn try_new() -> Result<Option<Self>, TrayError>;

    /// Apply backend state to the tray (icon + label + enabled flags).
    /// Idempotent — no-op when `state == previous`.
    pub fn reconcile(&mut self, state: TrayState, mode_locked: bool, mode: AppMode);

    /// Drain native menu/icon events into a vector the app can match on.
    /// On macOS/Windows: drains `MenuEvent::receiver()` directly.
    /// On Linux: drains the worker thread's outbound `crossbeam_channel`.
    pub fn poll(&self) -> Vec<TrayCommand>;
}

#[derive(Debug, Clone)]
pub enum TrayCommand {
    ShowWindow,
    SwitchMode(AppMode),
    StartOrStop,   // semantics depend on current controller state
    About,
    Quit,
}
```

#### Linux GTK worker — protocol

```rust
// crates/hop-ui/src/tray/backend_gtk.rs (sketch)

enum WorkerCmd {
    Reconcile { state: TrayState, mode_locked: bool, mode: AppMode },
    Shutdown,
}

pub struct GtkWorkerHandle {
    cmd_tx:  crossbeam_channel::Sender<WorkerCmd>,
    evt_rx:  crossbeam_channel::Receiver<TrayCommand>,
    join:    Option<std::thread::JoinHandle<()>>,
}

fn worker_main(cmd_rx: Receiver<WorkerCmd>, evt_tx: Sender<TrayCommand>) {
    if let Err(e) = gtk::init() { /* send TrayError back via a oneshot */ return; }

    let icons = TrayIcons::load();
    let (menu, items) = menu::build();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icons.idle.clone())
        .with_tooltip("Hop")
        .build()
        .expect("tray icon");

    // Forward muda menu events into our outbound channel.
    let evt_tx_menu = evt_tx.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        if let Some(cmd) = items.dispatch(&ev.id) {
            let _ = evt_tx_menu.send(cmd);
        }
    }));
    let evt_tx_icon = evt_tx.clone();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |ev| {
        if let Some(cmd) = TrayCommand::from_icon_event(&ev) {
            let _ = evt_tx_icon.send(cmd);
        }
    }));

    // Pump cmd_rx via gtk::glib::idle_add_local — runs on the GTK loop.
    glib::idle_add_local(move || {
        match cmd_rx.try_recv() {
            Ok(WorkerCmd::Reconcile { .. })  => { /* set_icon + label + enabled */ }
            Ok(WorkerCmd::Shutdown)          => { gtk::main_quit(); return ControlFlow::Break; }
            Err(TryRecvError::Empty)         => {}
            Err(TryRecvError::Disconnected)  => { gtk::main_quit(); return ControlFlow::Break; }
        }
        ControlFlow::Continue
    });

    gtk::main();
    drop(tray);
}
```

`Drop for GtkWorkerHandle` sends `Shutdown` and joins the worker. Failure to join within 1 s is logged and detached — never block app exit.

### Wiring into `HopApp`

```rust
impl HopApp {
    fn new(cc: &CreationContext<'_>) -> Self {
        // … existing init …
        let tray = Tray::try_new().unwrap_or_else(|err| {
            tracing::warn!(?err, "tray unavailable; running without it");
            None
        });
        Self { /* …, */ tray, window_visible: true }
    }

    fn handle_tray(&mut self, ctx: &Context, frame: &mut Frame) {
        let Some(tray) = self.tray.as_mut() else { return };
        for cmd in tray.poll() {
            match cmd {
                TrayCommand::ShowWindow => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCommand::SwitchMode(m) if !self.controller.is_running() => {
                    self.mode = m;
                }
                TrayCommand::StartOrStop => {
                    if self.controller.is_running() {
                        self.controller.stop();
                    } else {
                        self.start_for_current_mode(/* … */);
                    }
                }
                TrayCommand::About    => self.window_visible = true /* + open About modal */,
                TrayCommand::Quit     => self.shutdown_requested = true,
                _ => { /* mode switch ignored while running */ }
            }
        }
        let state = match (self.controller.is_running(), self.mode) {
            (false, _)              => TrayState::Idle,
            (true, AppMode::Server) => TrayState::ServerRunning {
                peer_count: self.server_state.connected_peers.len(),
            },
            (true, AppMode::Client) => TrayState::ClientConnected,
        };
        tray.reconcile(state, self.controller.is_running(), self.mode);
    }
}
```

### Close vs Quit

eframe forwards window-close requests via `viewport.close_requested()` (egui ≥ 0.27). Default behaviour is to exit; we override:

```rust
if ctx.input(|i| i.viewport().close_requested) {
    if self.tray.is_some() && cfg!(any(target_os = "linux", target_os = "windows")) {
        // hide instead of quit
        self.window_visible = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
    // macOS: standard close (window hides, app stays alive via NSStatusItem)
}
```

`Quit` from the tray sets `self.shutdown_requested = true`, which the next `update` checks and calls `ctx.send_viewport_cmd(ViewportCommand::Close)` after stopping the backend.

### Icon assets

`assets/hop.svg` already exists and is rendered to PNG / ICNS / ICO during packaging (`scripts/gen-icons.sh`). M14 adds:

- `assets/tray-idle.svg`        — neutral "h" mark, ~80% opacity dot.
- `assets/tray-server.svg`      — green dot accent.
- `assets/tray-client.svg`      — blue dot accent.
- `assets/tray-template.svg`    — black-only, sized 22×22 pt for macOS menubar.

`scripts/gen-icons.sh` is extended to emit the corresponding PNGs (16/22/32 px). Bytes are `include_bytes!`-ed into the binary so the tray works without filesystem reads at runtime.

## Task details

### Task #1 — `tray` crate plumbing

**Files:** `crates/hop-ui/Cargo.toml`, `crates/hop-ui/src/lib.rs`, `crates/hop-ui/src/tray/{mod.rs,icons.rs,menu.rs}`.

Add `tray-icon = "0.x"` (latest at implementation time). Bring in its `winit` integration features only if needed — the default features cover the channel-based event API used here.

`Tray::try_new()` builds the icon, the menu, and returns `Ok(Some(_))` on success. On Linux it must check `std::env::var("DISPLAY")` / `WAYLAND_DISPLAY` and probe the D-Bus session — wrap the call in `catch_unwind` defensively, since older `tray-icon` versions panicked on missing `StatusNotifierWatcher`.

### Task #2 — icon assets + `gen-icons.sh`

**Files:** `assets/tray-*.svg`, `scripts/gen-icons.sh`.

Ship 16, 22, 32 px PNGs per state. Add a smoke step in CI that fails if `tray-idle@2x.png` is older than `tray-idle.svg` (catches a forgotten regen).

### Task #3 — wire `Tray` into `HopApp`

**Files:** `crates/hop-ui/src/app.rs`.

- Hold `tray: Option<Tray>` (None = unavailable / `--no-tray`).
- Call `handle_tray()` once per `update()`, before drawing.
- Add `shutdown_requested: bool` to drive the post-frame close path (so we can wait one frame for `BackendController::stop` to settle before the window closes).
- Replace direct `frame.close()` callsites; route everything through `shutdown_requested`.

### Task #4 — close-to-tray behaviour

**Files:** `crates/hop-ui/src/app.rs`, new `crates/hop-ui/src/ui_state.rs` (persistence).

- Detect close request, redirect to hide on Linux/Windows.
- Flag the first hide with a one-shot toast: `"Hop is still running in the tray. Right-click the icon to quit."` so users do not think the app froze.
- Persist the hint-shown flag in `~/.config/hop/ui-state.json`. Synthetic example of the on-disk shape:
  ```json
  {
      "closed_to_tray_hint_shown": true
  }
  ```
  Single boolean field for now; the file is intended to grow as more UI-only preferences appear (so it lives separately from the structured TOML config).

### Task #5 — `--no-tray` and `[gui] tray = "off"`

**Files:** `bins/hop/src/main.rs`, `crates/hop-config/src/gui.rs` (new section if not present).

Useful for: CI, headless test rigs, users on Wayland without StatusNotifier, debug runs where the close-to-tray dance gets in the way. CLI wins over config; config wins over auto.

Configuration lives under the existing TOML config tree. New `[gui]` keys:

```toml
[gui]
tray = "auto"              # "auto" | "off"
close_to_tray = "auto"     # "auto" | "always" | "never"
```

`auto` resolves at runtime to "on for Linux/Windows when StatusNotifier is reachable, off for macOS" (close_to_tray) and "on if a tray could be constructed" (tray).

### Task #6 — Linux runtime detection

**Files:** `crates/hop-ui/src/tray/mod.rs`.

Probe `org.kde.StatusNotifierWatcher` on the session bus before constructing the tray. If absent, log a `warn!` once and fall back to no tray — do **not** disable close-to-tray (window will fully exit on close, expected).

Document the GNOME Wayland gotcha in `README.md` under "Known limitations": users must install the AppIndicator/KStatusNotifier extension. Provide a one-liner for Ubuntu (`sudo apt install gnome-shell-extension-appindicator`).

## Implementation order

Two coherent commits, each leaves the tree green.

### Commit 1 — "feat(hop-ui): tray icon scaffolding (no-op menu)" (~3h)

1. Task **#1**: dependency, module, three-state icon, static menu with placeholder ids.
2. Task **#2**: SVG assets + script.
3. Make `Tray` construction conditional on `cfg!(not(test))` and respect `--no-tray`.

At this commit the tray appears, the menu opens, but clicking does nothing yet (clicks are logged but discarded). Useful for screenshots and visual review on all three OSes.

### Commit 2 — "feat(hop-ui): wire tray to backend and close-to-tray" (~1d)

1. Task **#3**: `handle_tray()` + state reconciliation.
2. Task **#4**: close → hide on Linux/Windows; one-shot hint toast.
3. Task **#5**: CLI flag + config option.
4. Task **#6**: Linux probe + warn-and-skip.
5. Manual matrix run (see Tests).

## Tests

| Level | What | How |
|---|---|---|
| Unit | `Tray::try_new()` returns `Ok(None)` when `DISPLAY` and `WAYLAND_DISPLAY` are both unset | env-clearing test, run on Linux only |
| Unit | `TrayState` derived from `(is_running, mode, peer_count)` matrix matches snapshot | parametrised table |
| Unit | Menu-id → `TrayCommand` mapping is total (no `_` arm in production code, exhaustive `match` over `MenuId`s) | `cargo test`, fails to compile on omission |
| Integration | Starting the server through `controller.start_server()` flips `Tray::reconcile` to `ServerRunning` within one frame | reuse `coordinator_e2e` harness, headless `Tray` stub |
| Manual | macOS: install bundle, close window, re-open from menubar, Cmd-Q quits | covered by human |
| Manual | Windows: install MSI, close to tray, balloon hint shows once, double-click icon restores | covered by human |
| Manual | Linux/X11 (XFCE, KDE): icon visible, menu items work, switching mode blocked while running | covered by human |
| Manual | Linux/Wayland (GNOME without extension): app launches, `warn!` logged, close → exit (no tray); with AppIndicator extension installed: tray works | covered by human |

The `egui_kittest` harness mentioned in M13 is not useful here — `tray-icon` calls into native APIs that mocking would only paper over. The matrix is run by hand on real OSes per release.

## Edge cases

- **App hidden, all peers gone, server still running.** Backend keeps running; tray icon stays green. Reconcile must not auto-stop the server when the window hides. Confirmed in Task #3 by reading `controller.is_running()` directly, never inferring from window visibility.
- **Tray creation fails mid-session** (rare; happens on KDE Plasma session restart). `Tray::poll()` returns errors silently swallowed; on next reconcile we log once and stop reconciling. The user sees no tray but the GUI stays alive.
- **User invokes `Quit` while `Stop` is mid-flight.** Set `shutdown_requested = true`; the frame loop waits for `controller.is_running() == false` before sending `ViewportCommand::Close`, with a 2 s timeout to avoid hangs.
- **Two `hop` instances, one tray each.** Both register; OS shows two icons. Document as expected. The optional per-user lockfile mentioned in M13's edge cases would also fix this — out of scope here.
- **macOS Dock vs menubar.** The tray icon is a menubar item. The Dock icon is unchanged from M13 (always shown when the app is running). A follow-up could add `LSUIElement = true` to make `hop` a menubar-only app, but that hides the Dock icon and changes window behaviour (windows do not appear in Cmd-Tab). Defer.
- **`--no-tray` + close clicked.** With no tray, close exits the app the standard way. Verify the close-to-tray override is gated on `self.tray.is_some()`.
- **Theme changes mid-session.** macOS template icon updates automatically. Linux follows the icon theme; the SVG-derived PNG does not. Acceptable for MVP.
- **Right-click vs left-click on Linux.** AppIndicator only supports a single left-click action ("show menu"); we cannot bind left-click to "show window". Document as a platform limitation. On Windows / macOS left-click shows the window; right-click opens the menu.

## Estimate

- Commit 1 (scaffolding): **~3 hours**.
- Commit 2 (wiring + close-to-tray + flags): **~1 day**.
- Manual cross-platform matrix: **~2 hours**, ideally split across the same release dry-run pass that already covers M12 packaging.

**Total:** ~1.5 days of focused work + matrix.

## Risks

- **`tray-icon` API churn.** The crate is pre-1.0 and has had breaking renames in the recent past. Pin a single minor version in `Cargo.toml`; revisit on each release dry-run.
- **Wayland fragmentation.** GNOME Wayland without the AppIndicator extension is the single largest deployment of "Linux desktop with no tray". The `--no-tray` fallback covers it but UX regresses (close = exit). Document loudly; revisit when a portal-based standard arrives.
- **Main-thread invariant on macOS / Windows.** `NSStatusBar` is main-thread-only on macOS; `Shell_NotifyIcon` needs a thread that pumps Win32 messages. The eframe main thread is correct for both — assert this with a `debug_assert!(is_main_thread())` in `MainThreadTray::new`.
- **GTK loop ownership on Linux.** The worker thread must call `gtk::init()` exactly once per process — calling it twice panics. Guard with `std::sync::Once`. The worker also owns the `TrayIcon` for its full lifetime; any cross-thread access would race the GTK main loop. The handle on the main thread holds only channel ends, never the `TrayIcon` itself.
- **Worker thread crashes.** If the GTK worker panics, the channels disconnect; the main thread detects the disconnect on next `poll()`, logs once, and runs the rest of the session without a tray (graceful degradation, identical to "tray unavailable").
- **Background CPU.** A 250 ms repaint timer keeps `update` running while the window is hidden. On a quiet day this draws ~0.05% CPU on our test box, but on slow ARM laptops the `egui` redraw cost matters. Add a coarse benchmark in `xtask` (`cargo xtask bench-tray`) that asserts < 0.5% CPU over 60 s on the CI runner.
- **Icon scaling on HiDPI.** Windows expects 16×16 *and* 32×32 in the same `.ico`; macOS expects an `@2x` paired image. Capture this in `scripts/gen-icons.sh` so a fresh checkout produces all variants.
- **Testing.** The unit tests above are real, but the bulk of confidence comes from the manual matrix. Build a checklist into the release runbook so the matrix is not skipped under deadline pressure.

## Resolved / deferred decisions

1. **Tray library.** Resolved: `tray-icon`. Considered `tray-item-rs` (older, no Wayland AppIndicator pathway) and `ksni` (Linux-only). `tray-icon` covers all three OSes and ships with menu support.
2. **Process model for the tray.** Resolved: same process as the GUI / backend. **Thread model is per-OS:** macOS / Windows on the eframe main thread, Linux on a dedicated GTK worker thread (single-thread-of-truth for the `TrayIcon`). Driven by `tray-icon`'s own backend constraints, not preference.
3. **Close-to-tray default.** Resolved: on by default for Linux + Windows; macOS keeps native window behaviour. Override via `[gui] close_to_tray = "always" | "never" | "auto"` (auto = current rule).
4. **First-run wizard.** Deferred to **M15**.
5. **Notifications.** Deferred. The in-app toast covers everything while the window is open; the tray covers the rest while it is closed. Adding `notify-rust` is a small follow-up if users ask for it.
6. **Per-peer dynamic submenu.** Deferred. The status-header label ("Server: 2 peers connected") plus the in-app peers list cover the need for now.
7. **Auto-start on login.** Deferred. Plays into M12 packaging more than the tray itself.

## Readiness after M14

After M14:

- Closing the main window on Linux / Windows leaves the daemon alive and discoverable.
- Tray icon reflects backend state (Idle / Server / Client) at a glance.
- All three platforms pass the manual checklist.
- The follow-up wizard (M15), per-peer submenu, and notifications can all be added without re-architecting `Tray` — they slot into `TrayCommand` / `TrayState` cleanly.
