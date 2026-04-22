//! `PlatformScreen` trait plus a reusable `MockScreen` for tests.
//!
//! Backend crates (`input-leap-platform-x11`, `-macos`, `-windows`,
//! `-ei`) each implement [`PlatformScreen`] for their target OS.
//! Real backends land in M3+; [`MockScreen`] covers M2 so server and
//! client can be tested end-to-end without a display.

pub mod error;
pub mod events;
pub mod mock;
mod screen;

pub use self::error::PlatformError;
pub use self::events::{InjectedEvent, InputEvent};
pub use self::mock::MockScreen;
pub use self::screen::{PlatformScreen, ScreenInfo};
