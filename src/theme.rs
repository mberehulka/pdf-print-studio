//! Light and dark colour themes for the UI.

use crate::prefs::Theme;
use eframe::egui::{self, Color32, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ACCENT: Color32 = Color32::from_rgb(64, 156, 255);

/// Colours the app paints itself, on top of egui's own visuals.
pub struct Palette {
    /// Background behind the paper in the preview and viewer.
    pub canvas: Color32,
    pub muted: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    pub info: Color32,
    pub thumb_label: Color32,
    pub thumb_border: Color32,
    pub hover: Color32,
    pub shadow: Color32,
    pub tip_bg: Color32,
    pub tip_fg: Color32,
    pub code_bg: Color32,
    pub code_fg: Color32,
}

static DARK_PALETTE: Palette = Palette {
    canvas: Color32::from_rgb(22, 23, 26),
    muted: Color32::from_gray(140),
    ok: Color32::from_rgb(120, 210, 140),
    warn: Color32::from_rgb(255, 170, 90),
    err: Color32::from_rgb(255, 115, 105),
    info: Color32::from_rgb(150, 200, 255),
    thumb_label: Color32::from_gray(190),
    thumb_border: Color32::from_gray(70),
    hover: Color32::from_rgba_premultiplied(8, 8, 8, 8),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 120),
    tip_bg: Color32::from_rgb(30, 48, 70),
    tip_fg: Color32::from_rgb(200, 225, 255),
    code_bg: Color32::from_rgb(20, 21, 24),
    code_fg: Color32::from_rgb(170, 210, 255),
};

static LIGHT_PALETTE: Palette = Palette {
    canvas: Color32::from_rgb(221, 224, 230),
    muted: Color32::from_gray(105),
    ok: Color32::from_rgb(25, 125, 60),
    warn: Color32::from_rgb(180, 95, 0),
    err: Color32::from_rgb(200, 40, 30),
    info: Color32::from_rgb(20, 95, 185),
    thumb_label: Color32::from_gray(60),
    thumb_border: Color32::from_gray(170),
    hover: Color32::from_rgba_premultiplied(0, 0, 0, 12),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 55),
    tip_bg: Color32::from_rgb(224, 237, 252),
    tip_fg: Color32::from_rgb(20, 60, 110),
    code_bg: Color32::from_rgb(237, 239, 243),
    code_fg: Color32::from_rgb(20, 80, 160),
};

static DARK: AtomicBool = AtomicBool::new(false);

pub fn pal() -> &'static Palette {
    if DARK.load(Ordering::Relaxed) {
        &DARK_PALETTE
    } else {
        &LIGHT_PALETTE
    }
}

pub fn apply(ctx: &egui::Context, theme: Theme) {
    let dark = theme == Theme::Dark;
    DARK.store(dark, Ordering::Relaxed);
    let mut v = if dark {
        let mut v = egui::Visuals::dark();
        v.selection.bg_fill = ACCENT.linear_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        v.panel_fill = Color32::from_rgb(28, 30, 34);
        v.window_fill = Color32::from_rgb(34, 36, 41);
        v.extreme_bg_color = Color32::from_rgb(20, 21, 24);
        v.faint_bg_color = Color32::from_rgb(38, 40, 46);
        v.widgets.noninteractive.bg_fill = Color32::from_rgb(34, 36, 41);
        v
    } else {
        let mut v = egui::Visuals::light();
        v.selection.bg_fill = Color32::from_rgb(196, 222, 255);
        v.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(10, 50, 110));
        v.panel_fill = Color32::from_rgb(247, 248, 250);
        v.window_fill = Color32::from_rgb(252, 252, 253);
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(238, 240, 244);
        v.widgets.noninteractive.bg_fill = Color32::from_rgb(247, 248, 250);
        v
    };
    v.hyperlink_color = if dark { ACCENT } else { Color32::from_rgb(20, 100, 210) };
    v.window_rounding = 8.0.into();
    ctx.set_visuals(v);
}
