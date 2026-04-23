//! Wayland/libei backend for Hop.
//!
//! # Status
//!
//! **Scaffold only.** Real libei event emission requires:
//!   - a Remote Desktop portal session opened via D-Bus
//!     (`org.freedesktop.portal.RemoteDesktop`),
//!   - negotiating a `connect_to_eis` fd from the portal,
//!   - driving the libei event loop to emit key and pointer events.
//!
//! The `reis` 0.x API is still in flux and the portal dance is
//! multi-stage. Rather than ship a half-working implementation, this
//! crate currently detects whether the runtime environment *could*
//! speak Wayland/libei and returns a descriptive
//! [`PlatformError::Unavailable`] otherwise. A working implementation
//! is tracked as post-M10 work.
//!
//! # Usage
//!
//! Callers should try [`EiScreen::try_open`] first. On success, use the
//! returned [`EiScreen`] like any other [`PlatformScreen`]. On failure
//! (returned as `PlatformError::Unavailable`) fall back to the X11
//! backend.

#![cfg(target_os = "linux")]
// The stub makes most of the trait methods trivially async — suppress
// the pedantic lint at module scope.
#![allow(clippy::unused_async)]

mod screen;

pub use self::screen::EiScreen;
