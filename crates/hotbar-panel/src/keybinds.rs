//! Keyboard navigation for the panel.
//!
//! Maps key events to panel actions. Called each frame from the app module.

use egui::Key;

/// Actions that can be triggered by keyboard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Rotate spinner up (previous file)
    SpinnerPrev,
    /// Rotate spinner down (next file)
    SpinnerNext,
    /// Open selected file
    Open,
    /// Open containing folder
    OpenFolder,
    /// Focus the search bar
    FocusSearch,
    /// Pin/unpin selected file
    TogglePin,
    /// Close panel or clear search
    Escape,
    /// Switch to source filter by index (1-5)
    SourceFilter(usize),
    /// Request summary for selected file
    Summarize,
}

/// Process egui input and return any triggered actions.
///
/// Call this each frame before drawing widgets. The returned actions
/// should be dispatched by the app module.
pub fn process_keys(ctx: &egui::Context) -> Vec<KeyAction> {
    let mut actions = Vec::new();

    ctx.input(|input| {
        // Don't intercept keys when a text field has focus
        if input.focused {
            // Only handle Escape when text is focused (to unfocus)
            if input.key_pressed(Key::Escape) {
                actions.push(KeyAction::Escape);
            }
            return;
        }

        if input.key_pressed(Key::J) || input.key_pressed(Key::ArrowDown) {
            actions.push(KeyAction::SpinnerNext);
        }
        if input.key_pressed(Key::K) || input.key_pressed(Key::ArrowUp) {
            actions.push(KeyAction::SpinnerPrev);
        }
        if input.key_pressed(Key::Enter) {
            if input.modifiers.shift {
                actions.push(KeyAction::OpenFolder);
            } else {
                actions.push(KeyAction::Open);
            }
        }
        if input.key_pressed(Key::Slash) {
            actions.push(KeyAction::FocusSearch);
        }
        if input.key_pressed(Key::P) {
            actions.push(KeyAction::TogglePin);
        }
        if input.key_pressed(Key::Escape) {
            actions.push(KeyAction::Escape);
        }
        if input.key_pressed(Key::S) && input.modifiers.alt {
            actions.push(KeyAction::Summarize);
        }

        // Number keys for source filters
        for (i, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5]
            .iter()
            .enumerate()
        {
            if input.key_pressed(*key) {
                actions.push(KeyAction::SourceFilter(i));
            }
        }
    });

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_action_equality() {
        assert_eq!(KeyAction::SpinnerNext, KeyAction::SpinnerNext);
        assert_ne!(KeyAction::SpinnerNext, KeyAction::SpinnerPrev);
        assert_eq!(KeyAction::SourceFilter(2), KeyAction::SourceFilter(2));
        assert_ne!(KeyAction::SourceFilter(1), KeyAction::SourceFilter(2));
    }
}
