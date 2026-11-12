//! Chords and keys from audio in the app core: **Detect Chords from Audio** and **Detect Key from
//! Audio**.
//!
//! **What is read.** The selected audio clips, or, with none selected and the loop on, every audio
//! clip that plays in the loop. Each is read like Audio to Notes reads it (the span of its asset it
//! plays, mixed to mono, in play order) on a worker thread (`atomik-chords`), a minute at a time,
//! through the harmony analysis of [`atomik_dsp::harmony`] with the chord settings. The worker
//! publishes how far it is ([`AppCore::chords_progress`]) and looks for a stop
//! ([`AppCore::cancel_chords`]). A key request reads at most [`KEY_ANALYSIS_SECONDS`] of audio, and
//! with no clip selected it reads every audio clip of the song.
//!
//! **The cache.** A finished chord analysis is kept as frames on song ticks: each frame of each
//! clip is placed where the clip plays it (through the warp time map of a warped clip), so the
//! clips are mixed by where they sound. The Detect Chords panel's settings (how often chords may
//! change, triads or sevenths, slash chords) score the cache again in an instant
//! ([`AppCore::chords_preview`]): the units are the beats, half bars or bars of the song's meter
//! over the span the clips cover.
//!
//! **Applying.** [`AppCore::apply_detected_chords`] puts the chords in the lane over the span,
//! replacing the chords there or only filling the gaps, and (when asked and the song has no key)
//! sets the song key the audio points to: one `SetHarmony`, so one undo step. A key request sets
//! the key the same way as Detect Song Key does.

use super::audio_edit_ops::for_each_chunk;
use super::*;
use crate::audio_import::{ChordsHeard, spawn_chords};
use atomik_dsp::harmony::{
    Analysis, AnalysisParams, BeatChroma, ChordOptions, Frame, analyse_in_segments, key_profile,
    recognise_chords,
};
use atomik_engine::SongAsset;
use atomik_model::harmony::{ChordEvent, Key};
use atomik_model::{
    AudioClipContent, ClipContent, KeyGuess, SongCommand, WarpMode, detect_key_from_profile,
    tick_at_source,
};
use atomik_ui::{ChordChanges, ChordsPreview, ChordsValues, NotesProgress, PreviewChord};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The most audio a key request reads, in seconds (the clips are read in song order up to it).
pub const KEY_ANALYSIS_SECONDS: u64 = 600;
/// The most units (beats, half bars or bars) a span is cut into.
const MAX_UNITS: usize = 20_000;

/// What the UI thread and the worker share: how far the analysis is, and a stop.
#[derive(Debug, Default)]
struct Shared {
    done: AtomicU64,
    cancel: AtomicBool,
}

/// What a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Chords,
    Key,
}

/// One clip of a request.
#[derive(Clone)]
struct Source {
    start: i64,
    length: i64,
    content: AudioClipContent,
    asset: SongAsset,
    /// The first frame of the asset the clip plays, and how many frames are read.
    span: (u64, u64),
}

/// A request on its worker.
struct Running {
    id: u64,
    purpose: Purpose,
    sources: Vec<Source>,
    /// The ticks the chords go over.
    range: (i64, i64),
    /// What the panel says it reads.
    target: String,
    /// Frames of all the clips.
    total: u64,
    shared: Arc<Shared>,
}

/// Lane chords with their confidence.
type Scored = Vec<(ChordEvent, f32)>;

/// The clips of a request, the ticks the chords go over and what the panel says it reads.
type Sources = (Vec<Source>, (i64, i64), String);

/// A finished chord analysis.
struct Heard {
    range: (i64, i64),
    target: String,
    /// The frames of every clip on song ticks, sorted by tick.
    frames: Vec<(i64, Frame)>,
    key: Option<KeyGuess>,
    /// Bumped for every new analysis, so a preview is scored again.
    serial: u64,
}

/// The chords state of an open project.
#[derive(Default)]
pub(super) struct ChordsState {
    running: Option<Running>,
    heard: Option<Heard>,
    /// The last preview and what it was scored with.
    scored: Option<(u64, ChordsValues, Scored)>,
    next: u64,
}

impl ChordsState {
    /// Forgets the analysis that runs (its worker is told to stop) and the one kept.
    pub(super) fn reset(&mut self) {
        if let Some(run) = self.running.take() {
            run.shared.cancel.store(true, Ordering::Relaxed);
        }
        self.heard = None;
        self.scored = None;
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

/// The analysis of every source (what the worker runs). `None` when told to stop.
fn listen(sources: &[Source], rate: u32, shared: &Shared) -> Option<ChordsHeard> {
    let params = AnalysisParams::chords(rate);
    let mut out = Vec::with_capacity(sources.len());
    let mut base = 0u64;
    for s in sources {
        let (from, n) = s.span;
        let reverse = s.content.reverse;
        let mut read = |at: u64, len: u64| {
            let len = len.min(n.saturating_sub(at));
            if reverse {
                read_mono(&s.asset, from + n - at - len, len, true)
            } else {
                read_mono(&s.asset, from + at, len, false)
            }
        };
        let mut progress = |done: u64| {
            shared.done.store(base + done, Ordering::Relaxed);
            !shared.cancel.load(Ordering::Relaxed)
        };
        out.push(analyse_in_segments(
            n,
            rate,
            &params,
            &mut read,
            &mut progress,
        )?);
        base += n;
    }
    Some(out)
}

/// The tick bounds of the units of `range`: every beat, half bar or bar of the meter, with the
/// range's ends as the first and last bounds.
fn unit_bounds(song: &atomik_model::Song, (lo, hi): (i64, i64), changes: ChordChanges) -> Vec<i64> {
    let table = song.meter_table();
    let map = table.map();
    let mut out = vec![lo];
    let mut bar = map.bar_at(lo);
    loop {
        let start = map.bar_start(bar);
        if start >= hi || out.len() > MAX_UNITS {
            break;
        }
        let beat = map.beat_ticks_at(start).max(1);
        let (num, _) = map.meter_at(start);
        let beats = i64::from(num.max(1));
        let lines: Vec<i64> = match changes {
            ChordChanges::Bar => vec![start],
            ChordChanges::HalfBar => {
                if beats >= 2 {
                    vec![start, start + (beats + 1) / 2 * beat]
                } else {
                    vec![start]
                }
            }
            ChordChanges::Beat => (0..beats).map(|i| start + i * beat).collect(),
        };
        for t in lines {
            if t > lo && t < hi {
                out.push(t);
            }
        }
        bar = bar.saturating_add(1);
    }
    out.push(hi);
    out.dedup();
    out
}

/// How sure a key guess is, in words for the status line.
fn sureness(confidence: f64) -> &'static str {
    if confidence >= 0.75 {
        "a clear fit"
    } else if confidence >= 0.5 {
        "a likely fit"
    } else {
        "a rough guess"
    }
}

impl<S: EngineSink> AppCore<S> {
    /// True while a chord or key analysis runs on a worker.
    pub fn chords_busy(&self) -> bool {
        self.audio.chords.running.is_some()
    }

    /// How far the running analysis has got (`None`: none runs).
    pub fn chords_progress(&self) -> Option<NotesProgress> {
        let run = self.audio.chords.running.as_ref()?;
        let fraction = (run.shared.done.load(Ordering::Relaxed) as f32 / run.total.max(1) as f32)
            .clamp(0.0, 1.0);
        let what = match run.purpose {
            Purpose::Chords => "Listening for chords",
            Purpose::Key => "Listening for the key",
        };
        Some(NotesProgress {
            text: format!("{what}: {} %", (fraction * 100.0).floor() as u32),
            fraction,
        })
    }

    /// Stops the running analysis (nothing is changed). False when none runs.
    pub fn cancel_chords(&mut self) -> bool {
        let Some(run) = &self.audio.chords.running else {
            return false;
        };
        run.shared.cancel.store(true, Ordering::Relaxed);
        self.report("Detect chords: stopping\u{2026}");
        true
    }

    /// What the Detect Chords panel says it reads: the running request's, else the last heard.
    pub fn chords_target(&self) -> String {
        let c = &self.audio.chords;
        c.running
            .as_ref()
            .filter(|r| r.purpose == Purpose::Chords)
            .map(|r| r.target.clone())
            .or_else(|| c.heard.as_ref().map(|h| h.target.clone()))
            .unwrap_or_default()
    }

    /// The audio clips a request reads, with the span the chords go over: the selected audio
    /// clips, else the audio clips of the loop (cut to it). `all` takes every audio clip of the
    /// song when none is selected and the loop is off.
    fn chord_sources(&self, all: bool) -> Result<Sources, String> {
        let song = &self.doc().song;
        let rate = self.doc().sample_rate;
        let selected: Vec<ClipId> = self.selected_audio().into_iter().map(|s| s.id).collect();
        let mut range: Option<(i64, i64)> = None;
        let ids: Vec<ClipId> = if !selected.is_empty() {
            selected
        } else {
            let (lo, hi) = song.loop_range;
            let in_loop = song.loop_enabled && hi > lo;
            if in_loop {
                range = Some((lo, hi));
            } else if !all {
                return Err(
                    "Select audio clips (or set the loop over them) to detect chords from"
                        .to_owned(),
                );
            }
            let mut ids = Vec::new();
            for track in &song.tracks {
                for lane in &track.lanes {
                    for clip in lane.clips.iter().filter(|c| !c.muted) {
                        let inside = !in_loop || (clip.start < hi && clip.end() > lo);
                        if matches!(clip.content, ClipContent::Audio(_)) && inside {
                            ids.push(clip.id);
                        }
                    }
                }
            }
            ids
        };
        let mut sources = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut why = None;
        for id in ids {
            let Some((_, _, clip)) = song.clip(id) else {
                continue;
            };
            let ClipContent::Audio(a) = &clip.content else {
                continue;
            };
            if a.warp.enabled && a.warp.mode == WarpMode::Repitch {
                why = Some(
                    "Chords cannot be read from a clip warped in Repitch mode: use Beats or \
                     Tonal, or turn warp off"
                        .to_owned(),
                );
                continue;
            }
            let Some(asset) = self.audio.assets.get(&a.asset).cloned() else {
                why = Some("The audio file is missing".to_owned());
                continue;
            };
            let from = a.source_offset;
            let end = if a.warp.enabled {
                from + a
                    .warp
                    .source_frames_at(rate, clip.length as f64)
                    .round()
                    .max(0.0) as u64
            } else {
                from + song
                    .frame_at(clip.end(), rate)
                    .saturating_sub(song.frame_at(clip.start, rate))
            }
            .min(asset.frames());
            if end <= from {
                continue;
            }
            names.push(self.asset_name(&a.asset));
            sources.push(Source {
                start: clip.start,
                length: clip.length,
                content: a.clone(),
                asset,
                span: (from, end - from),
            });
        }
        if sources.is_empty() {
            return Err(why.unwrap_or_else(|| "There is no audio to listen to".to_owned()));
        }
        sources.sort_by_key(|s| s.start);
        let range = range.unwrap_or_else(|| {
            let lo = sources.iter().map(|s| s.start).min().unwrap_or(0);
            let hi = sources
                .iter()
                .map(|s| s.start + s.length)
                .max()
                .unwrap_or(lo);
            (lo, hi)
        });
        let table = song.meter_table();
        let map = table.map();
        let bars = (map.bar_at(range.0), map.bar_at((range.1 - 1).max(range.0)));
        let first = names.first().cloned().unwrap_or_default();
        let what = match names.len() {
            1 => first,
            n => format!("{first} and {} more clips", n - 1),
        };
        let target = if bars.0 == bars.1 {
            format!("{what}, bar {}", bars.0)
        } else {
            format!("{what}, bars {} to {}", bars.0, bars.1)
        };
        Ok((sources, range, target))
    }

    /// Starts listening for chords (see the module docs); the Detect Chords panel shows the
    /// result. False, with a message, when nothing could start.
    pub fn detect_chords_from_audio(&mut self) -> bool {
        self.start_chords(Purpose::Chords)
    }

    /// Starts listening for the song key in the audio (see the module docs); the key is set when
    /// the worker is done. False, with a message, when nothing could start.
    pub fn detect_key_from_audio(&mut self) -> bool {
        self.start_chords(Purpose::Key)
    }

    fn start_chords(&mut self, purpose: Purpose) -> bool {
        if self.chords_busy() {
            self.reject("A chord or key analysis is already running".to_owned());
            return false;
        }
        let (mut sources, range, target) = match self.chord_sources(purpose == Purpose::Key) {
            Ok(found) => found,
            Err(why) => {
                self.reject(why);
                return false;
            }
        };
        let rate = self.doc().sample_rate;
        if purpose == Purpose::Key {
            let mut left = KEY_ANALYSIS_SECONDS * u64::from(rate);
            sources.retain_mut(|s| {
                let keep = left > 0;
                s.span.1 = s.span.1.min(left);
                left -= s.span.1;
                keep
            });
        }
        let total = sources.iter().map(|s| s.span.1).sum();
        let id = self.audio.chords.next;
        self.audio.chords.next += 1;
        let shared = Arc::new(Shared::default());
        let (work_sources, work_shared) = (sources.clone(), Arc::clone(&shared));
        let work: Box<dyn FnOnce() -> Result<Option<ChordsHeard>, String> + Send> =
            Box::new(move || Ok(listen(&work_sources, rate, &work_shared)));
        match spawn_chords(self.audio.tx.clone(), self.audio.generation, id, work) {
            Ok(()) => {
                self.audio.pending += 1;
                if purpose == Purpose::Chords {
                    self.audio.chords.heard = None;
                    self.audio.chords.scored = None;
                }
                self.audio.chords.running = Some(Running {
                    id,
                    purpose,
                    sources,
                    range,
                    target,
                    total,
                    shared,
                });
                self.report(match purpose {
                    Purpose::Chords => "Listening for chords\u{2026}",
                    Purpose::Key => "Listening for the key\u{2026}",
                });
                true
            }
            Err(e) => {
                self.reject(format!("Cannot start the analysis: {e}"));
                false
            }
        }
    }

    /// Takes a finished analysis (see the module docs).
    pub(super) fn apply_chords(&mut self, id: u64, result: Result<Option<ChordsHeard>, String>) {
        if self
            .audio
            .chords
            .running
            .as_ref()
            .is_none_or(|r| r.id != id)
        {
            return;
        }
        let Some(run) = self.audio.chords.running.take() else {
            return;
        };
        let heard = match result {
            Ok(Some(h)) if !run.shared.cancel.load(Ordering::Relaxed) => h,
            Ok(_) => {
                self.report("Stopped: nothing was changed");
                return;
            }
            Err(e) => return self.reject(format!("Cannot listen to the audio: {e}")),
        };
        let mut profile = [0.0f64; 12];
        for a in &heard {
            for (p, v) in profile.iter_mut().zip(key_profile(a)) {
                *p += v;
            }
        }
        let key = detect_key_from_profile(&profile);
        match run.purpose {
            Purpose::Key => self.set_key_from_audio(key),
            Purpose::Chords => {
                let frames = self.frames_on_ticks(&run, &heard);
                let serial = self.audio.chords.next;
                self.audio.chords.next += 1;
                self.audio.chords.heard = Some(Heard {
                    range: run.range,
                    target: run.target,
                    frames,
                    key,
                    serial,
                });
                self.audio.chords.scored = None;
                self.report("Chords found: check them in the panel, then put them in the lane");
            }
        }
    }

    /// The frames of every analysed clip where the song plays them, sorted by tick.
    fn frames_on_ticks(&self, run: &Running, heard: &[Analysis]) -> Vec<(i64, Frame)> {
        let song = &self.doc().song;
        let rate = self.doc().sample_rate;
        let mut out = Vec::new();
        for (s, a) in run.sources.iter().zip(heard) {
            for (k, f) in a.frames.iter().enumerate() {
                let src = s.content.source_offset as f64 + a.centre(k) as f64;
                let tick = tick_at_source(song, rate, s.start, &s.content, src).round() as i64;
                if tick >= s.start && tick < s.start + s.length {
                    out.push((tick, f.clone()));
                }
            }
        }
        out.sort_by_key(|(t, _)| *t);
        out
    }

    /// Sets the song key from an audio key guess (one undo step), with a message.
    fn set_key_from_audio(&mut self, guess: Option<KeyGuess>) {
        let Some(guess) = guess else {
            self.report("No key could be heard in the audio");
            return;
        };
        if self.harmony().key == Some(guess.key) {
            self.report(format!("The song key is already {}", guess.key.name()));
            return;
        }
        let Ok(harmony) = self.harmony().with_key(Some(guess.key)) else {
            return;
        };
        if self.set_harmony(harmony).is_ok() {
            self.report(format!(
                "Song key set to {} from the audio ({}); Undo puts the old key back",
                guess.key.name(),
                sureness(guess.confidence)
            ));
        }
    }

    /// The chords of the last analysis scored with `values`, as lane chords with their
    /// confidence.
    fn scored_chords(&mut self, values: &ChordsValues) -> Option<Scored> {
        let heard = self.audio.chords.heard.as_ref()?;
        if let Some((serial, v, chords)) = &self.audio.chords.scored
            && *serial == heard.serial
            && (v.changes, v.full, v.slash) == (values.changes, values.full, values.slash)
        {
            return Some(chords.clone());
        }
        let bounds = unit_bounds(&self.doc().song, heard.range, values.changes);
        let mut units = vec![BeatChroma::default(); bounds.len().saturating_sub(1)];
        for (tick, f) in &heard.frames {
            let i = bounds.partition_point(|b| b <= tick);
            if i == 0 || i > units.len() {
                continue;
            }
            units[i - 1].add(f);
        }
        let opts = ChordOptions {
            full: values.full,
            slash: values.slash,
        };
        let chords: Scored = recognise_chords(&units, &opts)
            .into_iter()
            .map(|c| {
                let tick = bounds[c.first];
                let mut e = ChordEvent::new(tick, bounds[c.end] - tick, c.root, c.quality);
                e.bass = c.bass;
                (e, c.confidence)
            })
            .collect();
        let serial = heard.serial;
        self.audio.chords.scored = Some((serial, *values, chords.clone()));
        Some(chords)
    }

    /// What the Detect Chords panel shows for `values` (`None`: nothing heard yet). Scoring again
    /// reads the kept analysis only.
    pub fn chords_preview(&mut self, values: &ChordsValues) -> Option<ChordsPreview> {
        let chords = self.scored_chords(values)?;
        let heard = self.audio.chords.heard.as_ref()?;
        let key = heard
            .key
            .filter(|_| self.harmony().key.is_none())
            .map(|g| g.key.name());
        Some(ChordsPreview {
            span: heard.range,
            chords: chords
                .iter()
                .map(|(c, confidence)| PreviewChord {
                    tick: c.tick,
                    length: c.length,
                    symbol: c.symbol(),
                    confidence: *confidence,
                })
                .collect(),
            key,
        })
    }

    /// Puts the chords of the last analysis, scored with `values`, in the chord lane over its
    /// span: replacing the chords there, or only in the gaps when `merge`. With
    /// `values.set_key` and no song key, the key the audio points to is set too. One undo step.
    /// False, with a message, when there is nothing to put.
    pub fn apply_detected_chords(&mut self, values: &ChordsValues, merge: bool) -> bool {
        let Some(scored) = self.scored_chords(values) else {
            self.reject("Nothing to put in the chord lane: detect chords first".to_owned());
            return false;
        };
        let Some(heard) = self.audio.chords.heard.as_ref() else {
            return false;
        };
        let (range, key) = (heard.range, heard.key.map(|g| g.key));
        let chords: Vec<ChordEvent> = scored.into_iter().map(|(c, _)| c).collect();
        let mut harmony = match self.harmony().with_chords_in_range(range, &chords, merge) {
            Ok(h) => h,
            Err(e) => {
                self.reject(format!("Cannot change the key and chords: {e}"));
                return false;
            }
        };
        let mut said_key: Option<Key> = None;
        if values.set_key
            && harmony.key.is_none()
            && let Some(k) = key
            && let Ok(h) = harmony.with_key(Some(k))
        {
            harmony = h;
            said_key = Some(k);
        }
        if &harmony == self.harmony() {
            self.report("The chord lane already holds these chords");
            return true;
        }
        if self
            .apply_command(Command::Song(SongCommand::SetHarmony(harmony)))
            .is_err()
        {
            self.reject("Cannot change the key and chords".to_owned());
            return false;
        }
        let n = chords.len();
        let mut said = format!(
            "{n} chord{} put in the chord lane",
            if n == 1 { "" } else { "s" }
        );
        if let Some(k) = said_key {
            said.push_str(&format!(", song key {}", k.name()));
        }
        self.report(format!("{said}; Undo takes them out"));
        true
    }
}

#[cfg(test)]
#[path = "app_core_chords_tests.rs"]
mod tests;
