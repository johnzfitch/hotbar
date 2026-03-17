//! Pit Stop shelf — horizontal scroll area of pinned files.
//!
//! Each pin is a compact card with chrome-styled border. Click to open,
//! drag handle for reorder.

use egui::{Color32, Sense, Ui, Vec2};
use hotbar_common::Pin;

use crate::theme;

/// Action from interacting with a pin in the pit stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PitStopAction {
    /// User clicked to open a pinned file
    Open(String),
    /// User wants to unpin
    Unpin(String),
    /// User reordered: moved from_index to to_index
    Reorder { from: usize, to: usize },
}

/// Pit stop shelf state.
#[derive(Debug, Default)]
pub struct PitStopState {
    /// Index of pin being dragged (if any)
    drag_source: Option<usize>,
    /// Hovered pin index during drag
    drag_target: Option<usize>,
}

/// Draw the pit stop shelf.
///
/// Returns any action the user triggered.
pub fn draw_pit_stop(
    ui: &mut Ui,
    pins: &[Pin],
    state: &mut PitStopState,
) -> Option<PitStopAction> {
    if pins.is_empty() {
        return None;
    }

    let mut action = None;

    ui.horizontal(|ui| {
        ui.add_space(4.0);

        // Label
        ui.colored_label(
            theme::CHROME,
            egui::RichText::new("PIT STOP").small(),
        );
        ui.add_space(8.0);

        egui::ScrollArea::horizontal()
            .max_height(theme::PIT_STOP_HEIGHT)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, pin) in pins.iter().enumerate() {
                        let pin_action = draw_pin_card(ui, pin, i, state);
                        if action.is_none() {
                            action = pin_action;
                        }
                    }
                });
            });
    });

    // Handle completed drag
    if let (Some(from), Some(to)) = (state.drag_source, state.drag_target)
        && !ui.input(|i| i.pointer.any_pressed()) && from != to {
            action = Some(PitStopAction::Reorder { from, to });
            state.drag_source = None;
            state.drag_target = None;
        }

    action
}

/// Draw a single pinned file card.
fn draw_pin_card(
    ui: &mut Ui,
    pin: &Pin,
    index: usize,
    state: &mut PitStopState,
) -> Option<PitStopAction> {
    let display_name = pin
        .label
        .as_deref()
        .unwrap_or_else(|| {
            pin.path
                .rsplit('/')
                .next()
                .unwrap_or(&pin.path)
        });

    let card_size = Vec2::new(120.0, theme::PIT_STOP_HEIGHT - 8.0);
    let (rect, response) = ui.allocate_exact_size(card_size, Sense::click_and_drag());

    let is_hovered = response.hovered();
    let is_drag_target = state.drag_source.is_some()
        && state.drag_source != Some(index)
        && is_hovered;

    if is_drag_target {
        state.drag_target = Some(index);
    }

    // Card background
    let bg = if is_drag_target {
        Color32::from_rgba_premultiplied(0xE6, 0x1E, 0x25, 0x30)
    } else if is_hovered {
        theme::BG_ELEVATED
    } else {
        theme::BG_SURFACE
    };

    let stroke_color = if is_drag_target {
        theme::FLAME_RED
    } else {
        theme::CHROME_DARK
    };

    ui.painter().rect(
        rect,
        egui::CornerRadius::same(theme::CORNER_RADIUS as u8),
        bg,
        egui::Stroke::new(1.0, stroke_color),
        egui::StrokeKind::Outside
    );

    // Pin name
    let text_pos = rect.left_center() + Vec2::new(8.0, 0.0);
    ui.painter().text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        truncate_str(display_name, 14),
        egui::FontId::new(theme::FONT_SIZE_SMALL, egui::FontFamily::Proportional),
        if is_hovered { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY },
    );

    // Handle interactions
    if response.clicked() {
        return Some(PitStopAction::Open(pin.path.clone()));
    }

    if response.secondary_clicked() {
        return Some(PitStopAction::Unpin(pin.path.clone()));
    }

    if response.drag_started() {
        state.drag_source = Some(index);
    }

    None
}

/// Truncate a string to max_chars, adding ellipsis if needed.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 1).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate_str("very_long_filename.rs", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 13); // 9 + "..."
    }

    #[test]
    fn pit_stop_state_default() {
        let state = PitStopState::default();
        assert!(state.drag_source.is_none());
        assert!(state.drag_target.is_none());
    }
}
