//! MIDI files in the app core: File > Import MIDI and File > Export MIDI (clip, track, song).
//!
//! The conversion between files and the model is in [`crate::midi_file`]; this module picks what
//! to export, reads and writes the files and applies an import as one undo step.
//!
//! **Export** writes the note clip open in the piano roll or selected in the arrangement
//! (type 0), the selected note track (type 0 with the song's tempo map, meter and markers) or
//! the whole song (type 1). The command is disabled when there is nothing to write.
//!
//! **Import** has two forms:
//! - *Import MIDI* puts the whole file in **one clip** on the selected note track at the snapped
//!   playhead (all MIDI tracks and channels merged, controllers included, the song's tempo and
//!   meter untouched). With no note track selected, it makes **new tracks** instead, one per
//!   MIDI track and channel with notes, each with a poly synth, all at the playhead.
//! - *Import MIDI as Song* always makes new tracks, from tick 0, and also takes the file's
//!   tempo map and meter map (they replace the song's) and its markers (added to the song's).
//!
//! A MIDI file dropped on the window always makes new tracks at the playhead (the selection does
//! not matter). All three are one undo step and select the new clips. A file that cannot be read
//! is reported by name; nothing changes then.

use super::*;
use crate::midi_file::{self, MidiSong, MidiTrack};
use atomik_model::{ClipContent, ImportedTrack, MidiImport, WorkflowCommand};

/// The instrument a track made by a MIDI import plays.
pub const IMPORT_INSTRUMENT: &str = "atomik.poly-synth";

/// The biggest MIDI file read (a real file is a few hundred kilobytes at most).
const MAX_MIDI_BYTES: u64 = 64 * 1024 * 1024;

/// What File > Export MIDI writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiExport {
    /// The selected note clip.
    Clip,
    /// The selected note track.
    Track,
    /// The whole song.
    Song,
}

impl MidiExport {
    /// What the user has to do first when there is nothing to export.
    pub fn needs(self) -> &'static str {
        match self {
            MidiExport::Clip => "select a note clip to export",
            MidiExport::Track => "select a note track to export",
            MidiExport::Song => "the song has no note tracks to export",
        }
    }

    /// The word for messages and file names.
    pub fn word(self) -> &'static str {
        match self {
            MidiExport::Clip => "clip",
            MidiExport::Track => "track",
            MidiExport::Song => "song",
        }
    }
}

/// Where an import puts the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportMode {
    /// One clip on the selected note track, else new tracks.
    Auto,
    /// New tracks at the playhead.
    Tracks,
    /// New tracks from the start, with the tempo map, meter map and markers.
    Song,
}

impl<S: EngineSink> AppCore<S> {
    fn is_note_track(&self, device: &DeviceKey) -> bool {
        self.doc()
            .song
            .track(device)
            .is_some_and(|t| t.lanes.first().is_some_and(|l| l.kind.is_notes()))
    }

    /// The note clip File > Export MIDI Clip writes: the first note clip selected in the
    /// arrangement, else the clip open in the piano roll.
    pub fn midi_clip_source(&self) -> Option<ClipId> {
        let song = &self.doc().song;
        let note_clip = |id: &ClipId| {
            song.clip(*id)
                .is_some_and(|(_, _, c)| matches!(c.content, ClipContent::Notes(_)))
        };
        self.arrangement
            .selected_clips()
            .into_iter()
            .find(note_clip)
            .or_else(|| self.piano_clip.filter(note_clip))
    }

    /// The note track File > Export MIDI Track writes and File > Import MIDI fills: the track
    /// selected in the arrangement, else the track of a selected or open clip, else the
    /// selected device when it has a note track.
    pub fn midi_track_source(&self) -> Option<DeviceKey> {
        let song = &self.doc().song;
        let of_clip = |id: ClipId| song.clip(id).map(|(t, _, _)| t.device.clone());
        self.arrangement
            .selected_track()
            .cloned()
            .or_else(|| {
                self.arrangement
                    .selected_clips()
                    .into_iter()
                    .find_map(of_clip)
            })
            .or_else(|| self.piano_clip.and_then(of_clip))
            .filter(|d| self.is_note_track(d))
            .or_else(|| {
                self.view
                    .selected()
                    .cloned()
                    .filter(|d| self.is_note_track(d))
            })
    }

    /// True when File > Export MIDI can write `what`.
    pub fn can_export_midi(&self, what: MidiExport) -> bool {
        match what {
            MidiExport::Clip => self.midi_clip_source().is_some(),
            MidiExport::Track => self.midi_track_source().is_some(),
            MidiExport::Song => self
                .doc()
                .song
                .tracks
                .iter()
                .any(|t| self.is_note_track(&t.device)),
        }
    }

    fn device_name(&self, key: &DeviceKey) -> String {
        self.doc()
            .device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    }

    fn project_name(&self) -> String {
        self.file
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| "Song".to_owned(), |n| n.to_string_lossy().into_owned())
    }

    /// The file name the export dialog offers (without the extension).
    pub fn midi_export_name(&self, what: MidiExport) -> String {
        let name = match what {
            MidiExport::Clip => self
                .midi_clip_source()
                .and_then(|id| self.doc().song.clip(id))
                .map(|(t, _, _)| self.device_name(&t.device)),
            MidiExport::Track => self.midi_track_source().map(|d| self.device_name(&d)),
            MidiExport::Song => None,
        };
        name.unwrap_or_else(|| self.project_name())
    }

    /// The bytes of the MIDI file for `what`, or why there are none.
    fn midi_bytes(&self, what: MidiExport) -> Result<Vec<u8>, String> {
        let song = &self.doc().song;
        match what {
            MidiExport::Clip => {
                let id = self.midi_clip_source().ok_or(what.needs())?;
                let (track, _, clip) = song.clip(id).ok_or("the clip is gone")?;
                Ok(midi_file::clip_file(
                    song,
                    clip,
                    &self.device_name(&track.device),
                ))
            }
            MidiExport::Track => {
                let device = self.midi_track_source().ok_or(what.needs())?;
                let track = song.track(&device).ok_or("the track is gone")?;
                Ok(midi_file::track_file(
                    song,
                    track,
                    &self.device_name(&device),
                ))
            }
            MidiExport::Song => {
                if !self.can_export_midi(MidiExport::Song) {
                    return Err(what.needs().to_owned());
                }
                Ok(midi_file::song_file(song, &self.project_name(), &|k| {
                    self.device_name(k)
                }))
            }
        }
    }

    /// Writes the MIDI file for `what` to `path` and says so (or why not) in the view.
    pub fn export_midi(&mut self, what: MidiExport, path: &Path) {
        let message = match self.midi_bytes(what).and_then(|bytes| {
            std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
        }) {
            Ok(()) => format!(
                "Exported the {} to {}",
                what.word(),
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            ),
            Err(e) => e,
        };
        self.report(message);
    }

    /// Reads the file at `path` and imports it (see the module docs): `as_song` also takes the
    /// tempo map, the meter map and the markers and always makes new tracks.
    pub fn import_midi(&mut self, path: &Path, as_song: bool) {
        let mode = if as_song {
            ImportMode::Song
        } else {
            ImportMode::Auto
        };
        self.import_midi_as(path, mode);
    }

    /// Reads the file at `path` and always makes new tracks from it, at the snapped playhead,
    /// whatever is selected (what a MIDI file dropped on the window does).
    pub fn import_midi_tracks(&mut self, path: &Path) {
        self.import_midi_as(path, ImportMode::Tracks);
    }

    fn import_midi_as(&mut self, path: &Path, mode: ImportMode) {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let result = read_midi(path).and_then(|song| self.apply_midi(song, mode, &name));
        if let Err(e) = result {
            self.report(format!("{name}: {e}"));
        }
    }

    fn apply_midi(&mut self, midi: MidiSong, mode: ImportMode, name: &str) -> Result<(), String> {
        let as_song = mode == ImportMode::Song;
        let playhead = self.snapped_playhead();
        let target = if mode == ImportMode::Auto {
            self.midi_track_source()
        } else {
            None
        };
        let notes: usize = midi.tracks.iter().map(|t| t.notes.len()).sum();
        let left_out = match midi.skipped {
            0 => String::new(),
            n => format!("; {n} channel{} with only controllers left out", plural(n)),
        };
        if let Some(device) = target {
            let clip = midi.merged().into_clip(playhead, &self.doc().song);
            // The note lane picked in the arrangement (the first one without a pick).
            let lane = self.arrangement.record_lane(&device);
            let add = SongCommand::AddClip { device, lane, clip };
            self.apply_command(Command::Song(add))
                .map_err(|e| e.to_string())?;
            let id = self.document.last_clip_id();
            self.after_view_change();
            self.arrangement
                .set_selected_clips(&id.into_iter().collect::<Vec<_>>());
            self.report(format!(
                "Imported {notes} notes from {name} as one clip{left_out}"
            ));
            return Ok(());
        }
        let start = if as_song { 0 } else { playhead };
        // A meter that is going to replace the song's decides the bar a short track rounds to.
        let mut scratch = self.doc().song.clone();
        if as_song && let Some(meter) = &midi.meter {
            scratch.meter.clone_from(meter);
        }
        let count = midi.tracks.len();
        let tracks: Vec<ImportedTrack> = midi
            .tracks
            .into_iter()
            .map(|t: MidiTrack| ImportedTrack {
                name: t.name.clone(),
                clip: t.into_clip(start, &scratch),
            })
            .collect();
        let import = MidiImport {
            instrument: IMPORT_INSTRUMENT.to_owned(),
            tracks,
            tempo: midi.tempo.filter(|_| as_song),
            meter: midi.meter.filter(|_| as_song),
            markers: if as_song { midi.markers } else { Vec::new() },
        };
        let song_data = if as_song {
            let tempo = import.tempo.as_ref().map_or(0, Vec::len);
            let markers = import.markers.len();
            format!(
                " with {tempo} tempo event{} and {markers} marker{}",
                plural(tempo),
                plural(markers)
            )
        } else {
            String::new()
        };
        self.apply_command(Command::Workflow(WorkflowCommand::ImportMidi(import)))
            .map_err(|e| e.to_string())?;
        self.report(format!(
            "Imported {count} track{} ({notes} notes) from {name}{song_data}{left_out}",
            plural(count)
        ));
        Ok(())
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Reads and converts the file at `path`.
fn read_midi(path: &Path) -> Result<MidiSong, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_MIDI_BYTES {
        return Err(format!(
            "the file is {size} bytes, too large for a MIDI file"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    midi_file::read_song(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "app_core_midi_tests.rs"]
mod tests;
