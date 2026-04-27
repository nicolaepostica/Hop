# M17 — Windows MVP

## Goal

Make Hop a working two-machine product on Windows. After M17, given two Windows desktops on the same LAN, a user can:

1. Move the cursor across the right edge of the server's screen and have it appear on the client's screen — keyboard and mouse work there in real time.
2. Copy text on either machine and paste it on the other.
3. Copy a file or folder on either machine and paste it on the other.
4. Close the main window: the daemon keeps running, a tray icon stays visible, all of the above keep working.
5. Quit from the tray: the daemon stops, both machines clean up cleanly.

Same five requirements as [M16](M16-linux-mvp.md). M16 ships the *integration* (client-side dispatcher, coordinator's transfer table, tray wiring, file-clipboard producers/consumers) once. M17 ships only the **`WindowsScreen` backend** that the M16 wiring already calls into. The protocol, coordinator, transfer engine, and GUI surface land unchanged.

`crates/platform/windows/src/screen.rs:21-28` is currently a stub: `try_open()` always returns `Err(PlatformError::Unavailable("scaffold; … post-M10 follow-up"))`. M17 replaces every `unsupported(...)` arm with real Win32 calls.

## Prerequisites

- [M11](M11-coordinator.md) — coordinator (done).
- [M13](M13-gui-backend.md) — GUI ↔ backend wiring (done).
- [M14](M14-tray.md) — tray scaffolding (done; on Windows the `MainThreadTray` path runs without a worker thread because eframe already pumps the Win32 message loop on the main thread).
- [M16](M16-linux-mvp.md) — client-side input dispatcher, clipboard sync, file-clipboard integration, tray wiring (done at the milestone level — these are platform-agnostic).
- A Windows 10 / 11 build & test environment (CI runner or local VM).

## Scope

**In scope:**

- `WindowsScreen` real implementation: cursor position read, screen geometry, key & mouse injection, mouse wheel, monitor enumeration.
- `WindowsClipboard` real implementation: text + `CF_HDROP` (file list).
- DPI-awareness: per-monitor v2, so cursor coordinates round-trip correctly on multi-DPI setups.
- Tray on Windows (already works through `tray-icon`'s `Shell_NotifyIcon` backend on the main thread — no extra code; this milestone verifies it on real hardware).
- Manual matrix on Windows 10 + Windows 11.
- MSI installer adjustments for the new Win32 dependencies.

**Out of scope:**

- Windows service mode (`hopd --service`). That is M10 and M17 explicitly does *not* require it — closing the GUI window leaves the embedded backend in `hop` running, which is enough for MVP. Service mode is an enterprise/pre-login feature.
- UAC elevation flow. Hop runs as a normal user; injecting into elevated windows is impossible without elevation. Document, do not implement.
- Auto-start on login. Defer to a follow-up; the registry key (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Hop`) is small but is its own UX concern.
- Code-signing the installer. Already covered by M12 packaging — confirm the new binary still passes the existing pipeline.
- Win7 / Win8 support. Win10 1809+ only (DPI APIs need it).
- Touch / pen input. Mouse + keyboard only.
- High-precision touchpad gestures. Standard `WHEEL_DELTA` units.

## Architecture

### Big decision: which Win32 layer

Three options:

| Option | Pros | Cons |
|---|---|---|
| **A. `windows` crate (Microsoft official, MIT)** — `windows = "0.58"` already in workspace deps. | First-party. Generated bindings, type-safe, complete. Already pulled in for the existing scaffold. | Crate is large; recompile cost. Some APIs feel non-idiomatic. |
| **B. `winapi` crate (legacy, MIT)** — older bindings, smaller. | Smaller compile time. | No longer recommended by Microsoft; `windows-rs` is the future. Mixed ecosystem. |
| **C. Hand-rolled FFI** — direct `extern "system"` blocks. | Smallest dependency surface. | Re-inventing what the `windows` crate already does correctly; high boilerplate. |

**Decision: Option A.** It's already on the dep tree, it's first-party, and the cost of a one-time longer compile is small for a desktop binary. The implementation imports only the namespaces it needs (`Win32::UI::Input::KeyboardAndMouse`, `Win32::UI::WindowsAndMessaging`, `Win32::System::DataExchange`, `Win32::Graphics::Gdi`) — `windows` re-exports per-namespace so feature-pruning the `Cargo.toml` keeps compile time bounded.

### Big decision: input injection API

Two shapes:

| API | Pros | Cons |
|---|---|---|
| **`SendInput`** | Standard. Synthesises mouse/keyboard at the OS level, indistinguishable from real hardware to most apps. Handles modifiers correctly. | Subject to UIPI — cannot inject into elevated processes (Task Manager, UAC dialogs, signed admin apps). |
| **Driver-level injection (Interception, ViGEm, custom WDK driver)** | Bypasses UIPI. | Requires installing a driver; out of scope for MVP. |

**Decision: `SendInput`.** UIPI gap is documented as a known limitation. Same gap applies to Synergy, Barrier, Mouse Without Borders — users expect it.

### `WindowsScreen` shape

```rust
pub struct WindowsScreen {
    info: ScreenInfo,
    /// Active monitor's HMONITOR. Re-resolved on every display-change
    /// notification (WM_DISPLAYCHANGE) — do not cache long-term.
    primary: HMONITOR,
    /// Cached scancode + virtual-key tables for keymap.
    keymap: WinKeyMap,
    /// Win32 clipboard listener — owns a hidden window that receives
    /// WM_CLIPBOARDUPDATE, fans out to a tokio mpsc.
    clipboard: WindowsClipboard,
}
```

`PlatformScreen` impl:

| Method | Win32 call |
|---|---|
| `inject_key(key, mods, down)` | `SendInput` with `KEYBDINPUT { wVk, wScan, dwFlags: KEYEVENTF_SCANCODE \| KEYEVENTF_KEYUP? }` |
| `inject_mouse_move(x, y)` | `SetCursorPos(x, y)` for absolute (when we own the cursor) + `SendInput MOUSEINPUT MOUSEEVENTF_MOVE \| ABSOLUTE` for delta accuracy |
| `inject_mouse_button(id, down)` | `SendInput MOUSEINPUT MOUSEEVENTF_LEFTDOWN/UP` etc. |
| `inject_mouse_wheel(dx, dy)` | `SendInput MOUSEINPUT MOUSEEVENTF_WHEEL` (vertical) / `MOUSEEVENTF_HWHEEL` (horizontal); `mouseData = WHEEL_DELTA * tick_count` |
| `read_cursor()` | `GetCursorPos` |
| `screen_info()` | `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)` (primary) or `EnumDisplayMonitors` for full layout |
| `set_clipboard(...)` | `OpenClipboard / EmptyClipboard / SetClipboardData(CF_UNICODETEXT \| CF_HDROP) / CloseClipboard` |
| `get_clipboard()` | mirrored |

### Clipboard listener

Win32 clipboard does not have a push API on the *content* — it has `WM_CLIPBOARDUPDATE` on a hidden window registered via `AddClipboardFormatListener`. Hop creates one such window on a dedicated message-pump thread (`std::thread::spawn`) — must not share with the main eframe loop because Win32 message dispatch wants a thread-local message queue and we already share eframe's main thread with `tray-icon`.

```text
┌──── eframe main thread (winit, tray-icon) ────┐
│                                                │
│  HopApp                                        │
│  └─ WindowsClipboard (handle: ClipboardHandle) │
│                                                │
└────────┬───────────────────────────────────────┘
         │ tokio mpsc (text/files)
         ▼
┌──── clipboard message-pump thread ────────────┐
│  hidden HWND, RegisterClass + CreateWindowExW │
│  AddClipboardFormatListener(self_hwnd)        │
│  PeekMessage / DispatchMessage loop           │
│  on WM_CLIPBOARDUPDATE: read CF_*, send mpsc  │
└────────────────────────────────────────────────┘
```

Symmetric with the M14 Linux GTK worker — different reason (Linux: GTK loop; Windows: Win32 message pump on a dedicated thread to avoid colliding with tray-icon's hidden window on the main thread). See spec §M14.

### File clipboard on Windows

`CF_HDROP` carries an `HDROP` handle backed by a `DROPFILES` struct followed by a double-NUL-terminated UTF-16 string list of absolute paths. Receiving side:

1. M16's `file_rx` finalises into `%USERPROFILE%\Downloads\Hop\<entry>\`.
2. After finalisation, write a `CF_HDROP` clipboard entry pointing at those staged paths via `OpenClipboard / SetClipboardData`. Now Ctrl-V in Explorer pastes.
3. The hash-suppression that M16 added prevents the immediate echo.

Sending side: when `WM_CLIPBOARDUPDATE` fires and `IsClipboardFormatAvailable(CF_HDROP)`, read paths, hand to `TransferSender`. M16 already wrapped this on the protocol side.

## Task details

### Task #1 — `WindowsScreen::try_open` + screen geometry

**Files:** `crates/platform/windows/src/screen.rs`, new `crates/platform/windows/src/keymap.rs`, new `crates/platform/windows/src/dpi.rs`.

- `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)` once at process startup (called from `hop_ui::run` on Windows). Without it, coordinates are scaled silently — mouse on a 4K display lands 200 px off.
- `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)` for primary; `EnumDisplayMonitors` for full layout (M11 layout already supports >1 monitor).
- `screen_info()` returns the active monitor's bounds.

### Task #2 — keyboard injection + keymap

**Files:** `crates/platform/windows/src/keymap.rs`, `crates/platform/windows/src/screen.rs`.

- Map `hop_common::KeyId` (USB HID usage codes from M1) to Win32 scancodes. Use scancodes (not VKs) so layouts work — `SendInput` with `KEYEVENTF_SCANCODE` skips the OS layout translation, and the receiving app's own translation kicks in correctly.
- Modifier handling: separate `KEYEVENTF_KEYDOWN/KEYUP` for each modifier in `mods`, *before* the main key, then release in reverse order. Mirror what `crates/platform/x11/src/keymap.rs` does.
- Extended-key flag (`KEYEVENTF_EXTENDEDKEY`) for arrows / Insert / Delete / right-Alt etc.

### Task #3 — mouse injection

**Files:** `crates/platform/windows/src/screen.rs`.

- Absolute-coordinate mode: `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE`, normalised to 0..65535 across the *virtual* desktop (`MOUSEEVENTF_VIRTUALDESK`). Calculation:
  ```
  norm_x = (x - virtual_left) * 65535 / (virtual_width - 1)
  ```
- Buttons: standard L/M/R + X1/X2.
- Wheel: `mouseData = (dy / 120) * WHEEL_DELTA`. dy already in 120ths from M1.

### Task #4 — Windows clipboard

**Files:** `crates/platform/windows/src/clipboard.rs` (new), `crates/platform/windows/src/screen.rs` (wires it in).

- Hidden message-pump thread (~80 LOC), `AddClipboardFormatListener`.
- Read text: `OpenClipboard, GetClipboardData(CF_UNICODETEXT)` → UTF-16 → UTF-8.
- Read files: `GetClipboardData(CF_HDROP)` → `DragQueryFileW`.
- Write text / files: symmetric.
- Loop suppression interaction: M16's hash registry; on Windows the `WM_CLIPBOARDUPDATE` fires *also* on our own writes, so the hash-suppression has to fire before re-broadcast.

### Task #5 — DPI fixture in the GUI

**Files:** `crates/hop-ui/src/lib.rs`.

- On Windows, before `eframe::run_native`, call `SetProcessDpiAwarenessContext`. eframe ≥ 0.27 declares it but does not call it for us.
- Verify in the manual matrix: 100% display + 200% display side-by-side; cursor crossing must land in the right pixel.

### Task #6 — Manual cross-OS matrix row

**Files:** `specs/milestones/M17-windows-mvp.md` (this file) — not a code task; tracking only.

| Scenario | Pass / Fail |
|---|---|
| Win10 → Win10, same network, cursor crosses, types | |
| Win10 → Win11, mixed | |
| Win11 100% DPI ↔ Win11 175% DPI | |
| Text Ctrl-C/V both directions | |
| File copy/paste 1 file, 100 files, 1 large file (1 GiB) | |
| Close window → tray; tray → Show window | |
| Tray Quit | |
| Reboot persistence (no auto-start expected; manual relaunch) | |

## Implementation order

Four commits, each leaves the tree green.

### Commit 1 — "feat(platform/windows): real screen geometry + DPI" (~1 day)

Tasks #1 + #5.

End: `cargo build -p hop-platform-windows` on a Windows host; `WindowsScreen::try_open()` returns `Ok(_)` and the cursor position is plausible.

### Commit 2 — "feat(platform/windows): keyboard injection" (~2 days)

Task #2.

End: a small CLI tool (`xtask windows-key-smoke`) types "hello" into Notepad after a 3 s delay; manual matrix.

### Commit 3 — "feat(platform/windows): mouse injection + wheel" (~2 days)

Task #3.

End: same tool moves and clicks; manual matrix.

### Commit 4 — "feat(platform/windows): clipboard text + CF_HDROP" (~2 days)

Task #4.

End: Win-Win round-trip text and file copy/paste manually; M16's clipboard hash suppression keeps the loop quiet.

## Tests

| Level | What | How |
|---|---|---|
| Unit | `KeyId` → scancode mapping is total over the modifier matrix | parametrised table in `keymap.rs` |
| Unit | `dy` scrolling math — 120 → `WHEEL_DELTA`, 60 → ½ | direct table |
| Integration | `WindowsScreen::try_open()` succeeds in a normal user session | guard with `#[cfg(target_os = "windows")] #[test]` and run only on Windows CI |
| Manual | Two Win10/11 boxes, cursor crossing, type, clipboard text, clipboard files, close-to-tray, Quit | covered by human |

CI: a Windows runner (GitHub `windows-latest`) builds the crate and runs the unit tests. Integration tests requiring a real session run only locally.

## Edge cases

- **UIPI / elevated processes.** `SendInput` from a non-elevated Hop cannot inject into Task Manager, UAC dialogs, or signed admin tools. Document; do not work around for MVP.
- **Mixed-DPI multi-monitor.** `SetCursorPos` uses physical coordinates after per-monitor v2 is enabled. The M11 layout already handles screen rectangles; verify that the conversion to absolute coordinates uses *virtual desktop* width.
- **Lock screen / Welcome screen.** Background user session, no input target. Hop will keep retrying connect; `SendInput` is a no-op in those sessions. Document.
- **Antivirus heuristics.** Some AVs flag `SendInput` apps. The MSI is code-signed (M12); reduces false positives but does not eliminate. Add a section to README about AV exceptions.
- **Clipboard lock contention.** Other apps (clipboard managers like Ditto) hold `OpenClipboard` for milliseconds. Retry with exponential backoff (50 ms → 200 ms → 800 ms) and surface a debug log if more than two retries needed; do not stall the UI.
- **Long file paths.** Win32 has a 260-char `MAX_PATH` unless the manifest opts in. Add `<longPathAware>true</longPathAware>` to `bins/hop/Hop.exe.manifest`.
- **Non-ASCII filenames.** `CF_HDROP` is UTF-16; safe by default, but the staging dir must be UTF-16-aware. `std::fs` already handles this on stable.
- **Power events / sleep.** When the server suspends mid-session, the TCP connection drops on resume. Already handled by the existing keep-alive + reconnect path.
- **Two GUI instances.** Same as Linux; per-user lockfile. Use `CreateMutexW(L"Hop-{user-sid}")` instead of an XDG file. Defer to follow-up.

## Estimate

- Implementation: ~7 working days
- Manual matrix: ~1 day on real hardware
- Buffer for AV / installer / signing surprises: ~1 day

**Total: ~9 working days** (≈ 2 weeks calendar) on a Windows-equipped engineer.

## Risks

- **Per-monitor DPI v2 quirks.** eframe's window already declares DPI awareness at the manifest level on its own builds; we must verify our manifest is the same. Cursor crossing on a mixed-DPI desktop is the single most likely place to find a regression.
- **`SendInput` rate limit.** Windows queues input; rapid bursts can be coalesced. Hop's typical event rate (mouse motion at 60 Hz, keys at typing speed) is well below the threshold, but high-DPI gaming-mouse polling could exceed. Cap to 240 events / sec server-side as a defensive measure.
- **CF_HDROP and modern apps.** Some sandboxed UWP apps reject `CF_HDROP`. The MVP target is Win32 desktop apps (Explorer, Notepad++, IDEs). Document the limitation; UWP support is a follow-up.
- **`tray-icon` on Windows.** Already pulled in; confirmed Windows-supported by upstream. The M14 main-thread topology works as-is; this milestone validates on real hardware.
- **No Windows CI runner today.** Workspace currently builds only on Linux in CI. Adding a `windows-latest` job is a small infra change but a hard prerequisite for landing M17 commits.

## Resolved / deferred decisions

1. **Win32 binding crate.** Resolved: `windows = "0.58"` (Option A).
2. **Injection API.** Resolved: `SendInput`. Driver-level out of scope.
3. **DPI mode.** Resolved: per-monitor v2.
4. **Service mode.** Deferred to M10 (`hopd --service`). MVP runs in user session only.
5. **UAC / elevation.** Deferred. Documented as known gap.
6. **Auto-start on login.** Deferred. Small follow-up.
7. **Touch / pen / high-precision touchpad.** Deferred. Mouse + keyboard cover the MVP.

## Readiness after M17

After M17:

- Windows is a peer-class platform alongside Linux/X11. The same `hop` binary, the same wire protocol, the same coordinator.
- All five user-visible requirements pass on the manual matrix on Windows 10 and Windows 11.
- The Win32 backend lives behind the same `PlatformScreen` trait as the X11 one — porting to macOS ([M18](M18-macos-mvp.md)) follows the same pattern with `CGEvent` / `NSPasteboard`.
- M10 (Windows service mode) becomes a self-contained packaging milestone afterwards: the *behaviour* is already correct in user-session mode.
