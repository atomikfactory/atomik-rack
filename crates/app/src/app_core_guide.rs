//! What the start screen and the guided first song need from the app: the example projects of the
//! factory folder, and the facts the guide watches ([`crate::guide::Progress`]).
//!
//! * [`AppCore::examples`] lists the projects of the factory folder's `Projects` section (each a
//!   folder `<name>/<name>.atomik`), with a line saying what each one is for. They open like the
//!   templates do ([`AppCore::open_template`]): as a new unsaved song, so the factory copy is
//!   never the song's file.
//! * [`AppCore::guide_progress`] is what the guide reads each frame.

use super::*;
use crate::guide::Progress;
use atomik_library::LibrarySection;

/// The most examples the start screen shows as cards.
pub const MAX_EXAMPLES: usize = 3;

/// What each factory example is about, in a line that fits a card. An example that is not here
/// gets its tempo and track count instead ([`example_line`]).
const EXAMPLE_LINES: &[(&str, &str)] = &[
    (
        "Basalt Beat",
        "A slow broken beat with a sampled kit, a sub and a mallet hook.",
    ),
    (
        "Moonrise Modular",
        "Drifting arpeggios and a slow sweep on the semi-modular synth.",
    ),
    (
        "Bus Room",
        "A small mix: buses, groups, sends and automation to open up.",
    ),
];

/// One example project of the factory folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleEntry {
    /// The project's name (its folder and file name).
    pub name: String,
    /// The project file.
    pub path: PathBuf,
    /// What it is for, in one line.
    pub line: String,
}

/// The line the card of example `name` shows, when the table has one.
pub(super) fn known_line(name: &str) -> Option<&'static str> {
    EXAMPLE_LINES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, line)| *line)
}

impl<S: EngineSink> AppCore<S> {
    /// The example projects of the factory folder, by name (at most [`MAX_EXAMPLES`]). Empty when
    /// the application has no factory folder or it has no projects.
    pub fn examples(&self) -> Vec<ExampleEntry> {
        let Some(folder) = self.factory_folder(LibrarySection::Projects) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&folder) else {
            return Vec::new();
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let file = e.path().join(format!("{name}.atomik"));
                file.is_file().then_some((name, file))
            })
            .collect();
        found.sort();
        found.truncate(MAX_EXAMPLES);
        found
            .into_iter()
            .map(|(name, path)| {
                let line = known_line(&name)
                    .map(str::to_owned)
                    .or_else(|| self.template_preview(&path).map(|p| preview_line(&p)))
                    .unwrap_or_default();
                ExampleEntry { name, path, line }
            })
            .collect()
    }

    /// How many exports finished since this song was opened.
    pub fn exports_finished(&self) -> u32 {
        self.audio.export.finished()
    }

    /// What the guided first song sees of the song now.
    pub fn guide_progress(&self) -> Progress {
        Progress::read(
            self.doc(),
            &self.registry,
            self.file().is_some(),
            self.exports_finished(),
        )
    }
}

/// "96 BPM, 5 tracks": the line of an example the table does not know.
fn preview_line(p: &TemplatePreview) -> String {
    let tempo = if p.tempo_bpm.fract().abs() < 0.005 {
        format!("{:.0}", p.tempo_bpm)
    } else {
        format!("{:.2}", p.tempo_bpm)
    };
    match p.tracks.len() {
        1 => format!("{tempo} BPM, 1 track"),
        n => format!("{tempo} BPM, {n} tracks"),
    }
}

#[cfg(test)]
#[path = "app_core_guide_tests.rs"]
mod tests;
