//! Server coordinator — single-actor core that owns screen layout,
//! cursor, held state, clipboard-grab table, and the client registry.
//!
//! See [`specs/milestones/M11-coordinator.md`](../../../../../specs/milestones/M11-coordinator.md)
//! for the full design.
//!
//! Step A (this commit) introduces the pure leaf modules; the
//! [`Coordinator`] actor that ties them together ships in step B.

pub mod clipboard;
pub mod held;
pub mod layout;

pub use self::clipboard::{ClipboardGrabState, GrabRecord};
pub use self::held::HeldState;
pub use self::layout::{
    LayoutError, LayoutStore, ScreenEntry, ScreenLayout, ScreenName, SharedLayout,
};
