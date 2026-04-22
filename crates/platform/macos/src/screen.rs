//! `MacOsScreen` scaffold.
//!
//! All async trait methods return `Unavailable` for now. Screen
//! geometry is a fixed stub; the first real implementation replaces
//! it with a call to `CGDisplayBounds(CGMainDisplayID())`.

use bytes::Bytes;
use futures::stream::{self, Stream};
use input_leap_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};
use input_leap_platform::{InputEvent, PlatformError, PlatformScreen, ScreenInfo};

/// macOS platform backend (scaffold).
#[derive(Debug)]
pub struct MacOsScreen {
    info: ScreenInfo,
}

impl MacOsScreen {
    /// Attempt to open the local macOS session.
    ///
    /// Returns `Err(PlatformError::Unavailable)` on all platforms /
    /// sessions for now — see the crate docs.
    pub fn try_open() -> Result<Self, PlatformError> {
        Err(PlatformError::Unavailable(
            "macOS CGEvent backend is a scaffold; real implementation \
             requires Accessibility permission and CGEvent APIs \
             (post-M10 follow-up)"
                .into(),
        ))
    }

    /// Construct with explicit geometry. Used by tests and by a future
    /// `MacOsScreen::open` that reads `CGDisplayBounds`.
    #[must_use]
    pub fn with_info(info: ScreenInfo) -> Self {
        Self { info }
    }
}

impl PlatformScreen for MacOsScreen {
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

    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send + 'static {
        stream::empty()
    }
}

fn unsupported(op: &'static str) -> PlatformError {
    PlatformError::Unavailable(format!(
        "macOS backend is a scaffold; operation `{op}` is not implemented"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_open_errors_with_scaffold_message() {
        match MacOsScreen::try_open() {
            Err(PlatformError::Unavailable(msg)) => {
                assert!(msg.contains("scaffold"), "got {msg}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn with_info_round_trips() {
        let info = ScreenInfo::stub(2560, 1440);
        let screen = MacOsScreen::with_info(info);
        assert_eq!(screen.screen_info(), info);
    }
}
