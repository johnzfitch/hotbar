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
