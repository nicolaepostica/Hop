//! Top-level eframe app.
//!
//! Owns: the mode switcher, per-mode state, the local TLS identity
//! (loaded once on startup), and the toast stack shared between views.

use std::time::Duration;

use eframe::{CreationContext, Frame};
use egui::{Context, RichText};
use egui_notify::Toasts;

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

/// Cross-view services: clipboard copy + toast notifications.
pub struct Shared<'a> {
    /// This machine's TLS fingerprint (sha256:hex). `None` if identity
    /// loading failed at startup; views should display a muted error.
    pub fingerprint: Option<&'a str>,
    toasts: &'a mut Toasts,
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
}

/// Hop's egui application.
pub struct HopApp {
    mode: AppMode,
    server_state: ServerState,
    client_state: ClientState,
    fingerprint: Option<String>,
    toasts: Toasts,
}

impl HopApp {
    /// Build a fresh app, install the theme, load the local identity.
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
        Self {
            mode: AppMode::default(),
            server_state: ServerState::default(),
            client_state: ClientState::default(),
            fingerprint,
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
