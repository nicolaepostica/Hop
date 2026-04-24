//! The [`BackendController`] — a thin facade that the egui thread uses
//! to start, stop, and observe a `hop_server::run` / `hop_client::run`
//! task without ever awaiting inside the UI loop.
//!
//! Lifecycle:
//! - Built once in [`HopApp::new`](crate::HopApp) alongside a dedicated
//!   multi-thread `tokio::Runtime`.
//! - [`start_server`](BackendController::start_server) /
//!   [`start_client`](BackendController::start_client) spawn the
//!   backend task and return immediately; the task drives the real
//!   event loop.
//! - [`stop`](BackendController::stop) cancels the active task's
//!   shutdown token; the `Stopped` event arrives back through
//!   [`drain_events`](BackendController::drain_events) when the task
//!   actually finishes.
//! - `Drop` cancels any running task and waits up to 2 s for graceful
//!   teardown.

use std::time::Duration;

use hop_client::ClientConfig;
use hop_server::ServerConfig;
use thiserror::Error;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::app::AppMode;

use super::platform::PlatformBackend;

/// Bounded status-event queue. A slow UI dropping events is preferable
/// to the backend blocking on send.
const STATUS_CAPACITY: usize = 256;

/// Errors surfaced back to the UI as red toasts.
#[derive(Debug, Error)]
pub enum ControllerError {
    /// Start called while a backend is already running. The UI should
    /// disable Start while `is_running()` returns true; this guards
    /// against races.
    #[error("a backend is already running")]
    AlreadyRunning,
}

/// Events the backend task emits back to the UI.
///
/// Drained once per frame via [`BackendController::drain_events`].
#[derive(Debug, Clone)]
pub enum StatusEvent {
    /// Backend task exited. `exit == Ok(())` is a clean shutdown
    /// triggered by [`BackendController::stop`]; `Err(msg)` means
    /// the backend itself failed (bind error, handshake error, etc).
    Stopped {
        /// Which mode the stopped task was running in.
        mode: AppMode,
        /// Result of the backend's event loop.
        exit: Result<(), String>,
    },
}

/// Bookkeeping for the currently-running backend.
struct Running {
    shutdown: CancellationToken,
    /// Held so the task isn't detached; we await it on `Drop`.
    task: JoinHandle<()>,
}

/// Owns the embedded runtime and the currently-running backend task.
///
/// Built once in [`HopApp::new`](crate::HopApp) and stored for the
/// app's lifetime. Cloning is deliberately not implemented — there is
/// exactly one runtime per app.
pub struct BackendController {
    runtime: Runtime,
    backend: PlatformBackend,
    active: Option<Running>,
    status_tx: mpsc::Sender<StatusEvent>,
    status_rx: mpsc::Receiver<StatusEvent>,
}

impl BackendController {
    /// Build the runtime and select a platform backend.
    ///
    /// # Errors
    /// Returns `std::io::Error` if `tokio::Runtime::new()` fails
    /// (e.g. the OS denies thread creation).
    pub fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("hop-backend")
            .build()?;
        let backend = PlatformBackend::select();
        let (status_tx, status_rx) = mpsc::channel(STATUS_CAPACITY);
        Ok(Self {
            runtime,
            backend,
            active: None,
            status_tx,
            status_rx,
        })
    }

    /// Label of the selected platform backend (for status text).
    #[must_use]
    pub fn backend_label(&self) -> &'static str {
        self.backend.label()
    }

    /// `true` if the selected backend is the [`MockScreen`](hop_platform::MockScreen)
    /// fallback — callers should surface a "input will not be injected"
    /// banner when this is `true`.
    #[must_use]
    pub fn backend_is_mock(&self) -> bool {
        self.backend.is_mock()
    }

    /// `true` between a successful `start_*` and the arrival of the
    /// corresponding [`StatusEvent::Stopped`].
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    /// Spawn the server event loop.
    ///
    /// Returns immediately. On failure the UI receives a
    /// [`StatusEvent::Stopped`] with `exit == Err(_)`.
    ///
    /// # Errors
    /// [`ControllerError::AlreadyRunning`] if a backend is active.
    pub fn start_server(&mut self, cfg: ServerConfig) -> Result<(), ControllerError> {
        if self.active.is_some() {
            return Err(ControllerError::AlreadyRunning);
        }
        let shutdown = CancellationToken::new();
        let backend = self.backend.clone();
        let task_shutdown = shutdown.clone();
        let tx = self.status_tx.clone();
        let task = self.runtime.spawn(async move {
            let exit = backend
                .run_server(cfg, task_shutdown)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(StatusEvent::Stopped {
                    mode: AppMode::Server,
                    exit,
                })
                .await;
        });
        self.active = Some(Running { shutdown, task });
        Ok(())
    }

    /// Spawn the client session.
    ///
    /// Returns immediately. On failure the UI receives a
    /// [`StatusEvent::Stopped`] with `exit == Err(_)`.
    ///
    /// # Errors
    /// [`ControllerError::AlreadyRunning`] if a backend is active.
    pub fn start_client(&mut self, cfg: ClientConfig) -> Result<(), ControllerError> {
        if self.active.is_some() {
            return Err(ControllerError::AlreadyRunning);
        }
        let shutdown = CancellationToken::new();
        let backend = self.backend.clone();
        let task_shutdown = shutdown.clone();
        let tx = self.status_tx.clone();
        let task = self.runtime.spawn(async move {
            let exit = backend
                .run_client(cfg, task_shutdown)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(StatusEvent::Stopped {
                    mode: AppMode::Client,
                    exit,
                })
                .await;
        });
        self.active = Some(Running { shutdown, task });
        Ok(())
    }

    /// Ask the active backend to shut down. No-op if nothing is running.
    ///
    /// The `Stopped` event arrives through `drain_events` once the
    /// task actually winds down.
    pub fn stop(&mut self) {
        if let Some(running) = &self.active {
            running.shutdown.cancel();
        }
    }

    /// Drain every pending status event. Call once per egui frame.
    ///
    /// A [`StatusEvent::Stopped`] automatically flips [`Self::is_running`]
    /// back to `false`; the caller still receives the event so it can
    /// show a toast / update button labels.
    pub fn drain_events(&mut self) -> Vec<StatusEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.status_rx.try_recv() {
            if matches!(ev, StatusEvent::Stopped { .. }) {
                self.active = None;
            }
            out.push(ev);
        }
        out
    }
}

impl Drop for BackendController {
    fn drop(&mut self) {
        if let Some(running) = self.active.take() {
            running.shutdown.cancel();
            // Best-effort drain — waiting forever on a runaway task
            // would hang the GUI close path.
            let _ = self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), running.task).await
            });
        }
    }
}
