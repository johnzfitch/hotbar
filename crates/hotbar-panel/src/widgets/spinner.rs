//! Radial file spinner widget.
//!
//! Files are arranged in a vertical list that scrolls with momentum physics.
//! The selected file sits at the center, with adjacent files fading out.
//! Scroll or j/k to rotate, click to select, flick for momentum.

use std::collections::{HashMap, HashSet};

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Ui, Vec2, Widget};
use hotbar_common::HotFile;

use crate::anim;
use crate::theme;
use crate::widgets::torch;

/// How many files are visible above/below the selected item.
const VISIBLE_SLOTS: usize = 6;

/// Momentum friction per frame (multiply velocity by this).
const FRICTION: f32 = 0.92;

/// Minimum velocity before stopping.
const MIN_VELOCITY: f32 = 0.1;

/// Height of each file slot in the spinner.
const SLOT_HEIGHT: f32 = 52.0;

/// A file fading out of the spinner after removal from the active list.
#[derive(Debug, Clone)]
pub struct DepartingFile {
    /// The file data (cloned at time of departure).
    pub file: HotFile,
    /// Elapsed time since departure started.
    pub elapsed: f32,
    /// Y offset relative to spinner center (pixels) at departure.
    pub y_offset: f32,
}

/// State for the spinner widget, persisted across frames.
#[derive(Debug)]
pub struct SpinnerState {
    /// Current scroll offset (fractional slot index)
    pub offset: f32,
    /// Current scroll velocity (slots per frame)
    pub velocity: f32,
    /// Index of the selected file (derived from offset)
    pub selected_index: usize,
    /// Whether the user is currently dragging
    dragging: bool,
    /// Last drag Y position (for velocity calculation during drag)
    pub last_drag_y: f32,
    /// Active arrival animations (path -> elapsed seconds).
    arrivals: HashMap<String, f32>,
    /// Files animating out of the spinner.
    departing: Vec<DepartingFile>,
    /// Paths seen on the previous frame (for detecting changes).
    prev_paths: HashSet<String>,
    /// Previous frame's file list (for cloning departing file data).
    prev_files: Vec<HotFile>,
}

impl Default for SpinnerState {
    fn default() -> Self {
        Self {
            offset: 0.0,
            velocity: 0.0,
            selected_index: 0,
            dragging: false,
            last_drag_y: 0.0,
            arrivals: HashMap::new(),
            departing: Vec::new(),
            prev_paths: HashSet::new(),
            prev_files: Vec::new(),
        }
    }
}

impl SpinnerState {
    /// Advance physics one frame. Call each frame before drawing.
    pub fn tick(&mut self, file_count: usize) {
        if file_count == 0 {
            return;
        }

        if !self.dragging {
            // Apply momentum
            self.offset += self.velocity;
            self.velocity *= FRICTION;
            if self.velocity.abs() < MIN_VELOCITY {
                self.velocity = 0.0;
                // Snap to nearest integer
                self.offset = self.offset.round();
            }
        }

        // Clamp to valid range
        let max_offset = (file_count as f32 - 1.0).max(0.0);
        self.offset = self.offset.clamp(0.0, max_offset);

        // Update selected index
        self.selected_index = self.offset.round() as usize;
        if self.selected_index >= file_count {
            self.selected_index = file_count - 1;
        }
    }

    /// Rotate by a number of slots (positive = down, negative = up).
    pub fn rotate(&mut self, slots: i32) {
        self.velocity = 0.0;
        self.offset += slots as f32;
    }

    /// Get the currently selected file index.
    pub fn selected(&self) -> usize {
        self.selected_index
    }

    /// Jump to a specific index.
    pub fn select(&mut self, index: usize) {
        self.offset = index as f32;
        self.velocity = 0.0;
        self.selected_index = index;
    }

    /// Sync transition tracking with the current file list.
    ///
    /// Detects newly arrived files (triggers slide-in animation) and
    /// departed files (triggers fade-out animation). Call once per frame
    /// before drawing.
    pub fn sync_files(&mut self, files: &[HotFile], dt: f32) {
        let _span = tracing::trace_span!("file_transitions").entered();

        let current_paths: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();

        // Skip first frame (no previous data to diff against)
        if !self.prev_paths.is_empty() {
            // Detect new arrivals
            for f in files {
                if !self.prev_paths.contains(&f.path) && !self.arrivals.contains_key(&f.path) {
                    self.arrivals.insert(f.path.clone(), 0.0);
                    tracing::debug!(path = %f.filename, "file arrived");
                }
            }

            // Detect departures — clone file data for the fade-out animation
            for prev_path in &self.prev_paths {
                if !current_paths.contains(prev_path)
                    && let Some(prev_file) = self.prev_files.iter().find(|f| &f.path == prev_path)
                {
                    let prev_idx = self.prev_files.iter()
                        .position(|f| &f.path == prev_path)
                        .unwrap_or(0);
                    let y_offset = (prev_idx as f32 - self.offset) * SLOT_HEIGHT;
                    self.departing.push(DepartingFile {
                        file: prev_file.clone(),
                        elapsed: 0.0,
                        y_offset,
                    });
                    tracing::debug!(path = %prev_path, "file departing");
                }
            }
        }

        // Advance arrival timers, remove completed
        self.arrivals.retain(|_, elapsed| {
            *elapsed += dt;
            *elapsed < anim::FileTransition::ARRIVAL_DURATION
        });

        // Advance departure timers, remove completed
        self.departing.retain_mut(|d| {
            d.elapsed += dt;
            d.elapsed < anim::FileTransition::DEPARTURE_DURATION
        });

        self.prev_paths = current_paths;
        self.prev_files = files.to_vec();
    }

    /// Get the arrival transition for a file (if actively animating in).
    pub fn arrival_transition(&self, path: &str) -> Option<anim::FileTransition> {
        self.arrivals.get(path).map(|&elapsed| anim::FileTransition::arrival_at(elapsed))
    }

    /// Active departing files (for drawing during fade-out).
    pub fn departing_files(&self) -> &[DepartingFile] {
        &self.departing
    }
}

/// The spinner widget. Draws a scrollable list of files with the selected
/// file highlighted in the center.
pub struct Spinner<'a> {
    files: &'a [HotFile],
    state: &'a mut SpinnerState,
}

impl<'a> Spinner<'a> {
    /// Create a new spinner widget.
    pub fn new(files: &'a [HotFile], state: &'a mut SpinnerState) -> Self {
        Self { files, state }
    }
}

impl Widget for Spinner<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let desired_height = SLOT_HEIGHT * (VISIBLE_SLOTS * 2 + 1) as f32;
        let desired_size = Vec2::new(ui.available_width(), desired_height);
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());

        if self.files.is_empty() {
            // Empty state
            let painter = ui.painter_at(rect);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No files yet",
                egui::FontId::new(theme::FONT_SIZE_HEADING, egui::FontFamily::Proportional),
                theme::TEXT_DIMMED,
            );
            return response;
        }

        // Handle scroll input
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta.abs() > 0.5 {
            self.state.velocity -= scroll_delta / SLOT_HEIGHT;
        }

        // Handle drag
        if response.dragged() {
            let delta_y = response.drag_delta().y;
            self.state.offset -= delta_y / SLOT_HEIGHT;
            self.state.dragging = true;
            self.state.velocity = -delta_y / SLOT_HEIGHT;
        } else if self.state.dragging {
            self.state.dragging = false;
        }

        // Handle click to select
        if response.clicked()
            && let Some(pointer_pos) = response.interact_pointer_pos() {
                let center_y = rect.center().y;
                let click_offset = (pointer_pos.y - center_y) / SLOT_HEIGHT;
                let target = (self.state.offset + click_offset).round() as usize;
                if target < self.files.len() {
                    self.state.select(target);
                }
            }

        // Advance physics
        self.state.tick(self.files.len());

        // Sync file transitions (arrivals/departures)
        let dt = ui.input(|i| i.predicted_dt);
        self.state.sync_files(self.files, dt);

        // Draw
        let _draw_span = tracing::trace_span!("spinner_draw").entered();
        let painter = ui.painter_at(rect);
        let center_y = rect.center().y;
        let selected_idx = self.state.selected_index;
        let offset_frac = self.state.offset - self.state.offset.floor();
        let time = ui.input(|i| i.time) as f32;

        for slot in -(VISIBLE_SLOTS as i32)..=(VISIBLE_SLOTS as i32) {
            let file_idx = self.state.offset.round() as i32 + slot;
            if file_idx < 0 || file_idx >= self.files.len() as i32 {
                continue;
            }
            let file_idx = file_idx as usize;
            let file = &self.files[file_idx];

            let y_offset = (slot as f32 - offset_frac + self.state.offset.floor() - self.state.offset.round()) * SLOT_HEIGHT;
            let slot_y = center_y + y_offset;

            // Fade based on distance from center
            let distance = (y_offset / (SLOT_HEIGHT * VISIBLE_SLOTS as f32)).abs();
            let base_alpha = ((1.0 - distance) * 255.0).clamp(40.0, 255.0);

            // Apply arrival transition (slide + fade)
            let (extra_x, alpha_mult) = match self.state.arrival_transition(&file.path) {
                Some(trans) => (trans.x_offset(), trans.alpha()),
                None => (0.0, 1.0),
            };
            let alpha = (base_alpha * alpha_mult).clamp(0.0, 255.0) as u8;

            let is_selected = file_idx == selected_idx;

            draw_file_slot(
                &painter,
                file,
                Pos2::new(rect.left() + 8.0 + extra_x, slot_y),
                rect.width() - 16.0,
                SLOT_HEIGHT,
                is_selected,
                alpha,
                time,
            );
        }

        // Draw departing files (fading out at their last position)
        for dep in self.state.departing_files() {
            let trans = anim::FileTransition::departure_at(dep.elapsed);
            let dep_alpha = (trans.alpha() * 255.0) as u8;
            if dep_alpha > 5 {
                let slot_y = center_y + dep.y_offset;
                draw_file_slot(
                    &painter,
                    &dep.file,
                    Pos2::new(rect.left() + 8.0 + trans.x_offset(), slot_y),
                    rect.width() - 16.0,
                    SLOT_HEIGHT,
                    false,
                    dep_alpha,
                    time,
                );
            }
        }

        // Draw selection highlight bar
        let sel_rect = Rect::from_center_size(
            Pos2::new(rect.center().x, center_y),
            Vec2::new(rect.width() - 4.0, SLOT_HEIGHT + 4.0),
        );
        painter.rect_stroke(
            sel_rect,
            egui::CornerRadius::same(theme::CORNER_RADIUS as u8),
            egui::Stroke::new(2.0, theme::FLAME_RED),
            egui::StrokeKind::Outside
        );

        response
    }
}

/// Draw a single file slot in the spinner.
#[allow(clippy::too_many_arguments)]
fn draw_file_slot(
    painter: &Painter,
    file: &HotFile,
    top_left: Pos2,
    width: f32,
    height: f32,
    is_selected: bool,
    alpha: u8,
    time: f32,
) {
    let rect = Rect::from_min_size(top_left, Vec2::new(width, height));
    let src_color = theme::source_color(file.source);
    let active_write = torch::is_active_write(file.action);

    // Background for selected
    if is_selected {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(theme::CORNER_RADIUS as u8),
            Color32::from_rgba_premultiplied(
                src_color.r() / 4,
                src_color.g() / 4,
                src_color.b() / 4,
                alpha / 3,
            ),
        );
    }

    // Source indicator dot -- flicker modulation for active writes
    let dot_center = Pos2::new(top_left.x + 10.0, top_left.y + height / 2.0);
    let dot_alpha = if active_write {
        let flicker = anim::flicker_intensity(time, torch::path_hash(&file.path));
        (alpha as f32 * flicker) as u8
    } else {
        alpha
    };
    let dot_color = Color32::from_rgba_premultiplied(
        src_color.r(),
        src_color.g(),
        src_color.b(),
        dot_alpha,
    );
    painter.circle_filled(dot_center, 4.0, dot_color);

    // Torch sprite for active writes (next to source dot, pointing upward)
    if active_write {
        torch::draw_torch(painter, dot_center, time, src_color);
    }

    // Filename
    let text_color = if is_selected {
        Color32::from_rgba_premultiplied(
            theme::TEXT_PRIMARY.r(),
            theme::TEXT_PRIMARY.g(),
            theme::TEXT_PRIMARY.b(),
            alpha,
        )
    } else {
        Color32::from_rgba_premultiplied(
            theme::TEXT_SECONDARY.r(),
            theme::TEXT_SECONDARY.g(),
            theme::TEXT_SECONDARY.b(),
            alpha,
        )
    };

    let font_size = if is_selected {
        theme::FONT_SIZE_HEADING
    } else {
        theme::FONT_SIZE_BODY
    };

    painter.text(
        Pos2::new(top_left.x + 24.0, top_left.y + 8.0),
        egui::Align2::LEFT_TOP,
        &file.filename,
        egui::FontId::new(font_size, egui::FontFamily::Proportional),
        text_color,
    );

    // Directory path (below filename)
    let dir_color = Color32::from_rgba_premultiplied(
        theme::TEXT_DIMMED.r(),
        theme::TEXT_DIMMED.g(),
        theme::TEXT_DIMMED.b(),
        alpha,
    );
    painter.text(
        Pos2::new(top_left.x + 24.0, top_left.y + 8.0 + font_size + 4.0),
        egui::Align2::LEFT_TOP,
        &file.dir,
        egui::FontId::new(theme::FONT_SIZE_SMALL, egui::FontFamily::Proportional),
        dir_color,
    );

    // Action badge (right side)
    let action_color = theme::action_color(file.action);
    let badge_color = Color32::from_rgba_premultiplied(
        action_color.r(),
        action_color.g(),
        action_color.b(),
        alpha,
    );
    let action_text = file.action.to_string();
    painter.text(
        Pos2::new(top_left.x + width - 8.0, top_left.y + height / 2.0),
        egui::Align2::RIGHT_CENTER,
        &action_text,
        egui::FontId::new(theme::FONT_SIZE_BADGE, egui::FontFamily::Proportional),
        badge_color,
    );
}

/// Draw the showcase area: full metadata for the selected file.
pub fn draw_showcase(ui: &mut Ui, file: &HotFile) {
    ui.vertical(|ui| {
        ui.add_space(8.0);

        // Filename heading
        ui.colored_label(theme::TEXT_PRIMARY, egui::RichText::new(&file.filename).heading());

        // Full path
        ui.colored_label(
            theme::TEXT_SECONDARY,
            egui::RichText::new(&file.path).small().monospace(),
        );

        ui.add_space(4.0);

        // Source and action badges inline
        ui.horizontal(|ui| {
            let src_color = theme::source_color(file.source);
            let action_color = theme::action_color(file.action);

            // Source badge
            let badge_rect = ui.available_rect_before_wrap();
            ui.painter().rect_filled(
                Rect::from_min_size(
                    badge_rect.min,
                    Vec2::new(60.0, theme::FILTER_CHIP_HEIGHT),
                ),
                egui::CornerRadius::same(4),
                Color32::from_rgba_premultiplied(src_color.r(), src_color.g(), src_color.b(), 40),
            );
            ui.colored_label(
                src_color,
                egui::RichText::new(file.source.to_string()).small(),
            );

            ui.add_space(8.0);

            // Action badge
            ui.colored_label(
                action_color,
                egui::RichText::new(file.action.to_string()).small(),
            );

            ui.add_space(8.0);

            // Timestamp
            let age = format_age(file.timestamp);
            ui.colored_label(
                theme::TEXT_DIMMED,
                egui::RichText::new(age).small(),
            );
        });

        // MIME type
        if !file.mime_type.is_empty() {
            ui.colored_label(
                theme::TEXT_DIMMED,
                egui::RichText::new(&file.mime_type).small(),
            );
        }
    });
}

/// Format a Unix timestamp as a human-readable age string.
fn format_age(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let diff = now - timestamp;
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_state_default() {
        let state = SpinnerState::default();
        assert_eq!(state.offset, 0.0);
        assert_eq!(state.velocity, 0.0);
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn spinner_tick_empty() {
        let mut state = SpinnerState::default();
        state.tick(0);
        assert_eq!(state.offset, 0.0);
    }

    #[test]
    fn spinner_tick_clamps() {
        let mut state = SpinnerState {
            offset: 100.0,
            ..Default::default()
        };
        state.tick(5);
        assert!(state.offset <= 4.0);
    }

    #[test]
    fn spinner_rotate() {
        let mut state = SpinnerState::default();
        state.rotate(1);
        state.tick(10);
        assert_eq!(state.selected_index, 1);
    }

    #[test]
    fn spinner_select() {
        let mut state = SpinnerState::default();
        state.select(5);
        assert_eq!(state.selected_index, 5);
        assert_eq!(state.offset, 5.0);
    }

    #[test]
    fn spinner_momentum_decays() {
        let mut state = SpinnerState {
            velocity: 5.0,
            ..Default::default()
        };
        state.tick(100);
        assert!(state.velocity < 5.0);
        assert!(state.velocity > 0.0);
    }

    #[test]
    fn spinner_momentum_stops() {
        let mut state = SpinnerState {
            velocity: 0.05, // Below MIN_VELOCITY
            ..Default::default()
        };
        state.tick(100);
        assert_eq!(state.velocity, 0.0);
    }

    #[test]
    fn format_age_seconds() {
        // Can't test exact output since it depends on current time,
        // but we can test the function doesn't panic
        let result = format_age(0);
        assert!(result.contains("ago") || result.contains("d ago"));
    }

    #[test]
    fn format_age_recent() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let result = format_age(now - 30);
        assert!(result.contains("30s ago"));
    }

    #[test]
    fn format_age_minutes() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let result = format_age(now - 300);
        assert!(result.contains("5m ago"));
    }
}
