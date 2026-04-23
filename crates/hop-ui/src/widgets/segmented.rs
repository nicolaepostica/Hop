//! Segmented toggle — the pill-shaped Server/Client switcher at the
//! top of the main window. Animates the thumb position so the
//! transition feels intentional.

use egui::{
    pos2, vec2, Color32, FontId, Response, Rounding, Sense, TextStyle, Ui, Widget, WidgetText,
};

use crate::theme::palette;

/// Render a segmented picker with two or more options.
///
/// `value` is written in-place when the user clicks a different segment.
pub fn segmented<T>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response
where
    T: PartialEq + Clone,
{
    assert!(!options.is_empty(), "segmented needs at least one option");

    let height = 34.0_f32;
    let pad = 3.0_f32;
    let font = ui
        .style()
        .text_styles
        .get(&TextStyle::Button)
        .cloned()
        .unwrap_or_else(|| FontId::proportional(14.0));

    // Measure each label to find an even segment width.
    let label_widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            let galley = WidgetText::from(*label).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                font.clone(),
            );
            galley.size().x
        })
        .collect();
    let max_label = label_widths.iter().copied().fold(0.0_f32, f32::max);
    let seg_width = max_label + 28.0;
    #[allow(clippy::cast_precision_loss, reason = "options.len() is tiny (2–4)")]
    let total_width = seg_width * options.len() as f32 + pad * 2.0;

    // Hover-aware so the whole pill reacts, but we'll draw ourselves —
    // no default background from egui.
    let (rect, response) = ui.allocate_exact_size(vec2(total_width, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(8.0));

    // ── Track: flat pill, no stroke (strokes on rounded corners give
    //    subpixel haloing that looks "dirty" on dark themes). ─────────
    let track_round = Rounding::same(height / 2.0);
    painter.rect_filled(rect, track_round, palette::SURFACE);

    let active_idx = options.iter().position(|(v, _)| v == value).unwrap_or(0);

    // ── Thumb: animated slide, soft shadow for elevation ───────────
    let id = response.id.with("segmented_thumb");
    #[allow(clippy::cast_precision_loss, reason = "options.len() is tiny (2–4)")]
    let target_x = rect.left() + pad + seg_width * active_idx as f32;
    let thumb_x = ui.ctx().animate_value_with_time(id, target_x, 0.18);
    let thumb_rect = egui::Rect::from_min_size(
        pos2(thumb_x, rect.top() + pad),
        vec2(seg_width, height - pad * 2.0),
    );
    let thumb_round = Rounding::same((height - pad * 2.0) / 2.0);
    // Soft shadow — cheap fake with a slightly offset, translucent fill.
    painter.rect_filled(
        thumb_rect.translate(vec2(0.0, 1.5)),
        thumb_round,
        Color32::from_black_alpha(60),
    );
    painter.rect_filled(thumb_rect, thumb_round, palette::ACCENT);

    // ── Labels + clickable per-segment areas ────────────────────────
    for (i, (variant, label)) in options.iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "options.len() is tiny (2–4)")]
        let seg_x = rect.left() + pad + seg_width * i as f32;
        let seg_rect = egui::Rect::from_min_size(
            pos2(seg_x, rect.top() + pad),
            vec2(seg_width, height - pad * 2.0),
        );
        let seg_response = ui.interact(seg_rect, response.id.with(i), Sense::click());
        if seg_response.clicked() && value != variant {
            *value = variant.clone();
        }

        let is_active = i == active_idx;
        let text_color = if is_active {
            Color32::WHITE
        } else if seg_response.hovered() {
            palette::TEXT
        } else {
            palette::MUTED
        };

        painter.text(
            seg_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            text_color,
        );
    }

    if (thumb_x - target_x).abs() > 0.1 {
        ui.ctx().request_repaint();
    }

    response
}

/// Trivial `Widget` bridge so `ui.add(...)` also works when convenient.
#[allow(dead_code, reason = "alternative call style, handy in wizards")]
pub struct Segmented<'a, T> {
    value: &'a mut T,
    options: &'a [(T, &'a str)],
}

impl<'a, T: PartialEq + Clone> Segmented<'a, T> {
    /// Construct a new segmented widget bound to `value`.
    #[allow(dead_code, reason = "alternative call style, handy in wizards")]
    pub fn new(value: &'a mut T, options: &'a [(T, &'a str)]) -> Self {
        Self { value, options }
    }
}

impl<T: PartialEq + Clone> Widget for Segmented<'_, T> {
    fn ui(self, ui: &mut Ui) -> Response {
        segmented(ui, self.value, self.options)
    }
}
