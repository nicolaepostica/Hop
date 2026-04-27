# M18 — macOS MVP

## Goal

Make Hop a working two-machine product on macOS. After M18, given two Macs on the same LAN, a user can:

1. Move the cursor across the right edge of the server's screen and have it appear on the client's screen — keyboard and mouse work there in real time.
2. Copy text on either machine and paste it on the other.
3. Copy a file or folder on either machine and paste it on the other.
4. Close the main window: the daemon keeps running, a tray icon stays visible in the menu bar, all of the above keep working.
5. Quit from the tray (or `Cmd-Q`): the daemon stops, both machines clean up cleanly.

Same five requirements as [M16](M16-linux-mvp.md) and [M17](M17-windows-mvp.md). Like M17, M18 is a **backend swap**: M16's wiring (client dispatcher, coordinator transfers, tray actions, file-clipboard producers/consumers) is platform-agnostic and reused unchanged. M18 only replaces `MacOsScreen`'s stub with real `CGEvent` / `NSPasteboard` calls and adds the macOS-specific permission flow.

`crates/platform/macos/src/screen.rs:23-30` is currently a stub: `try_open()` returns `Err(PlatformError::Unavailable("macOS CGEvent backend is a scaffold; …"))`. M18 replaces every `unsupported(...)` arm.

## Prerequisites

- [M11](M11-coordinator.md) — coordinator (done).
- [M13](M13-gui-backend.md) — GUI ↔ backend (done).
- [M14](M14-tray.md) — tray scaffolding (done; on macOS the `MainThreadTray` path runs unchanged because `NSStatusItem` lives on the main thread, which eframe owns).
- [M16](M16-linux-mvp.md) — platform-agnostic wiring (done at the milestone level).
- An Apple silicon **and** an Intel Mac for the universal-binary smoke. Apple Developer ID for signing + notarisation (M12 packaging already covers).

## Scope

**In scope:**

- `MacOsScreen` real implementation: cursor read, screen geometry, key & mouse injection, mouse wheel, multi-display support.
- `MacPasteboard` real implementation: text + `NSFilenamesPboardType` / `NSPasteboardTypeFileURL` (file URLs).
- Accessibility permission first-run flow: `AXIsProcessTrustedWithOptions` with `kAXTrustedCheckOptionPrompt`, plus a clear UX path through `System Settings → Privacy & Security → Accessibility`.
- Tray on macOS (already works through `tray-icon`'s `NSStatusItem` backend on the main thread — verify on real hardware).
- `LSUIElement` flag in the bundle's `Info.plist` so closing the window leaves Hop as a menu-bar-only app (no Dock icon, no `Cmd-Tab` entry). Toggleable via `[gui] menubar_only = true|false` config; default `true` for MVP.
- `CGEventTap`-based local cursor watcher for the local clipboard / cursor-edge detection on the *server* side (note: M16's coordinator does the edge logic; the platform supplies the cursor stream).
- Manual matrix on Apple silicon (M-series) and Intel.
- Notarised `.dmg` build going through the existing pipeline.

**Out of scope:**

- Pre-login agent (LaunchAgent / LaunchDaemon). Defer to a follow-up; the embedded backend in `hop` is enough for MVP.
- Auto-start on login. `osascript -e 'tell application "System Events" to make login item …'` is one line, but the UX of "do you want Hop to start at login?" deserves its own milestone slot.
- Touch Bar input. The legacy MacBook Pro Touch Bar is dead hardware; do not invest.
- Apple Silicon GPU acceleration paths beyond what eframe already does.
- iPadOS / Universal Control parity. Apple's own Universal Control is a different beast.
- Continuity Camera / sidecar style features.
- Sandbox / App Store distribution. Direct Developer-ID distribution only.
- Multi-user / Fast User Switching. Hop runs in one session; documented limitation.

## Architecture

### Big decision: which Cocoa layer

Three options:

| Option | Pros | Cons |
|---|---|---|
| **A. `objc2` ecosystem (`objc2`, `objc2-app-kit`, `objc2-foundation`)** — modern, type-safe Rust ↔ Objective-C bindings. `objc2 = "0.5"` already in workspace deps. | Type-checked at compile time. Active maintenance. Idiomatic. | Younger ecosystem; some rough edges around `MainThreadMarker`. |
| **B. Hand-written FFI via `extern "C"` + `core_foundation`** | Smallest dependency cost. | Type-erased; runtime crash potential; reinventing what objc2 covers. |
| **C. `cocoa` crate (deprecated)** | Older, broader API surface. | Officially deprecated by `objc2` upstream; do not adopt new. |

**Decision: Option A.** `objc2 = "0.5"` is already a workspace dep; `objc2-app-kit` and `objc2-core-graphics` are routine additions. Aligns with the same direction `tray-icon` itself uses.

### Big decision: input injection API

Two shapes:

| API | Pros | Cons |
|---|---|---|
| **`CGEvent` family (`CGEventCreateKeyboardEvent`, `CGEventPost`)** | Standard. Works for almost every app. Handles modifiers cleanly. | Requires Accessibility permission. |
| **HID-level injection via `IOKit` HID-driver APIs** | No Accessibility prompt. | Requires loading a kext or using DriverKit; far out of scope. |

**Decision: `CGEvent`.** Accessibility permission is the standard cost of admission for KVM-class apps on macOS; users expect it once.

### `MacOsScreen` shape

```rust
pub struct MacOsScreen {
    info: ScreenInfo,
    /// Source for synthesised events. `kCGHIDEventTap` posts at the HID
    /// level so receivers think the events came from real hardware.
    event_source: CGEventSource,
    /// Mapping from `KeyId` to macOS `kVK_*` virtual keycodes. Tracks
    /// the active layout via `TISCopyCurrentKeyboardLayoutInputSource`
    /// so non-US layouts type the right characters.
    keymap: MacKeyMap,
    /// `NSPasteboard.general` watcher. Cocoa has no push API; poll the
    /// `changeCount` every 250 ms. Cheap (an `NSInteger` read).
    pasteboard: MacPasteboard,
}
```

`PlatformScreen` impl:

| Method | Cocoa / CG call |
|---|---|
| `inject_key(key, mods, down)` | `CGEventCreateKeyboardEvent` + `CGEventSetFlags(modifier_bits)` + `CGEventPost(kCGHIDEventTap, event)` |
| `inject_mouse_move(x, y)` | `CGEventCreateMouseEvent(.., kCGEventMouseMoved, CGPoint, kCGMouseButtonLeft)` |
| `inject_mouse_button(id, down)` | `CGEventCreateMouseEvent` with `kCGEventLeftMouseDown/Up` etc. |
| `inject_mouse_wheel(dx, dy)` | `CGEventCreateScrollWheelEvent` (continuous) — convert 120-units back to point delta |
| `read_cursor()` | `CGEventCreate(NULL).location` or `NSEvent.mouseLocation` |
| `screen_info()` | `CGDisplayBounds(CGMainDisplayID())` for primary; `NSScreen.screens` for layout |
| `set_clipboard(...)` | `NSPasteboard.general.declareTypes([.string], owner: nil); setString(_:forType:)` |
| `get_clipboard()` | mirrored |

### Pasteboard listener — polling, not push

NSPasteboard's `changeCount` is the only watch primitive. Implementation:

```text
┌──── eframe main thread ────┐
│                             │
│  HopApp                     │
│  └─ MacPasteboardHandle     │
│      tx: tokio mpsc<Event>  │
│                             │
└──────────┬──────────────────┘
           │
           ▼  (250 ms poll)
┌──── pasteboard poll task (tokio) ──┐
│  loop {                              │
│    let cc = NSPasteboard            │
│      .general.changeCount;          │
│    if cc != last { read & send }    │
│    sleep(250 ms);                   │
│  }                                   │
└──────────────────────────────────────┘
```

A 4 Hz tick is invisible CPU; iterm2 / Alfred / 1Password do the same. Push-style listeners exist only via private API or via `CGEventTap` filtering for `kCGEventTapOptionListenOnly` Cmd-C presses, both more invasive.

### File clipboard on macOS

Modern API: `NSPasteboardTypeFileURL` (`public.file-url`) — pasteboard items each carry a `file://` URL. Receiving:

1. M16's `file_rx` finalises into `~/Downloads/Hop/<entry>/`.
2. After finalisation, write `file://` URLs as pasteboard items via `NSPasteboard.writeObjects([NSURL])`.
3. M16's hash suppression prevents the immediate echo.

Sending: when the polled `changeCount` ticks and the new item carries `NSPasteboardTypeFileURL`, read URLs, convert to filesystem paths, hand to `TransferSender`.

### Accessibility permission flow

Two-stage UX:

1. **First start, before any backend run.** Call `AXIsProcessTrustedWithOptions(&[kAXTrustedCheckOptionPrompt: true])`. macOS shows the system prompt and adds Hop to the Accessibility list (un-checked).
2. **User toggles checkbox in System Settings.** Permission is persistent. Hop polls `AXIsProcessTrusted()` once a second; on `true`, it removes the in-app warning banner and unblocks Start.

Banner UX (when not yet trusted):

```
⚠  Hop needs Accessibility permission to forward keyboard / mouse.
   System Settings → Privacy & Security → Accessibility → Hop.
   [Open System Settings]   [Re-check]
```

`Open System Settings` runs `open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"`.

Persisted state: `~/Library/Preferences/com.hop.app.plist` flag `accessibility_prompt_shown: bool` so the system prompt only fires once per install (on subsequent launches we go straight to the banner if untrusted).

### LSUIElement (menu-bar-only mode)

Add to the bundle's `Info.plist`:

```xml
<key>LSUIElement</key>
<true/>
```

Effect: no Dock icon, no `Cmd-Tab` entry, no `File → Quit` in a per-app menu bar (eframe handles `Cmd-Q` directly). The tray icon becomes the *only* affordance once the window is closed. Match what 1Password 8, Bartender, Rectangle ship.

Flag is conditional on the `[gui] menubar_only` config key; default `true`. Setting `false` regenerates the bundle without `LSUIElement` (M12 packaging task).

## Task details

### Task #1 — `MacOsScreen::try_open` + screen geometry

**Files:** `crates/platform/macos/src/screen.rs`, new `crates/platform/macos/src/keymap.rs`, new `crates/platform/macos/src/permission.rs`.

- `CGDisplayBounds(CGMainDisplayID())` for primary geometry.
- `NSScreen.screens` (via `objc2-app-kit`) for the full multi-display layout.
- `try_open()` calls `permission::is_trusted()`; if `false`, returns `PlatformError::Unavailable("Accessibility permission required")` — the GUI surfaces the banner.

### Task #2 — keyboard injection + keymap

**Files:** `crates/platform/macos/src/keymap.rs`, `crates/platform/macos/src/screen.rs`.

- `KeyId` → `kVK_*` mapping (US-keyboard-baseline; layout translation happens in the receiving app via the active TIS input source).
- Modifier handling: pre-set `CGEventSetFlags(CGEventFlags::MASK_SHIFT | …)` *before* `CGEventPost`. Mirrors X11 behaviour from `crates/platform/x11/src/keymap.rs`.
- Synthesise key-down + key-up as separate events (CGEvent does not have a "press" primitive).

### Task #3 — mouse injection

**Files:** `crates/platform/macos/src/screen.rs`.

- `CGEventCreateMouseEvent` with `CGPoint` in *display coordinates* (not normalised). Multi-display: choose target display from M11 layout, translate.
- Buttons: `CGEvent` distinguishes left / right / center / others by `CGMouseButton`.
- Wheel: `CGEventCreateScrollWheelEvent2(.., kCGScrollEventUnitPixel, 2 /* wheel count */, dy_pixels, dx_pixels, 0)`. Convert from 120-units (`dy_pixels = dy * 1`).

### Task #4 — pasteboard sync

**Files:** `crates/platform/macos/src/pasteboard.rs` (new), `crates/platform/macos/src/screen.rs`.

- 250 ms poll task. `NSPasteboard.general.changeCount` comparison.
- Read text: `NSPasteboard.string(forType: .string)`.
- Read files: `NSPasteboard.readObjects(for: [NSURL.self], options: nil)`.
- Write: `NSPasteboard.clearContents(); NSPasteboard.writeObjects([…])`.
- Loop suppression interaction: M16's hash registry; the *next* `changeCount` poll after our own write ticks too — suppress as in-flight echo.

### Task #5 — Accessibility permission UX

**Files:** `crates/platform/macos/src/permission.rs`, `crates/hop-ui/src/views/server.rs`, `crates/hop-ui/src/views/client.rs` (banner widget).

- `permission::is_trusted()` → `bool`.
- `permission::request()` → spawns the system prompt, returns immediately.
- `permission::open_settings()` → `open` deeplink.
- GUI banner widget shown above the central panel on macOS when `!is_trusted()`. Hidden as soon as the poll flips. Banner does not block other UI.

### Task #6 — LSUIElement + bundle changes

**Files:** `bundle/macos/Info.plist.tmpl`, `bins/hop/build.rs` (or M12's existing bundle script), `crates/hop-ui/src/lib.rs`.

- Templated `LSUIElement` based on `cargo:rustc-cfg=hop_menubar_only`.
- Default `true`. CI builds also default-true; opt-out is a manual config.
- Verify behaviour: open Hop, close window, look at Dock — no icon. Tray remains.

### Task #7 — Manual cross-arch matrix

**Files:** `specs/milestones/M18-macos-mvp.md` (this file) — tracking only.

| Scenario | Pass / Fail |
|---|---|
| Apple silicon → Apple silicon, cursor crosses, types | |
| Apple silicon → Intel, mixed | |
| Retina ↔ non-Retina mixed | |
| Text Cmd-C/V both directions | |
| File copy/paste 1 file, 100 files, 1 large file (1 GiB) | |
| Close window → tray; tray → Show window | |
| Cmd-Q quits cleanly | |
| Reboot persistence (no auto-start expected) | |
| Disable Accessibility → banner reappears, Start blocked | |

## Implementation order

Five commits, each leaves the tree green.

### Commit 1 — "feat(platform/macos): screen geometry + permission probe" (~1 day)

Tasks #1 + #5 (banner only, not the runtime polling task yet).

End: `cargo build -p hop-platform-macos` on a Mac; `MacOsScreen::try_open()` returns `Err(Unavailable)` cleanly when Accessibility is off, `Ok(_)` when on.

### Commit 2 — "feat(platform/macos): keyboard injection" (~2 days)

Task #2.

End: type "hello" into TextEdit via a manual smoke; manual matrix.

### Commit 3 — "feat(platform/macos): mouse injection + wheel" (~2 days)

Task #3.

End: cursor moves, clicks, scrolls; manual matrix.

### Commit 4 — "feat(platform/macos): pasteboard text + file URLs" (~2 days)

Task #4.

End: Mac-Mac round-trip text and files manually; M16 suppression keeps the loop quiet.

### Commit 5 — "chore(packaging): LSUIElement + permission banner UX" (~1 day)

Task #6 + the runtime poll part of Task #5.

End: bundle launches as menu-bar-only, banner toggles correctly, notarised DMG passes the existing M12 pipeline.

## Tests

| Level | What | How |
|---|---|---|
| Unit | `KeyId` → `kVK_*` mapping is total over the modifier matrix | parametrised table |
| Unit | `dy` 120 → CGEvent `kCGScrollEventUnitPixel` ratio | direct table |
| Unit | `permission::is_trusted` returns `false` when called from a non-trusted process | `#[cfg(target_os = "macos")] #[test]`, run only on macOS CI |
| Integration | `MacOsScreen::try_open()` succeeds in a trusted session | macOS CI runner with pre-granted Accessibility (system-test profile) |
| Manual | Two real Macs (one Apple silicon, one Intel), cursor + clipboard text + file copy + close-to-tray + Cmd-Q | covered by human |

CI: a macOS runner (GitHub `macos-latest`) builds the crate and runs unit tests. Accessibility-dependent tests run on a self-hosted Mac (or are gated `#[ignore]` on hosted runners).

## Edge cases

- **Accessibility list resets on macOS upgrades.** A new major macOS version sometimes wipes the list. Banner reappears, user re-enables. Document.
- **Notarisation revocation.** Apple can revoke a Developer ID; the binary stops launching. Out of scope at the milestone level; M12 release runbook owns the response.
- **Mouse capture on full-screen apps.** Some games / Zoom-share use `CGAssociateMouseAndMouseCursorPosition(false)` and trap the cursor. `CGEventPost` still works, but the visible cursor may not move; document.
- **Mission Control / Stage Manager spaces.** The pasteboard is per-user, not per-space; no special handling needed.
- **Sleep / wake.** `tcp` connection drops on lid-close; M2 keepalive + reconnect handles. No special CGEvent handling needed.
- **Blocked CGEventPost on lock screen.** `loginwindow` ignores synthetic events; expected. Hop pauses while screen is locked.
- **Two GUI instances.** Per-user lockfile via `flock` on `~/Library/Application Support/Hop/lock`. Defer to follow-up.
- **Privacy-permissions-not-yet-prompted state.** `AXIsProcessTrustedWithOptions(prompt:true)` only prompts *once* per process lifetime — second call returns `false` without prompting. So the persisted flag is essential.

## Estimate

- Implementation: ~8 working days
- Permission UX iteration: ~1 day
- Manual matrix on two Macs: ~1 day
- Notarisation troubleshooting buffer: ~½ day

**Total: ~10–11 working days** (≈ 2 weeks calendar) on a Mac-equipped engineer.

## Risks

- **`objc2` API churn.** The crate is pre-1.0 and has had breaking renames. Pin a minor version; revisit on each release dry-run.
- **Notarisation slowness.** Apple's notarisation queue can take minutes; CI must tolerate. The M12 pipeline already does, but tray + new entitlements may shake out new staple-bag failures.
- **Accessibility-prompt UX.** First-launch prompts are notoriously confusing; document the exact sequence (open Hop → click Start → see prompt → open Settings → toggle → return to Hop). Capture screenshots in the README.
- **Universal binary build.** `lipo`-merging Apple silicon + Intel slices works today (M12), but adding new dependencies (e.g. `objc2-app-kit`) can fail on one slice if the dep doesn't ship pre-built bottles. Build both arches in CI.
- **Menu-bar-only behaviour conflicts with eframe.** `LSUIElement = true` changes how `NSApp.activationPolicy` works; if the user opens the Hop window, it must still come to the foreground. Verify that `ctx.send_viewport_cmd(ViewportCommand::Focus)` works under `LSUIElement`. Possible workaround: temporarily call `[NSApp activateIgnoringOtherApps:YES]` from Rust.
- **`tray-icon` on macOS.** Already pulled in. The M14 main-thread topology works (`NSStatusItem` is main-thread-only). Verify on Apple silicon and Intel.

## Resolved / deferred decisions

1. **Cocoa binding crate.** Resolved: `objc2` family (Option A).
2. **Injection API.** Resolved: `CGEvent` (`kCGHIDEventTap`).
3. **Pasteboard watch strategy.** Resolved: 250 ms poll on `changeCount`. Push via `CGEventTap` is over-invasive.
4. **Menu-bar-only mode.** Resolved: default `true`. Toggleable.
5. **LaunchAgent / LaunchDaemon.** Deferred. MVP's embedded backend in `hop` is enough.
6. **Universal Control parity.** Deferred — different design space.
7. **Auto-start on login.** Deferred. Small follow-up.

## Readiness after M18

After M18:

- macOS is a peer-class platform alongside Linux/X11 ([M16](M16-linux-mvp.md)) and Windows ([M17](M17-windows-mvp.md)).
- All five user-visible requirements pass on the manual matrix on Apple silicon and Intel.
- The Cocoa backend lives behind the same `PlatformScreen` trait — Hop is now a real cross-platform KVM.
- Accessibility-permission UX is documented, tested, and sticky.
- Menu-bar-only mode delivers the "background app" feel macOS users expect; the tray is the canonical affordance.
- The remaining Linux follow-ups (Wayland in M6, file-transfer progress UI, auto-start, drag-and-drop) are independent of M18.
