//! File > Export Song as Audio File and Export Loop as Audio File in the shell.
//!
//! The flow is: the command asks the platform for a file ([`ShellRequest::PickExportAudio`], a
//! native save dialog), the platform calls [`Shell::export_audio_to`] with the path, and the
//! shell shows the options panel ([`ExportView`]: bit depth, dither, sample rate, tail). EXPORT
//! starts the render on its worker ([`AppCore::start_audio_export`]), remembers the choices in
//! `export.json` ([`ExportPrefs`]) and turns the panel into a progress bar with CANCEL; the panel
//! closes when the export is over (the result is the toast). Esc cancels, like CANCEL. The panel
//! is modal while it is open; the song keeps playing underneath.

use super::*;
use crate::export_prefs::{ExportPrefs, export_path};
use atomik_ui::{ExportAction, ExportScope, ExportValues, ExportView};

impl<B: AudioBackend> Shell<B> {
    /// Whether the export panel (options or progress) is open.
    pub fn export_open(&self) -> bool {
        self.export.is_some()
    }

    /// The export panel, while it is open.
    pub fn export_view(&self) -> Option<&ExportView> {
        self.export.as_ref()
    }

    /// The choices the panel starts with (the last ones, from `export.json`).
    pub fn export_values(&self) -> ExportValues {
        self.export_prefs.values()
    }

    /// File > Export Song / Export Loop as Audio File: asks for the file unless the export is
    /// refused (an export runs, or the range is empty), which is said in the toast.
    pub(super) fn export_audio_request(&mut self, scope: ExportScope) -> Vec<ShellRequest> {
        match self.core.export_audio_refusal(scope) {
            None => vec![ShellRequest::PickExportAudio(scope)],
            Some(why) => {
                self.notify(why);
                Vec::new()
            }
        }
    }

    /// The file name the dialog proposes: the project's name, and `loop` for a loop.
    pub fn export_audio_name(&self, scope: ExportScope) -> String {
        let stem = self.core.file().and_then(|p| p.file_stem()).map_or_else(
            || "project".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        match scope {
            ExportScope::Song => format!("{stem}.wav"),
            ExportScope::Loop => format!("{stem} loop.wav"),
        }
    }

    /// The folder the dialog starts in: where the last export went, else the project's folder.
    pub fn export_audio_dir(&self) -> Option<PathBuf> {
        self.export_prefs
            .folder()
            .filter(|f| f.is_dir())
            .or_else(|| {
                self.core
                    .file()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
    }

    /// The file dialog picked `path`: shows the options for exporting `scope` to it.
    pub fn export_audio_to(&mut self, scope: ExportScope, path: &Path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut view = ExportView::new(self.core.view().theme().clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.open(
            scope,
            &name,
            self.core.project_sample_rate(),
            self.export_prefs.values(),
        );
        self.export = Some(view);
        self.export_target = Some((scope, path.to_path_buf()));
        self.repaint = true;
    }

    /// What the panel asked for.
    pub(super) fn export_action(&mut self, action: ExportAction) {
        match action {
            ExportAction::Cancel => {
                if self.export.as_ref().is_some_and(ExportView::in_progress) {
                    // The panel stays until the worker has stopped (`sync_export`).
                    self.core.cancel_audio_export();
                } else {
                    self.export = None;
                    self.export_target = None;
                }
            }
            ExportAction::Start(values) => {
                let Some((scope, path)) = self.export_target.clone() else {
                    self.export = None;
                    return;
                };
                match self.core.start_audio_export(scope, path.clone(), values) {
                    Ok(()) => {
                        self.export_prefs = ExportPrefs::from_values(&values, path.parent());
                        if let Some(prefs_path) = &self.export_prefs_path
                            && let Err(e) = self.export_prefs.save(prefs_path)
                        {
                            self.notify(format!("cannot save the export choices: {e}"));
                        }
                        if let Some(view) = &mut self.export {
                            view.show_progress();
                        }
                        self.sync_export();
                    }
                    // The toast says why.
                    Err(_) => {
                        self.export = None;
                        self.export_target = None;
                    }
                }
            }
        }
        self.repaint = true;
    }

    /// Feeds the progress bar, and closes the panel when the export is over.
    pub(super) fn sync_export(&mut self) {
        let Some(view) = &mut self.export else {
            return;
        };
        if !view.in_progress() {
            return;
        }
        match self.core.audio_export_progress() {
            Some((fraction, text)) => {
                view.set_progress(fraction, &text);
                if view.needs_repaint() {
                    self.repaint = true;
                }
            }
            None => {
                self.export = None;
                self.export_target = None;
                self.repaint = true;
            }
        }
    }

    /// Where the export choices are saved, next to the settings (`None` without a settings file).
    pub(super) fn export_prefs_file(settings_path: Option<&Path>) -> Option<PathBuf> {
        settings_path.map(export_path)
    }
}
