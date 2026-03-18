//! Search bar widget with debounced input.
//!
//! Renders a text input with placeholder. Debounces keystrokes by 200ms
//! before triggering a search callback.

use std::time::Instant;

use crate::theme;

/// Debounce interval for search queries.
const DEBOUNCE_MS: u128 = 200;

/// State for the search bar.
#[derive(Debug, Default)]
pub struct SearchBarState {
    /// Current text in the search field
    pub query: String,
    /// Whether the search bar is focused
    pub focused: bool,
    /// Last keystroke time (for debounce)
    last_input: Option<Instant>,
    /// Last query that was actually dispatched
    last_dispatched: String,
    /// Whether we're in search mode (have results to show)
    pub active: bool,
}

impl SearchBarState {
    /// Whether a new search should be dispatched (debounce expired, query changed).
    pub fn should_dispatch(&self) -> bool {
        if self.query == self.last_dispatched {
            return false;
        }
        match self.last_input {
            Some(t) => t.elapsed().as_millis() >= DEBOUNCE_MS,
            None => false,
        }
    }

    /// Mark the current query as dispatched.
    pub fn mark_dispatched(&mut self) {
        self.last_dispatched = self.query.clone();
        self.active = !self.query.is_empty();
    }

    /// Clear the search bar and exit search mode.
    pub fn clear(&mut self) {
        self.query.clear();
        self.last_dispatched.clear();
        self.active = false;
        self.focused = false;
    }

    /// Focus the search bar.
    pub fn focus(&mut self) {
        self.focused = true;
    }
}

/// Search event returned from the search bar widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// User wants to search for this query
    Search(String),
    /// User cleared/escaped the search
    Clear,
}

/// Draw the search bar.
///
/// Returns a `SearchEvent` if the user triggered a search or cleared.
pub fn draw_search_bar(
    ui: &mut egui::Ui,
    state: &mut SearchBarState,
) -> Option<SearchEvent> {
    let mut event = None;

    ui.horizontal(|ui| {
        ui.add_space(8.0);

        // Search icon placeholder
        ui.colored_label(
            theme::CHROME_DARK,
            egui::RichText::new("/").monospace(),
        );

        let text_edit = egui::TextEdit::singleline(&mut state.query)
            .hint_text("Search files...")
            .desired_width(ui.available_width() - 40.0)
            .text_color(theme::TEXT_PRIMARY)
            .font(egui::FontId::new(
                theme::FONT_SIZE_BODY,
                egui::FontFamily::Proportional,
            ));

        let response = ui.add(text_edit);

        // Track focus
        if state.focused && !response.has_focus() {
            response.request_focus();
        }
        state.focused = response.has_focus();

        // Track input timing
        if response.changed() {
            state.last_input = Some(Instant::now());
        }

        // Check debounce
        if state.should_dispatch() {
            if state.query.is_empty() {
                event = Some(SearchEvent::Clear);
                state.mark_dispatched();
            } else {
                tracing::debug!(query = %state.query, "search dispatched");
                event = Some(SearchEvent::Search(state.query.clone()));
                state.mark_dispatched();
            }
        }

        // Clear button
        if state.active
            && ui
                .small_button(egui::RichText::new("x").color(theme::TEXT_DIMMED))
                .clicked()
            {
                state.clear();
                event = Some(SearchEvent::Clear);
            }
    });

    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state() {
        let state = SearchBarState::default();
        assert!(state.query.is_empty());
        assert!(!state.focused);
        assert!(!state.active);
    }

    #[test]
    fn should_not_dispatch_unchanged() {
        let state = SearchBarState::default();
        assert!(!state.should_dispatch());
    }

    #[test]
    fn clear_resets_everything() {
        let mut state = SearchBarState {
            query: "test".into(),
            active: true,
            focused: true,
            ..Default::default()
        };
        state.clear();
        assert!(state.query.is_empty());
        assert!(!state.active);
        assert!(!state.focused);
    }

    #[test]
    fn mark_dispatched_activates() {
        let mut state = SearchBarState {
            query: "test".into(),
            ..Default::default()
        };
        state.mark_dispatched();
        assert!(state.active);
        assert_eq!(state.last_dispatched, "test");
    }

    #[test]
    fn mark_dispatched_empty_deactivates() {
        let mut state = SearchBarState {
            active: true,
            ..Default::default()
        };
        state.mark_dispatched();
        assert!(!state.active);
    }
}
