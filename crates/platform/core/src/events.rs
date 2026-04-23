//! Events produced by a screen ([`InputEvent`]) and events injected into
//! it ([`InjectedEvent`]).

use hop_common::{ButtonId, KeyId, ModifierMask};

/// An input event observed on the local machine.
///
/// Produced by the platform backend on the primary and consumed by the
/// server, which routes it to the active client.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Key pressed.
    KeyDown {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys active at the time of the press.
        mods: ModifierMask,
    },
    /// Key released.
    KeyUp {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys active at the time of the release.
        mods: ModifierMask,
    },
    /// Mouse button changed state.
    MouseButton {
        /// Button identifier.
        button: ButtonId,
        /// `true` on press, `false` on release.
        down: bool,
    },
    /// Absolute cursor move.
    MouseMove {
        /// X coordinate in pixels.
        x: i32,
        /// Y coordinate in pixels.
        y: i32,
    },
    /// Mouse wheel scrolled.
    MouseWheel {
        /// Horizontal scroll in protocol units.
        dx: i32,
        /// Vertical scroll in protocol units.
        dy: i32,
    },
}

/// Record of an injection call used by test fixtures.
///
/// [`MockScreen`](crate::MockScreen) stores these so tests can assert
/// that the server/client caused the expected platform calls.
#[derive(Debug, Clone, PartialEq)]
pub enum InjectedEvent {
    /// Recorded [`PlatformScreen::inject_key`](crate::PlatformScreen::inject_key) call.
    Key {
        /// Key identifier.
        key: KeyId,
        /// Modifier keys passed alongside.
        mods: ModifierMask,
        /// `true` for press, `false` for release.
        down: bool,
    },
    /// Recorded mouse-button injection.
    MouseButton {
        /// Button identifier.
        button: ButtonId,
        /// `true` for press, `false` for release.
        down: bool,
    },
    /// Recorded absolute mouse move.
    MouseMove {
        /// X coordinate in pixels.
        x: i32,
        /// Y coordinate in pixels.
        y: i32,
    },
    /// Recorded mouse-wheel scroll.
    MouseWheel {
        /// Horizontal scroll in protocol units.
        dx: i32,
        /// Vertical scroll in protocol units.
        dy: i32,
    },
}
