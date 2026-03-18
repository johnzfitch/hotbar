//! Main application state — wires widgets into the egui frame.
//!
//! This module owns all widget state and coordinates between them.
//! It provides a single `draw()` method called by the SCTK shell each frame.

use hotbar_common::{ActionFilter, Filter, HotFile, Pin};

use crate::keybinds::{self, KeyAction};
use crate::theme;
use crate::widgets::{
    filter_bar::{self, FilterEvent},
    logo,
    pit_stop::{self, PitStopAction, PitStopState},
    search_bar::{self, SearchBarState, SearchEvent},
    spinner::{self, SpinnerState},
    summary::SummaryState,
    toast::ToastManager,
};

/// Actions that the panel wants the daemon to perform.
///
/// Returned from `HotbarApp::draw()` and dispatched by the integration layer.
#[derive(Debug, Clone)]
pub enum AppAction {
    /// Open a file path in the default editor
    OpenFile(String),
    /// Open a folder in the file manager
    OpenFolder(String),
    /// Copy text to clipboard
    CopyToClipboard(String),
    /// Pin a file
    PinFile(String),
    /// Unpin a file
    UnpinFile(String),
    /// Request file summary from inference
    Summarize(String),
    /// Execute a search query
    Search(String),
    /// Clear search results
    ClearSearch,
    /// Toggle panel visibility
    Toggle,
    /// Set source filter
    SetSourceFilter(Filter),
    /// Set action filter
    SetActionFilter(ActionFilter),
    /// Reorder pins
    ReorderPins { from: usize, to: usize },
}

/// Main application state.
pub struct HotbarApp {
    /// Spinner widget state
    pub spinner: SpinnerState,
    /// Pit stop shelf state
    pub pit_stop: PitStopState,
    /// Search bar state
    pub search: SearchBarState,
    /// Summary popover state
    pub summary: SummaryState,
    /// Toast notification manager
    pub toasts: ToastManager,
    /// Active source filter
    pub source_filter: Filter,
    /// Active action filter
    pub action_filter: ActionFilter,
}

impl Default for HotbarApp {
    fn default() -> Self {
        Self {
            spinner: SpinnerState::default(),
            pit_stop: PitStopState::default(),
            search: SearchBarState::default(),
            summary: SummaryState::default(),
            toasts: ToastManager::default(),
            source_filter: Filter::All,
            action_filter: ActionFilter::All,
        }
    }
}

impl HotbarApp {
    /// Route a single key action into app actions, mutating state as needed.
    ///
    /// Separated from `draw()` so the routing logic can be tested without egui.
    pub fn handle_key_action(
        &mut self,
        ka: KeyAction,
        files: &[HotFile],
        pins: &[Pin],
        actions: &mut Vec<AppAction>,
    ) {
        match ka {
            KeyAction::SpinnerNext => self.spinner.rotate(1),
            KeyAction::SpinnerPrev => self.spinner.rotate(-1),
            KeyAction::Open => {
                if let Some(file) = files.get(self.spinner.selected()) {
                    actions.push(AppAction::OpenFile(file.path.clone()));
                }
            }
            KeyAction::OpenFolder => {
                if let Some(file) = files.get(self.spinner.selected()) {
                    actions.push(AppAction::OpenFolder(file.full_dir.clone()));
                }
            }
            KeyAction::FocusSearch => {
                self.search.focus();
            }
            KeyAction::TogglePin => {
                if let Some(file) = files.get(self.spinner.selected()) {
                    let is_pinned = pins.iter().any(|p| p.path == file.path);
                    if is_pinned {
                        actions.push(AppAction::UnpinFile(file.path.clone()));
                    } else {
                        actions.push(AppAction::PinFile(file.path.clone()));
                    }
                }
            }
            KeyAction::Escape => {
                if self.search.active {
                    self.search.clear();
                    actions.push(AppAction::ClearSearch);
                } else if self.summary.visible {
                    self.summary.close();
                } else {
                    actions.push(AppAction::Toggle);
                }
            }
            KeyAction::SourceFilter(idx) => {
                let filters = [
                    Filter::All,
                    Filter::Claude,
                    Filter::Codex,
                    Filter::User,
                    Filter::System,
                ];
                if let Some(&f) = filters.get(idx) {
                    self.source_filter = f;
                    actions.push(AppAction::SetSourceFilter(f));
                }
            }
            KeyAction::Summarize => {
                if let Some(file) = files.get(self.spinner.selected()) {
                    self.summary.start_loading(file.path.clone());
                    actions.push(AppAction::Summarize(file.path.clone()));
                }
            }
        }
    }

    /// Draw the entire panel UI. Returns any actions to dispatch.
    ///
    /// `files` — filtered file list to display in the spinner.
    /// `pins` — pinned files for the pit stop shelf.
    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        files: &[HotFile],
        pins: &[Pin],
    ) -> Vec<AppAction> {
        let mut actions = Vec::new();

        // Process keyboard input
        let key_actions = keybinds::process_keys(ctx);
        for ka in key_actions {
            self.handle_key_action(ka, files, pins, &mut actions);
        }

        // Draw the UI
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_PANEL))
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Logo
                    logo::draw_logo(ui);

                    ui.add_space(4.0);

                    // Filter bar
                    if let Some(fe) = filter_bar::draw_filter_bar(
                        ui,
                        self.source_filter,
                        self.action_filter,
                    ) {
                        match fe {
                            FilterEvent::SetSource(f) => {
                                self.source_filter = f;
                                actions.push(AppAction::SetSourceFilter(f));
                            }
                            FilterEvent::SetAction(a) => {
                                self.action_filter = a;
                                actions.push(AppAction::SetActionFilter(a));
                            }
                        }
                    }

                    ui.add_space(4.0);

                    // Search bar
                    if let Some(se) = search_bar::draw_search_bar(ui, &mut self.search) {
                        match se {
                            SearchEvent::Search(q) => {
                                actions.push(AppAction::Search(q));
                            }
                            SearchEvent::Clear => {
                                actions.push(AppAction::ClearSearch);
                            }
                        }
                    }

                    ui.add_space(8.0);

                    // Pit stop shelf (pinned files)
                    if let Some(pa) = pit_stop::draw_pit_stop(
                        ui,
                        pins,
                        &mut self.pit_stop,
                    ) {
                        match pa {
                            PitStopAction::Open(path) => {
                                actions.push(AppAction::OpenFile(path));
                            }
                            PitStopAction::Unpin(path) => {
                                actions.push(AppAction::UnpinFile(path));
                            }
                            PitStopAction::Reorder { from, to } => {
                                actions.push(AppAction::ReorderPins { from, to });
                            }
                        }
                    }

                    ui.separator();

                    // Main spinner
                    ui.add(spinner::Spinner::new(files, &mut self.spinner));

                    // Showcase area for selected file
                    if let Some(file) = files.get(self.spinner.selected()) {
                        ui.separator();
                        spinner::draw_showcase(ui, file);
                    }

                    // Summary popover
                    crate::widgets::summary::draw_summary(ui, &mut self.summary);

                    // Toast notifications (bottom layer)
                    self.toasts.draw(ui);
                });
            });

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotbar_common::types::{Action, Confidence, Source};

    fn make_file(path: &str, source: Source) -> HotFile {
        let filename = path.rsplit('/').next().unwrap_or(path).to_string();
        let full_dir = path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        HotFile {
            path: path.into(),
            filename,
            dir: full_dir.clone(),
            full_dir,
            timestamp: 1710500000,
            source,
            mime_type: "text/x-rust".into(),
            action: Action::Modified,
            confidence: Confidence::High,
            metadata: None,
        }
    }

    fn make_pin(path: &str) -> Pin {
        Pin {
            path: path.into(),
            label: None,
            pin_group: "default".into(),
            position: 0,
            pinned_at: 1710500000,
        }
    }

    fn test_files() -> Vec<HotFile> {
        vec![
            make_file("/home/zack/dev/main.rs", Source::Claude),
            make_file("/home/zack/dev/lib.rs", Source::User),
            make_file("/home/zack/dev/utils.rs", Source::Codex),
        ]
    }

    fn dispatch(app: &mut HotbarApp, ka: KeyAction, files: &[HotFile], pins: &[Pin]) -> Vec<AppAction> {
        let mut actions = Vec::new();
        app.handle_key_action(ka, files, pins, &mut actions);
        // Tick spinner so selected_index catches up with offset changes
        app.spinner.tick(files.len());
        actions
    }

    // ── Default state ────────────────────────────────────────

    #[test]
    fn default_state() {
        let app = HotbarApp::default();
        assert_eq!(app.source_filter, Filter::All);
        assert_eq!(app.action_filter, ActionFilter::All);
        assert!(!app.search.active);
        assert!(!app.summary.visible);
    }

    // ── Spinner navigation ───────────────────────────────────

    #[test]
    fn spinner_next_rotates() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        assert!(actions.is_empty()); // rotate is state mutation, not an action
        assert_eq!(app.spinner.selected(), 1);
    }

    #[test]
    fn spinner_prev_rotates() {
        let mut app = HotbarApp::default();
        app.spinner.rotate(2); // start at index 2
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::SpinnerPrev, &files, &[]);
        assert!(actions.is_empty());
        assert_eq!(app.spinner.selected(), 1);
    }

    // ── Open file / folder ───────────────────────────────────

    #[test]
    fn open_emits_open_file() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::Open, &files, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::OpenFile(p) if p == "/home/zack/dev/main.rs"));
    }

    #[test]
    fn open_folder_emits_open_folder() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::OpenFolder, &files, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::OpenFolder(p) if p == "/home/zack/dev"));
    }

    #[test]
    fn open_on_empty_files_does_nothing() {
        let mut app = HotbarApp::default();
        let actions = dispatch(&mut app, KeyAction::Open, &[], &[]);
        assert!(actions.is_empty());
    }

    #[test]
    fn open_folder_on_empty_files_does_nothing() {
        let mut app = HotbarApp::default();
        let actions = dispatch(&mut app, KeyAction::OpenFolder, &[], &[]);
        assert!(actions.is_empty());
    }

    // ── Pin / Unpin ──────────────────────────────────────────

    #[test]
    fn toggle_pin_unpinned_emits_pin() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::TogglePin, &files, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::PinFile(p) if p == "/home/zack/dev/main.rs"));
    }

    #[test]
    fn toggle_pin_already_pinned_emits_unpin() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let pins = vec![make_pin("/home/zack/dev/main.rs")];
        let actions = dispatch(&mut app, KeyAction::TogglePin, &files, &pins);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::UnpinFile(p) if p == "/home/zack/dev/main.rs"));
    }

    #[test]
    fn toggle_pin_on_second_file() {
        let mut app = HotbarApp::default();
        let files = test_files();
        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]); // select lib.rs
        let pins = vec![make_pin("/home/zack/dev/main.rs")]; // only main.rs pinned
        let actions = dispatch(&mut app, KeyAction::TogglePin, &files, &pins);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::PinFile(p) if p == "/home/zack/dev/lib.rs"));
    }

    #[test]
    fn toggle_pin_empty_files_does_nothing() {
        let mut app = HotbarApp::default();
        let actions = dispatch(&mut app, KeyAction::TogglePin, &[], &[]);
        assert!(actions.is_empty());
    }

    // ── Escape priority chain ────────────────────────────────

    #[test]
    fn escape_clears_search_first() {
        let mut app = HotbarApp::default();
        app.search.query = "test".into();
        app.search.active = true;
        app.summary.start_loading("/file.rs".into());

        let actions = dispatch(&mut app, KeyAction::Escape, &[], &[]);

        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::ClearSearch));
        assert!(!app.search.active);
        // Summary should still be visible (search took priority)
        assert!(app.summary.visible);
    }

    #[test]
    fn escape_closes_summary_second() {
        let mut app = HotbarApp::default();
        app.summary.start_loading("/file.rs".into());
        assert!(app.summary.visible);

        let actions = dispatch(&mut app, KeyAction::Escape, &[], &[]);

        assert!(actions.is_empty()); // closing summary is internal, no action emitted
        assert!(!app.summary.visible);
    }

    #[test]
    fn escape_toggles_panel_last() {
        let mut app = HotbarApp::default();
        // Nothing active — escape should toggle panel
        let actions = dispatch(&mut app, KeyAction::Escape, &[], &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::Toggle));
    }

    // ── Search focus ─────────────────────────────────────────

    #[test]
    fn focus_search_sets_focused() {
        let mut app = HotbarApp::default();
        assert!(!app.search.focused);
        let actions = dispatch(&mut app, KeyAction::FocusSearch, &[], &[]);
        assert!(actions.is_empty()); // internal state only
        assert!(app.search.focused);
    }

    // ── Source filter ────────────────────────────────────────

    #[test]
    fn source_filter_sets_filter_and_emits() {
        let mut app = HotbarApp::default();
        assert_eq!(app.source_filter, Filter::All);

        let actions = dispatch(&mut app, KeyAction::SourceFilter(1), &[], &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::SetSourceFilter(Filter::Claude)));
        assert_eq!(app.source_filter, Filter::Claude);
    }

    #[test]
    fn source_filter_all_indices() {
        let expected = [Filter::All, Filter::Claude, Filter::Codex, Filter::User, Filter::System];
        for (idx, expected_filter) in expected.iter().enumerate() {
            let mut app = HotbarApp::default();
            let actions = dispatch(&mut app, KeyAction::SourceFilter(idx), &[], &[]);
            assert_eq!(actions.len(), 1);
            assert!(matches!(&actions[0], AppAction::SetSourceFilter(f) if f == expected_filter));
            assert_eq!(app.source_filter, *expected_filter);
        }
    }

    #[test]
    fn source_filter_out_of_range_does_nothing() {
        let mut app = HotbarApp::default();
        let actions = dispatch(&mut app, KeyAction::SourceFilter(99), &[], &[]);
        assert!(actions.is_empty());
        assert_eq!(app.source_filter, Filter::All);
    }

    // ── Summarize ────────────────────────────────────────────

    #[test]
    fn summarize_emits_and_sets_loading() {
        let mut app = HotbarApp::default();
        let files = test_files();
        let actions = dispatch(&mut app, KeyAction::Summarize, &files, &[]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], AppAction::Summarize(p) if p == "/home/zack/dev/main.rs"));
        assert!(app.summary.visible);
        assert!(app.summary.is_loading());
        assert_eq!(app.summary.path.as_deref(), Some("/home/zack/dev/main.rs"));
    }

    #[test]
    fn summarize_empty_files_does_nothing() {
        let mut app = HotbarApp::default();
        let actions = dispatch(&mut app, KeyAction::Summarize, &[], &[]);
        assert!(actions.is_empty());
        assert!(!app.summary.visible);
    }

    // ── Spinner + action interaction ─────────────────────────

    #[test]
    fn navigate_then_open_targets_correct_file() {
        let mut app = HotbarApp::default();
        let files = test_files();

        // Navigate to third file
        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        assert_eq!(app.spinner.selected(), 2);

        let actions = dispatch(&mut app, KeyAction::Open, &files, &[]);
        assert!(matches!(&actions[0], AppAction::OpenFile(p) if p == "/home/zack/dev/utils.rs"));
    }

    #[test]
    fn navigate_then_pin_targets_correct_file() {
        let mut app = HotbarApp::default();
        let files = test_files();

        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        let actions = dispatch(&mut app, KeyAction::TogglePin, &files, &[]);
        assert!(matches!(&actions[0], AppAction::PinFile(p) if p == "/home/zack/dev/lib.rs"));
    }

    #[test]
    fn navigate_then_summarize_targets_correct_file() {
        let mut app = HotbarApp::default();
        let files = test_files();

        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        dispatch(&mut app, KeyAction::SpinnerNext, &files, &[]);
        let actions = dispatch(&mut app, KeyAction::Summarize, &files, &[]);
        assert!(matches!(&actions[0], AppAction::Summarize(p) if p == "/home/zack/dev/utils.rs"));
    }

    // ── Full escape chain sequence ───────────────────────────

    #[test]
    fn escape_chain_search_then_summary_then_toggle() {
        let mut app = HotbarApp::default();
        let files = test_files();

        // Activate search and summary
        app.search.query = "query".into();
        app.search.active = true;
        app.summary.start_loading("/file.rs".into());

        // First escape: clears search
        let a1 = dispatch(&mut app, KeyAction::Escape, &files, &[]);
        assert!(matches!(&a1[0], AppAction::ClearSearch));
        assert!(app.summary.visible); // still up

        // Second escape: closes summary
        let a2 = dispatch(&mut app, KeyAction::Escape, &files, &[]);
        assert!(a2.is_empty());
        assert!(!app.summary.visible);

        // Third escape: toggles panel
        let a3 = dispatch(&mut app, KeyAction::Escape, &files, &[]);
        assert!(matches!(&a3[0], AppAction::Toggle));
    }

    // ── Draw smoke test (headless egui) ──────────────────────

    #[test]
    fn draw_runs_without_panic() {
        let mut app = HotbarApp::default();
        let ctx = egui::Context::default();
        let files = test_files();
        let pins = vec![make_pin("/home/zack/dev/main.rs")];

        // Run two frames to exercise state transitions
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.draw(ctx, &files, &pins);
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.draw(ctx, &files, &pins);
        });
    }

    #[test]
    fn draw_empty_state_no_panic() {
        let mut app = HotbarApp::default();
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.draw(ctx, &[], &[]);
        });
    }
}
