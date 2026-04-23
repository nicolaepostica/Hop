//! `WindowsScreen` scaffold.

use bytes::Bytes;
use futures::stream;
use hop_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};
use hop_platform::{EventStream, PlatformError, PlatformScreen, ScreenInfo};

/// Windows platform backend (scaffold).
#[derive(Debug)]
pub struct WindowsScreen {
    info: ScreenInfo,
}

impl WindowsScreen {
    /// Attempt to open the local Windows session.
    ///
    /// Returns `Err(PlatformError::Unavailable)` for now — the real
    /// implementation requires Win32 `SendInput` / `SetCursorPos` and
    /// is deferred until a Windows build/test environment is
    /// available. See the crate docs.
    pub fn try_open() -> Result<Self, PlatformError> {
        Err(PlatformError::Unavailable(
            "Windows backend is a scaffold; real implementation uses \
             Win32 SendInput / clipboard APIs and is a post-M10 \
             follow-up once a Windows CI loop is available"
                .into(),
        ))
    }

    /// Construct with explicit geometry. Used by tests and by a future
    /// `WindowsScreen::open` that reads `GetSystemMetrics`.
    #[must_use]
    pub fn with_info(info: ScreenInfo) -> Self {
        Self { info }
    }
}

impl PlatformScreen for WindowsScreen {
    async fn inject_key(
        &self,
        _key: KeyId,
        _mods: ModifierMask,
        _down: bool,
    ) -> Result<(), PlatformError> {
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

    fn event_stream(&self) -> EventStream {
        EventStream::detached(stream::empty())
    }
}

fn unsupported(op: &'static str) -> PlatformError {
    PlatformError::Unavailable(format!(
        "Windows backend is a scaffold; operation `{op}` is not implemented"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_open_errors_with_scaffold_message() {
        match WindowsScreen::try_open() {
            Err(PlatformError::Unavailable(msg)) => {
                assert!(msg.contains("scaffold"), "got {msg}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn with_info_round_trips() {
        let info = ScreenInfo::stub(1920, 1080);
        let screen = WindowsScreen::with_info(info);
        assert_eq!(screen.screen_info(), info);
    }
}
