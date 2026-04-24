//! Top-level eframe app.
//!
//! Owns: the mode switcher, per-mode state, the local TLS identity
//! (loaded once on startup), the trusted-peer fingerprint DB, and the
//! toast stack shared between views.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use eframe::{CreationContext, Frame};
use egui::{Context, RichText};
use egui_notify::Toasts;
use hop_net::{FingerprintDb, PeerEntry};

use crate::identity;
use crate::theme::{self, palette};
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

/// Cross-view services: clipboard copy, toast notifications, trust-DB edits.
pub struct Shared<'a> {
    /// This machine's TLS fingerprint (sha256:hex). `None` if identity
    /// loading failed at startup; views should display a muted error.
    pub fingerprint: Option<&'a str>,
    toasts: &'a mut Toasts,
    fingerprint_db: &'a mut FingerprintDb,
    fingerprint_db_path: &'a std::path::Path,
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
}

/// Hop's egui application.
pub struct HopApp {
    mode: AppMode,
    server_state: ServerState,
    client_state: ClientState,
    fingerprint: Option<String>,
    fingerprint_db: FingerprintDb,
    fingerprint_db_path: PathBuf,
    toasts: Toasts,
}

impl HopApp {
    /// Build a fresh app, install the theme, load the local identity
    /// and the trusted-peer DB.
    #[must_use]
    pub fn new(cc: &CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);

        let fingerprint = match identity::load_or_create() {
            Ok(id) => Some(id.fingerprint.to_string()),
            Err(err) => {
                tracing::warn!(error = %err, "failed to load/generate TLS identity");
                None
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

        Self {
            mode: AppMode::default(),
            server_state: ServerState::new(),
            client_state: ClientState::default(),
            fingerprint,
            fingerprint_db,
            fingerprint_db_path,
            toasts: Toasts::default(),
        }
    }
}

impl eframe::App for HopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(palette::BG)
                    .inner_margin(egui::Margin::symmetric(24.0, 20.0)),
            )
            .show(ctx, |ui| {
                header(ui, &mut self.mode);
                ui.add_space(14.0);

                let mut shared = Shared {
                    fingerprint: self.fingerprint.as_deref(),
                    toasts: &mut self.toasts,
                    fingerprint_db: &mut self.fingerprint_db,
                    fingerprint_db_path: &self.fingerprint_db_path,
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
                toasts: &mut self.toasts,
                fingerprint_db: &mut self.fingerprint_db,
                fingerprint_db_path: &self.fingerprint_db_path,
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

fn header(ui: &mut egui::Ui, mode: &mut AppMode) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(egui_phosphor::regular::RABBIT)
                .size(22.0)
                .color(palette::ACCENT),
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Hop").size(22.0).strong());
        ui.add_space(18.0);
        let _ = widgets::segmented(
            ui,
            mode,
            &[(AppMode::Server, "Server"), (AppMode::Client, "Client")],
        );
    });
}
