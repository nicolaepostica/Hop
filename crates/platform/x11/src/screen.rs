//! `PlatformScreen` implementation backed by a real X11 connection.

use std::sync::Arc;

use bytes::Bytes;
use futures::stream::{self, Stream};
use input_leap_common::{ButtonId, ClipboardFormat, ClipboardId, KeyId, ModifierMask};
use input_leap_platform::{InputEvent, PlatformError, PlatformScreen, ScreenInfo};
use tracing::{debug, warn};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::Window;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::clipboard::X11Clipboard;
use crate::keymap::KeyMap;

// X11 event type constants used with `xtest_fake_input`.
// See `Xlib.h` / `xproto.h`; reproduced here to avoid pulling in
// another extension crate just for four integer constants.
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// Mouse buttons per the core X11 protocol.
const BUTTON_WHEEL_UP: u8 = 4;
const BUTTON_WHEEL_DOWN: u8 = 5;
const BUTTON_WHEEL_LEFT: u8 = 6;
const BUTTON_WHEEL_RIGHT: u8 = 7;

/// Scroll delta per protocol "tick". Input Leap sends integer deltas
/// in 120ths (same convention as Windows `WHEEL_DELTA`); anything
/// non-zero translates to at least one button press.
const WHEEL_TICK: i32 = 120;

/// Connection + cached metadata for a local X display.
///
/// `X11Screen` is `Send + Sync` because `RustConnection` is — all
/// backend methods take `&self` and the connection handles its own
/// internal locking.
pub struct X11Screen {
    conn: Arc<RustConnection>,
    root: Window,
    info: ScreenInfo,
    keymap: KeyMap,
    clipboard: X11Clipboard,
}

impl std::fmt::Debug for X11Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Screen")
            .field("root", &self.root)
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl X11Screen {
    /// Open the display named by `$DISPLAY` (or the explicit argument
    /// if provided).
    pub fn open(display: Option<&str>) -> Result<Self, PlatformError> {
        let (conn, screen_num) = x11rb::connect(display).map_err(wrap)?;

        // Make sure XTest is available before we promise injection.
        let ext = conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(wrap)?;
        if ext.is_none() {
            return Err(PlatformError::Unavailable(
                "X server does not expose the XTEST extension".into(),
            ));
        }

        let keymap = KeyMap::load(&conn)?;

        let screen = &conn.setup().roots[screen_num];
        let info = ScreenInfo {
            width: u32::from(screen.width_in_pixels),
            height: u32::from(screen.height_in_pixels),
            cursor_x: 0,
            cursor_y: 0,
            scale_factor_pct: 100,
        };
        let root = screen.root;

        // Spawn the clipboard worker on its own connection so
        // selection round-trips never block the injection path.
        let clipboard = X11Clipboard::spawn(display)?;

        Ok(Self {
            conn: Arc::new(conn),
            root,
            info,
            keymap,
            clipboard,
        })
    }

    fn fake_input(
        &self,
        event_type: u8,
        detail: u8,
        root_x: i16,
        root_y: i16,
    ) -> Result<(), PlatformError> {
        self.conn
            .as_ref()
            .xtest_fake_input(
                event_type,
                detail,
                x11rb::CURRENT_TIME,
                self.root,
                root_x,
                root_y,
                0,
            )
            .map_err(wrap)?
            .check()
            .map_err(wrap)?;
        Ok(())
    }
}

impl PlatformScreen for X11Screen {
    async fn inject_key(
        &self,
        key: KeyId,
        _mods: ModifierMask,
        down: bool,
    ) -> Result<(), PlatformError> {
        // Modifiers are tracked by the server via separate Key events
        // for each modifier key; we do not synthesise them here.
        let Some(keycode) = self.keymap.keycode(key.get()) else {
            debug!(
                keysym = format!("0x{:x}", key.get()),
                "no keycode for keysym under the current layout; ignoring"
            );
            return Ok(());
        };
        let event = if down { KEY_PRESS } else { KEY_RELEASE };
        self.fake_input(event, keycode, 0, 0)
    }

    async fn inject_mouse_button(&self, button: ButtonId, down: bool) -> Result<(), PlatformError> {
        let event = if down { BUTTON_PRESS } else { BUTTON_RELEASE };
        self.fake_input(event, button.get(), 0, 0)
    }

    async fn inject_mouse_move(&self, x: i32, y: i32) -> Result<(), PlatformError> {
        let x = i16::try_from(x).unwrap_or(i16::MAX);
        let y = i16::try_from(y).unwrap_or(i16::MAX);
        self.fake_input(MOTION_NOTIFY, 0, x, y)
    }

    async fn inject_mouse_wheel(&self, dx: i32, dy: i32) -> Result<(), PlatformError> {
        // Convert Input Leap's signed-int wheel deltas into the
        // press/release of the appropriate wheel button. Each 120-unit
        // tick is one button click; sub-tick deltas are dropped rather
        // than accumulated, because the platform layer has no state.
        click_wheel(self, dy, BUTTON_WHEEL_DOWN, BUTTON_WHEEL_UP).await?;
        click_wheel(self, dx, BUTTON_WHEEL_RIGHT, BUTTON_WHEEL_LEFT).await?;
        Ok(())
    }

    async fn get_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
    ) -> Result<Bytes, PlatformError> {
        self.clipboard.read(id, format).await
    }

    async fn set_clipboard(
        &self,
        id: ClipboardId,
        format: ClipboardFormat,
        data: Bytes,
    ) -> Result<(), PlatformError> {
        self.clipboard.write(id, format, data).await
    }

    fn screen_info(&self) -> ScreenInfo {
        self.info
    }

    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send + 'static {
        warn!("X11 event capture not implemented yet (M4); event_stream is empty");
        stream::empty()
    }
}

async fn click_wheel(
    screen: &X11Screen,
    delta: i32,
    pos_button: u8,
    neg_button: u8,
) -> Result<(), PlatformError> {
    if delta == 0 {
        return Ok(());
    }
    let (button, ticks) = if delta > 0 {
        (pos_button, delta / WHEEL_TICK)
    } else {
        (neg_button, (-delta) / WHEEL_TICK)
    };
    for _ in 0..ticks.max(1) {
        screen.fake_input(BUTTON_PRESS, button, 0, 0)?;
        screen.fake_input(BUTTON_RELEASE, button, 0, 0)?;
    }
    Ok(())
}

fn wrap<E: std::fmt::Display>(err: E) -> PlatformError {
    PlatformError::Other(err.to_string())
}
