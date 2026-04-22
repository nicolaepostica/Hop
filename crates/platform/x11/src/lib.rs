//! X11 backend for Input Leap (Linux and BSD).
//!
//! At M3 this implements the injection half of [`PlatformScreen`] via
//! the `XTest` extension. Global input capture (`event_stream`) and
//! clipboard sharing come in M4; for now those methods return empty
//! results with a `tracing::warn!` so the surrounding server/client
//! loops still work end-to-end against the real X server.

#![cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
// x11rb is synchronous; several of our PlatformScreen methods look
// `async` only because the trait is async. Suppress the lint crate-wide
// instead of decorating every method.
#![allow(clippy::unused_async)]

mod keymap;
mod screen;

pub use self::screen::X11Screen;
