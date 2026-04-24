//! Client view — first sketch. Real connect/disconnect wiring lands
//! in iteration 5; here we render the form + action buttons so the
//! shell can be evaluated end-to-end.

use egui::{RichText, Ui};

use crate::app::Shared;
use crate::theme::palette;
use crate::widgets;

/// Persistent state backing the Client view.
#[derive(Debug, Clone)]
pub struct ClientState {
    /// Name we advertise to the server.
    pub name: String,
    /// `host:port` of the server we want to reach.
    pub server_addr: String,
    /// Peer's fingerprint (pasted or scanned).
    pub server_fingerprint: String,
    /// Whether we're currently connected. Stubbed in iteration 1.
    pub connected: bool,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            name: hostname_fallback(),
            server_addr: "192.168.1.10:25900".into(),
            server_fingerprint: String::new(),
            connected: false,
        }
    }
}

/// Draw the Client view into `ui`.
#[allow(clippy::too_many_lines, reason = "declarative UI code, easier to read inline")]
pub fn show(ui: &mut Ui, state: &mut ClientState, shared: &mut Shared<'_>) {
    ui.vertical(|ui| {
        ui.add_space(4.0);

        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot_color, status_text) = if state.connected {
                    (palette::SUCCESS, "Connected")
                } else {
                    (palette::MUTED, "Offline")
                };
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, dot_color);
                ui.label(RichText::new(status_text).strong());
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Borrow another computer's keyboard and mouse.")
                        .color(palette::MUTED),
                );

                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let (label, icon, fill) = if state.connected {
                            (
                                "Disconnect",
                                egui_phosphor::regular::PLUGS,
                                palette::SURFACE_HOVER,
                            )
                        } else {
                            (
                                "Connect",
                                egui_phosphor::regular::PLUG,
                                palette::ACCENT,
                            )
                        };
                        let btn = egui::Button::new(
                            RichText::new(format!("{icon}  {label}")).color(egui::Color32::WHITE),
                        )
                        .min_size(egui::vec2(120.0, 32.0))
                        .fill(fill);
                        if ui.add(btn).clicked() {
                            state.connected = !state.connected;
                        }
                    },
                );
            });
        });

        ui.add_space(10.0);

        widgets::card(ui, |ui| {
            ui.heading("This computer");
            ui.add_space(8.0);
            egui::Grid::new("client_identity")
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
                });
        });

        ui.add_space(10.0);

        widgets::card(ui, |ui| {
            ui.heading("Connect to server");
            ui.add_space(8.0);

            egui::Grid::new("client_connect")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .min_col_width(120.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Address").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.server_addr)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Server fingerprint").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.server_fingerprint)
                            .hint_text("paste fingerprint")
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                });

        });

        ui.add_space(10.0);

        widgets::card(ui, |ui| {
            ui.heading("My fingerprint");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Give this to the server admin so they can trust this computer.")
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
    });
}

fn hostname_fallback() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "this-computer".into())
}
