//! Tokyo Night palette + Phosphor icon font installation.
//!
//! Applied once at startup via [`install`]. Targets the egui 0.29 API.

use egui::{
    epaint::Shadow, style::Selection, Color32, FontFamily, Margin, Rounding, Stroke, Style, Vec2,
    Visuals,
};

/// Tokyo Night "night" palette.
#[allow(dead_code, reason = "palette is used progressively across iterations")]
pub mod palette {
    use egui::Color32;

    /// Main window / panel background.
    pub const BG: Color32 = rgb(0x1a, 0x1b, 0x26);
    /// Card / elevated surface.
    pub const SURFACE: Color32 = rgb(0x24, 0x28, 0x3b);
    /// Subtle hover / secondary surface.
    pub const SURFACE_HOVER: Color32 = rgb(0x2f, 0x33, 0x4d);
    /// Primary body text.
    pub const TEXT: Color32 = rgb(0xc0, 0xca, 0xf5);
    /// Secondary / dim text.
    pub const MUTED: Color32 = rgb(0x56, 0x5f, 0x89);
    /// Hairline border between cards.
    pub const BORDER: Color32 = rgb(0x3b, 0x42, 0x61);
    /// Primary accent (bright blue).
    pub const ACCENT: Color32 = rgb(0x7a, 0xa2, 0xf7);
    /// Accent hover (slightly lighter).
    pub const ACCENT_HOVER: Color32 = rgb(0x8a, 0xb0, 0xff);
    /// Success / "connected" indicator.
    pub const SUCCESS: Color32 = rgb(0x9e, 0xce, 0x6a);
    /// Warning / "pending" indicator.
    pub const WARN: Color32 = rgb(0xe0, 0xaf, 0x68);
    /// Error / "disconnected" indicator.
    pub const DANGER: Color32 = rgb(0xf7, 0x76, 0x8e);

    const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
        Color32::from_rgb(r, g, b)
    }
}

/// Install fonts + visuals. Call once at app startup.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    install_visuals(ctx);
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontId, TextStyle};

    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(16.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(16.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(15.0, FontFamily::Monospace)),
    ]
    .into();
    ctx.set_style(style);
}

fn install_visuals(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = palette::BG;
    visuals.window_fill = palette::SURFACE;
    visuals.window_stroke = Stroke::new(1.0, palette::BORDER);
    visuals.window_rounding = Rounding::same(12.0);
    visuals.window_shadow = soft_shadow();

    visuals.override_text_color = Some(palette::TEXT);
    visuals.hyperlink_color = palette::ACCENT;

    visuals.selection = Selection {
        bg_fill: palette::ACCENT.gamma_multiply(0.35),
        stroke: Stroke::new(1.0, palette::ACCENT),
    };

    let r = Rounding::same(8.0);
    visuals.widgets.noninteractive.bg_fill = palette::SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = palette::SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette::BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette::TEXT);
    visuals.widgets.noninteractive.rounding = r;

    visuals.widgets.inactive.bg_fill = palette::SURFACE;
    visuals.widgets.inactive.weak_bg_fill = palette::SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette::BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette::TEXT);
    visuals.widgets.inactive.rounding = r;

    visuals.widgets.hovered.bg_fill = palette::SURFACE_HOVER;
    visuals.widgets.hovered.weak_bg_fill = palette::SURFACE_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette::ACCENT.gamma_multiply(0.6));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette::TEXT);
    visuals.widgets.hovered.rounding = r;

    visuals.widgets.active.bg_fill = palette::ACCENT;
    visuals.widgets.active.weak_bg_fill = palette::ACCENT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette::ACCENT_HOVER);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = r;

    visuals.widgets.open.bg_fill = palette::SURFACE_HOVER;
    visuals.widgets.open.weak_bg_fill = palette::SURFACE_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette::BORDER);
    visuals.widgets.open.rounding = r;

    let mut style: Style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(10.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.window_margin = Margin::same(16.0);
    style.spacing.menu_margin = Margin::same(8.0);
    style.spacing.indent = 18.0;
    style.spacing.interact_size.y = 32.0;
    ctx.set_style(style);
}

/// Soft drop shadow used under elevated surfaces.
pub fn soft_shadow() -> Shadow {
    Shadow {
        offset: Vec2::new(0.0, 6.0),
        blur: 24.0,
        spread: 0.0,
        color: Color32::from_black_alpha(90),
    }
}
