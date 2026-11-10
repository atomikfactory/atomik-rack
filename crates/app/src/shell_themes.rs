//! The themes in the shell: the theme in use, the Themes tab of the settings dialog, and the theme
//! editor.
//!
//! The theme in use is named by `workflow.json` (`theme_id` for Synthwave and the user's themes,
//! `theme` for Dark, Light and System). A theme chosen in the Themes tab, in the View menu or with
//! Next Theme applies at once, in every view ([`Shell::apply_theme_with`]), and is saved. The
//! editor previews the theme it edits in the whole interface while it is open
//! (`theme_preview`), writes the theme file when it closes (or on SAVE) and then goes back to the
//! theme in use.
//!
//! The user's themes are files of the `themes` folder next to `settings.json`, kept by
//! [`crate::themes::ThemeLibrary`]; a run without a settings folder has the built-in themes only.

use super::*;
use crate::themes::{self, ThemeLibrary};
use crate::themes_tab::{ThemeAction, ThemeRow};
use atomik_ui::{ThemeEditorAction, ThemeEditorView};

impl<B: AudioBackend> Shell<B> {
    /// The themes the program offers: the built-in ones and the user's.
    pub fn themes(&self) -> &ThemeLibrary {
        &self.themes
    }

    /// Reads the user's themes again (the themes folder changed).
    pub fn reload_themes(&mut self) {
        self.themes.reload();
        self.apply_theme();
    }

    /// The id of the theme in use (see [`crate::themes`]); a theme that is gone is the choice
    /// of Dark, Light or System.
    pub fn active_theme_id(&self) -> String {
        let id = if self.themes.get(&self.workflow.theme_id).is_some() {
            self.workflow.theme_id.as_str()
        } else {
            ""
        };
        themes::active_id(id, self.workflow.theme, self.system_dark)
    }

    /// The rows of the Themes tab.
    pub(super) fn theme_rows(&self) -> Vec<ThemeRow> {
        self.themes.entries().iter().map(ThemeRow::of).collect()
    }

    /// The colours the theme in use paints with: a theme being edited first, then the theme of
    /// the preferences.
    pub(super) fn current_theme(&self) -> atomik_ui::Theme {
        if let Some(t) = &self.theme_preview {
            return t.clone();
        }
        if !self.workflow.theme_id.is_empty()
            && let Some(t) = self.themes.theme_of(&self.workflow.theme_id)
        {
            return t;
        }
        self.workflow.theme.theme(self.system_dark)
    }

    /// Uses the theme `id` now: applied, saved. `dark` and `light` are the Dark and Light
    /// choices; an id the library does not have is refused.
    pub(super) fn use_theme(&mut self, id: &str) {
        match id {
            "dark" | "light" => {
                self.workflow.theme = if id == "dark" {
                    atomik_ui::ThemeChoice::Dark
                } else {
                    atomik_ui::ThemeChoice::Light
                };
                self.workflow.theme_id.clear();
            }
            _ if self.themes.get(id).is_some() => self.workflow.theme_id = id.to_string(),
            _ => {
                self.notify("That theme is not there any more");
                return;
            }
        }
        self.save_workflow();
        self.apply_theme();
    }

    /// View > Theme > Next Theme: the theme after the one in use, in list order.
    pub(super) fn next_theme(&mut self) {
        let next = self.themes.next_id(&self.active_theme_id());
        self.use_theme(&next);
    }

    /// Shows the list of the Themes tab again, with `message` under the buttons.
    fn refresh_theme_tab(&mut self, select: Option<&str>, message: String) {
        let rows = self.theme_rows();
        if let Some(dialog) = &mut self.dialog {
            dialog.set_themes(rows, select, message);
        }
        self.repaint = true;
    }

    /// What the Themes tab asked for.
    pub(super) fn theme_action(&mut self, action: ThemeAction) -> Vec<ShellRequest> {
        match action {
            ThemeAction::Use(id) => self.use_theme(&id),
            ThemeAction::Duplicate(id) => match self.themes.duplicate(&id) {
                Ok(copy) => self.refresh_theme_tab(Some(&copy), String::new()),
                Err(e) => self.refresh_theme_tab(None, e.to_string()),
            },
            ThemeAction::Rename(id, name) => match self.themes.rename(&id, &name) {
                Ok(()) => self.refresh_theme_tab(Some(&id), String::new()),
                Err(e) => self.refresh_theme_tab(Some(&id), e.to_string()),
            },
            ThemeAction::Delete(id) => match self.themes.delete(&id) {
                Ok(()) => {
                    if self.workflow.theme_id == id {
                        self.workflow.theme_id.clear();
                        self.save_workflow();
                        self.apply_theme();
                        if let Some(dialog) = &mut self.dialog {
                            dialog.forget_custom_theme();
                        }
                    }
                    self.refresh_theme_tab(None, String::new());
                }
                Err(e) => self.refresh_theme_tab(Some(&id), e.to_string()),
            },
            ThemeAction::Edit(id) => self.open_theme_editor(&id),
            ThemeAction::Import => return vec![ShellRequest::PickImportTheme],
            ThemeAction::Export(id) => {
                let name = self
                    .themes
                    .get(&id)
                    .map(|e| format!("{}.json", themes::slug(e.name())))
                    .unwrap_or_else(|| "theme.json".into());
                self.theme_export = Some(id);
                return vec![ShellRequest::PickExportTheme(name)];
            }
            ThemeAction::OpenFolder => {
                let Some(dir) = self.themes.dir().map(Path::to_path_buf) else {
                    self.refresh_theme_tab(
                        None,
                        "There is no settings folder to keep themes in".into(),
                    );
                    return Vec::new();
                };
                match std::fs::create_dir_all(&dir) {
                    Ok(()) => return vec![ShellRequest::OpenFolder(dir)],
                    Err(e) => self.refresh_theme_tab(None, e.to_string()),
                }
            }
        }
        Vec::new()
    }

    /// The file a theme export was sent to (the platform asked for it).
    pub fn export_theme_to(&mut self, path: &Path) {
        let Some(id) = self.theme_export.take() else {
            return;
        };
        let message = match self.themes.export(&id, path) {
            Ok(()) => {
                let name = self.themes.get(&id).map(|e| e.name().to_string());
                format!("Theme {} exported", name.unwrap_or_default())
            }
            Err(e) => e.to_string(),
        };
        self.notify(message.clone());
        self.refresh_theme_tab(None, message);
    }

    /// The theme file an import was sent to: checked, copied into the themes folder, listed.
    pub fn import_theme_from(&mut self, path: &Path) {
        match self.themes.import(path) {
            Ok(id) => {
                let name = self.themes.get(&id).map(|e| e.name().to_string());
                self.notify(format!("Theme {} imported", name.unwrap_or_default()));
                self.refresh_theme_tab(Some(&id), String::new());
            }
            Err(e) => {
                let message = format!("Not imported: {e}");
                self.notify(message.clone());
                self.refresh_theme_tab(None, message);
            }
        }
    }

    // ------------------------------------------------------------------ the editor

    /// Whether the theme editor is open.
    pub fn theme_editor_open(&self) -> bool {
        self.theme_editor.is_some()
    }

    /// The theme editor, while it is open.
    pub fn theme_editor_view(&self) -> Option<&ThemeEditorView> {
        self.theme_editor.as_ref()
    }

    /// Opens the editor on the user theme `id`; a built-in theme must be duplicated first.
    pub(super) fn open_theme_editor(&mut self, id: &str) {
        let Some(entry) = self.themes.get(id) else {
            return;
        };
        if entry.builtin {
            self.refresh_theme_tab(
                Some(id),
                "A built-in theme cannot be changed: duplicate it first.".into(),
            );
            return;
        }
        let mut view =
            ThemeEditorView::new(self.current_theme(), id.to_string(), entry.doc.clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.theme_preview = Some(view.preview());
        self.theme_editor = Some(view);
        self.apply_theme();
        self.repaint = true;
    }

    /// Writes the theme file of the editor.
    fn save_theme_editor(&mut self) -> bool {
        let Some(view) = &self.theme_editor else {
            return false;
        };
        let (id, doc) = (view.id().to_string(), view.doc().clone());
        match self.themes.save(&id, &doc) {
            Ok(()) => {
                if let Some(view) = &mut self.theme_editor {
                    view.mark_saved();
                }
                true
            }
            Err(e) => {
                self.notify(format!("Theme not saved: {e}"));
                false
            }
        }
    }

    /// Closes the editor, writing the theme first when it changed; the interface goes back to the
    /// theme in use.
    fn close_theme_editor(&mut self) {
        if self.theme_editor.as_ref().is_some_and(|v| v.dirty()) && !self.save_theme_editor() {
            // Not written: the editor stays, so the work is not lost.
            return;
        }
        let id = self.theme_editor.as_ref().map(|v| v.id().to_string());
        self.theme_editor = None;
        self.theme_preview = None;
        self.apply_theme();
        self.refresh_theme_tab(id.as_deref(), String::new());
    }

    /// Hands the preview of the editor to every view.
    pub(super) fn sync_theme_preview(&mut self) {
        if let Some(view) = &self.theme_editor {
            self.theme_preview = Some(view.preview());
            self.apply_theme();
        }
    }

    /// The colour the picker gave a token of the editor.
    pub(super) fn theme_token_picked(&mut self, index: usize, rgb: [u8; 3]) {
        if let Some(view) = &mut self.theme_editor {
            view.set_color(index, rgb);
        }
        self.sync_theme_preview();
    }

    /// What the theme editor asked for.
    pub(super) fn theme_editor_action(&mut self, action: ThemeEditorAction) {
        match action {
            ThemeEditorAction::Changed => self.sync_theme_preview(),
            ThemeEditorAction::Save => {
                if self.save_theme_editor() {
                    self.notify("Theme saved");
                }
            }
            ThemeEditorAction::Close => self.close_theme_editor(),
            ThemeEditorAction::PickColor(i) => self.open_color_picker(ColorTarget::ThemeToken(i)),
        }
        self.repaint = true;
    }
}
