//! Windows backend for Hop (Win32 via `windows-rs`).
//!
//! # Status
//!
//! **Scaffold only.** Windows input injection uses `SendInput` /
//! `SetCursorPos`, screen enumeration uses `GetSystemMetrics` /
//! `EnumDisplayMonitors`, and clipboard uses the `CF_UNICODETEXT` /
//! `CF_HTML` clipboard format family through `OpenClipboard`.
//!
//! The author does not have a Windows build / test environment here.
//! Shipping untested Win32 calls would be worse than shipping a clean
//! scaffold the dispatcher can point at. CI's `windows-latest` job
//! validates that this crate compiles against the real `windows-rs`
//! bindings.
//!
//! Follow-up:
//!   - `SendInput`-based keyboard + mouse injection (keysym → VK map).
//!   - `SetCursorPos` for absolute mouse moves.
//!   - `CF_UNICODETEXT` + `CF_HTML` clipboard read/write.
//!   - Raw-input capture for the server role.

#![cfg(windows)]
#![allow(clippy::unused_async)]

mod screen;

pub use self::screen::WindowsScreen;
