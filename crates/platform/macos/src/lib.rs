//! macOS backend for Input Leap (Core Graphics / `IOKit`).
//!
//! # Status
//!
//! **Scaffold only.** macOS input injection goes through Quartz
//! `CGEvent` APIs (and requires Accessibility permission). Screen
//! enumeration goes through `CGDisplay`. Clipboard uses `NSPasteboard`
//! via `objc2`.
//!
//! (`CGEvent` is Apple's Quartz event type.)
//!
//! The author does not have a macOS build/test environment, so rather
//! than ship code that might fail to compile against Apple's SDK this
//! crate publishes a typed [`MacOsScreen`] scaffold that the
//! dispatcher in `input-leaps` / `input-leapc` wires up, and returns
//! descriptive [`PlatformError::Unavailable`] messages naming what is
//! missing. CI's `macos-latest` job verifies the scaffold compiles.
//!
//! Follow-up work:
//!   - Replace the scaffold with real `CGEvent` injection
//!     (keyboard, mouse move/button, scroll wheel).
//!   - Clipboard via `NSPasteboard` (text + HTML + file-list).
//!   - Event capture via `CGEventTap` (server role).

#![cfg(target_os = "macos")]
#![allow(clippy::unused_async)]

mod screen;

pub use self::screen::MacOsScreen;
