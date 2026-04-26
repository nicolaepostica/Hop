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
| **A. Run the tray on a dedicated background thread with its own `tao::EventLoop`** | Zero coupling with eframe internals. eframe upgrades cannot break us. Works the same on all three OSes. | One extra thread. Cross-thread channel from tray → app needed for menu actions. macOS requires the tray to be created on the main thread (NSStatusBar is main-thread-only) — this option **breaks on macOS**. |
| **B. Create the tray on the main thread inside `HopApp::new`, pump tray events from `update`** | macOS-correct (NSStatusBar lives on main thread). One event loop conceptually. Direct access to `BackendController` without channels. | Couples our timing to `egui::Context::request_repaint`. If the window is closed and eframe sleeps, tray menu clicks might not be polled. Need to schedule a periodic repaint while a tray exists. |

**Decision: Option B with a hidden window.** macOS forces our hand. To work around eframe sleeping when the window is hidden:

- Keep eframe alive even when the user clicks Close — instead of letting `Frame::close()` run, set `state.window_visible = false` and hide the viewport via `ViewportCommand::Visible(false)`. eframe keeps spinning, the OS hides the window.
- Drive `tray-icon`'s native event channel from `HopApp::update`: each frame calls `TrayIconEvent::receiver().try_iter()` and `MenuEvent::receiver().try_iter()`, dispatching matched events to the controller.
- Request a low-rate repaint (e.g. `ctx.request_repaint_after(Duration::from_millis(250))`) so the loop keeps pumping at 4 Hz when the window is hidden. Drops to ~0.05% CPU in our prototype.

### Runtime topology

```
┌────── GUI thread (eframe / winit, never exits while app runs) ──────┐
│                                                                      │
│   HopApp                                                             │
│   ├─ window_visible: bool                                            │
│   ├─ BackendController     ──── from M13                             │
│   ├─ Tray                  ──── new in M14                           │
│   │   ├─ icon: TrayIcon                                              │
│   │   ├─ menu: Menu                                                  │
│   │   ├─ items: TrayMenuItems  (handles for enable/disable)          │
│   │   └─ icons: TrayIcons      (Idle / Server / Client variants)     │
│   ├─ Toasts                                                          │
│   └─ Shared { … }                                                    │
│                                                                      │
│   update():                                                          │
│     1. drain BackendController status events                         │
│     2. drain TrayIconEvent::receiver()  → ShowWindow / Toggle        │
│     3. drain MenuEvent::receiver()       → Start / Stop / Quit / …   │
│     4. reconcile tray icon + status header from controller state     │
│     5. request_repaint_after(250 ms)                                 │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### `Tray` module

Lives in `crates/hop-ui/src/tray/mod.rs` plus:

- `tray/icons.rs` — load PNGs for the three states from embedded bytes (`include_bytes!`), pre-decoded at startup.
- `tray/menu.rs` — build the menu, return strongly-typed `MenuItemId`s so `update` can match against them.

```rust
pub struct Tray {
    icon: tray_icon::TrayIcon,
    menu_items: TrayMenuItems,
    icons: TrayIcons,
    last_state: TrayState,
}

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
    /// Construct on the main thread. Returns `Ok(None)` if the platform
    /// reports tray is unavailable (e.g. no StatusNotifier on Wayland).
    pub fn try_new() -> Result<Option<Self>, TrayError>;

    /// Apply backend state to the tray (icon + label + enabled flags).
    /// Idempotent — called every frame, no-op when `state == self.last_state`.
    pub fn reconcile(&mut self, state: TrayState, mode_locked: bool, mode: AppMode);

    /// Drain native menu/icon events into a vector the app can match on.
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
- **Main-thread invariant on macOS.** NSStatusBar API must be called from the main thread. `tray-icon` enforces this with a runtime panic in debug builds. Make sure no helper thread ever touches `Tray`.
- **Background CPU.** A 250 ms repaint timer keeps `update` running while the window is hidden. On a quiet day this draws ~0.05% CPU on our test box, but on slow ARM laptops the `egui` redraw cost matters. Add a coarse benchmark in `xtask` (`cargo xtask bench-tray`) that asserts < 0.5% CPU over 60 s on the CI runner.
- **Icon scaling on HiDPI.** Windows expects 16×16 *and* 32×32 in the same `.ico`; macOS expects an `@2x` paired image. Capture this in `scripts/gen-icons.sh` so a fresh checkout produces all variants.
- **Testing.** The unit tests above are real, but the bulk of confidence comes from the manual matrix. Build a checklist into the release runbook so the matrix is not skipped under deadline pressure.

## Resolved / deferred decisions

1. **Tray library.** Resolved: `tray-icon`. Considered `tray-item-rs` (older, no Wayland AppIndicator pathway) and `ksni` (Linux-only). `tray-icon` covers all three OSes and ships with menu support.
2. **Process model for the tray.** Resolved: same process as the GUI / backend (Option B above). macOS forces this.
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
