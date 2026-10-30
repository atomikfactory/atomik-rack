//! Warped audio clips in the app core: the buffer cache and the commands that need the samples.
//!
//! # The buffer cache
//!
//! A warped clip plays a rendered buffer of its whole span ([`atomik_render::warp`]), made by a
//! worker thread from the clip's [`WarpJob`]: the song stays playable while it builds, and a clip
//! whose buffer is not ready plays its asset in real time, unwarped. Every [`AppCore::submit_song`]
//! calls [`AppCore::sync_warps`], which
//!
//! * lists the jobs the document needs now ([`atomik_model::warp_jobs`]: audible warped clips),
//! * drops the buffers nobody needs (a changed setting, tempo or clip geometry gives another
//!   job key, so the old buffer is stale), and cancels the workers whose job is no longer
//!   needed (they poll a flag about every thousandth of a second of audio),
//! * queues the missing jobs whose asset is loaded and starts at most [`MAX_WARP_WORKERS`].
//!
//! A finished buffer is kept when its job is still needed and the song is submitted again, so it
//! plays from the next batch on. The cache holds at most [`WARP_CACHE_BYTES`] of audio (finished
//! buffers and running jobs); a job that does not fit is skipped and reported once (its clip plays
//! unwarped). Opening another project empties the cache.
//!
//! # Commands
//!
//! * **Warp panel apply** sets the panel's values on the selected audio clips (markers stay).
//! * **Detect tempo** and **Warp to Song Tempo** read the first (each) selected clip's audio,
//!   find its transients ([`atomik_dsp::detect_onsets`]) and estimate a tempo
//!   ([`atomik_model::estimate_bpm`]); a clip whose span is a whole number of beats at that tempo
//!   gets the exact tempo of its length ([`atomik_model::bpm_for_beats`]).
//! * **Warp Loop as N Bars** says the asset from the clip's offset to its end is N bars: the tempo
//!   follows from the length and the clip becomes N bars long, warped.
//! * **Quantize Audio** detects the transients of each selected clip and sets warp markers that
//!   move them to the grid ([`atomik_model::quantize_audio`]) with the grid and strength of the
//!   quantize options.
//!
//! Each is one undo step however many clips are selected.

use super::audio_edit_ops::{Selected, for_each_chunk};
use super::*;
use crate::audio_import::spawn_warp;
use atomik_engine::SongAsset;
use atomik_model::{
    AudioClipContent, AudioQuantize, ClipContent, WarpJob, bpm_for_beats, estimate_bpm,
    quantize_audio, warp_jobs,
};
use atomik_render::warp::job_bytes;
use atomik_ui::WarpValues;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Most warp buffers rendered at the same time.
pub const MAX_WARP_WORKERS: usize = 2;
/// Most audio the warp cache holds, in bytes (finished buffers and running jobs).
pub const WARP_CACHE_BYTES: u64 = 512 << 20;
/// Longest stretch of a source the detectors read, in seconds.
const DETECT_SECONDS: u64 = 300;
/// Onset detector sensitivity for tempo and quantize (`0..=1`).
const SENSITIVITY: f32 = 0.5;

/// The warp workers of an open project.
#[derive(Default)]
pub(super) struct WarpWork {
    /// Running jobs by key, with the flag that stops them.
    inflight: BTreeMap<u64, Arc<AtomicBool>>,
    /// Jobs waiting for a worker.
    queue: Vec<(u64, WarpJob)>,
    /// A job was skipped for the memory budget (reported once).
    over_budget: bool,
}

impl WarpWork {
    /// Stops every worker and forgets the queue (the project changed).
    pub(super) fn reset(&mut self) {
        for flag in self.inflight.values() {
            flag.store(true, Ordering::Relaxed);
        }
        self.inflight.clear();
        self.queue.clear();
        self.over_budget = false;
    }

    /// Jobs running or waiting.
    pub(super) fn busy(&self) -> usize {
        self.inflight.len() + self.queue.len()
    }
}

/// Channels the warp buffer of `job` has (the asset's, at most two).
fn channels_of(assets: &BTreeMap<String, SongAsset>, job: &WarpJob) -> u16 {
    assets.get(&job.asset).map_or(2, SongAsset::channels)
}

impl<S: EngineSink> AppCore<S> {
    /// Brings the warp cache in line with the document (see the module docs).
    pub(super) fn sync_warps(&mut self) {
        let needed = warp_jobs(self.doc());
        self.audio.warps.retain(|k, _| needed.contains_key(k));
        let work = &mut self.audio.warp_work;
        for (key, flag) in &work.inflight {
            if !needed.contains_key(key) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        work.queue.retain(|(k, _)| needed.contains_key(k));
        let mut bytes: u64 = self
            .audio
            .warps
            .values()
            .map(|a| a.frames() * u64::from(a.channels().clamp(1, 2)) * 4)
            .sum();
        for (key, job) in needed {
            let work = &self.audio.warp_work;
            if self.audio.warps.contains_key(&key)
                || work.inflight.contains_key(&key)
                || work.queue.iter().any(|(k, _)| *k == key)
            {
                continue;
            }
            // Without its asset (not loaded yet, or missing) the clip stays silent or dry; the
            // load of the asset submits the song again.
            if !self.audio.assets.contains_key(&job.asset) {
                continue;
            }
            let size = job_bytes(&job, channels_of(&self.audio.assets, &job));
            let pending: u64 = self
                .audio
                .warp_work
                .queue
                .iter()
                .map(|(_, j)| job_bytes(j, channels_of(&self.audio.assets, j)))
                .sum();
            if bytes + pending + size > WARP_CACHE_BYTES {
                if !self.audio.warp_work.over_budget {
                    self.audio.warp_work.over_budget = true;
                    self.reject(
                        "Warped audio is over the memory budget: some clips play unwarped"
                            .to_owned(),
                    );
                }
                continue;
            }
            bytes += size;
            self.audio.warp_work.queue.push((key, job));
        }
        self.start_warps();
    }

    /// Starts queued jobs while there are free workers.
    fn start_warps(&mut self) {
        while self.audio.warp_work.inflight.len() < MAX_WARP_WORKERS {
            let Some((key, job)) = self.audio.warp_work.queue.pop() else {
                break;
            };
            let Some(asset) = self.audio.assets.get(&job.asset).cloned() else {
                continue;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let started = spawn_warp(
                self.audio.tx.clone(),
                self.audio.generation,
                job,
                asset,
                Arc::clone(&cancel),
            );
            match started {
                Ok(()) => {
                    self.audio.pending += 1;
                    self.audio.warp_work.inflight.insert(key, cancel);
                }
                Err(e) => self.reject(format!("warp: cannot start the renderer: {e}")),
            }
        }
    }

    /// A worker finished: keeps the buffer when it is still wanted. True when the song must be
    /// submitted again to play it.
    pub(super) fn apply_warped(&mut self, key: u64, result: Option<SongAsset>) -> bool {
        let cancelled = self
            .audio
            .warp_work
            .inflight
            .remove(&key)
            .is_none_or(|flag| flag.load(Ordering::Relaxed));
        let wanted = !cancelled && warp_jobs(self.doc()).contains_key(&key);
        self.start_warps();
        match result {
            Some(buffer) if wanted => {
                self.audio.warps.insert(key, buffer);
                true
            }
            _ => false,
        }
    }

    /// True while warp buffers are rendering or queued.
    pub fn warp_busy(&self) -> bool {
        self.audio.warp_work.busy() > 0
    }

    // ------------------------------------------------------------------ the commands

    /// What the warp panel's APPLY works on, and whether it can.
    pub fn warp_target(&self) -> (String, bool) {
        self.strip_silence_target()
    }

    /// The panel's values for the first selected audio clip.
    pub fn warp_values(&self) -> Option<WarpValues> {
        self.selected_audio()
            .first()
            .map(|s| WarpValues::from_settings(&s.content.warp))
    }

    /// The command that sets the content of selected clips (`None` content: unchanged).
    fn set_contents(&mut self, changes: Vec<(&Selected, AudioClipContent, Option<i64>)>) {
        let song = &self.doc().song;
        let commands: Vec<Command> = changes
            .into_iter()
            .filter_map(|(s, content, length)| {
                let (_, _, clip) = song.clip(s.id)?;
                (content != s.content || length.is_some_and(|l| l != clip.length)).then(|| {
                    Command::Song(SongCommand::SetAudioClip {
                        id: s.id,
                        start: clip.start,
                        length: length.unwrap_or(clip.length),
                        muted: clip.muted,
                        content,
                        merge: None,
                    })
                })
            })
            .collect();
        if let Some(command) = Self::one_step(commands) {
            let _ = self.apply_command(command);
        }
    }

    /// Sets the panel's values on every selected audio clip (a reversed clip cannot be warped:
    /// it keeps its settings), as one undo step.
    pub fn apply_warp(&mut self, values: &WarpValues) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to warp".to_owned());
        }
        let mut skipped = 0;
        let mut changes = Vec::new();
        for s in &clips {
            let mut content = s.content.clone();
            content.warp = values.apply_to(&s.content.warp);
            if content.reverse && content.warp.active() {
                skipped += 1;
                continue;
            }
            changes.push((s, content, None));
        }
        if changes.is_empty() {
            return self.reject("A reversed clip cannot be warped or transposed".to_owned());
        }
        self.set_contents(changes);
        if skipped > 0 {
            self.reject(format!("{skipped} reversed clip(s) left unwarped"));
        }
    }

    /// Reset to Unstretched Tempo: warp off and the markers cleared on every selected audio clip,
    /// as one undo step. A warped clip is resized to play the same stretch of audio at its own
    /// speed; mode, source tempo and transposition are kept.
    pub fn reset_warp(&mut self) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to reset".to_owned());
        }
        let rate = self.doc().sample_rate;
        let song = &self.doc().song;
        let changes = clips
            .iter()
            .map(|s| {
                let was_warped = s.content.warp.enabled;
                let mut content = s.content.clone();
                content.warp.enabled = false;
                content.warp.markers.clear();
                let length = if was_warped && !content.reverse {
                    let span = s.source_end(rate).saturating_sub(content.source_offset);
                    song.clip(s.id).map(|(_, _, clip)| {
                        crate::audio_import::ticks_for_frames(song, clip.start, span, rate)
                    })
                } else {
                    None
                };
                (s, content, length)
            })
            .collect();
        self.set_contents(changes);
    }

    /// The samples a detector reads for `s`: the mono mix of its source span (at most
    /// [`DETECT_SECONDS`]), with the asset frame of its first sample.
    fn detector_samples(&self, s: &Selected) -> Option<(Vec<f32>, u64)> {
        let asset = self.audio.assets.get(&s.content.asset)?;
        let rate = self.doc().sample_rate;
        let from = s.content.source_offset;
        let to = s
            .source_end(rate)
            .min(from + DETECT_SECONDS * u64::from(rate));
        let mut mono: Vec<f32> = Vec::new();
        for_each_chunk(asset, from, to, |planes| {
            let n = planes.first().map_or(0, |p| p.len());
            let k = planes.len().max(1) as f32;
            mono.extend((0..n).map(|i| planes.iter().map(|p| p[i]).sum::<f32>() / k));
        });
        (!mono.is_empty()).then_some((mono, from))
    }

    /// The tempo of the audio `s` plays: the onset-based estimate, snapped to the exact tempo of
    /// the clip's source span when that holds a whole number of beats near the estimate.
    fn detect_tempo_of(&self, s: &Selected) -> Option<f64> {
        let rate = self.doc().sample_rate;
        let (mono, _) = self.detector_samples(s)?;
        let onsets = atomik_dsp::detect_onsets(&[&mono], rate, SENSITIVITY);
        let bpm = estimate_bpm(&onsets, rate)?;
        let frames = mono.len() as u64;
        let beats = (frames as f64 * bpm / (60.0 * f64::from(rate))).round();
        match bpm_for_beats(frames, beats, rate) {
            Some(exact) if beats >= 1.0 && (exact - bpm).abs() / bpm < 0.03 => Some(exact),
            _ => Some(bpm),
        }
    }

    /// Estimates the source tempo of the first selected audio clip, in tenths of a BPM, for the
    /// warp panel's DETECT TEMPO.
    pub fn detect_warp_tempo(&mut self) -> Option<u32> {
        let clips = self.selected_audio();
        let Some(first) = clips.first() else {
            self.reject("Select an audio clip to listen to".to_owned());
            return None;
        };
        match self.detect_tempo_of(first) {
            Some(bpm) => Some((bpm * 10.0).round() as u32),
            None => {
                self.reject("No tempo found in the audio".to_owned());
                None
            }
        }
    }

    /// Warps every selected audio clip at the tempo found in its audio (the song tempo at the
    /// clip's start when none is found), as one undo step.
    pub fn warp_to_song_tempo(&mut self) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to warp".to_owned());
        }
        let song = &self.doc().song;
        let mut changes = Vec::new();
        let (mut guessed, mut reversed) = (0, 0);
        for s in &clips {
            if s.content.reverse {
                reversed += 1;
                continue;
            }
            let Some((_, _, clip)) = song.clip(s.id) else {
                continue;
            };
            let found = self.detect_tempo_of(s);
            let bpm = found.unwrap_or_else(|| {
                guessed += 1;
                song.tempo_table().map().bpm(clip.start as f64)
            });
            let mut content = s.content.clone();
            content.warp.enabled = true;
            content.warp.source_bpm =
                bpm.clamp(atomik_model::WARP_BPM_MIN, atomik_model::WARP_BPM_MAX);
            changes.push((s, content, None));
        }
        if changes.is_empty() {
            return self.reject("A reversed clip cannot be warped".to_owned());
        }
        self.set_contents(changes);
        if guessed > 0 {
            self.reject(format!(
                "No tempo found in {guessed} clip(s): they were set to the song tempo, edit it in Warp Settings"
            ));
        } else if reversed > 0 {
            self.reject(format!("{reversed} reversed clip(s) left unwarped"));
        }
    }

    /// Says the asset from each selected clip's offset to its end is `bars` bars: the source
    /// tempo follows from its length, the clip becomes `bars` bars long and warps, as one undo
    /// step.
    pub fn warp_bars(&mut self, bars: u32) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to warp".to_owned());
        }
        let rate = self.doc().sample_rate;
        let song = &self.doc().song;
        let (beats_per_bar, _) = song.meter();
        let beats = f64::from(bars) * f64::from(beats_per_bar);
        let mut changes = Vec::new();
        let mut failed = 0;
        for s in &clips {
            let Some(meta) = self.doc().assets.get(&s.content.asset) else {
                failed += 1;
                continue;
            };
            let frames = meta.frames.saturating_sub(s.content.source_offset);
            let (Some(bpm), false) = (bpm_for_beats(frames, beats, rate), s.content.reverse) else {
                failed += 1;
                continue;
            };
            let mut content = s.content.clone();
            content.warp.enabled = true;
            content.warp.source_bpm = bpm;
            content.warp.markers.clear();
            let length = i64::from(bars) * song.bar_ticks();
            changes.push((s, content, Some(length)));
        }
        if changes.is_empty() {
            return self.reject(
                "That many bars gives a tempo outside 20 to 999 BPM for this audio".to_owned(),
            );
        }
        self.set_contents(changes);
        if failed > 0 {
            self.reject(format!(
                "{failed} clip(s) could not be warped as {bars} bars"
            ));
        }
    }

    /// Moves the transients of every selected audio clip to the grid with warp markers, as one
    /// undo step. `grid` is in ticks and `strength` in `0..=1`.
    pub fn quantize_audio_clips(&mut self, grid: i64, strength: f32) {
        let clips = self.selected_audio();
        if clips.is_empty() {
            return self.reject("Select an audio clip to quantize".to_owned());
        }
        let rate = self.doc().sample_rate;
        let song = &self.doc().song;
        let mut changes = Vec::new();
        let mut why: Option<String> = None;
        for s in &clips {
            let Some((_, _, clip)) = song.clip(s.id) else {
                continue;
            };
            let ClipContent::Audio(_) = &clip.content else {
                continue;
            };
            let Some((mono, from)) = self.detector_samples(s) else {
                why = Some("The audio file is missing".to_owned());
                continue;
            };
            let onsets: Vec<u64> = atomik_dsp::detect_onsets(&[&mono], rate, SENSITIVITY)
                .into_iter()
                .map(|o| from + o)
                .collect();
            match quantize_audio(
                song,
                rate,
                clip.start,
                clip.length,
                &s.content,
                &onsets,
                &AudioQuantize {
                    grid,
                    strength: f64::from(strength),
                },
            ) {
                Ok(content) => changes.push((s, content, None)),
                Err(e) => why = Some(e),
            }
        }
        if changes.is_empty() {
            return self.reject(why.unwrap_or_else(|| "Nothing to quantize".to_owned()));
        }
        self.set_contents(changes);
    }
}
