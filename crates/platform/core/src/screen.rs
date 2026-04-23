//! The `PlatformScreen` trait and its supporting `ScreenInfo` type.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::stream::Stream;
use input_leap_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};
use tokio_util::sync::{CancellationToken, DropGuard};

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

/// Cancellable stream of local input events.
///
/// Wraps any `Stream<Item = InputEvent>` together with a
/// [`CancellationToken`] the backend's producer task monitors. Dropping
/// the `EventStream` cancels the token (via [`DropGuard`]), signalling
/// any background thread / task feeding the stream that it should stop
/// and release its resources — X connection, libei socket, `wait_for_*`
/// blocking calls. This gives callers an explicit handle on the
/// backend's lifecycle instead of relying on implicit "the stream was
/// dropped, the task will figure it out."
///
/// The token can also be cancelled early via [`Self::shutdown`] while
/// keeping the stream alive to drain in-flight events.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = InputEvent> + Send + 'static>>,
    /// Held for its Drop side-effect only; cancels the token when
    /// `EventStream` is dropped.
    _shutdown: DropGuard,
    /// Separate clone the caller can trigger cancellation through.
    token: CancellationToken,
}

impl EventStream {
    /// Wrap `stream` so its producer can be signalled via `shutdown`.
    ///
    /// `shutdown` is the same token the backend's producer task is
    /// watching; cancelling it (either directly or by dropping the
    /// `EventStream`) tells the producer to exit.
    pub fn new<S>(stream: S, shutdown: CancellationToken) -> Self
    where
        S: Stream<Item = InputEvent> + Send + 'static,
    {
        let token = shutdown.clone();
        Self {
            inner: Box::pin(stream),
            _shutdown: shutdown.drop_guard(),
            token,
        }
    }

    /// Convenience for streams that have no owned background work —
    /// the returned `EventStream` uses a fresh, detached token.
    pub fn detached<S>(stream: S) -> Self
    where
        S: Stream<Item = InputEvent> + Send + 'static,
    {
        Self::new(stream, CancellationToken::new())
    }

    /// Cancel the backend's producer without waiting for the stream to
    /// be dropped. Further [`Stream::poll_next`] calls will still yield
    /// whatever has already been produced until the backend closes the
    /// channel.
    pub fn shutdown(&self) {
        self.token.cancel();
    }

    /// Has the backend been asked to stop?
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream")
            .field("cancelled", &self.token.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl Stream for EventStream {
    type Item = InputEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
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
    /// Returns an [`EventStream`] — a `Stream<Item = InputEvent>` that
    /// carries an embedded shutdown signal. Dropping the `EventStream`
    /// (or calling [`EventStream::shutdown`]) tells the backend's
    /// producer task to release any held resources (X connection,
    /// libei socket, `wait_for_*` calls). Called once per server
    /// lifetime; returning an empty stream is legal for backends that
    /// cannot capture local input.
    fn event_stream(&self) -> EventStream;
}
