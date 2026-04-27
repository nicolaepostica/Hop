# M16 — Linux MVP

## Goal

Make Hop a working two-machine product on Linux/X11. After M16, given two Linux desktops on the same LAN, a user can:

1. Move the cursor across the right edge of the server's screen and have it appear on the client's screen — keyboard and mouse work there in real time.
2. Copy text on either machine and paste it on the other.
3. Copy a file or folder on either machine and paste it on the other.
4. Close the main window: the daemon keeps running, a tray icon stays visible, all of the above keep working.
5. Quit from the tray: the daemon stops, both machines clean up cleanly.

This is the first milestone where the project earns the name on the box. Until M16 the server *talks* but the client *listens silently* — `client::run::session_loop` decodes incoming `Mouse*` / `Key*` / `Clipboard*` / `FileTransfer*` messages and `debug!`-logs them without acting. M16 wires the receive path end-to-end on the one OS where every dependent crate is already real (X11).

Five existing-but-stranded subsystems converge here:

- **#1** — `crates/server/coordinator` (M11) already does cross-screen edge detection, but the client never injects what gets sent.
- **#2** — `crates/platform/x11` already has `inject_key` / `inject_mouse_*` / `X11Clipboard`, used by no one outside its own tests.
- **#3** — server-side clipboard merge logic (`coordinator/clipboard.rs`) exists, but no producer reads the local pasteboard, no consumer writes the remote one.
- **#4** — `crates/transfer` (`TransferSender` / `TransferReceiver`) is fully implemented with round-trip tests, but `grep hop_transfer:: crates/server crates/client` returns zero hits.
- **#5** — `crates/hop-ui/tray` (M14 commit 1) installs the icon and the menu, but `Tray::poll` results are never consumed.

M16 is the integration milestone that turns the existing scaffolding into a product on Linux.

## Prerequisites

- [M11](M11-coordinator.md) — server coordinator (done).
- [M13](M13-gui-backend.md) — embedded `BackendController` in the GUI (done).
- [M14](M14-tray.md) commit 1 — tray scaffolding (done).
- `crates/transfer` round-trip tests passing (done).
- `crates/platform/x11` X11Screen + X11Clipboard (done).

Cross-OS work (macOS in [M18](M18-macos-mvp.md), Windows in [M17](M17-windows-mvp.md)) is independent and can be parallelised.

## Scope

**In scope:**

- Client-side event injection: every `Message::Mouse*` / `Key*` from the server reaches the local `PlatformScreen` on the client.
- Bidirectional text clipboard sync: changes on either side propagate within ~250 ms.
- File-clipboard transfer: copying a file or folder on one machine produces it under `~/Downloads/Hop/` on the other, with a toast.
- Tray wiring (M14 commit 2 absorbed here): tray menu actions actually call into `BackendController`; closing the main window hides instead of exits.
- Wayland-only-session detection: if `WAYLAND_DISPLAY` is set without `DISPLAY`, refuse start with a clear error pointing at the future M6 milestone. The existing `EiScreen` scaffold is *not* upgraded here — Wayland is its own milestone.

**Out of scope:**

- libei / Wayland injection — separate milestone (M6 in the index).
- HTML / RTF clipboard formats. Plain text + file URIs only.
- Drag-and-drop between windows. Standard cut+paste only — drag is a separate IPC channel even on X11.
- Multi-monitor cursor maths beyond what M11 layout already supports.
- Auto-start on login (`.desktop` autostart entry). Separate, small follow-up.
- Two-way primary-selection sync (the X11 middle-click buffer). The CLIPBOARD selection is enough for MVP; documenting primary as a known gap.
- File-transfer progress UI. A single toast on completion; per-file progress in the GUI is a follow-up.
- File-transfer cancellation from the UI. Cancellation works on the protocol level (`FileTransferCancel`) but no UI surface yet.

## Architecture

### Big decision: where does the file-clipboard *trigger* live

Two options:

| Option | Pros | Cons |
|---|---|---|
| **A. Trigger off the local clipboard** — when the X11 selection contains `text/uri-list` or `x-special/gnome-copied-files`, scan paths and emit `FileTransferStart` proactively. Recipient writes files to `~/Downloads/Hop/`, also writes the staging paths into its own clipboard so paste in a file manager works. | Pure cut+paste UX. No new gestures or buttons. | The clipboard owns *every* file-copy in the session — over-eager: any cp-then-paste-locally also fires a transfer. Need a per-paste suppression. |
| **B. Explicit "send selection" via the tray menu and a toast** — user copies, then clicks "Send copied files" in the tray. Recipient gets toast + path. | No spurious transfers. Easier to debug. | Extra step. Less of a "shared clipboard" feel. Defeats requirement #3 ("paste on the other machine"). |

**Decision: Option A with two safeguards.**

1. **Per-content suppression.** Keep a `recently_sent: HashSet<ContentHash>` (FIFO, capped at 32). When the local clipboard changes and matches a hash we've recently *received*, do nothing — that's the round-trip echo. The hash covers manifest paths + sizes; cheap to compute.
2. **Size cap.** A single transfer larger than `transfer.max_total_bytes` (default 5 GiB, configurable) is refused with a toast, not silently started. Protects users from accidentally tarring the home directory across the LAN.

### Receive flow on the client (M3 work)

Currently `crates/client/src/lib.rs:117-126` has:

```rust
incoming = framed.next() => {
    match incoming {
        Some(Ok(msg)) => {
            keepalive.mark_seen();
            if matches!(msg, Message::Disconnect { .. }) { return Ok(()); }
            debug!(?msg, "message from server");
        }
        // …
    }
}
```

After M16:

```rust
Some(Ok(msg)) => {
    keepalive.mark_seen();
    match msg {
        Message::Disconnect { reason } => return Ok(()),
        Message::MouseMove { x, y, .. }       => screen.inject_mouse_move(x, y).await?,
        Message::MouseButton { id, down }     => screen.inject_mouse_button(id, down).await?,
        Message::MouseWheel { dx, dy }        => screen.inject_mouse_wheel(dx, dy).await?,
        Message::KeyDown { key, mods }        => screen.inject_key(key, mods, true).await?,
        Message::KeyUp { key, mods }          => screen.inject_key(key, mods, false).await?,
        Message::ScreenEnter { cursor }       => session.entered(cursor).await,
        Message::ScreenLeave                  => session.left().await,
        Message::ClipboardOffer { seq, fmt }  => clipboard.recv_offer(seq, fmt).await,
        Message::ClipboardData { seq, bytes } => clipboard.recv_data(seq, bytes).await,
        Message::FileTransferStart { … }      => file_rx.start(…).await?,
        Message::FileChunk { … }              => file_rx.chunk(…).await?,
        Message::FileTransferEnd { … }        => file_rx.finalise(…).await?,
        Message::FileTransferCancel { … }     => file_rx.cancel(…),
        Message::KeepAlive | Message::Hello { .. } | Message::DeviceInfo { .. } => {}
    }
}
```

`PlatformScreen` injection methods are already on the trait and already implemented on `X11Screen`. The wiring is mechanical.

### Send flow on the server (clipboard + file)

Already partly there: the coordinator broadcasts `Mouse*`/`Key*` based on the layout. What's missing on the producer side:

- **Local clipboard watcher.** Wrap `X11Clipboard` in a small async task that polls XFIXES `SelectionNotify` (or, fallback, polls `selection_owner` every 250 ms) and emits a `LocalClipboardChanged { seq, payload }` event into the coordinator. Coordinator already has `clipboard.rs::merge` and the right outbound message shape.
- **File trigger.** Same watcher: when the new selection contains `text/uri-list`, parse `file://` URIs, hand them to a fresh `TransferSender::new(transfer_id)`, register it in the coordinator's transfer table (new sub-state in `coordinator/state.rs`), and pump chunks into the outbound channel.

Symmetric on the *client* side — the client also watches its local clipboard and sends back `ClipboardOffer` / `FileTransferStart`. Same code, same crate, just the other actor.

### Tray wiring (absorbs M14 commit 2)

`HopApp::update` gains a `pump_tray()` step before `pump_backend_events()`:

```rust
fn pump_tray(&mut self, ctx: &Context) {
    let Some(tray) = self.tray.as_mut() else { return };
    for cmd in tray.poll() {
        match cmd {
            TrayCommand::ShowWindow            => self.show_window(ctx),
            TrayCommand::SwitchMode(m)         => if !self.controller.is_running() { self.mode = m; },
            TrayCommand::StartOrStop           => self.toggle_backend(),
            TrayCommand::About                 => { self.show_window(ctx); self.about_open = true; }
            TrayCommand::Quit                  => self.shutdown_requested = true,
        }
    }
    let state = TrayState::derive(&self.controller, &self.server_state);
    tray.reconcile(state, self.controller.is_running(), self.mode);
}
```

Close-to-tray: viewport `close_requested` redirected to `Visible(false)` on Linux.

### Crate-boundary changes

| Crate | Change |
|---|---|
| `crates/client` | New `mod input` and `mod clipboard_rx` and `mod file_rx`. `session_loop` becomes a thin dispatcher over them. |
| `crates/server` | New `mod local_clipboard` watcher. Coordinator gains a `transfers: HashMap<TransferId, TransferState>` table. |
| `crates/protocol` | Likely no new messages — the existing `ClipboardOffer/Data` + `FileTransfer*` cover the surface. Validate during impl. |
| `crates/platform/x11` | Add `X11Clipboard::watch() -> impl Stream<Item = SelectionEvent>` if the existing API is poll-only. |
| `crates/platform/core` | Possibly extend `PlatformScreen` with `clipboard_watch()` returning a stream. Default impl: poll-based. |
| `crates/hop-ui` | `pump_tray` + close-to-tray. |
| `bins/hops` / `bins/hopc` | Pick up the new client-side dispatcher automatically (they call `hop_client::run`). |

## Task details

### Task #1 — client input injection

**Files:** `crates/client/src/lib.rs`, new `crates/client/src/input.rs`.

Extend `session_loop` to dispatch `Mouse*`/`Key*` to `PlatformScreen`. Convert `client_handshake` outcome into a session struct that holds `screen: Arc<dyn PlatformScreen>` plus active-cursor state (do we own the cursor right now or not).

Edge: on `ScreenLeave`, the client may need to "warp" the cursor to a sentinel position so the user doesn't see a frozen cursor — define a no-op contract; X11 already hides the warp via `xtest_fake_input MOTION_NOTIFY`.

### Task #2 — text clipboard sync

**Files:** `crates/server/src/local_clipboard.rs` (new), `crates/client/src/clipboard_rx.rs` (new), `crates/platform/core/src/screen.rs`, `crates/platform/x11/src/clipboard.rs`.

- Server: spawn a watcher task that emits `Selection { seq, content }` into the coordinator's inbound queue. Coordinator routes via existing `coordinator/clipboard.rs::merge`.
- Client: same watcher (symmetric). Bidirectional.
- Loop suppression: `merge` already keeps a sequence counter; bump-and-skip when the content matches the last *received* one (compare hash, not bytes — bytes can be huge).

### Task #3 — file clipboard

**Files:** `crates/server/src/coordinator/transfers.rs` (new), `crates/client/src/file_rx.rs` (new), `crates/server/src/coordinator/state.rs` (gains `TransferTable`).

Server side, on `LocalClipboardChanged`:

```text
if payload.is_uri_list() {
    let manifest = TransferSender::scan(&paths, max_total_bytes)?;
    let id = transfer_table.allocate();
    spawn task: TransferSender::run(id, manifest, send_to_coordinator).await
}
```

The `send_to_coordinator` callback funnels `Message::FileTransferStart/Chunk/End` through the existing client-fanout in coordinator. No new wire shape.

Client side, on `Message::FileTransferStart`:

- Pick the staging directory (`$XDG_RUNTIME_DIR/hop/staging-<id>` or `/tmp/hop-staging-<id>` fallback).
- Drive `TransferReceiver` to completion.
- On `End`, atomically move the staged files into `~/Downloads/Hop/` (configurable). Toast: *"Received N file(s) (M MiB) from <peer>"*.
- Also write `~/Downloads/Hop/<entry>` paths into the local clipboard as `text/uri-list` so `Ctrl+V` in Nautilus pastes them — closes the loop on requirement #3.

### Task #4 — tray actions + close-to-tray

**Files:** `crates/hop-ui/src/app.rs`, `crates/hop-ui/src/lib.rs` (drop the `#[allow(dead_code)]`), `crates/hop-ui/src/tray/{backend_main.rs, backend_gtk.rs}` (real reconcile logic).

- Drop the M14-commit-1 `allow(dead_code)` once `pump_tray` consumes everything.
- Implement real `reconcile`: swap icons, update status header, flip `start_stop` label between Start / Stop, toggle `mode_*` checked flags, lock mode entries while running.
- Close-to-tray: handle `close_requested`, hide on Linux with a one-shot toast hint persisted to `~/.config/hop/ui-state.json`.
- Quit: stop backend, wait up to 2 s for `is_running == false`, then `ViewportCommand::Close`.

### Task #5 — Wayland refusal

**Files:** `crates/hop-ui/src/runtime/platform.rs`, `crates/client/src/lib.rs`, `crates/server/src/lib.rs`.

If a Linux session is detected and X11 is unavailable but Wayland is, fail `try_open()` with a message: *"Wayland sessions are not supported in M16. Use an X11 session or wait for M6 (libei). Run `echo $XDG_SESSION_TYPE` to check."* No silent MockScreen — the user must not think it works while injection is a no-op.

## Implementation order

Five commits, each leaves the tree green. Each commit ships its own integration test on `MockScreen` and its own row in the manual matrix.

### Commit 1 — "feat(client): inject mouse/key from incoming messages" (~2 days)

Task #1.

End: `cargo test -p hop-client` exercises a fake `framed` that pushes `Message::MouseMove` and asserts `MockScreen.recorded()` contains the call. No real X11 required.

### Commit 2 — "feat: bidirectional clipboard text sync" (~2 days)

Task #2.

End: an integration test that runs two `Coordinator + Mock` pairs over a duplex `tokio::io::DuplexStream`, sets the server's clipboard to "hello", asserts the client's mock received `set_clipboard("hello")` within 500 ms, then reverses direction.

### Commit 3 — "feat: file-clipboard transfer integration" (~3 days)

Task #3.

End: same harness as commit 2, but copies a temp directory of three files; asserts `~/Downloads/Hop/<dir>/` contents on the receiving side match. Reuses `crates/transfer`'s round-trip harness.

### Commit 4 — "feat(hop-ui): wire tray actions + close-to-tray" (~1 day)

Task #4. Drops `#[allow(dead_code)]`.

End: GUI manual matrix. Unit test on `Tray::reconcile` idempotence.

### Commit 5 — "feat: refuse Wayland sessions with a clear error" (~½ day)

Task #5.

End: integration test that sets `WAYLAND_DISPLAY=foo` + clears `DISPLAY` and asserts the runtime returns `PlatformError::Unavailable("…")`.

## Tests

| Level | What | How |
|---|---|---|
| Unit | `client::session_loop` dispatches each `Mouse*`/`Key*` to the right `PlatformScreen` method | `MockScreen.recorded()` |
| Unit | `local_clipboard` watcher dedupes by content hash | direct test on the watcher with synthetic events |
| Unit | `Tray::reconcile` is idempotent (same state twice = no native call) | counter on the backend |
| Integration | Two-coordinator clipboard text round-trip | `tokio::io::duplex` |
| Integration | File-clipboard 3-file directory round-trip | `tempfile::TempDir` + `assert_fs` |
| Integration | Wayland-only session is refused | env-clearing test |
| Manual | Two real Linux/X11 desktops on the same LAN: cursor crossing, Ctrl-C/V text, file copy/paste, close-to-tray, Quit | covered by human |

## Edge cases

- **Clipboard echo loop.** Without suppression, the receiving side immediately re-broadcasts what it just received. Hash-based suppression covers this; verify by setting clipboards on both sides simultaneously and asserting the system stabilises.
- **Huge file selection.** Copying `/usr` would attempt a 30 GiB transfer. `max_total_bytes` cap fires before scan completes; toast: *"Selection too large (45 GiB > 5 GiB cap). Increase `[transfer] max_total_bytes` or copy smaller chunks."*.
- **Recipient disk full mid-transfer.** `TransferReceiver` already returns `TransferError::Io`; surface as a red toast and emit `FileTransferCancel { reason: ReceiverError }` to the sender so it stops chunking.
- **Cursor stuck on the wrong screen after disconnect.** When the client disconnects mid-cross, the server already emits `ScreenLeave` to all peers and snaps the cursor back to the local active screen via `coordinator/state.rs::cross_to`. Verify in the manual matrix.
- **File names with non-UTF-8 bytes.** X11 file URIs are bytes; Rust filesystem paths on Linux are bytes. Pipe through `Path::new(OsStr::from_bytes(...))`. Test with a fixture filename `caf\xe9.txt`.
- **Already-pasted file moves into the staging dir.** If the user pastes before the transfer completes, the file manager sees a half-written file. Mitigation: Hop writes into a hidden `.hop-staging-<id>/` first and moves only on `End`. Document in the README.
- **Two `hop` processes on one machine.** Both watch the local clipboard and both will broadcast on change. Race. Mitigated by an `XDG_RUNTIME_DIR/hop/lock` lockfile (planned for M14 edge cases).
- **Modifier "stuck down" if connection drops mid-keystroke.** Server already has `coordinator/held.rs` to track held keys. On disconnect, server synthesises key-up events to the local screen and to remaining clients before dropping the proxy. Already covered; verify in the matrix.

## Estimate

| Commit | Effort |
|---|---|
| #1 client injection | ~2 days |
| #2 clipboard text | ~2 days |
| #3 file clipboard | ~3 days |
| #4 tray + close-to-tray | ~1 day |
| #5 Wayland refusal | ~½ day |
| Manual matrix on 2 real X11 boxes | ~1 day |

**Total: ~9–10 working days** for one engineer.

## Risks

- **XFIXES not available on every X server.** Some minimal X servers (Xvnc, lightweight remote desktops) do not advertise XFIXES. Fallback: 250 ms poll of `selection_owner`. Slightly worse latency, still inside the "feels live" budget.
- **`text/uri-list` decoding ambiguity.** Different file managers emit different MIME labels (`text/uri-list`, `x-special/gnome-copied-files`, `x-special/nautilus-clipboard`). Implementation must accept all three; test fixtures cover Nautilus, Dolphin, Thunar, plain `xclip -i -selection clipboard -t text/uri-list`.
- **`AT_SECURE` and `setuid` clients.** Hop is a normal-user app; not a concern, but document that injecting into another user's session requires that user to be running their own Hop, not yours.
- **NumLock / CapsLock state divergence.** X11 servers track lock state; injecting a key with a synthetic modmask drops the user's lock state. Bug fixed in `crates/platform/x11/src/keymap.rs::compose_mods`; manual matrix must include "type while CapsLock is on, both OSes".
- **Coordinator's transfer table grows unbounded.** Cap to N concurrent transfers per peer (default 4), reject with `FileTransferCancel { reason: TooManyConcurrent }` beyond.

## Resolved / deferred decisions

1. **File-clipboard trigger.** Resolved: clipboard-driven (Option A) with hash suppression + size cap.
2. **Drop directory.** Resolved: `~/Downloads/Hop/<original-name>/`. Configurable via `[transfer] drop_dir`.
3. **Wayland on Linux MVP.** Resolved: refuse with a clear error. Real support is M6 (libei).
4. **HTML / RTF clipboard.** Deferred. Plain text + file URIs cover requirement #2 + #3.
5. **Drag-and-drop.** Deferred. Cut+paste covers the requirement.
6. **Auto-start on login.** Deferred. Small follow-up after first user feedback.

## Readiness after M16

After M16:

- Two-machine Linux/X11 deployment is a real product against the requirement list.
- Every receive path that existed only in the protocol is now wired through `client::run`.
- The transfer crate has its first production caller; round-trip tests now also have an end-to-end peer over `DuplexStream`.
- The tray is a fully-featured desktop affordance, not a stub.
- All five user-visible requirements pass on the manual matrix.
- A clean platform abstraction: porting to Windows ([M17](M17-windows-mvp.md)) and macOS ([M18](M18-macos-mvp.md)) is *only* `WindowsScreen`/`MacOsScreen` work — no protocol, coordinator, or wiring changes needed.
