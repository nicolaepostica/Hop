//! In-memory [`PlatformScreen`] implementation for tests.
//!
//! `MockScreen` records every injection call and replays a scripted
//! event stream. Tests use it to drive `hop-server` and
//! `hop-client` without a real display.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::stream;
use hop_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};

use crate::error::PlatformError;
use crate::events::{InjectedEvent, InputEvent};
use crate::screen::{EventStream, PlatformScreen, ScreenInfo};

/// Configurable no-op platform backend used in tests.
///
/// Clones of a `MockScreen` share the same recording buffer; this lets
/// one reference hold the backend for the code under test while another
/// inspects the recorded calls.
#[derive(Debug, Clone)]
pub struct MockScreen {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    info: ScreenInfo,
    injected: Mutex<Vec<InjectedEvent>>,
    scripted: Mutex<Vec<InputEvent>>,
    clipboard: Mutex<Vec<(ClipboardId, ClipboardFormat, Bytes)>>,
}

impl MockScreen {
    /// Construct a new mock with the given screen geometry and an empty
    /// event script.
    #[must_use]
    pub fn new(info: ScreenInfo) -> Self {
        Self {
            inner: Arc::new(Inner {
                info,
                injected: Mutex::new(Vec::new()),
                scripted: Mutex::new(Vec::new()),
                clipboard: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Construct a mock with the default stub geometry (1920x1080).
    #[must_use]
    pub fn default_stub() -> Self {
        Self::new(ScreenInfo::stub(1920, 1080))
    }

    /// Pre-load events that [`event_stream`](PlatformScreen::event_stream)
    /// will yield, in order, the next time it is called.
    pub fn script_events(&self, events: impl IntoIterator<Item = InputEvent>) {
        let mut guard = self.inner.scripted.lock().expect("scripted mutex");
        guard.extend(events);
    }

    /// Snapshot the recorded injection calls so far.
    #[must_use]
    pub fn injected(&self) -> Vec<InjectedEvent> {
        self.inner.injected.lock().expect("injected mutex").clone()
    }

    /// Snapshot the current clipboard entries.
    #[must_use]
    pub fn clipboard_entries(&self) -> Vec<(ClipboardId, ClipboardFormat, Bytes)> {
        self.inner
            .clipboard
            .lock()
            .expect("clipboard mutex")
            .clone()
    }

    fn record(&self, event: InjectedEvent) {
        self.inner
            .injected
            .lock()
            .expect("injected mutex")
            .push(event);
    }
}

impl PlatformScreen for MockScreen {
    async fn inject_key(
        &self,
        key: KeyId,
        mods: ModifierMask,
        down: bool,
    ) -> Result<(), PlatformError> {
        self.record(InjectedEvent::Key { key, mods, down });
        Ok(())
    }

    async fn inject_mouse_button(&self, button: ButtonId, down: bool) -> Result<(), PlatformError> {
        self.record(InjectedEvent::MouseButton { button, down });
        Ok(())
    }

    async fn inject_mouse_move(&self, x: i32, y: i32) -> Result<(), PlatformError> {
        self.record(InjectedEvent::MouseMove { x, y });
        Ok(())
    }

    async fn inject_mouse_wheel(&self, dx: i32, dy: i32) -> Result<(), PlatformError> {
        self.record(InjectedEvent::MouseWheel { dx, dy });
        Ok(())
    }

    async fn get_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
    ) -> Result<Bytes, PlatformError> {
        let guard = self.inner.clipboard.lock().expect("clipboard mutex");
        let entry = guard
            .iter()
            .rev()
            .find(|(eid, efmt, _)| *eid == id && *efmt == format);
        Ok(entry.map_or_else(Bytes::new, |(_, _, data)| data.clone()))
    }

    async fn set_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
        data: Bytes,
    ) -> Result<(), PlatformError> {
        self.inner
            .clipboard
            .lock()
            .expect("clipboard mutex")
            .push((id, format, data));
        Ok(())
    }

    fn screen_info(&self) -> ScreenInfo {
        self.inner.info
    }

    fn event_stream(&self) -> EventStream {
        let taken = {
            let mut guard = self.inner.scripted.lock().expect("scripted mutex");
            std::mem::take(&mut *guard)
        };
        EventStream::detached(stream::iter(taken))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn inject_calls_are_recorded_in_order() {
        let screen = MockScreen::default_stub();
        screen
            .inject_key(KeyId::new(0x61), ModifierMask::SHIFT, true)
            .await
            .unwrap();
        screen.inject_mouse_move(100, 200).await.unwrap();
        screen
            .inject_mouse_button(ButtonId::LEFT, false)
            .await
            .unwrap();
        screen.inject_mouse_wheel(0, 120).await.unwrap();

        let recorded = screen.injected();
        assert_eq!(recorded.len(), 4);
        assert!(matches!(
            recorded[0],
            InjectedEvent::Key {
                key: KeyId(0x61),
                mods: ModifierMask::SHIFT,
                down: true
            }
        ));
        assert!(matches!(
            recorded[1],
            InjectedEvent::MouseMove { x: 100, y: 200 }
        ));
        assert!(matches!(
            recorded[2],
            InjectedEvent::MouseButton {
                button: ButtonId(1),
                down: false
            }
        ));
        assert!(matches!(
            recorded[3],
            InjectedEvent::MouseWheel { dx: 0, dy: 120 }
        ));
    }

    #[tokio::test]
    async fn clipboard_round_trip_per_format() {
        let screen = MockScreen::default_stub();
        // Empty clipboard returns empty bytes.
        let empty = screen
            .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
            .await
            .unwrap();
        assert!(empty.is_empty());

        screen
            .set_clipboard(
                ClipboardId::Clipboard,
                ClipboardFormat::Text,
                Bytes::from_static(b"hello"),
            )
            .await
            .unwrap();
        screen
            .set_clipboard(
                ClipboardId::Clipboard,
                ClipboardFormat::Html,
                Bytes::from_static(b"<b>hi</b>"),
            )
            .await
            .unwrap();

        let text = screen
            .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
            .await
            .unwrap();
        assert_eq!(text.as_ref(), b"hello");
        let html = screen
            .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Html)
            .await
            .unwrap();
        assert_eq!(html.as_ref(), b"<b>hi</b>");
    }

    #[tokio::test]
    async fn clipboard_get_returns_most_recent_set() {
        let screen = MockScreen::default_stub();
        screen
            .set_clipboard(
                ClipboardId::Clipboard,
                ClipboardFormat::Text,
                Bytes::from_static(b"v1"),
            )
            .await
            .unwrap();
        screen
            .set_clipboard(
                ClipboardId::Clipboard,
                ClipboardFormat::Text,
                Bytes::from_static(b"v2"),
            )
            .await
            .unwrap();
        let got = screen
            .get_clipboard(ClipboardId::Clipboard, ClipboardFormat::Text)
            .await
            .unwrap();
        assert_eq!(got.as_ref(), b"v2");
    }

    #[tokio::test]
    async fn event_stream_yields_scripted_events_then_empties() {
        let screen = MockScreen::default_stub();
        screen.script_events([
            InputEvent::MouseMove { x: 1, y: 2 },
            InputEvent::KeyDown {
                key: KeyId::new(0x41),
                mods: ModifierMask::empty(),
            },
        ]);

        let events: Vec<InputEvent> = screen.event_stream().collect().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], InputEvent::MouseMove { x: 1, y: 2 }));

        // A second call sees an empty stream — script is drained.
        let again: Vec<InputEvent> = screen.event_stream().collect().await;
        assert!(again.is_empty());
    }

    #[tokio::test]
    async fn clones_share_the_same_recording() {
        let a = MockScreen::default_stub();
        let b = a.clone();
        a.inject_mouse_move(7, 8).await.unwrap();
        assert_eq!(b.injected().len(), 1);
    }
}
