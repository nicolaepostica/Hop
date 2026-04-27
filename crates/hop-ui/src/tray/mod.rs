//! System-tray surface for the Hop desktop UI.
//!
//! Public API is per-OS-uniform (`Tray::try_new`, `reconcile`, `poll`);
//! the implementation behind it is split because `tray-icon`'s native
//! backends impose different threading rules:
//!
//! - **macOS / Windows** — eframe's main thread already pumps the
//!   right native loop; tray lives on it. See [`backend_main`].
//! - **Linux** — `tray-icon` requires a live `gtk::main()` on the same
//!   thread that owns the [`tray_icon::TrayIcon`], which eframe does
//!   not provide. A dedicated worker thread runs GTK; the eframe side
//!   talks to it through `crossbeam_channel`. See [`backend_gtk`].
//!
//! See `specs/milestones/M14-tray.md §Architecture`.

mod icons;
mod menu;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod backend_main;

#[cfg(target_os = "linux")]
mod backend_gtk;

use thiserror::Error;
#[cfg(target_os = "linux")]
use tracing::warn;

use crate::AppMode;

/// Backend state reflected in the tray icon and menu header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    /// No backend is running.
    Idle,
    /// Server is listening; `peer_count` is the number of connected clients.
    ServerRunning { peer_count: usize },
    /// Client is connected to a remote server.
    ClientConnected,
}

/// Action requested by the user through the tray (icon click or menu).
#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Bring the main window to the foreground.
    ShowWindow,
    /// Switch the app's `AppMode`. Ignored by the app while a backend
    /// is running (matches the segmented-toggle lock in the window).
    SwitchMode(AppMode),
    /// Toggle backend run state — start if idle, stop if running.
    StartOrStop,
    /// Open the About dialog (also brings the window forward).
    About,
    /// Quit the whole app — stop the backend then close the viewport.
    Quit,
}

/// Failure constructing the tray. All variants are recoverable: the
/// caller should log and run without a tray, never panic.
#[derive(Debug, Error)]
pub enum TrayError {
    /// Failed to decode or build the tray icon image.
    #[error("icon load: {0}")]
    Icons(#[from] icons::IconError),
    /// `tray-icon` rejected the build (e.g. no D-Bus session,
    /// no `StatusNotifierWatcher` on Linux, `NSStatusBar` refused on macOS).
    #[error("tray-icon build: {0}")]
    Build(#[from] tray_icon::Error),
    /// `gtk::init()` failed (no display, missing dependencies). Linux only.
    #[cfg(target_os = "linux")]
    #[error("gtk::init: {0}")]
    GtkInit(String),
    /// Failed to spawn the dedicated GTK worker thread. Linux only.
    #[cfg(target_os = "linux")]
    #[error("spawn tray worker: {0}")]
    WorkerSpawn(String),
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
type Backend = backend_main::MainThreadTray;

#[cfg(target_os = "linux")]
type Backend = backend_gtk::GtkWorkerHandle;

/// System-tray handle. Live for the duration of the app; dropping it
/// removes the icon from the system tray and stops the worker (Linux).
pub struct Tray {
    backend: Backend,
}

impl Tray {
    /// Try to construct a tray. Returns `Ok(None)` for environments
    /// where the tray is fundamentally unavailable but the app should
    /// still run (e.g. headless CI, Wayland without `StatusNotifier`).
    /// Returns `Err` for unexpected failures the caller may want to
    /// surface as a warning.
    pub fn try_new() -> Result<Option<Self>, TrayError> {
        // Cheap pre-flight: on Linux, refuse early if no graphical
        // session at all. Other variants (no D-Bus tray host, etc.)
        // surface as `TrayError::Build` from the backend.
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("DISPLAY").is_none()
                && std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                warn!("no DISPLAY or WAYLAND_DISPLAY; tray disabled");
                return Ok(None);
            }
        }

        let backend = Backend::try_new()?;
        Ok(Some(Self { backend }))
    }

    /// Apply backend state to the tray. Idempotent — repeated calls
    /// with the same arguments are cheap no-ops.
    pub fn reconcile(&mut self, state: TrayState, mode_locked: bool, mode: AppMode) {
        self.backend.reconcile(state, mode_locked, mode);
    }

    /// Drain pending tray events into a typed command list. Called
    /// once per `HopApp::update` frame.
    pub fn poll(&self) -> Vec<TrayCommand> {
        self.backend.poll()
    }
}
