//! Toast notification widget — auto-dismissing messages anchored bottom-right.

use std::collections::VecDeque;
use std::time::Instant;

use egui::{Color32, CornerRadius, Rect, Vec2};

use crate::theme;

/// How long a toast is visible before auto-dismiss.
const TOAST_DURATION_SECS: f32 = 3.0;

/// Maximum concurrent toasts.
const MAX_TOASTS: usize = 5;

/// Fade-out duration in seconds.
const FADE_DURATION: f32 = 0.3;

/// A single toast message.
struct Toast {
    message: String,
    created: Instant,
    kind: ToastKind,
}

/// Toast severity.
#[derive(Debug, Clone, Copy)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

impl ToastKind {
    fn color(self) -> Color32 {
        match self {
            ToastKind::Info => theme::CHROME,
            ToastKind::Success => theme::ACTION_CREATED,
            ToastKind::Error => theme::FLAME_RED,
        }
    }
}

/// Toast manager — queues and renders toast notifications.
#[derive(Default)]
pub struct ToastManager {
    toasts: VecDeque<Toast>,
}

impl ToastManager {
    /// Push a new toast message.
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.pop_front();
        }
        self.toasts.push_back(Toast {
            message: message.into(),
            created: Instant::now(),
            kind,
        });
    }

    /// Convenience: info toast.
    pub fn info(&mut self, message: impl Into<String>) {
        self.push(message, ToastKind::Info);
    }

    /// Convenience: success toast.
    pub fn success(&mut self, message: impl Into<String>) {
        self.push(message, ToastKind::Success);
    }

    /// Convenience: error toast.
    pub fn error(&mut self, message: impl Into<String>) {
        self.push(message, ToastKind::Error);
    }

    /// Draw all active toasts. Call this at the end of the frame.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        // Remove expired toasts
        let now = Instant::now();
        self.toasts.retain(|t| {
            now.duration_since(t.created).as_secs_f32() < TOAST_DURATION_SECS + FADE_DURATION
        });

        if self.toasts.is_empty() {
            return;
        }

        let panel_rect = ui.available_rect_before_wrap();
        let toast_width = 280.0_f32.min(panel_rect.width() - 16.0);
        let toast_height = 36.0;
        let padding = 8.0;

        for (i, toast) in self.toasts.iter().enumerate().rev() {
            let age = now.duration_since(toast.created).as_secs_f32();
            let alpha = if age > TOAST_DURATION_SECS {
                let fade_progress = (age - TOAST_DURATION_SECS) / FADE_DURATION;
                ((1.0 - fade_progress) * 255.0).max(0.0) as u8
            } else {
                255
            };

            let y_offset = (i as f32) * (toast_height + padding);
            let toast_rect = Rect::from_min_size(
                egui::pos2(
                    panel_rect.right() - toast_width - padding,
                    panel_rect.bottom() - toast_height - padding - y_offset,
                ),
                Vec2::new(toast_width, toast_height),
            );

            // Background
            ui.painter().rect_filled(
                toast_rect,
                CornerRadius::same(theme::TOAST_RADIUS as u8),
                Color32::from_rgba_premultiplied(
                    theme::BG_ELEVATED.r(),
                    theme::BG_ELEVATED.g(),
                    theme::BG_ELEVATED.b(),
                    alpha,
                ),
            );

            // Border
            let border_color = toast.kind.color();
            ui.painter().rect_stroke(
                toast_rect,
                CornerRadius::same(theme::TOAST_RADIUS as u8),
                egui::Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(
                        border_color.r(),
                        border_color.g(),
                        border_color.b(),
                        alpha,
                    ),
                ),
                egui::StrokeKind::Outside
            );

            // Text
            ui.painter().text(
                toast_rect.left_center() + Vec2::new(12.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &toast.message,
                egui::FontId::new(theme::FONT_SIZE_SMALL, egui::FontFamily::Proportional),
                Color32::from_rgba_premultiplied(
                    theme::TEXT_PRIMARY.r(),
                    theme::TEXT_PRIMARY.g(),
                    theme::TEXT_PRIMARY.b(),
                    alpha,
                ),
            );
        }
    }

    /// Number of active toasts.
    pub fn count(&self) -> usize {
        self.toasts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_count() {
        let mut mgr = ToastManager::default();
        mgr.info("hello");
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn max_toasts_evicts() {
        let mut mgr = ToastManager::default();
        for i in 0..10 {
            mgr.info(format!("toast {i}"));
        }
        assert!(mgr.count() <= MAX_TOASTS);
    }

    #[test]
    fn toast_kind_colors() {
        // Just verify they return distinct colors
        let info = ToastKind::Info.color();
        let success = ToastKind::Success.color();
        let error = ToastKind::Error.color();
        assert_ne!(info, success);
        assert_ne!(info, error);
    }
}
