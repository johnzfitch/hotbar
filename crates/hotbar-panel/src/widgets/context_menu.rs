//! Context menu for right-click on file entries.
//!
//! Shows: Open, Open Folder, Copy Path, Pin/Unpin, Summarize.

use hotbar_common::HotFile;

use crate::theme;

/// Action from the context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAction {
    /// Open file in default editor
    Open(String),
    /// Open containing folder in file manager
    OpenFolder(String),
    /// Copy file path to clipboard
    CopyPath(String),
    /// Pin the file
    Pin(String),
    /// Unpin the file
    Unpin(String),
    /// Request LLM summary
    Summarize(String),
}

/// Draw a context menu for a file.
///
/// Call this with `response.context_menu(|ui| draw_context_menu(ui, file, is_pinned))`
/// from the spinner or any file list item.
pub fn draw_context_menu(
    ui: &mut egui::Ui,
    file: &HotFile,
    is_pinned: bool,
) -> Option<ContextAction> {
    let mut action = None;
    let path = file.path.clone();

    if ui
        .button(egui::RichText::new("Open").color(theme::TEXT_PRIMARY))
        .clicked()
    {
        action = Some(ContextAction::Open(path.clone()));
        ui.close_menu();
    }

    if ui
        .button(egui::RichText::new("Open Folder").color(theme::TEXT_PRIMARY))
        .clicked()
    {
        action = Some(ContextAction::OpenFolder(file.full_dir.clone()));
        ui.close_menu();
    }

    ui.separator();

    if ui
        .button(egui::RichText::new("Copy Path").color(theme::TEXT_PRIMARY))
        .clicked()
    {
        action = Some(ContextAction::CopyPath(path.clone()));
        ui.close_menu();
    }

    if is_pinned {
        if ui
            .button(egui::RichText::new("Unpin").color(theme::TEXT_SECONDARY))
            .clicked()
        {
            action = Some(ContextAction::Unpin(path.clone()));
            ui.close_menu();
        }
    } else if ui
        .button(egui::RichText::new("Pin").color(theme::FLAME_ORANGE))
        .clicked()
    {
        action = Some(ContextAction::Pin(path.clone()));
        ui.close_menu();
    }

    ui.separator();

    if ui
        .button(egui::RichText::new("Summarize").color(theme::SOURCE_CLAUDE))
        .clicked()
    {
        action = Some(ContextAction::Summarize(path));
        ui.close_menu();
    }

    action
}
