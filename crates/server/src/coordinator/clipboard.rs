//! Server-side view of "who owns which clipboard right now".
//!
//! The state tracked here is intentionally small — this module only
//! records **ownership** and the monotonic sequence number that
//! stamped it, so stale `ClipboardGrab`/`ClipboardRequest` messages
//! racing in from a previous screen-cross can be discarded. The
//! actual payload movement (`ClipboardData`) does not touch this
//! structure; it flows directly from the owner back to whoever asked.

use std::collections::HashMap;

use hop_common::ClipboardId;

use crate::coordinator::layout::ScreenName;

/// One per clipboard id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabRecord {
    /// Screen that currently owns this clipboard.
    pub owner: ScreenName,
    /// Monotonic seq the grab was announced with.
    pub seq: u32,
}

/// Per-clipboard ownership table.
#[derive(Debug, Clone, Default)]
pub struct ClipboardGrabState {
    owner: HashMap<ClipboardId, GrabRecord>,
}

impl ClipboardGrabState {
    /// Current seq for a given clipboard. `0` if we've never seen a
    /// grab for it.
    #[must_use]
    pub fn current_seq(&self, id: ClipboardId) -> u32 {
        self.owner.get(&id).map_or(0, |r| r.seq)
    }

    /// Who currently owns this clipboard, if anyone.
    #[must_use]
    pub fn owner_of(&self, id: ClipboardId) -> Option<&ScreenName> {
        self.owner.get(&id).map(|r| &r.owner)
    }

    /// Apply a newly-received grab.
    ///
    /// Returns `true` when the grab was accepted (i.e. `seq` is newer
    /// than whatever we had). A `false` return means the grab was
    /// stale and callers should drop the associated message.
    pub fn on_grab(&mut self, from: ScreenName, id: ClipboardId, seq: u32) -> bool {
        if let Some(existing) = self.owner.get(&id) {
            if seq <= existing.seq {
                return false;
            }
        }
        self.owner.insert(id, GrabRecord { owner: from, seq });
        true
    }

    /// Drop every grab held by this peer. Called when that peer
    /// disconnects, so we don't keep routing `ClipboardRequest`
    /// messages toward a hung-up socket.
    pub fn drop_owner(&mut self, name: &str) {
        self.owner.retain(|_, r| r.owner != name);
    }

    /// How many clipboard slots currently have a recorded owner.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.owner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_initial_state() {
        let s = ClipboardGrabState::default();
        assert_eq!(s.current_seq(ClipboardId::Clipboard), 0);
        assert!(s.owner_of(ClipboardId::Clipboard).is_none());
        assert_eq!(s.tracked_count(), 0);
    }

    #[test]
    fn first_grab_accepted() {
        let mut s = ClipboardGrabState::default();
        assert!(s.on_grab("laptop".into(), ClipboardId::Clipboard, 1));
        assert_eq!(s.current_seq(ClipboardId::Clipboard), 1);
        assert_eq!(s.owner_of(ClipboardId::Clipboard).unwrap(), "laptop");
    }

    #[test]
    fn newer_seq_replaces_owner() {
        let mut s = ClipboardGrabState::default();
        s.on_grab("laptop".into(), ClipboardId::Clipboard, 1);
        assert!(s.on_grab("desk".into(), ClipboardId::Clipboard, 2));
        assert_eq!(s.owner_of(ClipboardId::Clipboard).unwrap(), "desk");
        assert_eq!(s.current_seq(ClipboardId::Clipboard), 2);
    }

    #[test]
    fn stale_or_equal_seq_rejected() {
        let mut s = ClipboardGrabState::default();
        s.on_grab("laptop".into(), ClipboardId::Clipboard, 5);
        // equal seq is stale — newer must strictly exceed
        assert!(!s.on_grab("desk".into(), ClipboardId::Clipboard, 5));
        // strictly-older definitely stale
        assert!(!s.on_grab("desk".into(), ClipboardId::Clipboard, 3));
        assert_eq!(s.owner_of(ClipboardId::Clipboard).unwrap(), "laptop");
    }

    #[test]
    fn primary_and_clipboard_track_independently() {
        let mut s = ClipboardGrabState::default();
        s.on_grab("laptop".into(), ClipboardId::Clipboard, 1);
        s.on_grab("desk".into(), ClipboardId::Primary, 1);
        assert_eq!(s.owner_of(ClipboardId::Clipboard).unwrap(), "laptop");
        assert_eq!(s.owner_of(ClipboardId::Primary).unwrap(), "desk");
        assert_eq!(s.tracked_count(), 2);
    }

    #[test]
    fn drop_owner_removes_only_matching_entries() {
        let mut s = ClipboardGrabState::default();
        s.on_grab("laptop".into(), ClipboardId::Clipboard, 1);
        s.on_grab("laptop".into(), ClipboardId::Primary, 2);
        s.on_grab("desk".into(), ClipboardId::Clipboard, 3);

        s.drop_owner("laptop");
        // laptop's Primary grab is gone
        assert!(s.owner_of(ClipboardId::Primary).is_none());
        // desk's newer Clipboard grab survives
        assert_eq!(s.owner_of(ClipboardId::Clipboard).unwrap(), "desk");
    }
}
