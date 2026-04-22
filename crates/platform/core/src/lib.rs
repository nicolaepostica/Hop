//! `PlatformScreen` trait and shared types.
//!
//! Backend crates (`input-leap-platform-x11`, `-macos`, `-windows`, `-ei`)
//! each implement [`PlatformScreen`] for their target OS.
//! Implementation lands alongside M2 (`MockScreen`) and M3+ (real backends).
