//! HOTBAR wordmark with flame energy.
//!
//! Renders the "HOTBAR" text with gradient coloring and flame accent.

use egui::{Pos2, Vec2};

use crate::theme;

/// Draw the HOTBAR logo.
///
/// Uses gradient text coloring: "HOT" in flame colors, "BAR" in chrome.
/// Height adapts to available space.
pub fn draw_logo(ui: &mut egui::Ui) {
    let available = ui.available_size();
    let logo_height = theme::FONT_SIZE_LOGO + 12.0;

    let (rect, _response) = ui.allocate_exact_size(
        Vec2::new(available.x, logo_height),
        egui::Sense::hover(),
    );

    let painter = ui.painter_at(rect);
    let center_x = rect.center().x;
    let baseline_y = rect.center().y;

    // "HOT" in flame gradient
    let hot_font = egui::FontId::new(theme::FONT_SIZE_LOGO, egui::FontFamily::Proportional);

    // Draw H
    painter.text(
        Pos2::new(center_x - 48.0, baseline_y),
        egui::Align2::CENTER_CENTER,
        "H",
        hot_font.clone(),
        theme::FLAME_YELLOW,
    );

    // Draw O
    painter.text(
        Pos2::new(center_x - 24.0, baseline_y),
        egui::Align2::CENTER_CENTER,
        "O",
        hot_font.clone(),
        theme::FLAME_ORANGE,
    );

    // Draw T
    painter.text(
        Pos2::new(center_x, baseline_y),
        egui::Align2::CENTER_CENTER,
        "T",
        hot_font.clone(),
        theme::FLAME_RED,
    );

    // "BAR" in chrome
    let bar_font = egui::FontId::new(theme::FONT_SIZE_LOGO, egui::FontFamily::Proportional);

    painter.text(
        Pos2::new(center_x + 24.0, baseline_y),
        egui::Align2::CENTER_CENTER,
        "B",
        bar_font.clone(),
        theme::CHROME,
    );

    painter.text(
        Pos2::new(center_x + 48.0, baseline_y),
        egui::Align2::CENTER_CENTER,
        "A",
        bar_font.clone(),
        theme::CHROME,
    );

    painter.text(
        Pos2::new(center_x + 72.0, baseline_y),
        egui::Align2::CENTER_CENTER,
        "R",
        bar_font.clone(),
        theme::CHROME,
    );

    // Flame accent line under "HOT"
    painter.line_segment(
        [
            Pos2::new(center_x - 60.0, baseline_y + theme::FONT_SIZE_LOGO / 2.0 + 2.0),
            Pos2::new(center_x + 12.0, baseline_y + theme::FONT_SIZE_LOGO / 2.0 + 2.0),
        ],
        egui::Stroke::new(2.0, theme::FLAME_ORANGE),
    );
}
