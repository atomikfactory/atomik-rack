//! File > Export > Film with Mix in the shell.
//!
//! The flow is the one of the audio export ([`crate::shell_export`]): the command asks the
//! platform for a file ([`ShellRequest::PickExportFilm`], a native save dialog named
//! `<project>.mp4`), the platform calls [`Shell::export_film_to`] with the path, and the shell
//! shows the options panel ([`FilmView`]: what the film covers, what happens to the picture, the
//! sound and the film's own audio). EXPORT starts the job on the export worker
//! ([`AppCore::start_film_export`]), remembers the choices in `export.json` and turns the panel
//! into a progress bar with CANCEL; the panel closes when the job is over (the result is the
//! toast). The command is refused with a toast when no export can start (see
//! [`AppCore::film_export_refusal`]: no video in the song, the film's file missing, no FFmpeg).
//! The sound decides the container (AAC: MP4, PCM: MOV), so the file may get another extension
//! than the dialog picked: a file at that name is not replaced until EXPORT is pressed twice.

use super::export_impl::replace_warning;
use super::*;
use crate::export_prefs::ExportPrefs;
use atomik_ui::{FilmAction, FilmValues, FilmView};

impl<B: AudioBackend> Shell<B> {
    /// Whether the film export panel (options or progress) is open.
    pub fn film_open(&self) -> bool {
        self.film.is_some()
    }

    /// The film export panel, while it is open.
    pub fn film_view(&self) -> Option<&FilmView> {
        self.film.as_ref()
    }

    /// The choices the panel starts with (the last ones, from `export.json`).
    pub fn film_values(&self) -> FilmValues {
        self.export_prefs.film_values()
    }

    /// File > Export > Film with Mix: asks for the file unless the export is refused (the toast
    /// says why).
    pub(super) fn export_film_request(&mut self) -> Vec<ShellRequest> {
        match self.core.film_export_refusal() {
            None => vec![ShellRequest::PickExportFilm],
            Some(why) => {
                self.notify(why);
                Vec::new()
            }
        }
    }

    /// The file name the dialog proposes: the project's name with the extension of the sound
    /// chosen last (`.mp4` for AAC, `.mov` for PCM).
    pub fn export_film_name(&self) -> String {
        let stem = self.core.file().and_then(|p| p.file_stem()).map_or_else(
            || "project".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        format!("{stem}.{}", self.film_values().sound.extension())
    }

    /// The file dialog picked `path`: shows the options for exporting the film to it.
    pub fn export_film_to(&mut self, path: &Path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut view = FilmView::new(self.core.view().theme().clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        // The film is made with FFmpeg: look for it now (unless that was done) so the panel knows
        // whether EXPORT can start.
        self.core.refresh_ffmpeg_status();
        view.set_encoder(self.core.film_encoder());
        view.open(&name, self.core.film_has_audio(), self.film_values());
        self.film = Some(view);
        self.film_target = Some(path.to_path_buf());
        self.export_replace = None;
        self.repaint = true;
    }

    /// What the panel asked for.
    pub(super) fn film_action(&mut self, action: FilmAction) {
        match action {
            FilmAction::Cancel => {
                if self.film.as_ref().is_some_and(FilmView::in_progress) {
                    // The panel stays until the worker has stopped (`sync_film`).
                    self.core.cancel_audio_export();
                } else {
                    self.film = None;
                    self.film_target = None;
                }
            }
            FilmAction::Start(values) => {
                let Some(path) = self.film_target.clone() else {
                    self.film = None;
                    return;
                };
                // The sound decides the extension: a file at that name that the dialog did not
                // ask about is replaced only after a second EXPORT.
                let file = self.core.film_file_for(&path, &values);
                if file != path && file.exists() && self.export_replace.as_ref() != Some(&file) {
                    let warning = replace_warning(&file, "sound");
                    self.export_replace = Some(file);
                    if let Some(view) = &mut self.film {
                        view.set_warning(Some(warning));
                    }
                    self.repaint = true;
                    return;
                }
                match self.core.start_film_export(path.clone(), values) {
                    Ok(()) => {
                        let mut prefs = self.export_prefs.clone().with_film(&values);
                        prefs.folder = path.parent().map(|f| f.to_string_lossy().into_owned());
                        self.export_prefs = prefs;
                        self.save_export_prefs();
                        if let Some(view) = &mut self.film {
                            view.show_progress();
                        }
                        self.sync_film();
                    }
                    // The toast says why.
                    Err(_) => {
                        self.film = None;
                        self.film_target = None;
                    }
                }
            }
        }
        self.repaint = true;
    }

    /// Writes the remembered export choices next to the settings.
    fn save_export_prefs(&mut self) {
        if let Some(prefs_path) = &self.export_prefs_path
            && let Err(e) = ExportPrefs::save(&self.export_prefs, prefs_path)
        {
            self.notify(format!("cannot save the export choices: {e}"));
        }
    }

    /// Feeds the progress bar, and closes the panel when the job is over.
    pub(super) fn sync_film(&mut self) {
        let encoder = self.core.film_encoder();
        let Some(view) = &mut self.film else {
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
                self.film = None;
                self.film_target = None;
                self.repaint = true;
            }
        }
    }
}
