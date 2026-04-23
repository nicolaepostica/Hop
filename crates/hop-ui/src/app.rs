//! Top-level eframe app.
//!
//! Owns: the mode switcher, per-mode state, the local TLS identity
//! (loaded once on startup), the toast stack, and the QR-modal state
//! shared between views.

use std::time::Duration;

use eframe::{CreationContext, Frame};
use egui::{Context, RichText};
use egui_notify::Toasts;

use crate::identity;
use crate::theme::{self, palette};
use crate::views::{client::ClientState, server::ServerState};
use crate::widgets::{self, Qr};

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

/// Cross-view services: copy-to-clipboard, toast notifications, QR modal.
///
/// Passed as `&mut` to each view's `show` so buttons can fire side
/// effects without the view owning global state.
pub struct Shared<'a> {
    /// This machine's TLS fingerprint (sha256:hex). `None` if identity
    /// loading failed at startup; views should display a muted error.
    pub fingerprint: Option<&'a str>,
    toasts: &'a mut Toasts,
    qr_open: &'a mut Option<QrPayload>,
}

/// What the QR modal is currently showing.
#[derive(Clone)]
pub struct QrPayload {
    /// Short label above the QR (e.g. "Your fingerprint").
    pub title: String,
    /// Raw string encoded into the QR.
    pub data: String,
    /// Pre-computed QR matrix.
    pub qr: Qr,
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

    /// Open the QR modal showing `data` under the given `title`.
    pub fn show_qr(&mut self, title: impl Into<String>, data: impl Into<String>) {
        let data = data.into();
        let Some(qr) = Qr::encode(&data) else {
            let _ = self
                .toasts
                .error("Value too large for a QR code")
                .duration(Some(Duration::from_secs(3)));
            return;
        };
        *self.qr_open = Some(QrPayload {
            title: title.into(),
            data,
            qr,
        });
    }
}

/// Hop's egui application.
pub struct HopApp {
    mode: AppMode,
    server_state: ServerState,
    client_state: ClientState,
    fingerprint: Option<String>,
    toasts: Toasts,
    qr_open: Option<QrPayload>,
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
            qr_open: None,
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
                    qr_open: &mut self.qr_open,
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

        // Modal: QR display. Closed via Esc or the close button.
        if let Some(payload) = self.qr_open.clone() {
            show_qr_modal(ctx, &payload, &mut self.qr_open);
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

fn show_qr_modal(ctx: &Context, payload: &QrPayload, qr_open: &mut Option<QrPayload>) {
    let screen = ctx.screen_rect();
    // Dim backdrop.
    egui::Area::new(egui::Id::new("qr_backdrop"))
        .order(egui::Order::Background)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter();
            painter.rect_filled(screen, egui::Rounding::ZERO, egui::Color32::from_black_alpha(160));
            let response = ui.allocate_rect(screen, egui::Sense::click());
            if response.clicked() {
                *qr_open = None;
            }
        });

    // Esc closes.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *qr_open = None;
        return;
    }

    egui::Window::new(payload.title.as_str())
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(palette::SURFACE)
                .stroke(egui::Stroke::new(1.0, palette::BORDER))
                .rounding(egui::Rounding::same(16.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_min_width(340.0);

            // QR itself — fixed 280×280 for readable density on most displays.
            let (rect, _) = ui.allocate_exact_size(egui::vec2(280.0, 280.0), egui::Sense::hover());
            payload.qr.paint(ui.painter(), rect);

            ui.add_space(14.0);
            ui.add(
                egui::Label::new(
                    RichText::new(&payload.data)
                        .monospace()
                        .color(palette::MUTED),
                )
                .truncate(),
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.button("Close").clicked() {
                            *qr_open = None;
                        }
                    },
                );
            });
        });
}
