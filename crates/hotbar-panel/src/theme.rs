//! Hot Wheels inspired theme — color tokens, font config, egui style overrides.
//!
//! Every widget pulls colors from here. No hardcoded `Color32` values in widget code.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Style, Visuals};

// ── Color Palette ────────────────────────────────────────────────────

/// Flame red — primary accent, selected items, hot glow
pub const FLAME_RED: Color32 = Color32::from_rgb(0xE6, 0x1E, 0x25);

/// Flame orange — warm state, secondary accent
pub const FLAME_ORANGE: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x00);

/// Flame yellow — hottest point, starburst center
pub const FLAME_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xC1, 0x07);

/// Chrome silver — brushed metal surfaces, borders, secondary text
pub const CHROME: Color32 = Color32::from_rgb(0xC0, 0xC0, 0xCC);

/// Chrome dark — muted metal, inactive elements
pub const CHROME_DARK: Color32 = Color32::from_rgb(0x60, 0x60, 0x6E);

/// Panel background — near-black with slight blue shift
pub const BG_PANEL: Color32 = Color32::from_rgb(0x0D, 0x0D, 0x14);

/// Surface background — slightly lighter than panel
pub const BG_SURFACE: Color32 = Color32::from_rgb(0x16, 0x16, 0x1E);

/// Elevated surface — popups, context menus, toasts
pub const BG_ELEVATED: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x28);

/// Text primary — high contrast on dark
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xF0, 0xF0, 0xF0);

/// Text secondary — paths, timestamps, muted info
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x90, 0x90, 0xA0);

/// Text dimmed — placeholder text, disabled items
pub const TEXT_DIMMED: Color32 = Color32::from_rgb(0x50, 0x50, 0x60);

// ── Source Badge Colors ──────────────────────────────────────────────

/// Claude — Anthropic orange-tan
pub const SOURCE_CLAUDE: Color32 = Color32::from_rgb(0xD9, 0x7F, 0x4A);

/// Codex — OpenAI green
pub const SOURCE_CODEX: Color32 = Color32::from_rgb(0x10, 0xA3, 0x7F);

/// User — cool blue
pub const SOURCE_USER: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);

/// System — muted gray
pub const SOURCE_SYSTEM: Color32 = Color32::from_rgb(0x6B, 0x72, 0x80);

// ── Action Colors ────────────────────────────────────────────────────

/// Created — green
pub const ACTION_CREATED: Color32 = Color32::from_rgb(0x22, 0xC5, 0x5E);

/// Modified — amber
pub const ACTION_MODIFIED: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);

/// Opened — blue
pub const ACTION_OPENED: Color32 = Color32::from_rgb(0x60, 0xA5, 0xFA);

/// Deleted — red
pub const ACTION_DELETED: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);

// ── Geometry Tokens ──────────────────────────────────────────────────

/// Panel width when anchored to screen edge
pub const PANEL_WIDTH: f32 = 420.0;

/// Minimum panel margin from screen edges
pub const PANEL_MARGIN: f32 = 8.0;

/// Standard corner rounding for cards/buttons
pub const CORNER_RADIUS: f32 = 6.0;

/// Toast popup rounding (slightly more rounded)
pub const TOAST_RADIUS: f32 = 8.0;

/// Spinner slot size (diameter of file circle)
pub const SPINNER_SLOT_SIZE: f32 = 56.0;

/// Spinner slot size when selected
pub const SPINNER_SLOT_SIZE_SELECTED: f32 = 72.0;

/// Spinner radius — distance from center to slots
pub const SPINNER_RADIUS: f32 = 140.0;

/// Pit stop shelf height
pub const PIT_STOP_HEIGHT: f32 = 64.0;

/// Filter chip height
pub const FILTER_CHIP_HEIGHT: f32 = 28.0;

/// Search bar height
pub const SEARCH_BAR_HEIGHT: f32 = 36.0;

// ── Font Config ──────────────────────────────────────────────────────

/// Body text font size
pub const FONT_SIZE_BODY: f32 = 13.0;

/// Small text (timestamps, muted labels)
pub const FONT_SIZE_SMALL: f32 = 11.0;

/// Badge text
pub const FONT_SIZE_BADGE: f32 = 10.0;

/// Heading / filename in showcase
pub const FONT_SIZE_HEADING: f32 = 16.0;

/// Logo text
pub const FONT_SIZE_LOGO: f32 = 28.0;

// ── Helpers ──────────────────────────────────────────────────────────

/// Get the badge color for a source.
pub fn source_color(source: hotbar_common::Source) -> Color32 {
    match source {
        hotbar_common::Source::Claude => SOURCE_CLAUDE,
        hotbar_common::Source::Codex => SOURCE_CODEX,
        hotbar_common::Source::User => SOURCE_USER,
        hotbar_common::Source::System => SOURCE_SYSTEM,
    }
}

/// Get the color for an action.
pub fn action_color(action: hotbar_common::Action) -> Color32 {
    match action {
        hotbar_common::Action::Created => ACTION_CREATED,
        hotbar_common::Action::Modified => ACTION_MODIFIED,
        hotbar_common::Action::Opened => ACTION_OPENED,
        hotbar_common::Action::Deleted => ACTION_DELETED,
    }
}

/// Interpolate between two colors by t (0.0..=1.0).
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_premultiplied(
        (a.r() as f32 * inv + b.r() as f32 * t) as u8,
        (a.g() as f32 * inv + b.g() as f32 * t) as u8,
        (a.b() as f32 * inv + b.b() as f32 * t) as u8,
        (a.a() as f32 * inv + b.a() as f32 * t) as u8,
    )
}

/// Heat-mapped color: cold (chrome) → warm (orange) → hot (red) → on_fire (yellow core).
/// Input: 0.0..=1.0 intensity from ActivityLevel.
pub fn heat_color(intensity: f32) -> Color32 {
    if intensity < 0.33 {
        lerp_color(CHROME_DARK, FLAME_ORANGE, intensity / 0.33)
    } else if intensity < 0.66 {
        lerp_color(FLAME_ORANGE, FLAME_RED, (intensity - 0.33) / 0.33)
    } else {
        lerp_color(FLAME_RED, FLAME_YELLOW, (intensity - 0.66) / 0.34)
    }
}

/// Apply the Hot Wheels theme to an egui context.
pub fn apply_theme(ctx: &egui::Context) {
    let mut style = Style {
        visuals: Visuals::dark(),
        ..Default::default()
    };

    // Override specific colors
    let v = &mut style.visuals;
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_ELEVATED;
    v.extreme_bg_color = BG_SURFACE;
    v.faint_bg_color = BG_SURFACE;

    // Widget styling
    v.widgets.noninteractive.bg_fill = BG_SURFACE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(CORNER_RADIUS as u8);

    v.widgets.inactive.bg_fill = BG_SURFACE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.inactive.corner_radius = CornerRadius::same(CORNER_RADIUS as u8);

    v.widgets.hovered.bg_fill = BG_ELEVATED;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, FLAME_ORANGE);
    v.widgets.hovered.corner_radius = CornerRadius::same(CORNER_RADIUS as u8);

    v.widgets.active.bg_fill = Color32::from_rgb(0x2A, 0x18, 0x10);
    v.widgets.active.fg_stroke = Stroke::new(2.0, FLAME_RED);
    v.widgets.active.corner_radius = CornerRadius::same(CORNER_RADIUS as u8);

    // Focus ring — chrome colored
    v.selection.bg_fill = Color32::from_rgba_premultiplied(0xE6, 0x1E, 0x25, 0x40);
    v.selection.stroke = Stroke::new(2.0, FLAME_RED);

    // Window shadow/rounding
    v.window_corner_radius = CornerRadius::same(CORNER_RADIUS as u8);
    v.window_stroke = Stroke::new(1.0, CHROME_DARK);

    // Text colors
    style.visuals.override_text_color = Some(TEXT_PRIMARY);

    // Spacing
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 4.0);

    // Set default font sizes
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(FONT_SIZE_BODY, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        FontId::new(FONT_SIZE_SMALL, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(FONT_SIZE_HEADING, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        FontId::new(FONT_SIZE_BODY, FontFamily::Monospace),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(FONT_SIZE_BODY, FontFamily::Proportional),
    );

    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_colors_all_distinct() {
        let colors = [SOURCE_CLAUDE, SOURCE_CODEX, SOURCE_USER, SOURCE_SYSTEM];
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j]);
            }
        }
    }

    #[test]
    fn action_colors_all_distinct() {
        let colors = [ACTION_CREATED, ACTION_MODIFIED, ACTION_OPENED, ACTION_DELETED];
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j]);
            }
        }
    }

    #[test]
    fn lerp_color_endpoints() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(255, 255, 255);
        assert_eq!(lerp_color(a, b, 0.0), Color32::from_rgba_premultiplied(0, 0, 0, 255));
        assert_eq!(lerp_color(a, b, 1.0), Color32::from_rgba_premultiplied(255, 255, 255, 255));
    }

    #[test]
    fn lerp_color_midpoint() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 200, 200);
        let mid = lerp_color(a, b, 0.5);
        // Should be approximately 100
        assert!((mid.r() as i32 - 100).abs() <= 1);
    }

    #[test]
    fn heat_color_cold_is_dark() {
        let c = heat_color(0.0);
        // Should be close to CHROME_DARK
        assert_eq!(c, CHROME_DARK);
    }

    #[test]
    fn heat_color_max_is_yellow() {
        let c = heat_color(1.0);
        // Allow 1-unit tolerance from floating-point lerp rounding
        assert!((c.r() as i16 - FLAME_YELLOW.r() as i16).abs() <= 1);
        assert!((c.g() as i16 - FLAME_YELLOW.g() as i16).abs() <= 1);
        assert!((c.b() as i16 - FLAME_YELLOW.b() as i16).abs() <= 1);
    }

    #[test]
    fn source_color_maps_correctly() {
        assert_eq!(source_color(hotbar_common::Source::Claude), SOURCE_CLAUDE);
        assert_eq!(source_color(hotbar_common::Source::Codex), SOURCE_CODEX);
    }

    #[test]
    fn action_color_maps_correctly() {
        assert_eq!(action_color(hotbar_common::Action::Created), ACTION_CREATED);
        assert_eq!(action_color(hotbar_common::Action::Deleted), ACTION_DELETED);
    }
}
