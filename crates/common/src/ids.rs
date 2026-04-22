//! Strongly-typed IDs for keyboard keys and mouse buttons.
//!
//! These are thin newtype wrappers around integers. They exist so the
//! protocol and platform layers cannot accidentally mix up a key code
//! with a modifier mask or a button number.

use serde::{Deserialize, Serialize};

/// A platform-neutral key identifier.
///
/// The numeric space follows X11 keysyms for printable keys and function
/// keys, with a few Input-Leap-specific extensions above `0x10000000` for
/// keys that do not exist in X11. Platform backends map to/from native
/// key codes on ingress/egress.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct KeyId(pub u32);

impl KeyId {
    /// Constructs a `KeyId` from its raw numeric value.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the raw numeric value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for KeyId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<KeyId> for u32 {
    fn from(value: KeyId) -> Self {
        value.0
    }
}

/// A mouse button identifier.
///
/// Follows the X11 convention: `1` is left, `2` is middle, `3` is right,
/// `4`–`7` are wheel up/down/left/right (legacy), `8`+ are extra buttons.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ButtonId(pub u8);

impl ButtonId {
    /// Primary button (usually left).
    pub const LEFT: Self = Self(1);
    /// Middle button / wheel click.
    pub const MIDDLE: Self = Self(2);
    /// Secondary button (usually right).
    pub const RIGHT: Self = Self(3);

    /// Constructs a `ButtonId` from its raw numeric value.
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self(raw)
    }

    /// Returns the raw numeric value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl From<u8> for ButtonId {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl From<ButtonId> for u8 {
    fn from(value: ButtonId) -> Self {
        value.0
    }
}
