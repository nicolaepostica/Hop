//! Held-key / held-button / modifier tracking across screen crossings.
//!
//! Tracks what the user currently has pressed on the active screen so
//! the coordinator can, on crossing:
//!
//! - emit `KeyUp` / `MouseButton { down: false }` to release everything
//!   cleanly on the **old** active screen (no stuck keys);
//! - re-emit modifier key-downs on the **new** active screen so that
//!   e.g. typing `Shift+A` while the pointer just crossed the border
//!   still produces a capital `A`.
//!
//! Non-modifier keys and mouse buttons are deliberately **not**
//! re-pressed on the new side — doing so would trigger auto-repeat /
//! start a new drag mid-gesture, which is almost never what the user
//! means. They simply get released on the old side and forgotten.

use std::collections::BTreeSet;

use hop_common::{ButtonId, KeyId, ModifierMask};
use hop_platform::InputEvent;
use hop_protocol::Message;

/// Tracked input state on the active screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldState {
    /// Non-modifier keysyms currently pressed.
    keys: BTreeSet<KeyId>,
    /// Mouse buttons currently pressed.
    buttons: BTreeSet<ButtonId>,
    /// Modifier mask after the last observed key event.
    mods: ModifierMask,
}

impl HeldState {
    /// Apply one observed [`InputEvent`] to the held state.
    pub fn apply(&mut self, event: &InputEvent) {
        match event {
            InputEvent::KeyDown { key, mods } => {
                if !is_modifier_keysym(key.get()) {
                    self.keys.insert(*key);
                }
                self.mods = *mods;
            }
            InputEvent::KeyUp { key, mods } => {
                self.keys.remove(key);
                self.mods = *mods;
            }
            InputEvent::MouseButton { button, down: true } => {
                self.buttons.insert(*button);
            }
            InputEvent::MouseButton {
                button,
                down: false,
            } => {
                self.buttons.remove(button);
            }
            InputEvent::MouseMove { .. } | InputEvent::MouseWheel { .. } => {}
        }
    }

    /// Messages needed to "unstick" everything on the old active
    /// screen before we leave it.
    ///
    /// Order: non-modifier keys up, buttons up, then modifiers up.
    /// Modifiers are released last so platform layers that synthesize
    /// events do not see a spurious `Shift+<letter>` on the old side
    /// right before the crossing.
    #[must_use]
    pub fn leave_messages(&self) -> Vec<Message> {
        let mut out = Vec::with_capacity(self.keys.len() + self.buttons.len() + 4);
        for &key in &self.keys {
            out.push(Message::KeyUp {
                key,
                mods: ModifierMask::empty(),
            });
        }
        for &button in &self.buttons {
            out.push(Message::MouseButton {
                button,
                down: false,
            });
        }
        for flag in modifier_flags() {
            if self.mods.contains(flag) {
                if let Some(key) = modifier_to_key(flag) {
                    out.push(Message::KeyUp {
                        key,
                        mods: ModifierMask::empty(),
                    });
                }
            }
        }
        out
    }

    /// Messages needed to restore held **modifiers** on the new active
    /// screen. Non-modifier keys and mouse buttons are intentionally
    /// not re-pressed — see module docs.
    #[must_use]
    pub fn enter_messages(&self) -> Vec<Message> {
        let mut out = Vec::with_capacity(4);
        for flag in modifier_flags() {
            if self.mods.contains(flag) {
                if let Some(key) = modifier_to_key(flag) {
                    out.push(Message::KeyDown {
                        key,
                        mods: self.mods,
                    });
                }
            }
        }
        out
    }

    /// Is at least one mouse button held down right now?
    ///
    /// The coordinator uses this to block screen crossings mid-drag:
    /// while a button is held, the cursor clamps to the current screen.
    #[must_use]
    pub fn any_button_held(&self) -> bool {
        !self.buttons.is_empty()
    }

    /// Current modifier mask (post-last-event).
    #[must_use]
    pub fn mods(&self) -> ModifierMask {
        self.mods
    }
}

/// Fixed-order iteration of modifier flags. Order matters for
/// deterministic test output.
const fn modifier_flags() -> [ModifierMask; 8] {
    [
        ModifierMask::SHIFT,
        ModifierMask::CTRL,
        ModifierMask::ALT,
        ModifierMask::META,
        ModifierMask::CAPS_LOCK,
        ModifierMask::NUM_LOCK,
        ModifierMask::SCROLL_LOCK,
        ModifierMask::ALT_GR,
    ]
}

/// Is this keysym a modifier key? We need to know so `apply` doesn't
/// double-track modifiers (they live in `self.mods`, not `self.keys`).
fn is_modifier_keysym(keysym: u32) -> bool {
    matches!(
        keysym,
        0xffe1 | 0xffe2 |   // Shift_L / Shift_R
        0xffe3 | 0xffe4 |   // Control_L / Control_R
        0xffe9 | 0xffea |   // Alt_L / Alt_R
        0xffe7 | 0xffe8 |   // Meta_L / Meta_R
        0xffeb | 0xffec |   // Super_L / Super_R
        0xffe5 |            // Caps_Lock
        0xff7f |            // Num_Lock
        0xff14 |            // Scroll_Lock
        0xfe03              // ISO_Level3_Shift (AltGr)
    )
}

/// Map a single modifier flag to the X11 keysym for its left-side
/// counterpart. We always re-press the left key — the OS sees a
/// modifier-down regardless of which side was originally pressed.
fn modifier_to_key(flag: ModifierMask) -> Option<KeyId> {
    let sym = match flag {
        ModifierMask::SHIFT => 0xffe1,       // Shift_L
        ModifierMask::CTRL => 0xffe3,        // Control_L
        ModifierMask::ALT => 0xffe9,         // Alt_L
        ModifierMask::META => 0xffe7,        // Meta_L (Cmd on macOS, Win on Windows)
        ModifierMask::CAPS_LOCK => 0xffe5,   // Caps_Lock
        ModifierMask::NUM_LOCK => 0xff7f,    // Num_Lock
        ModifierMask::SCROLL_LOCK => 0xff14, // Scroll_Lock
        ModifierMask::ALT_GR => 0xfe03,      // ISO_Level3_Shift
        _ => return None,
    };
    Some(KeyId::new(sym))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT_L: u32 = 0xffe1;
    const CTRL_L: u32 = 0xffe3;
    const KEY_A: u32 = 0x61;

    fn shift_press() -> InputEvent {
        InputEvent::KeyDown {
            key: KeyId::new(SHIFT_L),
            mods: ModifierMask::SHIFT,
        }
    }

    fn shift_release() -> InputEvent {
        InputEvent::KeyUp {
            key: KeyId::new(SHIFT_L),
            mods: ModifierMask::empty(),
        }
    }

    fn a_press() -> InputEvent {
        InputEvent::KeyDown {
            key: KeyId::new(KEY_A),
            mods: ModifierMask::empty(),
        }
    }

    #[test]
    fn initial_state_is_empty() {
        let h = HeldState::default();
        assert_eq!(h.mods(), ModifierMask::empty());
        assert!(!h.any_button_held());
        assert!(h.leave_messages().is_empty());
        assert!(h.enter_messages().is_empty());
    }

    #[test]
    fn non_modifier_key_is_tracked_in_key_set() {
        let mut h = HeldState::default();
        h.apply(&a_press());
        let leave = h.leave_messages();
        assert_eq!(leave.len(), 1);
        let Message::KeyUp { key, mods } = leave[0] else {
            panic!("expected KeyUp, got {:?}", leave[0]);
        };
        assert_eq!(key, KeyId::new(KEY_A));
        assert_eq!(mods, ModifierMask::empty());
    }

    #[test]
    fn modifier_key_is_not_duplicated_in_key_set() {
        // Shift press updates mods but must not also land in `keys`.
        // Otherwise leave_messages would emit KeyUp for the shift
        // keysym twice.
        let mut h = HeldState::default();
        h.apply(&shift_press());
        let leave = h.leave_messages();
        assert_eq!(leave.len(), 1, "exactly one shift-up: {leave:?}");
        assert!(matches!(
            leave[0],
            Message::KeyUp {
                key: KeyId(SHIFT_L),
                ..
            }
        ));
    }

    #[test]
    fn shift_held_replays_on_enter() {
        let mut h = HeldState::default();
        h.apply(&shift_press());
        let enter = h.enter_messages();
        assert_eq!(enter.len(), 1);
        let Message::KeyDown { key, mods } = enter[0] else {
            panic!("expected KeyDown, got {:?}", enter[0]);
        };
        assert_eq!(key, KeyId::new(SHIFT_L));
        assert_eq!(mods, ModifierMask::SHIFT);
    }

    #[test]
    fn letter_held_is_released_on_leave_not_replayed_on_enter() {
        let mut h = HeldState::default();
        h.apply(&a_press());
        assert_eq!(h.leave_messages().len(), 1, "A released on leave");
        assert!(
            h.enter_messages().is_empty(),
            "A is NOT re-pressed on enter"
        );
    }

    #[test]
    fn shift_release_clears_mask() {
        let mut h = HeldState::default();
        h.apply(&shift_press());
        h.apply(&shift_release());
        assert_eq!(h.mods(), ModifierMask::empty());
        assert!(h.leave_messages().is_empty());
        assert!(h.enter_messages().is_empty());
    }

    #[test]
    fn button_held_reports_true() {
        let mut h = HeldState::default();
        assert!(!h.any_button_held());
        h.apply(&InputEvent::MouseButton {
            button: ButtonId::LEFT,
            down: true,
        });
        assert!(h.any_button_held());
        h.apply(&InputEvent::MouseButton {
            button: ButtonId::LEFT,
            down: false,
        });
        assert!(!h.any_button_held());
    }

    #[test]
    fn leave_releases_buttons_and_keys() {
        let mut h = HeldState::default();
        h.apply(&shift_press());
        // Pressing A *while Shift is held* — platform reports the
        // correct mask in the event's mods field.
        h.apply(&InputEvent::KeyDown {
            key: KeyId::new(KEY_A),
            mods: ModifierMask::SHIFT,
        });
        h.apply(&InputEvent::MouseButton {
            button: ButtonId::LEFT,
            down: true,
        });

        let leave = h.leave_messages();
        let keys_up: Vec<_> = leave
            .iter()
            .filter(|m| matches!(m, Message::KeyUp { .. }))
            .collect();
        let buttons_up: Vec<_> = leave
            .iter()
            .filter(|m| matches!(m, Message::MouseButton { down: false, .. }))
            .collect();
        assert_eq!(keys_up.len(), 2, "A + Shift: {leave:?}");
        assert_eq!(buttons_up.len(), 1);

        // Last entry should be a modifier release — preserves the
        // "modifiers released last" contract.
        assert!(matches!(
            leave.last().unwrap(),
            Message::KeyUp {
                key: KeyId(SHIFT_L),
                ..
            }
        ));
    }

    #[test]
    fn mouse_move_does_not_affect_held_state() {
        let mut h = HeldState::default();
        h.apply(&InputEvent::MouseMove { x: 100, y: 200 });
        h.apply(&InputEvent::MouseWheel { dx: 0, dy: 120 });
        assert_eq!(h, HeldState::default());
    }

    #[test]
    fn multiple_modifiers_replay_in_order() {
        let mut h = HeldState::default();
        h.apply(&InputEvent::KeyDown {
            key: KeyId::new(SHIFT_L),
            mods: ModifierMask::SHIFT,
        });
        h.apply(&InputEvent::KeyDown {
            key: KeyId::new(CTRL_L),
            mods: ModifierMask::SHIFT | ModifierMask::CTRL,
        });
        let enter = h.enter_messages();
        assert_eq!(enter.len(), 2);
        assert!(matches!(
            enter[0],
            Message::KeyDown {
                key: KeyId(SHIFT_L),
                ..
            }
        ));
        assert!(matches!(
            enter[1],
            Message::KeyDown {
                key: KeyId(CTRL_L),
                ..
            }
        ));
    }
}
