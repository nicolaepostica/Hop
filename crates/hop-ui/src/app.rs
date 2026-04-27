//! Top-level eframe app.
//!
//! Owns: the mode switcher, per-mode state, the local TLS identity
//! (loaded once on startup), the trusted-peer fingerprint DB, the
//! embedded tokio runtime / backend controller, and the toast stack
//! shared between views.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use eframe::{CreationContext, Frame};
use egui::{Context, RichText};
use egui_notify::Toasts;
use hop_client::ClientConfig;
use hop_config::default_layout_path;
use hop_net::{FingerprintDb, LoadedIdentity, PeerEntry};
use hop_protocol::Capability;
use hop_server::coordinator::{LayoutStore, SharedLayout};
use hop_server::ServerConfig;

use crate::identity;
use crate::runtime::{BackendController, StatusEvent};
use crate::theme::{self, palette};
use crate::tray::Tray;
use crate::views::{client::ClientState, server::ServerState};
use crate::widgets;

/// Which top-level mode the user has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Share this computer's input with peers.
    Server,
    /// Borrow another computer's input.
    Client,
}

impl Default for AppMode {
    fn default() -> Self {
        Self::Server
    }
}

/// Cross-view services: clipboard copy, toast notifications, trust-DB
/// edits, and access to the embedded runtime controller.
pub struct Shared<'a> {
    /// This machine's TLS fingerprint (sha256:hex). `None` if identity
    /// loading failed at startup; views should display a muted error.
    pub fingerprint: Option<&'a str>,
    identity: Option<&'a LoadedIdentity>,
    layout: Option<&'a SharedLayout>,
    toasts: &'a mut Toasts,
    fingerprint_db: &'a mut FingerprintDb,
    fingerprint_db_path: &'a std::path::Path,
    controller: &'a mut BackendController,
}

impl Shared<'_> {
    /// Copy `text` to the system clipboard and flash a toast.
    pub fn copy(&mut self, text: &str) {
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.to_string())) {
            Ok(()) => {
                let _ = self
                    .toasts
                    .success("Copied to clipboard")
                    .duration(Some(Duration::from_millis(1500)));
            }
            Err(err) => {
                let _ = self
                    .toasts
                    .error(format!("Clipboard unavailable: {err}"))
                    .duration(Some(Duration::from_secs(3)));
            }
        }
    }

    /// Read-only view of the trusted peers.
    #[must_use]
    pub fn trusted_peers(&self) -> &FingerprintDb {
        self.fingerprint_db
    }

    /// Try to add a trusted peer. Returns `Err(msg)` with a human-
    /// readable reason on validation failure (duplicate name, bad
    /// fingerprint format) or I/O failure while persisting.
    ///
    /// On success the in-memory DB is mutated, the file on disk is
    /// rewritten atomically, and a green toast is shown.
    pub fn add_peer(&mut self, name: &str, fingerprint_str: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Name cannot be empty".into());
        }
        if self.fingerprint_db.iter().any(|p| p.name == name) {
            return Err(format!("A peer named '{name}' already exists"));
        }
        let fingerprint = fingerprint_str
            .trim()
            .parse()
            .map_err(|e| format!("Invalid fingerprint: {e}"))?;

        self.fingerprint_db.add(PeerEntry {
            name: name.to_string(),
            fingerprint,
            added: Utc::now(),
        });
        self.fingerprint_db
            .save(self.fingerprint_db_path)
            .map_err(|e| format!("Saving trust DB failed: {e}"))?;

        let _ = self
            .toasts
            .success(format!("Peer '{name}' added"))
            .duration(Some(Duration::from_millis(1800)));
        Ok(())
    }

    /// Remove a trusted peer by name. Saves the DB on success, shows
    /// a toast. Silently no-ops if the name isn't found.
    pub fn remove_peer(&mut self, name: &str) {
        if !self.fingerprint_db.remove(name) {
            return;
        }
        if let Err(err) = self.fingerprint_db.save(self.fingerprint_db_path) {
            let _ = self
                .toasts
                .error(format!("Saving trust DB failed: {err}"))
                .duration(Some(Duration::from_secs(3)));
            return;
        }
        let _ = self
            .toasts
            .info(format!("Peer '{name}' removed"))
            .duration(Some(Duration::from_millis(1500)));
    }

    /// `true` while any backend (server or client) is active.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.controller.is_running()
    }

    /// Ask the active backend to stop. No-op if nothing is running.
    pub fn stop_backend(&mut self) {
        self.controller.stop();
    }

    /// Start the server with the given display name and listen address.
    ///
    /// Handles config-building (identity + trust DB + layout come from
    /// `Shared`) and surfaces failures as red toasts.
    pub fn start_server(&mut self, name: &str, listen_addr_str: &str) {
        let Some(identity) = self.identity else {
            self.toast_error("Cannot start: local TLS identity failed to load; see logs.");
            return;
        };
        let Some(layout) = self.layout else {
            self.toast_error("Cannot start: screen layout failed to load; see logs.");
            return;
        };
        let listen_addr = match listen_addr_str.trim().parse() {
            Ok(addr) => addr,
            Err(err) => {
                self.toast_error(format!("Invalid listen address: {err}"));
                return;
            }
        };
        let cfg = ServerConfig {
            listen_addr,
            display_name: name.trim().to_string(),
            identity: identity.clone(),
            trusted_peers: std::sync::Arc::new(self.fingerprint_db.clone()),
            capabilities: default_capabilities(),
            layout: SharedLayout::clone(layout),
        };
        match self.controller.start_server(cfg) {
            Ok(()) => {
                let _ = self
                    .toasts
                    .success(format!("Server started on {listen_addr}"))
                    .duration(Some(Duration::from_millis(1800)));
            }
            Err(err) => self.toast_error(format!("Start failed: {err}")),
        }
    }

    /// Start the client connection. Auto-adds the server fingerprint
    /// to the local trust DB if not already present.
    pub fn start_client(
        &mut self,
        name: &str,
        server_addr_str: &str,
        fingerprint_str: &str,
    ) {
        let Some(identity) = self.identity else {
            self.toast_error("Cannot connect: local TLS identity failed to load; see logs.");
            return;
        };
        let server_addr = match server_addr_str.trim().parse() {
            Ok(addr) => addr,
            Err(err) => {
                self.toast_error(format!("Invalid server address: {err}"));
                return;
            }
        };
        let fingerprint = match fingerprint_str.trim().parse() {
            Ok(fp) => fp,
            Err(err) => {
                self.toast_error(format!("Invalid fingerprint: {err}"));
                return;
            }
        };

        // Auto-add the server's fingerprint if it's new.
        if self.fingerprint_db.lookup(&fingerprint).is_none() {
            let entry_name = format!("server@{server_addr}");
            self.fingerprint_db.add(PeerEntry {
                name: entry_name.clone(),
                fingerprint,
                added: Utc::now(),
            });
            if let Err(err) = self.fingerprint_db.save(self.fingerprint_db_path) {
                self.toast_error(format!("Could not persist trust DB: {err}"));
                return;
            }
            let _ = self
                .toasts
                .info(format!("Trusting new server '{entry_name}'"))
                .duration(Some(Duration::from_millis(1800)));
        }

        let cfg = ClientConfig {
            server_addr,
            display_name: name.trim().to_string(),
            identity: identity.clone(),
            trusted_peers: std::sync::Arc::new(self.fingerprint_db.clone()),
            capabilities: default_capabilities(),
        };
        match self.controller.start_client(cfg) {
            Ok(()) => {
                let _ = self
                    .toasts
                    .success(format!("Connecting to {server_addr}…"))
                    .duration(Some(Duration::from_millis(1800)));
            }
            Err(err) => self.toast_error(format!("Connect failed: {err}")),
        }
    }

    fn toast_error(&mut self, msg: impl Into<String>) {
        let _ = self
            .toasts
            .error(msg.into())
            .duration(Some(Duration::from_secs(4)));
    }
}

/// Capabilities Hop advertises in the handshake. Today the CLI daemons
/// also claim an empty set — capability negotiation lands in a later
/// milestone.
fn default_capabilities() -> Vec<Capability> {
    Vec::new()
}

/// Hop's egui application.
pub struct HopApp {
    mode: AppMode,
    server_state: ServerState,
    client_state: ClientState,
    identity: Option<LoadedIdentity>,
    fingerprint: Option<String>,
    fingerprint_db: FingerprintDb,
    fingerprint_db_path: PathBuf,
    layout: Option<SharedLayout>,
    controller: BackendController,
    toasts: Toasts,
    /// System tray. `None` when the platform refused construction
    /// (no display, no D-Bus tray host, etc.) — app keeps running
    /// without it. Wiring of menu actions to backend lands in M14
    /// commit 2.
    _tray: Option<Tray>,
}

impl HopApp {
    /// Build a fresh app, install the theme, load the local identity,
    /// the trusted-peer DB, the screen layout, and spin up the embedded
    /// runtime.
    #[must_use]
    pub fn new(cc: &CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);

        let (identity, fingerprint) = match identity::load_or_create() {
            Ok(id) => {
                let fp = id.fingerprint.to_string();
                (Some(id), Some(fp))
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to load/generate TLS identity");
                (None, None)
            }
        };

        // Store fingerprints.toml alongside the cert directory for
        // consistency with the CLI daemons' default layout.
        let fingerprint_db_path = identity::cert_dir()
            .parent()
            .map_or_else(identity::cert_dir, std::path::Path::to_path_buf)
            .join("fingerprints.toml");

        let fingerprint_db = match FingerprintDb::load(&fingerprint_db_path) {
            Ok(db) => db,
            Err(err) => {
                tracing::warn!(
                    path = %fingerprint_db_path.display(),
                    error = %err,
                    "failed to load fingerprint DB, starting empty"
                );
                FingerprintDb::new()
            }
        };

        let server_state = ServerState::new();
        let layout_path =
            default_layout_path().unwrap_or_else(|| PathBuf::from("layout.toml"));
        let layout = match LayoutStore::load(layout_path, &server_state.name) {
            Ok(store) => Some(store.handle()),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "failed to load screen layout; Server Start will be disabled"
                );
                None
            }
        };

        let controller = BackendController::new()
            .expect("tokio multi-thread runtime init is infallible on desktop OSes");
        if controller.backend_is_mock() {
            tracing::warn!(
                "running with MockScreen ({}); input will not be injected",
                controller.backend_label()
            );
        }

        let tray = match Tray::try_new() {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(error = %err, "tray unavailable; running without it");
                None
            }
        };

        Self {
            mode: AppMode::default(),
            server_state,
            client_state: ClientState::default(),
            identity,
            fingerprint,
            fingerprint_db,
            fingerprint_db_path,
            layout,
            controller,
            toasts: Toasts::default(),
            _tray: tray,
        }
    }

    /// Drain backend status events and convert them into toasts /
    /// running-flag updates. Called once per frame before the views
    /// see the new state.
    fn pump_backend_events(&mut self) {
        for event in self.controller.drain_events() {
            match event {
                StatusEvent::Stopped { mode, exit: Ok(()) } => {
                    let label = match mode {
                        AppMode::Server => "Server stopped",
                        AppMode::Client => "Disconnected",
                    };
                    let _ = self
                        .toasts
                        .info(label)
                        .duration(Some(Duration::from_millis(1500)));
                    self.server_state.running = false;
                    self.client_state.connected = false;
                }
                StatusEvent::Stopped {
                    mode,
                    exit: Err(msg),
                } => {
                    let label = match mode {
                        AppMode::Server => "Server exited with error",
                        AppMode::Client => "Connection ended",
                    };
                    let _ = self
                        .toasts
                        .error(format!("{label}: {msg}"))
                        .duration(Some(Duration::from_secs(5)));
                    self.server_state.running = false;
                    self.client_state.connected = false;
                }
            }
        }
    }
}

impl eframe::App for HopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        self.pump_backend_events();
        // Keep the UI responsive while a backend is running — egui
        // otherwise only repaints on user input.
        if self.controller.is_running() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(24.0, 20.0)),
            )
            .show(ctx, |ui| {
                let locked = self.controller.is_running();
                header(ui, &mut self.mode, locked);
                ui.add_space(14.0);

                let mut shared = Shared {
                    fingerprint: self.fingerprint.as_deref(),
                    identity: self.identity.as_ref(),
                    layout: self.layout.as_ref(),
                    toasts: &mut self.toasts,
                    fingerprint_db: &mut self.fingerprint_db,
                    fingerprint_db_path: &self.fingerprint_db_path,
                    controller: &mut self.controller,
                };

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.mode {
                        AppMode::Server => {
                            crate::views::server::show(ui, &mut self.server_state, &mut shared);
                        }
                        AppMode::Client => {
                            crate::views::client::show(ui, &mut self.client_state, &mut shared);
                        }
                    });
            });

        // Modals live outside the central panel so their backdrop
        // covers the whole window. Build a fresh `Shared` — the one
        // from the panel closure is already out of scope.
        {
            let mut shared = Shared {
                fingerprint: self.fingerprint.as_deref(),
                identity: self.identity.as_ref(),
                layout: self.layout.as_ref(),
                toasts: &mut self.toasts,
                fingerprint_db: &mut self.fingerprint_db,
                fingerprint_db_path: &self.fingerprint_db_path,
                controller: &mut self.controller,
            };
            widgets::add_peer_modal::show(
                ctx,
                &mut self.server_state.add_peer_modal,
                &mut shared,
            );
        }

        // Toasts — always drawn last so they layer above everything.
        self.toasts.show(ctx);
    }
}

fn header(ui: &mut egui::Ui, mode: &mut AppMode, locked: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(egui_phosphor::regular::RABBIT)
                .size(22.0)
                .color(palette::ACCENT),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Hop").size(22.0).strong());
        ui.add_space(18.0);
        let response = ui
            .add_enabled_ui(!locked, |ui| {
                widgets::segmented(
                    ui,
                    mode,
                    &[(AppMode::Server, "Server"), (AppMode::Client, "Client")],
                )
            })
            .inner;
        if locked {
            response.on_hover_text("Stop the running backend to switch modes.");
        }
    });
}
