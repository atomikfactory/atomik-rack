//! Tab files in the app core: File > Import Tab or Score File, File > Import Chord Sheet and the
//! tab, text and chord sheet exports.
//!
//! The conversion between files and the song is in [`crate::tab_file`]; this module reads and
//! writes the files, picks what to export and applies an import as one undo step.
//!
//! **Import Tab or Score File** reads a Guitar Pro file (`.gp3` to `.gp5`, `.gpx`, `.gp`) or a
//! MusicXML file (`.musicxml`, `.xml`, `.mxl`; the first bytes decide, not the extension) and makes
//! one new track per tab track at the snapped playhead, each with one note clip that carries the
//! tab (string, fret, techniques, tuning, capo, chord diagrams, lyrics). The song's tempo and
//! meter are untouched. **Import Tab or Score as Song** makes the tracks from tick 0 and also
//! takes the file's tempo map, meter map, section markers, chords and key, as Import MIDI as Song
//! does. A tab file dropped on the window, or opened from the file browser, is the first form.
//! What the file holds that the song cannot (an unknown element, a note beyond the neck) is
//! listed after the message.
//!
//! **Import Chord Sheet** reads a text file (chords over lyrics, or the bracket form with
//! directives; the form is detected) into the song's chord sheets as one undo step.
//!
//! **Export** writes the selected note clip, or the whole song when no clip is selected (the
//! scope can also be named: [`AppCore::export_tab_of`]): *Tab as MusicXML* (a `.mxl` file name
//! writes the compressed form), *Tab as Text* (ASCII tab), and *Chord Sheet as Text*, which
//! writes the first chord sheet of the song.

use super::midi_ops::{ImportMode, Place, plural};
use super::*;
use crate::tab_file::{self, TabSource, read_tab, score_from_song, tab_song};
use atomik_tab::ascii::{AsciiWriteOptions, write_ascii};
use atomik_tab::sheet::parse_sheet;

/// The biggest chord sheet text read.
const MAX_SHEET_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// What a tab export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabExportFormat {
    /// MusicXML (`.musicxml`, or the compressed `.mxl` when the file name says so).
    MusicXml,
    /// Plain-text (ASCII) tab.
    Text,
}

impl TabExportFormat {
    /// The word for messages.
    pub fn word(self) -> &'static str {
        match self {
            TabExportFormat::MusicXml => "MusicXML",
            TabExportFormat::Text => "text tab",
        }
    }
}

/// The message with up to three warnings after it.
fn with_warnings(mut message: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return message;
    }
    message.push_str(" \u{b7} ");
    message.push_str(
        &warnings
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" \u{b7} "),
    );
    if warnings.len() > 3 {
        message.push_str(&format!(
            " \u{b7} and {} more warning{}",
            warnings.len() - 3,
            plural(warnings.len() - 3)
        ));
    }
    message
}

fn short_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ import

    /// Reads the tab file at `path` and imports it (see the module docs): `as_song` also takes
    /// the tempo map, the meter map, the markers, the chords and the key and starts at tick 0;
    /// otherwise the new tracks start at the snapped playhead. What cannot be imported is
    /// reported by name and nothing changes.
    pub fn import_tab(&mut self, path: &Path, as_song: bool) {
        let name = short_name(path);
        let result = read_tab_file(path)
            .and_then(|(score, warnings)| self.apply_tab(&score, warnings, as_song, &name));
        if let Err(e) = result {
            self.report(format!("{name}: {e}"));
        }
    }

    /// Imports the tab file at `path` as new tracks at the snapped playhead (what a tab file
    /// dropped on the window or opened in the file browser does).
    pub fn import_tab_tracks(&mut self, path: &Path) {
        self.import_tab(path, false);
    }

    fn apply_tab(
        &mut self,
        score: &atomik_tab::score::TabScore,
        mut warnings: Vec<String>,
        as_song: bool,
        name: &str,
    ) -> Result<(), String> {
        let tab = tab_song(score);
        if tab.song.tracks.is_empty() {
            return Err("the file has no tracks".to_owned());
        }
        warnings.extend(tab.warnings);
        let mode = if as_song {
            ImportMode::Song
        } else {
            ImportMode::Tracks
        };
        let message =
            self.apply_midi_with(tab.instrument, tab.song, mode, Place::default(), name)?;
        self.report(with_warnings(message, &warnings));
        Ok(())
    }

    /// Reads the chord sheet at `path` (chords over lyrics or bracket form, detected) and adds it
    /// to the song's chord sheets, one undo step. A file that cannot be read or holds no sheet is
    /// reported and nothing changes.
    pub fn import_sheet(&mut self, path: &Path) {
        let name = short_name(path);
        if let Err(e) = self.add_sheet_file(path, &name) {
            self.report(format!("{name}: {e}"));
        }
    }

    fn add_sheet_file(&mut self, path: &Path, name: &str) -> Result<(), String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_SHEET_FILE_BYTES {
            return Err(format!(
                "the file is {size} bytes, too large for a chord sheet"
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse_sheet(&text).map_err(|e| e.to_string())?;
        let mut sheet = parsed.sheet;
        if sheet.title.trim().is_empty() {
            sheet.title = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        }
        let chords = sheet.chords_used().len();
        let lines = sheet.blocks.len();
        let sheets = atomik_model::chord_sheets::with_sheet(&self.doc().song.sheets, sheet)
            .ok_or("the song already holds as many chord sheets as it can")?;
        self.apply_command(Command::Song(SongCommand::SetSheets(sheets)))
            .map_err(|e| e.to_string())?;
        self.report(with_warnings(
            format!(
                "Imported the chord sheet {name} ({lines} line{}, {chords} different chord{})",
                plural(lines),
                plural(chords)
            ),
            &parsed.warnings,
        ));
        Ok(())
    }

    // ------------------------------------------------------------------ export

    /// What File > Export Tab writes: the selected note clip, else the whole song (`None` when
    /// the song has no notes to write).
    pub fn tab_export_scope(&self) -> Option<MidiExport> {
        if self.can_export_midi(MidiExport::Clip) {
            Some(MidiExport::Clip)
        } else if self.can_export_midi(MidiExport::Song) {
            Some(MidiExport::Song)
        } else {
            None
        }
    }

    /// True when File > Export Tab can write something.
    pub fn can_export_tab(&self) -> bool {
        self.tab_export_scope().is_some()
    }

    /// The file name the tab export dialog offers (without the extension).
    pub fn tab_export_name(&self) -> String {
        self.midi_export_name(self.tab_export_scope().unwrap_or(MidiExport::Song))
    }

    /// True when the song has a chord sheet to export.
    pub fn can_export_sheet(&self) -> bool {
        !self.doc().song.sheets.is_empty()
    }

    fn tab_source(&self, what: MidiExport) -> Result<TabSource, String> {
        match what {
            MidiExport::Clip => self
                .midi_clip_source()
                .map(TabSource::Clip)
                .ok_or_else(|| what.needs().to_owned()),
            MidiExport::Track => self
                .midi_track_source()
                .map(TabSource::Track)
                .ok_or_else(|| what.needs().to_owned()),
            MidiExport::Song => Ok(TabSource::Song),
        }
    }

    /// Writes the tab of the default scope ([`Self::tab_export_scope`]) to `path` in `format` and
    /// says so (or why not) in the view.
    pub fn export_tab(&mut self, format: TabExportFormat, path: &Path) {
        let what = self.tab_export_scope().unwrap_or(MidiExport::Song);
        self.export_tab_of(what, format, path);
    }

    /// [`Self::export_tab`] for an explicit scope.
    pub fn export_tab_of(&mut self, what: MidiExport, format: TabExportFormat, path: &Path) {
        let result = self
            .tab_bytes(what, format, path)
            .and_then(|(bytes, warnings)| {
                std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(with_warnings(
                    format!(
                        "Exported the {} as {} to {}",
                        what.word(),
                        format.word(),
                        short_name(path)
                    ),
                    &warnings,
                ))
            });
        self.report(result.unwrap_or_else(|e| e));
    }

    fn tab_bytes(
        &self,
        what: MidiExport,
        format: TabExportFormat,
        path: &Path,
    ) -> Result<(Vec<u8>, Vec<String>), String> {
        let source = self.tab_source(what)?;
        let (score, mut warnings) =
            score_from_song(&self.doc().song, &source, &self.project_name(), &|k| {
                self.device_name(k)
            })?;
        let lines = |w: Vec<atomik_tab::ImportWarning>| {
            w.iter().map(ToString::to_string).collect::<Vec<_>>()
        };
        let bytes = match format {
            TabExportFormat::Text => {
                write_ascii(&score, &AsciiWriteOptions::default()).into_bytes()
            }
            TabExportFormat::MusicXml => {
                let compressed = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mxl"));
                if compressed {
                    let (bytes, w) =
                        atomik_tab::musicxml::write_mxl(&score).map_err(|e| e.to_string())?;
                    warnings.extend(lines(w));
                    bytes
                } else {
                    let (text, w) =
                        atomik_tab::musicxml::write_musicxml(&score).map_err(|e| e.to_string())?;
                    warnings.extend(lines(w));
                    text.into_bytes()
                }
            }
        };
        Ok((bytes, warnings))
    }

    /// Writes the song's first chord sheet to `path` as chords over lyrics.
    pub fn export_sheet(&mut self, path: &Path) {
        let message = match self.doc().song.sheets.first() {
            None => "the song has no chord sheet to export".to_owned(),
            Some(sheet) => {
                let text = sheet.to_over_lyrics();
                match std::fs::write(path, text) {
                    Ok(()) => format!("Exported the chord sheet to {}", short_name(path)),
                    Err(e) => format!("{}: {e}", path.display()),
                }
            }
        };
        self.report(message);
    }
}

/// Reads and converts the file at `path`.
fn read_tab_file(path: &Path) -> Result<(atomik_tab::score::TabScore, Vec<String>), String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > tab_file::MAX_TAB_BYTES as u64 {
        return Err(format!(
            "the file is {size} bytes, too large for a tab file"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    read_tab(&bytes)
}

#[cfg(test)]
#[path = "app_core_tab_tests.rs"]
mod tests;
