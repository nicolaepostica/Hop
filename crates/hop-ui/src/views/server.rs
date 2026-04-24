//! Server view — first sketch.
//!
//! Iteration 1 renders a static layout so we can feel the typography,
//! card spacing, and segmented-toggle motion before wiring real state
//! in iteration 4.

use egui::{RichText, Ui};

use crate::app::Shared;
use crate::theme::palette;
use crate::widgets;

/// Persistent state backing the Server view.
#[derive(Debug, Clone)]
pub struct ServerState {
    /// Human-readable screen name advertised in `Hello`.
    pub name: String,
    /// Listen address (string form, validated on save in later iterations).
    pub listen_addr: String,
    /// Whether the server is currently "running". No runtime hooked up yet.
    pub running: bool,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            name: hostname_fallback(),
            listen_addr: "0.0.0.0:25900".into(),
            running: false,
        }
    }
}

/// Draw the Server view into `ui`.
#[allow(clippy::too_many_lines, reason = "declarative UI code, easier to read inline")]
pub fn show(ui: &mut Ui, state: &mut ServerState, shared: &mut Shared<'_>) {
    ui.vertical(|ui| {
        ui.add_space(4.0);

        // ── Status card (Start/Stop action lives here, top-right) ─────
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot_color, status_text) = if state.running {
                    (palette::SUCCESS, "Running")
                } else {
                    (palette::MUTED, "Stopped")
                };
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, dot_color);
                ui.label(RichText::new(status_text).strong());
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Share this computer's keyboard and mouse with peers.")
                        .color(palette::MUTED),
                );

                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let (label, icon, fill) = if state.running {
                            (
                                "Stop",
                                egui_phosphor::regular::STOP_CIRCLE,
                                palette::SURFACE_HOVER,
                            )
                        } else {
                            (
                                "Start",
                                egui_phosphor::regular::PLAY_CIRCLE,
                                palette::ACCENT,
                            )
                        };
                        let btn = egui::Button::new(
                            RichText::new(format!("{icon}  {label}")).color(egui::Color32::WHITE),
                        )
                        .min_size(egui::vec2(100.0, 32.0))
                        .fill(fill);
                        if ui.add(btn).clicked() {
                            state.running = !state.running;
                        }
                    },
                );
            });
        });

        ui.add_space(10.0);

        // ── Identity card ──────────────────────────────────────────────
        widgets::card(ui, |ui| {
            ui.heading("This computer");
            ui.add_space(8.0);

            egui::Grid::new("server_identity")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .min_col_width(120.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.name)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Listening on").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.listen_addr)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                });
        });

        ui.add_space(10.0);

        // ── Fingerprint card ───────────────────────────────────────────
        widgets::card(ui, |ui| {
            ui.heading("Your fingerprint");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Give this to clients so they can trust this computer.")
                    .color(palette::MUTED),
            );
            ui.add_space(8.0);

            let fp_display = shared
                .fingerprint
                .unwrap_or("<could not load identity — see logs>");
            ui.add(
                egui::Label::new(
                    RichText::new(fp_display)
                        .monospace()
                        .color(palette::TEXT),
                )
                .truncate(),
            );

            ui.add_space(8.0);
            if ui
                .button(format!("{} Copy", egui_phosphor::regular::COPY))
                .clicked()
            {
                if let Some(fp) = shared.fingerprint {
                    shared.copy(fp);
                }
            }
        });

        ui.add_space(10.0);

        // ── Peers card (placeholder) ───────────────────────────────────
        widgets::card(ui, |ui| {
            ui.heading("Connected peers");
            ui.add_space(6.0);
            ui.label(
                RichText::new("No clients connected yet.")
                    .color(palette::MUTED)
                    .italics(),
            );
        });

    });
}

fn hostname_fallback() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "this-computer".into())
}
