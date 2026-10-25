//! Audio to Notes in the app core: notes extracted from the audio of a clip, as a note clip.
//!
//! The Audio to Notes panel (Audio menu) picks what to look for ([`NotesValues`]): *Pitched*
//! follows one voice or instrument and cuts it into notes ([`atomik_dsp::notes::pitched`]), *Drums*
//! finds the hits of a drum loop and sorts them into kick, snare and hats on the General MIDI keys
//! ([`atomik_dsp::notes::drums`]).
//!
//! **What is read.** The first selected audio clip, the span of its asset that it plays (from the
//! clip's offset; for a warped clip the span the time map reads), mixed to mono, and in play order
//! for a reversed clip. Assets that stream from disk are read through the stream source in
//! chunks, like strip silence does. The analysis runs on a worker thread (`atomik-notes`) and
//! reads at most [`MAX_NOTES_SECONDS`] of the clip (the message says when it stopped early), so a
//! multi-minute clip never stops the UI; the song keeps playing meanwhile and one extraction runs
//! at a time.
//!
//! **Warped clips.** A clip warped in Beats or Tonal mode, and a clip that is only transposed, is
//! read from its source and its notes go where the clip plays them (through the warp time map,
//! see [`atomik_model::notes_clip`]); the transposition is added to the keys. A clip in Repitch
//! mode is refused with a message: its pitch follows the tempo, which the analysis cannot see.
//!
//! **Where the notes go.** Always one note clip that starts and ends with the audio clip: on a
//! **new track** (a poly synth, named after the audio track, through `ImportMidi`) or on the
//! **selected note track** (`AddClip`, lane 0; the instrument selected in the rack). It is one undo step and the new clip is selected.
//! Drum notes land on the keys 36, 38, 42 and 46 whatever the instrument: point the track at a
//! drum machine with samples on those pads to hear them. A result is dropped, with a message, when
//! the audio clip changed or the project closed while the worker ran.

use super::audio_edit_ops::for_each_chunk;
use super::*;
use crate::audio_import::{NotesFound, spawn_notes};
use atomik_core::PPQ;
use atomik_dsp::notes::{DrumParams, PitchParams, extract_drum_notes, extract_pitched_notes};
use atomik_engine::SongAsset;
use atomik_model::{
    AudioClipContent, Clip, ClipContent, FoundNote, ImportedTrack, MidiImport, NoteOptions,
    WarpMode, WorkflowCommand, notes_clip,
};
use atomik_ui::{NotesMode, NotesValues};

/// The longest stretch of a clip the analysis reads, in seconds.
pub const MAX_NOTES_SECONDS: u64 = 600;

/// What a running extraction was asked for, to check it against the document when it is done.
struct Run {
    id: u64,
    clip: ClipId,
    start: i64,
    length: i64,
    content: AudioClipContent,
    values: NotesValues,
    /// The note track the notes go on (`None`: a new track).
    target: Option<DeviceKey>,
    /// The name of the audio track, for the name of a new one.
    name: String,
}

/// The Audio to Notes state of an open project.
#[derive(Default)]
pub(super) struct NotesState {
    running: Option<Run>,
    next: u64,
}

impl NotesState {
    /// Forgets the running extraction (the project changed; its result is dropped by the
    /// generation).
    pub(super) fn reset(&mut self) {
        self.running = None;
    }
}

/// The mono mix of the frames `[from, from + n)` of `asset`, in play order when `reverse`.
fn read_mono(asset: &SongAsset, from: u64, n: u64, reverse: bool) -> Vec<f32> {
    let mut mono: Vec<f32> = Vec::with_capacity(n as usize);
    for_each_chunk(asset, from, from + n, |planes| {
        let len = planes.iter().map(|p| p.len()).min().unwrap_or(0);
        let inv = 1.0 / planes.len().max(1) as f64;
        for i in 0..len {
            let mut sum = 0.0f64;
            for p in planes {
                if p[i].is_finite() {
                    sum += f64::from(p[i]);
                }
            }
            mono.push((sum * inv) as f32);
        }
    });
    if reverse {
        mono.reverse();
    }
    mono
}

/// The analysis of one clip (what the worker runs): `n` frames of `asset` from `from`.
fn analyse(
    asset: &SongAsset,
    (from, n): (u64, u64),
    reverse: bool,
    (rate, cap_seconds): (u32, u64),
    v: &NotesValues,
) -> NotesFound {
    let cap = u64::from(rate) * cap_seconds;
    let (take, truncated) = if n > cap { (cap, true) } else { (n, false) };
    // A reversed clip plays the end of its span first.
    let from = if reverse { from + n - take } else { from };
    let mono = read_mono(asset, from, take, reverse);
    let sensitivity = v.sensitivity.min(100) as f32 / 100.0;
    let found = match v.mode {
        NotesMode::Pitched => extract_pitched_notes(
            &[&mono],
            rate,
            &PitchParams {
                sensitivity,
                min_note_ms: v.min_note_ms,
                key_lo: v.key_lo,
                key_hi: v.key_hi,
                tolerance_cents: v.drift_cents,
            },
        ),
        NotesMode::Drums => extract_drum_notes(
            &[&mono],
            rate,
            &DrumParams {
                sensitivity,
                open_hats: v.open_hats,
            },
        ),
    };
    NotesFound {
        notes: found
            .into_iter()
            .map(|n| FoundNote {
                start: n.start,
                length: n.length,
                key: n.key,
                velocity: n.velocity,
            })
            .collect(),
        truncated,
    }
}

impl<S: EngineSink> AppCore<S> {
    /// True while an extraction runs on a worker.
    pub fn notes_busy(&self) -> bool {
        self.audio.notes.running.is_some()
    }

    /// What the Audio to Notes panel's EXTRACT works on (the first selected audio clip, by the
    /// name of its file), and whether it can.
    pub fn audio_notes_target(&self) -> (String, bool) {
        if self.notes_busy() {
            return ("an extraction is running".to_owned(), false);
        }
        let clips = self.selected_audio();
        let Some(first) = clips.first() else {
            return (String::new(), false);
        };
        let name = self.asset_name(&first.content.asset);
        match clips.len() {
            1 => (name, true),
            n => (format!("{name} (the first of {n} selected clips)"), true),
        }
    }

    /// The note track the notes go on: the device selected in the rack (a click on a track header
    /// selects its device there, and a later click on a clip keeps it), else the track selected in
    /// the arrangement, when it has a note lane.
    fn notes_track(&self) -> Option<DeviceKey> {
        let song = &self.doc().song;
        let is_notes = |d: &DeviceKey| {
            song.track(d)
                .is_some_and(|t| t.lanes.first().is_some_and(|l| l.kind.is_notes()))
        };
        self.view
            .selected()
            .cloned()
            .filter(|d| is_notes(d))
            .or_else(|| {
                self.arrangement
                    .selected_track()
                    .cloned()
                    .filter(|d| is_notes(d))
            })
    }

    /// Starts the extraction for the first selected audio clip with the panel's settings; the
    /// notes arrive when the worker is done ([`AppCore::wait_background`] waits for them). False,
    /// with a message, when nothing could start.
    pub fn audio_to_notes(&mut self, values: &NotesValues) -> bool {
        if self.notes_busy() {
            self.reject("Audio to Notes is already running".to_owned());
            return false;
        }
        let clips = self.selected_audio();
        let Some(s) = clips.into_iter().next() else {
            self.reject("Select an audio clip to extract notes from".to_owned());
            return false;
        };
        if s.content.warp.enabled && s.content.warp.mode == WarpMode::Repitch {
            self.reject(
                "Audio to Notes cannot read a clip warped in Repitch mode (its pitch follows the \
                 tempo): use the Beats or Tonal mode, or turn warp off"
                    .to_owned(),
            );
            return false;
        }
        let Some(asset) = self.audio.assets.get(&s.content.asset).cloned() else {
            self.reject("The audio file is missing".to_owned());
            return false;
        };
        let target = if values.new_track {
            None
        } else {
            match self.notes_track() {
                Some(t) => Some(t),
                None => {
                    self.reject(
                        "Select a note track to put the notes on (or choose New track)".to_owned(),
                    );
                    return false;
                }
            }
        };
        let song = &self.doc().song;
        let Some((track, _, clip)) = song.clip(s.id) else {
            return false;
        };
        let name = self
            .doc()
            .device(&track.device)
            .map_or_else(|| track.device.0.clone(), |d| d.name.clone());
        let (start, length) = (clip.start, clip.length);
        let rate = self.doc().sample_rate;
        let from = s.content.source_offset;
        let end = s.source_end(rate).min(asset.frames());
        let n = end.saturating_sub(from);
        if n == 0 {
            self.reject("The clip has no audio to read".to_owned());
            return false;
        }
        let id = self.audio.notes.next;
        self.audio.notes.next += 1;
        let (reverse, job_values) = (s.content.reverse, values.clone());
        let work: Box<dyn FnOnce() -> Result<NotesFound, String> + Send> = Box::new(move || {
            Ok(analyse(
                &asset,
                (from, n),
                reverse,
                (rate, MAX_NOTES_SECONDS),
                &job_values,
            ))
        });
        match spawn_notes(self.audio.tx.clone(), self.audio.generation, id, work) {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.notes.running = Some(Run {
                    id,
                    clip: s.id,
                    start,
                    length,
                    content: s.content,
                    values: values.clone(),
                    target,
                    name,
                });
                self.report("Extracting notes\u{2026}");
                true
            }
            Err(e) => {
                self.reject(format!("Audio to Notes: cannot start the analysis: {e}"));
                false
            }
        }
    }

    /// Applies a finished extraction (see the module docs).
    pub(super) fn apply_noted(&mut self, id: u64, result: Result<NotesFound, String>) {
        if self.audio.notes.running.as_ref().is_none_or(|r| r.id != id) {
            return;
        }
        let Some(run) = self.audio.notes.running.take() else {
            return;
        };
        let found = match result {
            Ok(f) => f,
            Err(e) => return self.reject(format!("Audio to Notes: {e}")),
        };
        let song = &self.doc().song;
        let unchanged = song.clip(run.clip).is_some_and(|(_, _, c)| {
            c.start == run.start
                && c.length == run.length
                && matches!(&c.content, ClipContent::Audio(a) if *a == run.content)
        });
        if !unchanged {
            return self.reject(
                "The audio clip changed while its notes were extracted: run Audio to Notes again"
                    .to_owned(),
            );
        }
        let v = &run.values;
        let options = NoteOptions {
            snap: if v.grid > 0 {
                4 * PPQ / i64::from(v.grid)
            } else {
                0
            },
            transpose: (f64::from(run.content.warp.effective_cents()) / 100.0).round() as i32,
        };
        let clip = match notes_clip(
            song,
            self.doc().sample_rate,
            (run.start, run.length),
            &run.content,
            &found.notes,
            options,
        ) {
            Ok(c) => c,
            Err(_) => {
                let hint = match v.mode {
                    NotesMode::Pitched => "raise the sensitivity or widen the note range",
                    NotesMode::Drums => "raise the sensitivity",
                };
                return self.reject(format!("No notes found in the clip: {hint}"));
            }
        };
        let count = match &clip.content {
            ClipContent::Notes(n) => n.len(),
            _ => 0,
        };
        let added = match &run.target {
            Some(device) => self.add_notes_to(device.clone(), clip),
            None => {
                let suffix = match v.mode {
                    NotesMode::Pitched => "notes",
                    NotesMode::Drums => "drums",
                };
                self.add_notes_track(format!("{} {suffix}", run.name), clip)
            }
        };
        match added {
            Ok(place) => {
                let more = if found.truncated {
                    format!(" (from the first {} minutes)", MAX_NOTES_SECONDS / 60)
                } else {
                    String::new()
                };
                self.report(format!(
                    "Audio to Notes: {count} note{} on {place}{more}",
                    if count == 1 { "" } else { "s" }
                ));
            }
            Err(e) => self.reject(format!("Audio to Notes: {e}")),
        }
    }

    /// The note clip on a new poly synth track, named `name`; the clip is selected. Returns the
    /// track's name.
    fn add_notes_track(&mut self, name: String, clip: Clip) -> Result<String, String> {
        let import = MidiImport {
            instrument: IMPORT_INSTRUMENT.to_owned(),
            tracks: vec![ImportedTrack {
                name: name.clone(),
                clip,
            }],
            ..MidiImport::default()
        };
        self.apply_command(Command::Workflow(WorkflowCommand::ImportMidi(import)))
            .map_err(|e| e.to_string())?;
        let id = self
            .doc()
            .song
            .tracks
            .last()
            .and_then(|t| t.lanes.first())
            .and_then(|l| l.clips.first())
            .map(|c| c.id);
        self.after_view_change();
        self.arrangement
            .set_selected_clips(&id.into_iter().collect::<Vec<_>>());
        Ok(self
            .doc()
            .song
            .tracks
            .last()
            .and_then(|t| self.doc().device(&t.device))
            .map_or(name, |d| d.name.clone()))
    }

    /// The note clip on the note track `device`; the clip is selected. Returns the track's name.
    fn add_notes_to(&mut self, device: DeviceKey, clip: Clip) -> Result<String, String> {
        let name = self
            .doc()
            .device(&device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        let lane = self.arrangement.record_lane(&device);
        self.apply_command(Command::Song(SongCommand::AddClip { device, lane, clip }))
            .map_err(|e| e.to_string())?;
        let id = self.document.last_clip_id();
        self.after_view_change();
        self.arrangement
            .set_selected_clips(&id.into_iter().collect::<Vec<_>>());
        Ok(name)
    }
}

#[cfg(test)]
#[path = "app_core_notes_tests.rs"]
mod tests;
