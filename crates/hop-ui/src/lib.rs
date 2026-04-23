//! Hop — desktop UI for Hop.
//!
//! A native-feeling, single-window app for configuring and running an
//! Hop server or client. Iteration 1 ships the shell: window,
//! Tokyo Night theme, Phosphor icon font, and the primary Server/Client
//! segmented toggle. Later iterations fill in the views.

#![warn(missing_docs)]

mod app;
mod identity;
mod theme;
mod views;
mod widgets;

pub use self::app::{AppMode, HopApp};
pub use self::identity::cert_dir;

use eframe::NativeOptions;

/// Entry point callable from a binary (`fn main`).
///
/// Creates the native window with our preferred defaults and runs the
/// eframe event loop until the user closes the window.
///
/// # Errors
/// Returns whatever `eframe::run_native` returns — typically a failure
/// to open the native window (no display server, GPU init failed, ...).
pub fn run() -> Result<(), eframe::Error> {
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hop")
            .with_inner_size([720.0, 640.0])
            .with_min_inner_size([560.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Hop",
        options,
        Box::new(|cc| Ok(Box::new(HopApp::new(cc)))),
    )
}
