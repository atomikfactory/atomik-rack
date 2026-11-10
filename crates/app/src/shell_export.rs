//! File > Export Song as Audio File and Export Loop as Audio File in the shell.
//!
//! The flow is: the command asks the platform for a file ([`ShellRequest::PickExportAudio`], a
//! native save dialog), the platform calls [`Shell::export_audio_to`] with the path, and the
//! shell shows the options panel ([`ExportView`]: bit depth, dither, sample rate, file format and
//! quality, tail; MP3, AAC and Opus are greyed unless FFmpeg was found). EXPORT
//! starts the render on its worker ([`AppCore::start_audio_export`]), remembers the choices in
//! `export.json` ([`ExportPrefs`]) and turns the panel into a progress bar with CANCEL; the panel
//! closes when the export is over (the result is the toast). Esc cancels, like CANCEL. The panel
//! is modal while it is open; the song keeps playing underneath.

use super::*;
use crate::app_core::file_extension;
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
        // The format is chosen after the dialog: propose the one used last, so the name the dialog
        // asks about is the one the file gets.
        let ext = self.export_audio_extension();
        match scope {
            ExportScope::Song => format!("{stem}.{ext}"),
            ExportScope::Loop => format!("{stem} loop.{ext}"),
            ExportScope::Selection => format!("{stem} selection.{ext}"),
            // The folder the stems go into, made inside the one the dialog picks.
            ExportScope::Stems => atomik_render::stems::stems_folder_name(&stem),
        }
    }

    /// The extension the file dialog proposes: the one of the format the last export used.
    pub fn export_audio_extension(&self) -> &'static str {
        file_extension(&self.export_prefs.values())
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
        // MP3, AAC and Opus need FFmpeg: look for it now (unless that was done) so the panel
        // knows whether to offer them.
        self.core.refresh_ffmpeg_status();
        view.set_encoder(self.core.export_encoder());
        view.open(
            scope,
            &name,
            self.core.project_sample_rate(),
            self.export_prefs.values(),
        );
        self.export = Some(view);
        self.export_target = Some((scope, path.to_path_buf()));
        self.export_replace = None;
        self.repaint = true;
    }

    /// What the panel asked for.
    pub(super) fn export_action(&mut self, action: ExportAction) {
        match action {
            ExportAction::SavePreset | ExportAction::DeletePreset => {}
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
                // The dialog asked about the name it picked; the file gets the extension of the
                // format chosen since. A file at that name would be replaced unasked: say so, and
                // let a second EXPORT confirm.
                let file = self.core.export_file_for(scope, &path, &values);
                if file != path && file.exists() && self.export_replace.as_ref() != Some(&file) {
                    let warning = replace_warning(&file, "format");
                    self.export_replace = Some(file);
                    if let Some(view) = &mut self.export {
                        view.set_warning(Some(warning));
                    }
                    self.repaint = true;
                    return;
                }
                match self.core.start_audio_export(scope, path.clone(), values) {
                    Ok(()) => {
                        self.export_prefs = ExportPrefs::from_values(&values, path.parent())
                            .with_film_of(&self.export_prefs);
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
        let encoder = self.core.export_encoder();
        let Some(view) = &mut self.export else {
            return;
        };
        if !view.in_progress() {
            // The answer to "is FFmpeg there" may arrive while the options are open.
            view.set_encoder(encoder);
            if view.needs_repaint() {
                self.repaint = true;
            }
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

/// The line under the options when the export would replace `file` (the extension of the format
/// chosen after the dialog is not the one the dialog asked about).
pub(super) fn replace_warning(file: &Path, choice: &str) -> String {
    let name = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!("{name} already exists: press EXPORT again to replace it, or change the {choice}.")
}
