//! `EiScreen` scaffold.
//!
//! Detects whether the process is running under Wayland and reports
//! the backend as unavailable (the real libei implementation is
//! post-M10). Concrete `inject_*` methods exist so the type is
//! already usable as a `PlatformScreen`; they all return
//! `Unavailable` for now.

use bytes::Bytes;
use futures::stream::{self, Stream};
use input_leap_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};
use input_leap_platform::{InputEvent, PlatformError, PlatformScreen, ScreenInfo};
use tracing::warn;

/// Wayland / libei platform backend (scaffold).
#[derive(Debug)]
pub struct EiScreen {
    info: ScreenInfo,
}

impl EiScreen {
    /// Try to open a libei session via the Remote Desktop portal.
    ///
    /// Always returns `Err(PlatformError::Unavailable)` until the full
    /// portal + libei integration lands — see the crate-level docs.
    /// The error message names the missing piece so a GUI can surface
    /// it to the user.
    pub fn try_open() -> Result<Self, PlatformError> {
        let wayland_display = std::env::var_os("WAYLAND_DISPLAY");
        let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");

        if wayland_display.is_none() {
            return Err(PlatformError::Unavailable(
                "WAYLAND_DISPLAY not set; not running under Wayland".into(),
            ));
        }
        if xdg_runtime_dir.is_none() {
            return Err(PlatformError::Unavailable(
                "XDG_RUNTIME_DIR not set; cannot reach the user session bus".into(),
            ));
        }

        Err(PlatformError::Unavailable(
            "libei backend is a scaffold in M6; full \
             xdg-desktop-portal + reis integration lands post-M10"
                .into(),
        ))
    }
}

impl PlatformScreen for EiScreen {
    async fn inject_key(
        &self,
        _key: KeyId,
        _mods: ModifierMask,
        _down: bool,
    ) -> Result<(), PlatformError> {
        warn!("EiScreen::inject_key called on scaffold backend");
        Err(unsupported("inject_key"))
    }

    async fn inject_mouse_button(
        &self,
        _button: ButtonId,
        _down: bool,
    ) -> Result<(), PlatformError> {
        Err(unsupported("inject_mouse_button"))
    }

    async fn inject_mouse_move(&self, _x: i32, _y: i32) -> Result<(), PlatformError> {
        Err(unsupported("inject_mouse_move"))
    }

    async fn inject_mouse_wheel(&self, _dx: i32, _dy: i32) -> Result<(), PlatformError> {
        Err(unsupported("inject_mouse_wheel"))
    }

    async fn get_clipboard(
        &self,
        _id: ClipboardId,
        _format: ClipboardFormat,
    ) -> Result<Bytes, PlatformError> {
        Err(unsupported("get_clipboard"))
    }

    async fn set_clipboard(
        &self,
        _id: ClipboardId,
        _format: ClipboardFormat,
        _data: Bytes,
    ) -> Result<(), PlatformError> {
        Err(unsupported("set_clipboard"))
    }

    fn screen_info(&self) -> ScreenInfo {
        self.info
    }

    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send + 'static {
        stream::empty()
    }
}

fn unsupported(op: &'static str) -> PlatformError {
    PlatformError::Unavailable(format!(
        "libei backend is a scaffold in M6; operation `{op}` is not implemented"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial(env)]
    #[allow(
        unsafe_code,
        reason = "single-threaded env mutation; cleaned up in the same test"
    )]
    fn try_open_errors_without_wayland_display() {
        // SAFETY: test-only, single-threaded.
        let saved = std::env::var_os("WAYLAND_DISPLAY");
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let result = EiScreen::try_open();
        if let Some(val) = saved {
            // SAFETY: restoring the previous value we snapshotted.
            unsafe {
                std::env::set_var("WAYLAND_DISPLAY", val);
            }
        }
        match result {
            Err(PlatformError::Unavailable(msg)) => {
                assert!(msg.contains("WAYLAND_DISPLAY"), "got {msg}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    #[serial_test::serial(env)]
    #[allow(
        unsafe_code,
        reason = "single-threaded env mutation; cleaned up in the same test"
    )]
    fn try_open_errors_with_wayland_display_but_scaffold_message() {
        // SAFETY: test-only, single-threaded.
        let saved_display = std::env::var_os("WAYLAND_DISPLAY");
        let saved_runtime = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            if saved_runtime.is_none() {
                std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
            }
        }
        let result = EiScreen::try_open();
        unsafe {
            match saved_display {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
            if saved_runtime.is_none() {
                std::env::remove_var("XDG_RUNTIME_DIR");
            }
        }
        match result {
            Err(PlatformError::Unavailable(msg)) => {
                assert!(msg.contains("scaffold"), "got {msg}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}
