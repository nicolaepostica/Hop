//! The `PlatformScreen` trait and its supporting `ScreenInfo` type.

use std::future::Future;

use bytes::Bytes;
use futures::stream::Stream;
use input_leap_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};

use crate::error::PlatformError;
use crate::events::InputEvent;

/// Geometry and metadata describing the screen exposed by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Current cursor X at the time this snapshot was taken.
    pub cursor_x: i32,
    /// Current cursor Y at the time this snapshot was taken.
    pub cursor_y: i32,
    /// Integer DPI scale factor times 100 (100 = 1.0x, 150 = 1.5x).
    pub scale_factor_pct: u16,
}

impl ScreenInfo {
    /// Construct a "default desktop" `ScreenInfo`, useful for stub backends
    /// and tests.
    #[must_use]
    pub const fn stub(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            scale_factor_pct: 100,
        }
    }
}

/// Backend-neutral interface to a local screen.
///
/// Servers consume [`PlatformScreen::event_stream`] to observe local
/// input; clients call the `inject_*` methods to drive the local screen
/// from events received over the network. Implementations live in
/// `platform/{x11,macos,windows,ei}`; [`MockScreen`](crate::MockScreen)
/// provides an in-memory implementation for tests.
///
/// All async methods return `impl Future + Send` so callers from any
/// `tokio` task can await them without indirection.
pub trait PlatformScreen: Send + Sync + 'static {
    /// Inject a key press or release.
    fn inject_key(
        &self,
        key: KeyId,
        mods: ModifierMask,
        down: bool,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Inject a mouse button press or release.
    fn inject_mouse_button(
        &self,
        button: ButtonId,
        down: bool,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Move the cursor to an absolute coordinate.
    fn inject_mouse_move(
        &self,
        x: i32,
        y: i32,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Emit a mouse-wheel scroll event.
    fn inject_mouse_wheel(
        &self,
        dx: i32,
        dy: i32,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Read the current clipboard contents for a given format.
    fn get_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
    ) -> impl Future<Output = Result<Bytes, PlatformError>> + Send;

    /// Replace the clipboard contents for a given format.
    fn set_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
        data: Bytes,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Snapshot the screen geometry.
    fn screen_info(&self) -> ScreenInfo;

    /// Produce a stream of locally-observed input events.
    ///
    /// Called once per server lifetime; returning an empty stream is
    /// legal (backends that cannot capture, or `MockScreen` in tests
    /// that do not care about incoming events).
    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send + 'static;
}
