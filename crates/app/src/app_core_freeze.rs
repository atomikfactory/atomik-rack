//! Freeze and bounce in place in the app core.
//!
//! # Freeze
//!
//! **Freeze Track** renders the selected tracks (or the tracks of the selected clips) through
//! their instrument and their whole insert chain; **Freeze Track up to Selected Device** stops at
//! the rack's selected device (the instrument, or one of the insert effects of the track), so the
//! effects after it stay live. The render covers the span of the track's clips, plus a tail that is
//! kept while it is audible (**Freeze with Tail**, on by default, at most
//! [`FREEZE_TAIL_SECONDS`]). It runs on a worker thread ([`crate::freeze_worker`]) through the
//! offline bounce path, one track at a time (the others wait in a queue), with progress in the
//! status line and **Cancel Freeze**; the song keeps playing meanwhile. When it is done the audio
//! is registered in the project's pool (the project bundle, or the session's scratch bundle before
//! the first save) and one `FreezeTrack` step freezes the track: the instrument and the frozen
//! effects leave the engine, a player plays the render, and the engine restarts (playback stops).
//!
//! A render that was asked for is thrown away if the track, its chain, the tempo or the meter
//! changed while it ran (the audio would not match the track any more); the message says so.
//!
//! **Unfreeze Track** brings the original back (one undo step per track); **Unfreeze Track and
//! Keep Audio** also leaves the render as a clip on a new audio track. The rendered file goes at
//! the next save unless a clip or the undo history still uses it.
//!
//! # Bounce in place
//!
//! **Bounce in Place** renders the selected clips (note or audio clips of the main lane) of each
//! track through its instrument and its whole chain into **one clip on a new audio track** that
//! starts where the first of them does; the originals stay (**Bounce in Place and Mute
//! Originals** mutes them in the same step). It is the additive twin of a freeze: nothing is
//! frozen, nothing leaves the engine, and the sends and the rest of the channel strip apply to
//! the new track like to any.
//!
//! The commands are in the Track menu, the track header menu and (bounce) the clip menu, and in
//! the registry as `sequencer.freeze_track`, `sequencer.freeze_track_to_device`,
//! `sequencer.freeze_tail`, `sequencer.freeze_cancel`, `sequencer.unfreeze_track`,
//! `sequencer.unfreeze_track_keep`, `sequencer.bounce_in_place` and
//! `sequencer.bounce_in_place_mute`.

use super::*;
use crate::commands::FreezeMenu;
use crate::freeze_worker::{self, Done, PluginEnv, Work};
use atomik_model::{
    AssetMeta, AudioClipContent, Cable, ClipContent, Frozen, MeterEvent, TempoEvent, Track,
    clips_span, track_chain,
};
use atomik_render::freeze::FreezeRequest;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The longest tail a freeze keeps past the end of the clips, in seconds.
pub const FREEZE_TAIL_SECONDS: f64 = 10.0;

/// How far a freeze goes along a track's chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeThrough {
    /// The instrument and every effect of the insert chain.
    Track,
    /// Up to the rack's selected device: the instrument, or one of the insert effects.
    SelectedDevice,
}

/// What a render stands for.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// Freeze the track through its first `effects` insert effects.
    Freeze,
    /// Add the render as a new audio track; mute these clips (the originals) with it.
    Bounce { mute: Vec<ClipId> },
}

/// A render waiting for its turn.
#[derive(Debug, Clone, PartialEq)]
struct Job {
    id: u64,
    kind: Kind,
    request: FreezeRequest,
    /// The track's display name.
    name: String,
}

/// What a render depends on, to tell whether the document still matches it when it is done.
type Print = (
    Track,
    Vec<TempoEvent>,
    Vec<MeterEvent>,
    Vec<atomik_model::RackDevice>,
    Vec<Cable>,
);

/// The render that is running.
struct Running {
    job: Job,
    print: Print,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// The freeze state of an open project.
pub(super) struct FreezeState {
    running: Option<Running>,
    queue: VecDeque<Job>,
    tail: bool,
    next_job: u64,
}

impl Default for FreezeState {
    fn default() -> Self {
        Self {
            running: None,
            queue: VecDeque::new(),
            tail: true,
            next_job: 1,
        }
    }
}

impl FreezeState {
    /// Stops the running render and forgets the queue (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.queue.clear();
    }
}

/// What the render of `device` depends on in `doc`: the track, the tempo map and meter, and the
/// instrument, the channel and the insert effects with the cables that touch them.
fn print_of(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> Option<Print> {
    let track = doc.song.track(device)?.clone();
    let chain = track_chain(doc, registry, device).ok()?;
    let keys: Vec<&DeviceKey> = std::iter::once(device)
        .chain(std::iter::once(&chain.channel))
        .chain(&chain.effects)
        .collect();
    Some((
        track,
        doc.song.tempo.clone(),
        doc.song.meter.clone(),
        keys.iter().filter_map(|k| doc.device(k).cloned()).collect(),
        doc.cables
            .iter()
            .filter(|c| keys.contains(&&c.from.device) || keys.contains(&&c.to.device))
            .cloned()
            .collect(),
    ))
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// True while a render runs or waits.
    pub fn freeze_busy(&self) -> bool {
        self.audio.freeze.running.is_some() || !self.audio.freeze.queue.is_empty()
    }

    /// The status-bar text of a running render: "Freezing Lead 37%" (and how many wait).
    pub fn freeze_progress(&self) -> Option<String> {
        let running = self.audio.freeze.running.as_ref()?;
        let fraction = f32::from_bits(running.progress.load(Ordering::Relaxed));
        let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
        let verb = match running.job.kind {
            Kind::Freeze => "Freezing",
            Kind::Bounce { .. } => "Bouncing",
        };
        let waiting = self.audio.freeze.queue.len();
        let more = if waiting > 0 {
            format!(" (+{waiting} waiting)")
        } else {
            String::new()
        };
        Some(format!("{verb} {} {percent}%{more}", running.job.name))
    }

    /// Whether a freeze keeps the tail of the chain (reverb and delay decay).
    pub fn freeze_tail(&self) -> bool {
        self.audio.freeze.tail
    }

    /// Switches the tail of the next freezes and bounces.
    pub fn toggle_freeze_tail(&mut self) {
        self.audio.freeze.tail = !self.audio.freeze.tail;
    }

    /// What the menus need to know about the selection and the render.
    pub fn freeze_menu(&self) -> FreezeMenu {
        let doc = self.doc();
        let busy = self.freeze_busy();
        let tracks = self.arrangement.command_tracks();
        let freezable = |d: &DeviceKey| {
            doc.song.track(d).is_some_and(|t| {
                t.frozen.is_none()
                    && clips_span(t).is_some()
                    && track_chain(doc, &self.registry, d).is_ok()
            })
        };
        let bounceable = self.arrangement.selected_clips().iter().any(|id| {
            doc.song.clip(*id).is_some_and(|(t, lane, c)| {
                t.frozen.is_none()
                    && lane == 0
                    && !matches!(c.content, ClipContent::Automation(_))
                    && track_chain(doc, &self.registry, &t.device).is_ok()
            })
        });
        FreezeMenu {
            can_freeze: !busy && tracks.iter().any(freezable),
            can_unfreeze: tracks
                .iter()
                .any(|d| doc.song.track(d).is_some_and(|t| t.frozen.is_some())),
            can_bounce: !busy && bounceable,
            busy,
            tail: self.audio.freeze.tail,
        }
    }

    // ------------------------------------------------------------------ commands

    /// Freezes the selected tracks (see the module docs).
    pub fn freeze_tracks(&mut self, through: FreezeThrough) {
        let tracks = self.arrangement.command_tracks();
        if tracks.is_empty() {
            return self.reject("Select a track to freeze".to_owned());
        }
        let mut jobs = Vec::new();
        let mut skipped = Vec::new();
        for device in &tracks {
            match self.plan_freeze(device, through) {
                Ok(job) => jobs.push(job),
                Err(why) => skipped.push(why),
            }
        }
        if jobs.is_empty() {
            return self.reject(skipped.join(" \u{b7} "));
        }
        self.audio.freeze.queue.extend(jobs);
        self.start_next_job();
        if !skipped.is_empty() {
            self.reject(skipped.join(" \u{b7} "));
        }
    }

    /// Unfreezes the selected frozen tracks, one undo step each; with `keep_audio` the render
    /// also becomes a clip on a new audio track.
    pub fn unfreeze_tracks(&mut self, keep_audio: bool) {
        let frozen: Vec<DeviceKey> = self
            .arrangement
            .command_tracks()
            .into_iter()
            .filter(|d| self.doc().song.track(d).is_some_and(|t| t.frozen.is_some()))
            .collect();
        if frozen.is_empty() {
            return self.reject("Select a frozen track to unfreeze".to_owned());
        }
        for device in frozen {
            let _ = self.apply_command(Command::Song(SongCommand::UnfreezeTrack {
                device,
                keep_audio,
            }));
        }
    }

    /// Renders the selected clips of each track through the track's chain into a new audio track
    /// (see the module docs); with `mute_originals` the clips are muted in the same step.
    pub fn bounce_in_place(&mut self, mute_originals: bool) {
        let selected = self.arrangement.selected_clips();
        if selected.is_empty() {
            return self.reject("Select the clips to bounce".to_owned());
        }
        let doc = self.doc();
        let mut per_track: Vec<(DeviceKey, Vec<ClipId>, i64, i64)> = Vec::new();
        let mut skipped = Vec::new();
        for id in selected {
            let Some((track, lane, clip)) = doc.song.clip(id) else {
                continue;
            };
            let name = doc
                .device(&track.device)
                .map_or_else(|| track.device.0.clone(), |d| d.name.clone());
            if track.frozen.is_some() {
                skipped.push(format!("{name} is frozen: unfreeze it to bounce its clips"));
                continue;
            }
            if lane != 0 || matches!(clip.content, ClipContent::Automation(_)) {
                skipped.push("Only note and audio clips can be bounced".to_owned());
                continue;
            }
            match per_track.iter_mut().find(|(d, ..)| *d == track.device) {
                Some((_, ids, start, end)) => {
                    ids.push(id);
                    *start = (*start).min(clip.start);
                    *end = (*end).max(clip.end());
                }
                None => per_track.push((track.device.clone(), vec![id], clip.start, clip.end())),
            }
        }
        skipped.sort();
        skipped.dedup();
        let mut jobs = Vec::new();
        let tail = self.tail_seconds();
        for (device, ids, start, end) in per_track {
            let name = self.track_name(&device);
            let chain = match track_chain(self.doc(), &self.registry, &device) {
                Ok(chain) => chain,
                Err(why) => {
                    skipped.push(why);
                    continue;
                }
            };
            let id = self.next_freeze_job();
            jobs.push(Job {
                id,
                kind: Kind::Bounce {
                    mute: if mute_originals {
                        ids.clone()
                    } else {
                        Vec::new()
                    },
                },
                request: FreezeRequest {
                    device,
                    effects: chain.effects.len(),
                    clips: Some(ids),
                    span: (start, end),
                    tail_seconds: tail,
                },
                name,
            });
        }
        if jobs.is_empty() {
            return self.reject(skipped.join(" \u{b7} "));
        }
        self.audio.freeze.queue.extend(jobs);
        self.start_next_job();
        if !skipped.is_empty() {
            self.reject(skipped.join(" \u{b7} "));
        }
    }

    /// Stops the running render and drops the ones that wait.
    pub fn cancel_freeze(&mut self) {
        let waiting = std::mem::take(&mut self.audio.freeze.queue);
        match &self.audio.freeze.running {
            Some(r) => r.cancel.store(true, Ordering::Relaxed),
            None if waiting.is_empty() => self.reject("Nothing is being frozen".to_owned()),
            None => {}
        }
    }

    // ------------------------------------------------------------------ the queue

    fn tail_seconds(&self) -> f64 {
        if self.audio.freeze.tail {
            FREEZE_TAIL_SECONDS
        } else {
            0.0
        }
    }

    fn next_freeze_job(&mut self) -> u64 {
        let id = self.audio.freeze.next_job;
        self.audio.freeze.next_job += 1;
        id
    }

    fn track_name(&self, device: &DeviceKey) -> String {
        self.doc()
            .device(device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone())
    }

    /// The job that freezes `device` as far as `through` says, or why it cannot.
    fn plan_freeze(&mut self, device: &DeviceKey, through: FreezeThrough) -> Result<Job, String> {
        let name = self.track_name(device);
        let doc = self.doc();
        let track = doc
            .song
            .track(device)
            .ok_or_else(|| format!("{name} has no track"))?;
        if track.frozen.is_some() {
            return Err(format!("{name} is frozen already"));
        }
        let chain = track_chain(doc, &self.registry, device)?;
        let effects = match through {
            FreezeThrough::Track => chain.effects.len(),
            FreezeThrough::SelectedDevice => match self.view.selected() {
                Some(key) if key == device => 0,
                Some(key) => chain
                    .effects
                    .iter()
                    .position(|e| e == key)
                    .map(|i| i + 1)
                    .ok_or_else(|| format!("{key} is not an insert effect of {name}"))?,
                None => {
                    return Err(
                        "Select the instrument or an insert effect of the track in the rack"
                            .to_owned(),
                    );
                }
            },
        };
        let span = clips_span(track).ok_or_else(|| format!("{name} has no clips to freeze"))?;
        let request = FreezeRequest {
            device: device.clone(),
            effects,
            clips: None,
            span,
            tail_seconds: self.tail_seconds(),
        };
        let id = self.next_freeze_job();
        Ok(Job {
            id,
            kind: Kind::Freeze,
            request,
            name,
        })
    }

    /// Starts the next waiting render when none runs.
    fn start_next_job(&mut self) {
        if self.audio.freeze.running.is_some() {
            return;
        }
        while let Some(job) = self.audio.freeze.queue.pop_front() {
            let doc = self.doc().clone();
            let Some(print) = print_of(&doc, &self.registry, &job.request.device) else {
                self.reject(format!("{} cannot be frozen any more", job.name));
                continue;
            };
            let bundle = match self.import_bundle() {
                Ok(b) => b,
                Err(e) => {
                    self.reject(e);
                    continue;
                }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
            let label = match job.kind {
                Kind::Freeze => "frozen",
                Kind::Bounce { .. } => "bounce",
            };
            let work = Work {
                doc,
                request: job.request.clone(),
                bundle,
                threshold: self.stream_threshold(),
                name: format!("{} ({label}).wav", job.name),
                plugins: PluginEnv {
                    cache: self.plugins.cache.clone(),
                    settings: self.plugins.settings.clone(),
                    isolation: self.plugins.freeze_isolation.clone(),
                },
            };
            let started = freeze_worker::spawn(
                self.audio.tx.clone(),
                (self.audio.generation, job.id),
                work,
                (Arc::clone(&cancel), Arc::clone(&progress)),
            );
            match started {
                Ok(()) => {
                    self.audio.pending += 1;
                    self.audio.freeze.running = Some(Running {
                        job,
                        print,
                        cancel,
                        progress,
                    });
                    return;
                }
                Err(e) => self.reject(format!("cannot start the render: {e}")),
            }
        }
    }

    /// A render finished (or failed, or was stopped): puts it in the document, then starts the
    /// next one.
    pub(super) fn apply_frozen(&mut self, job: u64, result: Result<Option<Done>, String>) {
        let Some(running) = self.audio.freeze.running.take_if(|r| r.job.id == job) else {
            return;
        };
        let Running { job, print, .. } = running;
        match result {
            Err(e) => self.reject(format!("{}: {e}", job.name)),
            Ok(None) => self.reject(format!("Freeze of {} cancelled", job.name)),
            Ok(Some(done)) => self.finish_render(job, &print, done),
        }
        self.start_next_job();
    }

    /// Registers the render and applies the command that uses it.
    fn finish_render(&mut self, job: Job, print: &Print, done: Done) {
        let device = job.request.device.clone();
        if print_of(self.doc(), &self.registry, &device).as_ref() != Some(print) {
            return self.reject(format!(
                "{} changed while it was rendered: nothing was changed, render it again",
                job.name
            ));
        }
        let Some(info) = done.loaded.info.clone() else {
            return self.reject("the render has no file".to_owned());
        };
        self.insert_loaded(&done.loaded);
        let registered = self.apply_command(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        });
        if registered.is_err() {
            return;
        }
        let audio = AudioClipContent::new(info.hash, 0);
        let tail = (done.tail_frames > 0).then(|| {
            format!(
                " with {:.1} s of tail",
                done.tail_frames as f64 / f64::from(self.doc().sample_rate)
            )
        });
        match job.kind {
            Kind::Freeze => {
                let effects = track_chain(self.doc(), &self.registry, &device)
                    .map(|c| c.effects[..job.request.effects.min(c.effects.len())].to_vec())
                    .unwrap_or_default();
                let applied = self.apply_command(Command::Song(SongCommand::FreezeTrack {
                    device,
                    frozen: Frozen {
                        start: done.start,
                        length: done.length,
                        audio,
                        effects,
                    },
                }));
                if applied.is_ok() {
                    self.reject(format!("Froze {}{}", job.name, tail.unwrap_or_default()));
                }
            }
            Kind::Bounce { mute } => {
                let applied = self.apply_command(Command::Song(SongCommand::AddRenderedTrack {
                    name: Some(format!("{} (bounce)", job.name)),
                    start: done.start,
                    length: done.length,
                    audio,
                    mute,
                }));
                if applied.is_ok() {
                    if let Some(id) = self.document.last_clip_id() {
                        self.arrangement.set_selected_clips(&[id]);
                    }
                    self.reject(format!(
                        "Bounced {} to a new track{}",
                        job.name,
                        tail.unwrap_or_default()
                    ));
                }
            }
        }
        for warning in done.warnings {
            self.reject(warning);
        }
    }
}
