//! Elevated "card" container — rounded rectangle with a hairline
//! border and a soft shadow, used to group related controls.

use egui::{Frame, Margin, Response, Ui};

use crate::theme::{palette, soft_shadow};

/// Inner horizontal padding applied by the card frame.
const INNER_PAD_X: f32 = 16.0;

/// Wrap `content` in a Hop-styled card frame.
///
/// The frame is always full-width: we call `set_min_width` on the
/// inner `Ui` so short content (e.g. a single status line) doesn't
/// shrink the card below its siblings.
pub fn card<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> (Response, R) {
    // Reserve room for the frame's horizontal padding on both sides.
    let content_width = (ui.available_width() - INNER_PAD_X * 2.0).max(0.0);

    let frame = Frame::group(ui.style())
        .fill(palette::SURFACE)
        .stroke(egui::Stroke::new(1.0, palette::BORDER))
        .rounding(egui::Rounding::same(12.0))
        .shadow(soft_shadow())
        .inner_margin(Margin::symmetric(INNER_PAD_X, 14.0))
        .outer_margin(Margin::ZERO);
    let inner = frame.show(ui, |ui| {
        ui.set_min_width(content_width);
        content(ui)
    });
    (inner.response, inner.inner)
}
