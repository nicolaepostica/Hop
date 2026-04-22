//! Keyboard modifier mask.

use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

bitflags! {
    /// Active modifier keys at the time of an input event.
    ///
    /// Wire representation is the raw bit mask as a `u32`, preserving
    /// unknown bits on round-trip (`from_bits_retain`) so forward-
    /// compatible flags added in a future protocol version do not get
    /// silently dropped by older peers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ModifierMask: u32 {
        /// Shift (either side).
        const SHIFT       = 1 << 0;
        /// Control (either side).
        const CTRL        = 1 << 1;
        /// Alt (either side).
        const ALT         = 1 << 2;
        /// Meta / Windows / Cmd.
        const META        = 1 << 3;
        /// Caps Lock is engaged.
        const CAPS_LOCK   = 1 << 4;
        /// Num Lock is engaged.
        const NUM_LOCK    = 1 << 5;
        /// Scroll Lock is engaged.
        const SCROLL_LOCK = 1 << 6;
        /// AltGr (right-Alt on many European layouts).
        const ALT_GR      = 1 << 7;
    }
}

impl Serialize for ModifierMask {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bits().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ModifierMask {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bits = u32::deserialize(deserializer)?;
        Ok(Self::from_bits_retain(bits))
    }
}
