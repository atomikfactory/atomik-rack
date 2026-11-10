//! File > Export Song as Audio File and Export Loop as Audio File in the shell.
//!
//! The flow is: the command asks the platform for a file ([`ShellRequest::PickExportAudio`], a
//! native save dialog), the platform calls [`Shell::export_audio_to`] with the path, and the
//! shell shows the options panel ([`ExportView`]: bit depth, dither, sample rate, file format and
//! quality, tail, mix-down, loudness and extra file, and a Preset row that fills them; MP3, AAC
//! and Opus are greyed unless FFmpeg was found). EXPORT
//! starts the render on its worker ([`AppCore::start_audio_export`]), remembers the choices in
//! `export.json` ([`ExportPrefs`]) and turns the panel into a progress bar with CANCEL; the panel
//! closes when the export is over (the result is the toast). Esc cancels, like CANCEL. The panel
//! is modal while it is open; the song keeps playing underneath.

use super::*;
use crate::app_core::{extra_file, file_extension};
use crate::export_prefs::{ExportPrefs, export_path};
use crate::export_presets::VIDEO;
use atomik_ui::{EncoderState, ExportAction, ExportScope, ExportValues, ExportView, PresetChoice};

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

    /// The choices the file dialog is about: those of the preset File > Export for runs, else
    /// the last ones.
    fn export_values_now(&self) -> ExportValues {
        self.export_run
            .as_ref()
            .map_or_else(|| self.export_prefs.values(), |p| p.values)
    }

    /// Every export preset: the built-in ones, then the user's.
    pub fn export_presets(&self) -> Vec<PresetChoice> {
        self.export_presets.all()
    }

    /// Whether File > Export for runs `preset` as the film of the song: the Video preset when
    /// the song has a film that can be exported.
    fn preset_is_film(&self, preset: &PresetChoice) -> bool {
        !preset.user && preset.name == VIDEO && self.core.film_export_refusal().is_none()
    }

    /// The rows of the File menu's Export for submenu: each preset's label (with "Needs FFmpeg"
    /// when it cannot be used without it) and whether it can run now.
    pub(super) fn export_menu_rows(&self) -> Vec<(String, bool)> {
        let ffmpeg_ok = !matches!(self.core.export_encoder(), EncoderState::Missing(_));
        self.export_presets
            .all()
            .into_iter()
            .map(|p| {
                let scope = if p.stems {
                    ExportScope::Stems
                } else {
                    ExportScope::Song
                };
                let tool_ok = ffmpeg_ok || !p.needs_ffmpeg();
                let label = if tool_ok {
                    p.name.clone()
                } else {
                    format!("{} (Needs FFmpeg)", p.name)
                };
                let can = self.preset_is_film(&p) || self.core.can_export_audio(scope);
                (label, tool_ok && can)
            })
            .collect()
    }

    /// File > Export for > a preset: asks for the file (a folder for stems), unless the preset
    /// cannot run (the toast says why). The export then starts at once with the preset's choices.
    pub(super) fn export_for_request(&mut self, index: usize) -> Vec<ShellRequest> {
        self.export_run = None;
        let Some(preset) = self.export_presets.get(index) else {
            self.notify("There is no such export preset".to_owned());
            return Vec::new();
        };
        self.core.refresh_ffmpeg_status();
        if preset.needs_ffmpeg()
            && let EncoderState::Missing(why) = self.core.export_encoder()
        {
            self.notify(format!("{} needs FFmpeg. {why}", preset.name));
            return Vec::new();
        }
        let film = self.preset_is_film(&preset);
        let scope = if preset.stems {
            ExportScope::Stems
        } else {
            ExportScope::Song
        };
        if !film && let Some(why) = self.core.export_audio_refusal(scope) {
            self.notify(why);
            return Vec::new();
        }
        self.export_run = Some(preset);
        vec![if film {
            ShellRequest::PickExportFilm
        } else {
            ShellRequest::PickExportAudio(scope)
        }]
    }

    /// File > Export Song / Export Loop as Audio File: asks for the file unless the export is
    /// refused (an export runs, or the range is empty), which is said in the toast.
    pub(super) fn export_audio_request(&mut self, scope: ExportScope) -> Vec<ShellRequest> {
        self.export_run = None;
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
        file_extension(&self.export_values_now())
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

    /// A panel for exporting `scope` to the file called `name` with `values`, with the presets.
    fn export_view_for(
        &mut self,
        scope: ExportScope,
        name: &str,
        values: ExportValues,
    ) -> ExportView {
        let mut view = ExportView::new(self.core.view().theme().clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        // MP3, AAC and Opus need FFmpeg: look for it now (unless that was done) so the panel
        // knows whether to offer them.
        self.core.refresh_ffmpeg_status();
        view.set_encoder(self.core.export_encoder());
        view.open(scope, name, self.core.project_sample_rate(), values);
        view.set_presets(self.export_presets.all());
        view
    }

    /// The file dialog picked `path`: shows the options for exporting `scope` to it, or, when
    /// the file was asked for by File > Export for, starts the preset's export at once.
    pub fn export_audio_to(&mut self, scope: ExportScope, path: &Path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if let Some(preset) = self.export_run.take() {
            self.run_preset(scope, path, &preset);
            return;
        }
        let values = self.export_prefs.values();
        let view = self.export_view_for(scope, &name, values);
        self.export = Some(view);
        self.export_target = Some((scope, path.to_path_buf()));
        self.export_replace = None;
        self.repaint = true;
    }

    /// File > Export for: starts the export of `preset` to `path` with the progress bar, without
    /// the options. A file the preset would write besides the one the dialog asked about is not
    /// replaced unasked.
    fn run_preset(&mut self, scope: ExportScope, path: &Path, preset: &PresetChoice) {
        let values = preset.values;
        let file = self.core.export_file_for(scope, path, &values);
        let clash = [
            (file != path).then(|| file.clone()),
            extra_file(scope, &file, &values),
        ]
        .into_iter()
        .flatten()
        .find(|f| f.exists());
        if let Some(clash) = clash {
            let name = clash
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            self.notify(format!("{name} already exists: pick another name"));
            return;
        }
        let name = file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut view = self.export_view_for(scope, &name, values);
        view.choose_preset(&preset.name);
        if self
            .core
            .start_audio_export(scope, path.to_path_buf(), values)
            .is_ok()
        {
            view.show_progress();
            self.export = Some(view);
            self.export_target = Some((scope, path.to_path_buf()));
            self.export_replace = None;
            self.export_prefs.folder = path.parent().map(|f| f.to_string_lossy().into_owned());
            self.save_export_prefs();
            self.sync_export();
        }
        self.repaint = true;
    }

    /// Saves the user's presets next to the settings; a failure is a toast.
    fn save_export_presets(&mut self) {
        if let Some(path) = &self.export_presets_path
            && let Err(e) = self.export_presets.save(path)
        {
            self.notify(format!("cannot save the export presets: {e}"));
        }
    }

    /// Save Preset: keeps the panel's choices under the name typed.
    fn save_preset(&mut self) {
        let Some(view) = &self.export else {
            return;
        };
        let name = view.preset_name_entry().unwrap_or_default().to_owned();
        let (stems, values) = (view.scope() == ExportScope::Stems, view.values());
        match self.export_presets.add(&name, stems, values) {
            Ok(kept) => {
                self.save_export_presets();
                let all = self.export_presets.all();
                if let Some(view) = &mut self.export {
                    view.set_presets(all);
                    view.preset_saved(&kept);
                }
                self.notify(format!("Saved the export preset {kept}"));
            }
            Err(why) => {
                if let Some(view) = &mut self.export {
                    view.set_warning(Some(why));
                }
            }
        }
    }

    /// Delete Preset: removes the user preset the panel shows.
    fn delete_preset(&mut self) {
        let Some(name) = self
            .export
            .as_ref()
            .and_then(|v| v.preset().map(str::to_owned))
        else {
            return;
        };
        if self.export_presets.delete(&name) {
            self.save_export_presets();
            let all = self.export_presets.all();
            if let Some(view) = &mut self.export {
                view.set_presets(all);
            }
            self.notify(format!("Deleted the export preset {name}"));
        }
    }

    /// What the panel asked for.
    pub(super) fn export_action(&mut self, action: ExportAction) {
        match action {
            ExportAction::SavePreset => self.save_preset(),
            ExportAction::DeletePreset => self.delete_preset(),
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
                // A preset of stems turns a file into a folder of stems (and back).
                let now = self.export.as_ref().map_or(scope, ExportView::scope);
                let (scope, path) = if now == scope {
                    (scope, path)
                } else {
                    let moved = retarget(now, &path);
                    self.export_target = Some((now, moved.clone()));
                    self.export_replace = None;
                    (now, moved)
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

/// Where an export goes when the panel turned from a file into stems or back: the folder
/// `Bounced <name>` beside the file, or a file of that name beside the folder.
pub(super) fn retarget(scope: ExportScope, path: &Path) -> PathBuf {
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = path
        .file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if scope == ExportScope::Stems {
        parent.join(atomik_render::stems::stems_folder_name(&name))
    } else {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let song = name.strip_prefix("Bounced ").unwrap_or(&name);
        parent.join(song)
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
