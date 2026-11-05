//! Audio to Notes in the app core: notes extracted from the audio of a clip, as a note clip.
//!
//! The Audio to Notes panel (Audio menu) picks what to look for ([`NotesValues`]): *Pitched*
//! follows one voice or instrument and cuts it into notes ([`atomik_dsp::notes::pitched`]), *Drums*
//! finds the hits of a drum loop and sorts them into kick, snare and hats, and with the full kit
//! toms and a crash, on the keys of a chosen map ([`atomik_dsp::notes::drums`]).
//!
//! **What is read.** Every selected audio clip, one after the other: the span of its asset that
//! it plays (from the clip's offset; for a warped clip the span the time map reads), mixed to
//! mono, and in play order for a reversed clip. Assets that stream from disk are read through the
//! stream source in chunks, like strip silence does. The analysis runs on a worker thread
//! (`atomik-notes`), one clip at a time, and reads the **whole** clip, a minute of it at a time
//! ([`atomik_dsp::notes::extract_in_segments`]), so a clip of any length never stops the UI nor
//! holds more than a minute of audio in memory; the song keeps playing meanwhile. After each
//! minute the worker publishes how far it is ([`AppCore::notes_progress`], shown in the status
//! line and in the panel) and looks for a stop ([`AppCore::cancel_notes`]); a stopped run adds no
//! notes, and the clips that were already finished keep theirs.
//!
//! **Warped clips.** A clip warped in Beats or Tonal mode, and a clip that is only transposed, is
//! read from its source and its notes go where the clip plays them (through the warp time map,
//! see [`atomik_model::notes_clip`]); the transposition is added to the keys. A clip in Repitch
//! mode is refused with a message: its pitch follows the tempo, which the analysis cannot see.
//!
//! **Where the notes go.** One note clip for each audio clip, starting and ending with it: on a
//! **new track** each (a poly synth, named after the audio track, through `ImportMidi`) or on the
//! **selected note track** (`AddClip`, lane 0; the instrument selected in the rack). Each is one
//! undo step and the last new clip is selected. With *Keep pitch bend* a pitched note keeps how
//! the voice moves as a pitch-bend lane of the clip (see [`atomik_model::notes_clip`]). Drum
//! notes land on the keys of the chosen map whatever the instrument: point the track at a drum
//! machine with samples on those pads to hear them. A result is dropped, with a message, when the
//! audio clip changed or the project closed while the worker ran.

use super::audio_edit_ops::{Selected, for_each_chunk};
use super::*;
use crate::audio_import::{NotesFound, spawn_notes};
use atomik_core::PPQ;
use atomik_dsp::notes::{Analysis, DrumParams, PitchParams, extract_in_segments};
use atomik_engine::SongAsset;
use atomik_model::{
    AudioClipContent, Clip, ClipContent, FoundBend, FoundNote, ImportedTrack, MidiImport,
    NoteOptions, WarpMode, WorkflowCommand, notes_clip,
};
use atomik_ui::{NotesMode, NotesProgress, NotesValues};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What a worker that was told to stop answers with.
const CANCELLED: &str = "cancelled";

/// What the UI thread and the worker of one clip share: how far the analysis is, and a stop.
#[derive(Debug, Default)]
pub(super) struct Shared {
    /// Frames of the clip finished.
    done: AtomicU64,
    /// Set to stop the worker after the stretch it is on.
    cancel: AtomicBool,
}

/// One clip to extract, and what it was asked for, to check it against the document when it is
/// done.
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
    /// The audio to read.
    asset: SongAsset,
    /// The first frame of the asset the clip plays, and how many frames it plays.
    span: (u64, u64),
    rate: u32,
    shared: Arc<Shared>,
}

/// The clips of one request.
#[derive(Debug, Default)]
struct Batch {
    /// Clips asked for (those that could start).
    clips: usize,
    /// Clips whose run is over.
    finished: usize,
    /// Notes added so far.
    notes: usize,
    /// Clips left out because they could not be read.
    skipped: usize,
}

/// The Audio to Notes state of an open project.
#[derive(Default)]
pub(super) struct NotesState {
    running: Option<Run>,
    queue: VecDeque<Run>,
    next: u64,
    batch: Batch,
}

impl NotesState {
    /// Forgets the running extraction and the ones queued behind it (the project changed; the
    /// worker is told to stop and its result is dropped by the generation).
    pub(super) fn reset(&mut self) {
        if let Some(run) = self.running.take() {
            run.shared.cancel.store(true, Ordering::Relaxed);
        }
        self.queue.clear();
        self.batch = Batch::default();
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

/// The analysis of one clip (what the worker runs): `n` frames of `asset` from `from`, all of
/// them, a stretch at a time. `None` when `shared` says stop.
fn analyse(
    asset: &SongAsset,
    (from, n): (u64, u64),
    reverse: bool,
    rate: u32,
    v: &NotesValues,
    shared: &Shared,
) -> Option<NotesFound> {
    let sensitivity = v.sensitivity.min(100) as f32 / 100.0;
    let pitch = PitchParams {
        sensitivity,
        min_note_ms: v.min_note_ms,
        key_lo: v.key_lo,
        key_hi: v.key_hi,
        tolerance_cents: v.drift_cents,
        bend: v.keep_bend,
    };
    let drums = DrumParams {
        sensitivity,
        open_hats: v.open_hats,
        kit: v.kit,
        map: v.drum_map,
    };
    let what = match v.mode {
        NotesMode::Pitched => Analysis::Pitched(&pitch),
        NotesMode::Drums => Analysis::Drums(&drums),
    };
    // Frames count in play order: a reversed clip plays the end of its span first.
    let mut read = |at: u64, len: u64| {
        let len = len.min(n.saturating_sub(at));
        if reverse {
            read_mono(asset, from + n - at - len, len, true)
        } else {
            read_mono(asset, from + at, len, false)
        }
    };
    let mut progress = |done: u64| {
        shared.done.store(done, Ordering::Relaxed);
        !shared.cancel.load(Ordering::Relaxed)
    };
    let found = extract_in_segments(n, rate, what, &mut read, &mut progress)?;
    Some(NotesFound {
        notes: found
            .into_iter()
            .map(|n| FoundNote {
                start: n.start,
                length: n.length,
                key: n.key,
                velocity: n.velocity,
                bend: n
                    .bend
                    .into_iter()
                    .map(|p| FoundBend {
                        frame: p.frame,
                        semitones: p.semitones,
                    })
                    .collect(),
            })
            .collect(),
    })
}

impl<S: EngineSink> AppCore<S> {
    /// True while an extraction runs on a worker.
    pub fn notes_busy(&self) -> bool {
        self.audio.notes.running.is_some()
    }

    /// How far the running extraction has got (`None`: none runs): text for the status line and
    /// the panel, and the fraction of the clips asked for that is done.
    pub fn notes_progress(&self) -> Option<NotesProgress> {
        let run = self.audio.notes.running.as_ref()?;
        let batch = &self.audio.notes.batch;
        let within = run.shared.done.load(Ordering::Relaxed) as f32 / run.span.1.max(1) as f32;
        let fraction =
            ((batch.finished as f32 + within.min(1.0)) / batch.clips.max(1) as f32).clamp(0.0, 1.0);
        let percent = (fraction * 100.0).floor() as u32;
        let text = if batch.clips > 1 {
            format!(
                "Extracting notes, clip {} of {}: {percent} %",
                batch.finished + 1,
                batch.clips
            )
        } else {
            format!("Extracting notes: {percent} %")
        };
        Some(NotesProgress { text, fraction })
    }

    /// Stops the running extraction and drops the clips queued behind it. The worker ends after
    /// the minute it is on; notes already added stay. False when nothing runs.
    pub fn cancel_notes(&mut self) -> bool {
        let Some(run) = &self.audio.notes.running else {
            return false;
        };
        run.shared.cancel.store(true, Ordering::Relaxed);
        self.audio.notes.queue.clear();
        self.report("Audio to Notes: stopping\u{2026}");
        true
    }

    /// What the Audio to Notes panel's EXTRACT works on (the selected audio clips, by the name
    /// of the first one's file), and whether it can.
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
            n => (format!("{name} and {} more clips", n - 1), true),
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

    /// Starts the extraction for every selected audio clip with the panel's settings, one clip
    /// after the other; the notes arrive when the workers are done
    /// ([`AppCore::wait_background`] waits for them). False, with a message, when nothing could
    /// start.
    pub fn audio_to_notes(&mut self, values: &NotesValues) -> bool {
        if self.notes_busy() {
            self.reject("Audio to Notes is already running".to_owned());
            return false;
        }
        let clips = self.selected_audio();
        if clips.is_empty() {
            self.reject("Select an audio clip to extract notes from".to_owned());
            return false;
        }
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
        let mut runs: VecDeque<Run> = VecDeque::new();
        let mut skipped: Vec<String> = Vec::new();
        for s in clips {
            match self.notes_run(&s, values, &target) {
                Ok(run) => runs.push_back(run),
                Err(why) => skipped.push(why),
            }
        }
        if runs.is_empty() {
            let why = skipped.into_iter().next().unwrap_or_default();
            self.reject(why);
            return false;
        }
        self.audio.notes.batch = Batch {
            clips: runs.len(),
            skipped: skipped.len(),
            ..Batch::default()
        };
        self.audio.notes.queue = runs;
        if self.start_next_notes() {
            true
        } else {
            self.audio.notes.queue.clear();
            self.audio.notes.batch = Batch::default();
            false
        }
    }

    /// The run for the selected clip `s`, or why it cannot be read.
    fn notes_run(
        &mut self,
        s: &Selected,
        values: &NotesValues,
        target: &Option<DeviceKey>,
    ) -> Result<Run, String> {
        if s.content.warp.enabled && s.content.warp.mode == WarpMode::Repitch {
            return Err(
                "Audio to Notes cannot read a clip warped in Repitch mode (its pitch \
                 follows the tempo): use the Beats or Tonal mode, or turn warp off"
                    .to_owned(),
            );
        }
        let Some(asset) = self.audio.assets.get(&s.content.asset).cloned() else {
            return Err("The audio file is missing".to_owned());
        };
        let song = &self.doc().song;
        let Some((track, _, clip)) = song.clip(s.id) else {
            return Err("The clip is gone".to_owned());
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
            return Err("The clip has no audio to read".to_owned());
        }
        let id = self.audio.notes.next;
        self.audio.notes.next += 1;
        Ok(Run {
            id,
            clip: s.id,
            start,
            length,
            content: s.content.clone(),
            values: values.clone(),
            target: target.clone(),
            name,
            asset,
            span: (from, n),
            rate,
            shared: Arc::new(Shared::default()),
        })
    }

    /// Starts the worker for the next queued clip. False, with a message, when it cannot start.
    fn start_next_notes(&mut self) -> bool {
        let Some(run) = self.audio.notes.queue.pop_front() else {
            return false;
        };
        let (asset, span, rate) = (run.asset.clone(), run.span, run.rate);
        let (reverse, values, shared) = (
            run.content.reverse,
            run.values.clone(),
            Arc::clone(&run.shared),
        );
        let work: Box<dyn FnOnce() -> Result<NotesFound, String> + Send> = Box::new(move || {
            analyse(&asset, span, reverse, rate, &values, &shared)
                .ok_or_else(|| CANCELLED.to_owned())
        });
        match spawn_notes(self.audio.tx.clone(), self.audio.generation, run.id, work) {
            Ok(()) => {
                self.audio.pending += 1;
                let batch = &self.audio.notes.batch;
                let said = if batch.clips > 1 {
                    format!(
                        "Extracting notes, clip {} of {}\u{2026}",
                        batch.finished + 1,
                        batch.clips
                    )
                } else {
                    "Extracting notes\u{2026}".to_owned()
                };
                self.audio.notes.running = Some(run);
                self.report(said);
                true
            }
            Err(e) => {
                self.reject(format!("Audio to Notes: cannot start the analysis: {e}"));
                false
            }
        }
    }

    /// Applies a finished extraction (see the module docs) and starts the next clip.
    pub(super) fn apply_noted(&mut self, id: u64, result: Result<NotesFound, String>) {
        if self.audio.notes.running.as_ref().is_none_or(|r| r.id != id) {
            return;
        }
        let Some(run) = self.audio.notes.running.take() else {
            return;
        };
        let stopped = run.shared.cancel.load(Ordering::Relaxed);
        let outcome = match result {
            _ if stopped => Err(None),
            Ok(found) => self.add_found(&run, found).map_err(Some),
            Err(e) => Err(Some(format!("Audio to Notes: {e}"))),
        };
        let batch = &mut self.audio.notes.batch;
        batch.finished += 1;
        match outcome {
            Ok((count, said)) => {
                batch.notes += count;
                self.report(said);
            }
            Err(Some(why)) => self.reject(why),
            Err(None) => self.audio.notes.queue.clear(),
        }
        if !stopped && self.start_next_notes() {
            return;
        }
        // The last clip of the request is over.
        let batch = std::mem::take(&mut self.audio.notes.batch);
        if stopped {
            self.report("Audio to Notes: stopped".to_owned());
        } else if batch.clips > 1 || batch.skipped > 0 {
            let skipped = match batch.skipped {
                0 => String::new(),
                n => format!(
                    " ({n} clip{} could not be read)",
                    if n == 1 { "" } else { "s" }
                ),
            };
            self.report(format!(
                "Audio to Notes: {} note{} from {} clip{}{skipped}",
                batch.notes,
                if batch.notes == 1 { "" } else { "s" },
                batch.clips,
                if batch.clips == 1 { "" } else { "s" },
            ));
        }
    }

    /// Puts the notes `found` for `run` in the document. The number of notes and the message
    /// to show, or the refusal to show.
    fn add_found(&mut self, run: &Run, found: NotesFound) -> Result<(usize, String), String> {
        let song = &self.doc().song;
        let unchanged = song.clip(run.clip).is_some_and(|(_, _, c)| {
            c.start == run.start
                && c.length == run.length
                && matches!(&c.content, ClipContent::Audio(a) if *a == run.content)
        });
        if !unchanged {
            return Err(
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
                return Err(format!("No notes found in the clip: {hint}"));
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
            Ok(place) => Ok((
                count,
                format!(
                    "Audio to Notes: {count} note{} on {place}",
                    if count == 1 { "" } else { "s" }
                ),
            )),
            Err(e) => Err(format!("Audio to Notes: {e}")),
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
