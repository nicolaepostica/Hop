//! Modal for adding a trusted peer to the local fingerprint DB.
//!
//! Stateful — the caller keeps a [`State`] between frames and feeds it
//! to [`show`]. The modal renders only when `state.open` is true; it
//! closes itself on Cancel, on Esc, on backdrop-click, or after a
//! successful Add.

use egui::{Context, RichText};

use crate::app::Shared;
use crate::theme::palette;

/// Per-view state carried across frames.
#[derive(Debug, Default, Clone)]
pub struct State {
    /// Whether the modal is currently visible.
    pub open: bool,
    /// Text-edit buffer for the peer name.
    pub name: String,
    /// Text-edit buffer for the fingerprint (sha256:hex).
    pub fingerprint: String,
    /// Validation / persistence error shown inline under the Add button.
    pub error: Option<String>,
}

impl State {
    /// Open the modal with empty fields. Call on "+Add" button click.
    pub fn open(&mut self) {
        self.open = true;
        self.name.clear();
        self.fingerprint.clear();
        self.error = None;
    }

    fn close(&mut self) {
        self.open = false;
        self.name.clear();
        self.fingerprint.clear();
        self.error = None;
    }
}

/// Render the modal. Does nothing when `state.open == false`.
///
/// Handles Cancel / Esc / backdrop-click to close. On Add, calls
/// `shared.add_peer(...)`; on success closes the modal, on failure
/// stores the error in `state.error` and keeps it open.
#[allow(clippy::too_many_lines, reason = "declarative UI code, easier to read inline")]
pub fn show(ctx: &Context, state: &mut State, shared: &mut Shared<'_>) {
    if !state.open {
        return;
    }

    // Dim backdrop — clicking it closes the modal.
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("add_peer_backdrop"))
        .order(egui::Order::Background)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter();
            painter.rect_filled(
                screen,
                egui::Rounding::ZERO,
                egui::Color32::from_black_alpha(160),
            );
            let response = ui.allocate_rect(screen, egui::Sense::click());
            if response.clicked() {
                state.close();
            }
        });

    // Esc closes.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close();
        return;
    }

    egui::Window::new("Add a trusted peer")
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
            ui.set_min_width(420.0);

            ui.label(
                RichText::new(
                    "Paste the fingerprint exactly as the client shows it \
                     (including the `sha256:` prefix).",
                )
                .color(palette::MUTED),
            );
            ui.add_space(12.0);

            egui::Grid::new("add_peer_form")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .min_col_width(110.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.name)
                            .hint_text("laptop")
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();

                    ui.label(RichText::new("Fingerprint").color(palette::MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.fingerprint)
                            .hint_text("sha256:…")
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            if let Some(err) = &state.error {
                ui.label(RichText::new(err).color(palette::DANGER));
            } else {
                ui.label(RichText::new(" ").color(palette::MUTED));
            }

            ui.add_space(8.0);

            let form_ready =
                !state.name.trim().is_empty() && !state.fingerprint.trim().is_empty();

            ui.horizontal(|ui| {
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let add = ui.add_enabled(
                            form_ready,
                            egui::Button::new(
                                RichText::new("Add").color(egui::Color32::WHITE),
                            )
                            .min_size(egui::vec2(88.0, 32.0))
                            .fill(palette::ACCENT),
                        );
                        ui.add_space(8.0);
                        let cancel = ui.add(
                            egui::Button::new(RichText::new("Cancel"))
                                .min_size(egui::vec2(88.0, 32.0)),
                        );

                        if cancel.clicked() {
                            state.close();
                        }
                        if add.clicked() {
                            match shared.add_peer(&state.name, &state.fingerprint) {
                                Ok(()) => state.close(),
                                Err(msg) => state.error = Some(msg),
                            }
                        }
                    },
                );
            });
        });
}
