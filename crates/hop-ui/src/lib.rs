//! Hop — desktop UI for Hop.
//!
//! A native-feeling, single-window app for configuring and running an
//! Hop server or client. Iteration 1 ships the shell: window,
//! Tokyo Night theme, Phosphor icon font, and the primary Server/Client
//! segmented toggle. Later iterations fill in the views.

#![warn(missing_docs)]

mod app;
mod identity;
mod runtime;
mod theme;
mod util;
mod views;
mod widgets;

pub use self::app::{AppMode, HopApp};
pub use self::identity::cert_dir;

use eframe::NativeOptions;

/// PNG for the taskbar / window icon, compiled into the binary.
/// Sourced from the repository-root `assets/` directory so every
/// packaging target (bundle, deb, msi) reads the same file.
const ICON_PNG: &[u8] = include_bytes!("../../../assets/hop.png");

/// Entry point callable from a binary (`fn main`).
///
/// Creates the native window with our preferred defaults and runs the
/// eframe event loop until the user closes the window.
///
/// # Errors
/// Returns whatever `eframe::run_native` returns — typically a failure
/// to open the native window (no display server, GPU init failed, ...).
pub fn run() -> Result<(), eframe::Error> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Hop")
        .with_inner_size([720.0, 640.0])
        .with_min_inner_size([560.0, 480.0]);

    match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(err) => tracing::warn!(error = %err, "failed to decode bundled app icon"),
    }

    let options = NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Hop",
        options,
        Box::new(|cc| Ok(Box::new(HopApp::new(cc)))),
    )
}
