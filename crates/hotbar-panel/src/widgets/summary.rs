//! Summary popover — shows LLM-generated file summary near the selected spinner slot.

use crate::theme;

/// Summary popover state.
#[derive(Debug, Default)]
pub struct SummaryState {
    /// Path of the file being summarized (if any)
    pub path: Option<String>,
    /// Summary content (None = loading)
    pub content: Option<String>,
    /// Model that generated the summary
    pub model: Option<String>,
    /// Whether the popover is visible
    pub visible: bool,
}

impl SummaryState {
    /// Show loading state for a file.
    pub fn start_loading(&mut self, path: String) {
        self.path = Some(path);
        self.content = None;
        self.model = None;
        self.visible = true;
    }

    /// Set the completed summary.
    pub fn set_summary(&mut self, content: String, model: String) {
        self.content = Some(content);
        self.model = Some(model);
    }

    /// Close the popover.
    pub fn close(&mut self) {
        self.visible = false;
        self.path = None;
        self.content = None;
        self.model = None;
    }

    /// Whether we're waiting for a summary.
    pub fn is_loading(&self) -> bool {
        self.visible && self.content.is_none()
    }
}

/// Draw the summary popover.
pub fn draw_summary(ui: &mut egui::Ui, state: &mut SummaryState) {
    if !state.visible {
        return;
    }

    let Some(path) = state.path.clone() else {
        return;
    };

    let should_close = std::cell::Cell::new(false);

    egui::Frame::popup(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0, theme::CHROME_DARK))
        .corner_radius(egui::CornerRadius::same(theme::CORNER_RADIUS as u8))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_max_width(350.0);

            // Header with close button
            ui.horizontal(|ui| {
                let filename = path.rsplit('/').next().unwrap_or(&path);
                ui.colored_label(
                    theme::TEXT_PRIMARY,
                    egui::RichText::new(filename).strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(egui::RichText::new("x").color(theme::TEXT_DIMMED))
                        .clicked()
                    {
                        should_close.set(true);
                    }
                });
            });

            ui.separator();

            if state.is_loading() {
                // Loading spinner
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.colored_label(
                        theme::TEXT_SECONDARY,
                        "Generating summary...",
                    );
                });
            } else if let Some(content) = &state.content {
                // Summary text
                ui.colored_label(theme::TEXT_PRIMARY, content.as_str());

                // Model attribution
                if let Some(model) = &state.model {
                    ui.add_space(8.0);
                    ui.colored_label(
                        theme::TEXT_DIMMED,
                        egui::RichText::new(format!("via {model}")).small(),
                    );
                }
            }
        });

    if should_close.get() {
        state.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_hidden() {
        let state = SummaryState::default();
        assert!(!state.visible);
        assert!(state.path.is_none());
    }

    #[test]
    fn start_loading_shows_popover() {
        let mut state = SummaryState::default();
        state.start_loading("/test.rs".into());
        assert!(state.visible);
        assert!(state.is_loading());
        assert_eq!(state.path.as_deref(), Some("/test.rs"));
    }

    #[test]
    fn set_summary_completes() {
        let mut state = SummaryState::default();
        state.start_loading("/test.rs".into());
        state.set_summary("A test file.".into(), "qwen2.5".into());
        assert!(!state.is_loading());
        assert_eq!(state.content.as_deref(), Some("A test file."));
    }

    #[test]
    fn close_resets() {
        let mut state = SummaryState::default();
        state.start_loading("/test.rs".into());
        state.close();
        assert!(!state.visible);
        assert!(state.path.is_none());
    }
}
