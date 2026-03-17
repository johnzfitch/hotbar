//! Filter bar — row of chips for source and action filtering.

use hotbar_common::{ActionFilter, Filter};

use crate::theme;

/// Event from filter bar interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterEvent {
    SetSource(Filter),
    SetAction(ActionFilter),
}

/// Draw the source + action filter bar.
///
/// Returns a `FilterEvent` if the user clicked a chip.
pub fn draw_filter_bar(
    ui: &mut egui::Ui,
    active_source: Filter,
    active_action: ActionFilter,
) -> Option<FilterEvent> {
    let mut event = None;

    ui.horizontal_wrapped(|ui| {
        ui.add_space(4.0);

        // Source filters
        let source_chips: &[(Filter, &str, egui::Color32)] = &[
            (Filter::All, "All", theme::CHROME),
            (Filter::Claude, "Claude", theme::SOURCE_CLAUDE),
            (Filter::Codex, "Codex", theme::SOURCE_CODEX),
            (Filter::User, "User", theme::SOURCE_USER),
            (Filter::System, "System", theme::SOURCE_SYSTEM),
        ];

        for &(filter, label, color) in source_chips {
            let is_active = active_source == filter;
            if draw_chip(ui, label, color, is_active).clicked() {
                event = Some(FilterEvent::SetSource(filter));
            }
        }

        ui.add_space(12.0);
        ui.colored_label(theme::CHROME_DARK, "|");
        ui.add_space(12.0);

        // Action filters
        let action_chips: &[(ActionFilter, &str, egui::Color32)] = &[
            (ActionFilter::All, "All", theme::CHROME),
            (ActionFilter::Created, "Created", theme::ACTION_CREATED),
            (ActionFilter::Modified, "Modified", theme::ACTION_MODIFIED),
            (ActionFilter::Opened, "Opened", theme::ACTION_OPENED),
            (ActionFilter::Deleted, "Deleted", theme::ACTION_DELETED),
        ];

        for &(filter, label, color) in action_chips {
            let is_active = active_action == filter;
            if draw_chip(ui, label, color, is_active).clicked() {
                event = Some(FilterEvent::SetAction(filter));
            }
        }
    });

    event
}

/// Draw a single filter chip.
fn draw_chip(
    ui: &mut egui::Ui,
    label: &str,
    color: egui::Color32,
    active: bool,
) -> egui::Response {
    let text = if active {
        egui::RichText::new(label)
            .color(color)
            .small()
            .strong()
    } else {
        egui::RichText::new(label)
            .color(theme::TEXT_DIMMED)
            .small()
    };

    let button = egui::Button::new(text)
        .fill(if active {
            egui::Color32::from_rgba_premultiplied(color.r() / 5, color.g() / 5, color.b() / 5, 80)
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(egui::CornerRadius::same(theme::CORNER_RADIUS as u8))
        .stroke(if active {
            egui::Stroke::new(1.0, color)
        } else {
            egui::Stroke::NONE
        });

    let response = ui.add(button);

    // Flame underline for active chip
    if active {
        let rect = response.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 4.0, rect.bottom()),
                egui::pos2(rect.right() - 4.0, rect.bottom()),
            ],
            egui::Stroke::new(2.0, color),
        );
    }

    response
}
