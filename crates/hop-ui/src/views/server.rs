//! Server view — live reflection of the local trust DB + the
//! Start/Stop action. Real backend wiring lands in task #13 of M13;
//! this iteration covers hostname default (#11), LAN-IP surface (#12),
//! and the Add-peer modal (#14).

use egui::{RichText, Ui};

use crate::app::Shared;
use crate::theme::palette;
use crate::util;
use crate::widgets::{self, add_peer_modal};

/// Persistent state backing the Server view.
#[derive(Debug, Default, Clone)]
pub struct ServerState {
    /// Human-readable screen name advertised in `Hello`.
    pub name: String,
    /// Listen address (string form, parsed on Start in iteration 4).
    pub listen_addr: String,
    /// Whether the server is currently "running". No runtime hooked up yet.
    pub running: bool,
    /// Modal for adding a trusted peer.
    pub add_peer_modal: add_peer_modal::State,
}

impl ServerState {
    /// Construct with sane defaults — hostname from `uname`, default
    /// listen address on all interfaces.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: util::system_hostname(),
            listen_addr: "0.0.0.0:25900".into(),
            running: false,
            add_peer_modal: add_peer_modal::State::default(),
        }
    }
}

/// Draw the Server view into `ui`. The Add-peer modal is drawn
/// separately by `HopApp::update` because it lives outside the
/// scroll area.
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

                    // LAN-reachable address helper — shown only if we
                    // have a non-loopback IPv4 interface.
                    if let Some(ip) = util::lan_ipv4() {
                        let port = state
                            .listen_addr
                            .rsplit_once(':')
                            .map_or("25900", |(_, p)| p);
                        ui.label(RichText::new("Reachable at").color(palette::MUTED));
                        let reachable = format!("{ip}:{port}");
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&reachable)
                                    .monospace()
                                    .color(palette::TEXT),
                            );
                            if ui
                                .small_button(egui_phosphor::regular::COPY)
                                .on_hover_text("Copy")
                                .clicked()
                            {
                                shared.copy(&reachable);
                            }
                        });
                        ui.end_row();
                    }
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

        // ── Peers card ────────────────────────────────────────────────
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Peers");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{} Add", egui_phosphor::regular::PLUS))
                        .clicked()
                    {
                        state.add_peer_modal.open();
                    }
                });
            });
            ui.add_space(6.0);

            // Collect peers into owned tuples so the immutable borrow
            // on `shared` is released before the remove-loop below.
            let peers: Vec<(String, String)> = shared
                .trusted_peers()
                .iter()
                .map(|p| (p.name.clone(), p.fingerprint.to_string()))
                .collect();

            if peers.is_empty() {
                ui.label(
                    RichText::new(
                        "No peers yet — click + Add to trust your first client.",
                    )
                    .color(palette::MUTED)
                    .italics(),
                );
                return;
            }

            let mut to_remove: Option<String> = None;
            for (name, fp) in &peers {
                ui.horizontal(|ui| {
                    // Gray dot — turns green when a live connection
                    // exists (wired up in task #13).
                    let (dot_rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 5.0, palette::MUTED);
                    ui.label(RichText::new(name).strong());
                    ui.add_space(12.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(fp).monospace().color(palette::MUTED),
                        )
                        .truncate(),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui
                                .small_button(egui_phosphor::regular::X)
                                .on_hover_text("Remove")
                                .clicked()
                            {
                                to_remove = Some(name.clone());
                            }
                        },
                    );
                });
            }

            if let Some(name) = to_remove {
                shared.remove_peer(&name);
            }
        });
    });
}
