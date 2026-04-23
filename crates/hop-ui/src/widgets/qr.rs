//! QR-code rendering for fingerprint sharing.
//!
//! `egui` has no bitmap QR widget, so we generate the matrix with the
//! `qrcode` crate and paint each dark module as a tiny filled square.
//! Fast enough for the fingerprint use case — a sha256 hex string
//! encodes at version 5 / ECC-M, about 37×37 modules (<1500 cells).

use egui::{Color32, Painter, Pos2, Rect, Vec2};
use qrcode::{Color, QrCode};

/// A cached QR-code matrix — build once, reuse across repaints.
#[derive(Clone)]
pub struct Qr {
    modules: Vec<bool>,
    size: usize,
}

impl Qr {
    /// Encode `data` as a QR code. Returns `None` if the input is too
    /// large for the largest QR version (very unlikely for a
    /// fingerprint or `hop://` URI, but stay defensive).
    pub fn encode(data: &str) -> Option<Self> {
        let code = QrCode::new(data.as_bytes()).ok()?;
        let size = code.width();
        let modules = code
            .to_colors()
            .into_iter()
            .map(|c| c == Color::Dark)
            .collect();
        Some(Self { modules, size })
    }

    /// Paint this QR into `rect`. The matrix is stretched uniformly to
    /// fill `rect.shortest_side()`, centred, over a white background
    /// (standard convention — readers need the light quiet zone).
    pub fn paint(&self, painter: &Painter, rect: Rect) {
        #[allow(clippy::cast_precision_loss, reason = "size fits in f32 comfortably")]
        let size_with_quiet = (self.size + 8) as f32;
        let full = rect.width().min(rect.height());
        let module = full / size_with_quiet;
        let grid_total = module * size_with_quiet;
        let origin = Pos2::new(
            rect.center().x - grid_total / 2.0,
            rect.center().y - grid_total / 2.0,
        );

        painter.rect_filled(
            Rect::from_min_size(origin, Vec2::splat(grid_total)),
            egui::Rounding::same(6.0),
            Color32::WHITE,
        );

        let inset = module * 4.0;
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.modules[y * self.size + x] {
                    continue;
                }
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "QR matrix is small (<180 modules)"
                )]
                let cell_origin = Pos2::new(
                    origin.x + inset + x as f32 * module,
                    origin.y + inset + y as f32 * module,
                );
                painter.rect_filled(
                    Rect::from_min_size(cell_origin, Vec2::splat(module)),
                    egui::Rounding::ZERO,
                    Color32::BLACK,
                );
            }
        }
    }
}
