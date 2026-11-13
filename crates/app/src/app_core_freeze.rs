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
//! **Re-render Frozen Track** (`sequencer.refreeze_track`) renders a
//! frozen track again, through the same part of its chain, without unfreezing it first: the track
//! stays frozen, plays the old render until the new one is done, and one `RefreezeTrack` step
//! swaps them. It is for what the render depends on outside the track (a source that keys the
//! chain, the tail setting, a plugin that was updated). The worker renders a copy of the document
//! in which the track is unfrozen, so its devices run for the render.
//!
//! # Bounce in place
//!
//! **Bounce in Place** renders the selected clips (note clips of any note lane, or audio clips; the
//! clips of a muted lane still render) of each track through its instrument and its whole chain
//! into **one clip on a new audio track** that starts where the first of them does; the
//! originals stay (**Bounce in Place and Mute Originals** mutes them in the same step). It is the additive twin of a freeze: nothing is
//! frozen, nothing leaves the engine, and the sends and the rest of the channel strip apply to
//! the new track like to any.
//!
//! **Bounce Range in Place** (`sequencer.bounce_range`) renders a
//! time range instead of whole clips: the range selected on a lane of a track, or else the range
//! between the locators on each selected track. Everything the track plays in the range goes
//! through its chain into one clip on a new audio track that starts at the beginning of the range;
//! a clip that crosses an edge of the range contributes the part inside. The originals stay.
//!
//! **Bounce Clips to New Track (Dry)** does the same through no insert effect: the render is the
//! instrument's own output (an audio clip's gain, fades and warp included), before the effects
//! and the channel strip, so the new track holds the clip's sound alone.
//!
//! # Convert Loop to One Clip (audio)
//!
//! A looped audio clip has no content to unroll, so `sequencer.convert_loop` with `single` renders
//! it: each looped audio clip among the selected ones is bounced as the track plays it (its gain,
//! fades, envelope and warp, the repetitions one after the other, before the insert effects and
//! the strip), on the same worker and queue as a freeze, and the clip becomes one plain clip of
//! that render in one `RenderLoop` step. The clip keeps its id, place, name and colour. Looped
//! note and automation clips among the selected ones convert at once, as before.
//!
//! # Join Clips of different files (audio)
//!
//! Join on audio clips of one lane that play more than one file renders them (the same dry render
//! as the loop conversion: each clip's gain, fades, envelope and warp, the crossfades between
//! them, before the insert effects and the strip) on the freeze worker and queue, and the first
//! clip becomes one plain clip of that render, spanning all of them, in one `RenderJoin` step. The
//! other clips go. Clips of one file still join as they are, without a render. A muted clip, a gap
//! and a frozen track refuse with a message.
//!
//! The commands are in the Track menu, the track header menu and (bounce) the clip menu, and in
//! the registry as `sequencer.freeze_track`, `sequencer.freeze_track_to_device`,
//! `sequencer.freeze_tail`, `sequencer.freeze_cancel`, `sequencer.unfreeze_track`,
//! `sequencer.unfreeze_track_keep`, `sequencer.bounce_in_place`,
//! `sequencer.bounce_in_place_mute`, `sequencer.bounce_range`, `sequencer.refreeze_track` and
//! `sequencer.bounce_clip_dry`.

use super::*;
use crate::commands::FreezeMenu;
use crate::freeze_worker::{self, Done, PluginEnv, Post, Work};
use atomik_model::{
    AssetMeta, AudioClipContent, Cable, ClipContent, Frozen, LoopConvert, MeterEvent, TempoEvent,
    Track, WorkflowCommand, clips_span, strip_blocker, track_chain,
};
use atomik_render::freeze::FreezeRequest;
use std::collections::{BTreeSet, VecDeque};
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
    /// Add the render as a new audio track; mute these clips (the originals) with it. `dry`: the
    /// render is the instrument's own output, before the insert effects and the strip.
    Bounce { mute: Vec<ClipId>, dry: bool },
    /// Replace the looped audio clip by one clip of the render (dry, the clip alone).
    Loop { clip: ClipId },
    /// Replace these audio clips of different files by one clip of the render (dry, the clips
    /// alone).
    Join { clips: Vec<ClipId> },
    /// Flatten Comp: the render of the comp of this take group (dry) replaces the group.
    FlattenComp { group: u32 },
    /// Render the frozen track again through the same effects and swap the render in.
    Refreeze,
    /// Build a rumble layer from the render (a kick) and put it on a new track after the kick's
    /// (see [`rumble`]). `only_key` keeps only the notes on that key in the render (the kick pad
    /// of a drum machine); `hits` are the frames of the hits (`None`: found in the audio).
    Rumble {
        params: atomik_dsp::rumble::RumbleParams,
        options: atomik_model::rumble::RumbleTrackOptions,
        hits: Option<Vec<u64>>,
        only_key: Option<u8>,
    },
    /// Print the reverse reverb swell of the render (dry) on a new track so that it ends at the
    /// tick `anchor` (see [`rumble`]).
    ReverseReverb { seconds: f32, anchor: i64 },
    /// Print a riser: the reverse reverb swell of the render (the first hit of a section, dry),
    /// `seconds` long, on a new track that ends at the boundary `anchor` (see [`transition`]).
    Riser { seconds: f32, anchor: i64, bars: u8 },
    /// Print an impact: the render (the first hit of a section, dry) saturated, with a reverb of
    /// `decay` seconds, on a new track that starts at the boundary `at` (see [`transition`]).
    Impact { decay: f32, at: i64 },
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
    /// **Freeze with Strip**: the render is taken at the output of the channel.
    strip: bool,
    next_job: u64,
}

impl Default for FreezeState {
    fn default() -> Self {
        Self {
            running: None,
            queue: VecDeque::new(),
            tail: true,
            strip: false,
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

/// Takes the notes that are not on `key` out of the clips of the track `device` in `doc` (a copy
/// the worker renders): the track then plays the pad on that key alone.
fn keep_only_key(doc: &mut RackDoc, device: &DeviceKey, key: u8) {
    let Some(track) = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| &t.device == device)
    else {
        return;
    };
    for clip in track.lanes.iter_mut().flat_map(|l| &mut l.clips) {
        if let ClipContent::Notes(notes) = &mut clip.content {
            notes.retain(|n| n.key == key);
        }
    }
}

/// A track and the span `[start, end)` in ticks a range bounce renders on it.
type RangeJob = (DeviceKey, (i64, i64));

/// Whether `track` plays a note or audio clip somewhere in `[start, end)`.
fn plays_in(track: &Track, (start, end): (i64, i64)) -> bool {
    track.lanes.iter().flat_map(|l| &l.clips).any(|c| {
        !matches!(c.content, ClipContent::Automation(_)) && c.start < end && c.end() > start
    })
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
            Kind::Loop { .. } | Kind::Join { .. } | Kind::FlattenComp { .. } => "Rendering",
            Kind::Refreeze => "Re-rendering",
            Kind::Rumble { .. } => "Building a rumble from",
            Kind::ReverseReverb { .. } => "Printing a reverse reverb of",
            Kind::Riser { .. } => "Printing a riser from",
            Kind::Impact { .. } => "Printing an impact from",
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

    /// Whether Freeze Track bakes the channel strip into the render.
    pub fn freeze_strip(&self) -> bool {
        self.audio.freeze.strip
    }

    /// Switches **Freeze with Strip** (see the module docs).
    pub fn toggle_freeze_strip(&mut self) {
        self.audio.freeze.strip = !self.audio.freeze.strip;
    }

    /// What the menus need to know about the selection and the render.
    pub fn freeze_menu(&self) -> FreezeMenu {
        let doc = self.doc();
        let busy = self.freeze_busy();
        let tracks = self.arrangement.command_tracks();
        let freezable = |d: &DeviceKey| {
            doc.song.track(d).is_some_and(|t| {
                t.frozen.is_none()
                    && !t.is_automation_only()
                    && clips_span(t).is_some()
                    && track_chain(doc, &self.registry, d).is_ok()
            })
        };
        let bounceable = self.arrangement.selected_clips().iter().any(|id| {
            doc.song.clip(*id).is_some_and(|(t, _, c)| {
                t.frozen.is_none()
                    && !matches!(c.content, ClipContent::Automation(_))
                    && track_chain(doc, &self.registry, &t.device).is_ok()
            })
        });
        let (players, direct_record, players_folded, players_bypassed) = self.players_menu();
        let ranges = self.range_candidates().0;
        FreezeMenu {
            players,
            direct_record,
            players_folded,
            players_bypassed,
            can_freeze: !busy && tracks.iter().any(freezable),
            can_unfreeze: tracks
                .iter()
                .any(|d| doc.song.track(d).is_some_and(|t| t.frozen.is_some())),
            can_bounce: !busy && bounceable,
            can_bounce_range: !busy && !ranges.is_empty(),
            can_refreeze: !busy
                && tracks
                    .iter()
                    .any(|d| doc.song.track(d).is_some_and(|t| t.frozen.is_some())),
            busy,
            tail: self.audio.freeze.tail,
            strip: self.audio.freeze.strip,
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

    /// Freezes the track on `device` through its whole chain, whatever is selected (the freeze
    /// hint of the CPU readout uses it). A refusal is the toast the other freezes give.
    pub fn freeze_track_of(&mut self, device: &DeviceKey) {
        match self.plan_freeze(device, FreezeThrough::Track) {
            Ok(job) => {
                self.audio.freeze.queue.push_back(job);
                self.start_next_job();
            }
            Err(why) => self.reject(why),
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
        self.bounce_selected(mute_originals, false);
    }

    /// Renders the selected clips of each track as the instrument plays them, before its insert
    /// effects and its channel strip, into a new audio track (**Bounce Clips to New Track
    /// (Dry)**): the clip's own sound, to be treated again or kept as audio. The originals stay.
    pub fn bounce_clips_dry(&mut self) {
        self.bounce_selected(false, true);
    }

    fn bounce_selected(&mut self, mute_originals: bool, dry: bool) {
        let selected = self.arrangement.selected_clips();
        if selected.is_empty() {
            return self.reject("Select the clips to bounce".to_owned());
        }
        let doc = self.doc();
        let mut per_track: Vec<(DeviceKey, Vec<ClipId>, i64, i64)> = Vec::new();
        let mut skipped = Vec::new();
        for id in selected {
            let Some((track, _, clip)) = doc.song.clip(id) else {
                continue;
            };
            let name = doc
                .device(&track.device)
                .map_or_else(|| track.device.0.clone(), |d| d.name.clone());
            if track.frozen.is_some() {
                skipped.push(format!("{name} is frozen: unfreeze it to bounce its clips"));
                continue;
            }
            if matches!(clip.content, ClipContent::Automation(_)) {
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
                    dry,
                },
                request: FreezeRequest {
                    device,
                    effects: if dry { 0 } else { chain.effects.len() },
                    strip: false,
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

    /// Renders the selected frozen tracks again and swaps the new render in (see the module
    /// docs).
    pub fn refreeze_tracks(&mut self) {
        let frozen: Vec<DeviceKey> = self
            .arrangement
            .command_tracks()
            .into_iter()
            .filter(|d| self.doc().song.track(d).is_some_and(|t| t.frozen.is_some()))
            .collect();
        if frozen.is_empty() {
            return self.reject("Select a frozen track to render again".to_owned());
        }
        let mut jobs = Vec::new();
        let mut skipped = Vec::new();
        for device in &frozen {
            match self.plan_refreeze(device) {
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

    /// Renders the range of the selected lane, or else the range between the locators, of each
    /// selected track through its chain into a new audio track (see the module docs).
    pub fn bounce_range(&mut self) {
        let (spans, mut skipped) = self.range_candidates();
        if spans.is_empty() {
            return self.reject(skipped.join(" \u{b7} "));
        }
        let tail = self.tail_seconds();
        let mut jobs = Vec::new();
        for (device, span) in spans {
            let name = self.track_name(&device);
            let effects = match track_chain(self.doc(), &self.registry, &device) {
                Ok(chain) => chain.effects.len(),
                Err(why) => {
                    skipped.push(why);
                    continue;
                }
            };
            let id = self.next_freeze_job();
            jobs.push(Job {
                id,
                kind: Kind::Bounce {
                    mute: Vec::new(),
                    dry: false,
                },
                request: FreezeRequest {
                    device,
                    effects,
                    strip: false,
                    clips: None,
                    span,
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

    /// The tracks a range bounce renders with the span of each, and why the others are left out.
    /// The span is the range selected on a lane (the tracks of the lanes it covers) or the range
    /// between the locators on every selected track; a track qualifies when it is not frozen, has a
    /// chain and plays a note or audio clip inside the span.
    fn range_candidates(&self) -> (Vec<RangeJob>, Vec<String>) {
        let doc = self.doc();
        let wanted: Vec<RangeJob> = match self.arrangement.lane_ranges() {
            Some((lanes, start, end)) => {
                // Each track of the lanes the range covers renders once.
                let mut tracks: Vec<DeviceKey> = Vec::new();
                for (device, _) in lanes {
                    if !tracks.contains(&device) {
                        tracks.push(device);
                    }
                }
                tracks.into_iter().map(|d| (d, (start, end))).collect()
            }
            None => {
                let tracks = self.arrangement.command_tracks();
                let range = doc.song.loop_range;
                if tracks.is_empty() {
                    return (Vec::new(), vec!["Select a track to bounce".to_owned()]);
                }
                if range.0 >= range.1 {
                    let why = "The locators are at the same place: move them apart";
                    return (Vec::new(), vec![why.to_owned()]);
                }
                tracks.into_iter().map(|d| (d, range)).collect()
            }
        };
        let mut spans = Vec::new();
        let mut skipped = Vec::new();
        for (device, span) in wanted {
            let name = self.track_name(&device);
            let Some(track) = doc.song.track(&device) else {
                continue;
            };
            if track.frozen.is_some() {
                skipped.push(format!("{name} is frozen: unfreeze it to bounce"));
            } else if track.is_automation_only() {
                skipped.push("Only note and audio tracks can be bounced".to_owned());
            } else if let Err(why) = track_chain(doc, &self.registry, &device) {
                skipped.push(why);
            } else if !plays_in(track, span) {
                skipped.push(format!("{name} has nothing in the range"));
            } else {
                spans.push((device, span));
            }
        }
        skipped.sort();
        skipped.dedup();
        (spans, skipped)
    }

    /// Turns `ConvertLoop` to a single clip of looped audio clips into renders (see the module
    /// docs): returns the command to apply for what is left (the looped note and automation clips
    /// of the selection), or `None` when the renders were all there was.
    pub(super) fn render_audio_loops(&mut self, command: Command) -> Option<Command> {
        let Command::Workflow(WorkflowCommand::ConvertLoop {
            ids,
            mode: LoopConvert::Single,
        }) = &command
        else {
            return Some(command);
        };
        let doc = self.doc();
        let mut jobs = Vec::new();
        let mut skipped = Vec::new();
        let mut rest = Vec::new();
        let mut rest_loops = false;
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(*id) {
                continue;
            }
            let Some((track, _, clip)) = doc.song.clip(*id) else {
                rest.push(*id);
                continue;
            };
            if clip.loop_length.is_none() || !clip.content.is_audio() {
                rest_loops |= clip.loop_length.is_some();
                rest.push(*id);
                continue;
            }
            let name = doc
                .device(&track.device)
                .map_or_else(|| track.device.0.clone(), |d| d.name.clone());
            if track.frozen.is_some() {
                skipped.push(format!("{name} is frozen: unfreeze it to convert its loop"));
                continue;
            }
            jobs.push((track.device.clone(), *id, clip.start, clip.end(), name));
        }
        if jobs.is_empty() && skipped.is_empty() {
            return Some(command);
        }
        let jobs: Vec<Job> = jobs
            .into_iter()
            .map(|(device, clip, start, end, name)| Job {
                id: self.next_freeze_job(),
                kind: Kind::Loop { clip },
                request: FreezeRequest {
                    device,
                    effects: 0,
                    strip: false,
                    clips: Some(vec![clip]),
                    span: (start, end),
                    tail_seconds: 0.0,
                },
                name,
            })
            .collect();
        if !jobs.is_empty() {
            self.audio.freeze.queue.extend(jobs);
            self.start_next_job();
        }
        if !skipped.is_empty() {
            self.reject(skipped.join(" \u{b7} "));
        }
        rest_loops.then_some(Command::Workflow(WorkflowCommand::ConvertLoop {
            ids: rest,
            mode: LoopConvert::Single,
        }))
    }

    /// Turns `JoinClips` of audio clips of different files into a render (see the module docs):
    /// returns the command when the model is to judge it (anything that is not such a join), and
    /// `None` when a render was queued or the clips cannot be joined that way (the message says
    /// why).
    pub(super) fn render_clip_joins(&mut self, command: Command) -> Option<Command> {
        let Command::Workflow(WorkflowCommand::JoinClips { ids }) = &command else {
            return Some(command);
        };
        let Some(plan) = atomik_model::join_render(self.doc(), ids) else {
            return Some(command);
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(e) => {
                self.reject(e.to_string());
                return None;
            }
        };
        let name = self.track_name(&plan.device);
        if self
            .doc()
            .song
            .track(&plan.device)
            .is_some_and(|t| t.frozen.is_some())
        {
            self.reject(format!("{name} is frozen: unfreeze it to join its clips"));
            return None;
        }
        let job = Job {
            id: self.next_freeze_job(),
            kind: Kind::Join {
                clips: plan.ids.clone(),
            },
            request: FreezeRequest {
                device: plan.device,
                effects: 0,
                strip: false,
                clips: Some(plan.ids),
                span: plan.span,
                tail_seconds: 0.0,
            },
            name,
        };
        self.audio.freeze.queue.push_back(job);
        self.start_next_job();
        None
    }

    /// `UiAction::FlattenComp`: renders the comp of take group `group` (its compiled clips, dry:
    /// each take's gain, fades and envelope, the crossfades at the cuts, before the insert
    /// effects and the strip) to one new audio file on the freeze worker and queue, then replaces
    /// the group by one clip of that file in one undo step (`SongCommand::FlattenComp`). A group
    /// of note takes, an empty comp and a frozen track refuse with a message.
    pub fn flatten_comp(&mut self, group: u32) {
        let doc = self.doc();
        let Some((track, _, g)) = doc.song.take_group(group) else {
            return self.reject("That take group is gone".to_owned());
        };
        let name = self.track_name(&track.device);
        if track.frozen.is_some() {
            return self.reject(format!("{name} is frozen: unfreeze it to flatten its comp"));
        }
        if !g.takes.iter().all(|t| t.content.is_audio()) {
            return self.reject("Only a comp of audio takes can be rendered to a file".to_owned());
        }
        let (Some(first), Some(last)) = (g.comp.first(), g.comp.last()) else {
            return self.reject("The comp plays nothing: there is nothing to render".to_owned());
        };
        let (span, clips, device) = (
            (first.start, last.end),
            g.clips.clone(),
            track.device.clone(),
        );
        let job = Job {
            id: self.next_freeze_job(),
            kind: Kind::FlattenComp { group },
            request: FreezeRequest {
                device,
                effects: 0,
                strip: false,
                clips: Some(clips),
                span,
                tail_seconds: 0.0,
            },
            name,
        };
        self.audio.freeze.queue.push_back(job);
        self.start_next_job();
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

    /// The job that renders the frozen track `device` again through the effects it froze.
    fn plan_refreeze(&mut self, device: &DeviceKey) -> Result<Job, String> {
        let name = self.track_name(device);
        let doc = self.doc();
        let track = doc
            .song
            .track(device)
            .ok_or_else(|| format!("{name} has no track"))?;
        let frozen = track
            .frozen
            .as_ref()
            .ok_or_else(|| format!("{name} is not frozen"))?;
        let (effects, strip) = (frozen.effects.len(), frozen.strip);
        track_chain(doc, &self.registry, device)?;
        let span = clips_span(track).ok_or_else(|| format!("{name} has no clips to render"))?;
        let request = FreezeRequest {
            device: device.clone(),
            effects,
            strip,
            clips: None,
            span,
            tail_seconds: self.tail_seconds(),
        };
        let id = self.next_freeze_job();
        Ok(Job {
            id,
            kind: Kind::Refreeze,
            request,
            name,
        })
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
        if track.is_automation_only() {
            return Err(format!(
                "{name} is an automation track: there is nothing to freeze"
            ));
        }
        let chain = track_chain(doc, &self.registry, device)?;
        // The strip is baked in by a freeze of the whole track only.
        let strip = self.audio.freeze.strip && through == FreezeThrough::Track;
        if strip && let Some(why) = strip_blocker(doc, &self.registry, device) {
            return Err(why);
        }
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
            strip,
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
            let mut doc = self.doc().clone();
            let Some(print) = print_of(&doc, &self.registry, &job.request.device) else {
                self.reject(format!("{} cannot be frozen any more", job.name));
                continue;
            };
            if let Kind::Rumble {
                only_key: Some(key),
                ..
            } = &job.kind
            {
                keep_only_key(&mut doc, &job.request.device, *key);
            }
            if job.kind == Kind::Refreeze {
                // The worker renders the track as it plays when it is not frozen.
                let device = &job.request.device;
                if let Some(t) = doc
                    .song_mut()
                    .tracks
                    .iter_mut()
                    .find(|t| &t.device == device)
                {
                    t.frozen = None;
                }
            }
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
                Kind::Loop { .. } => "loop",
                Kind::Join { .. } => "joined",
                Kind::FlattenComp { .. } => "comp",
                Kind::Refreeze => "frozen",
                Kind::Rumble { .. } => "rumble",
                Kind::ReverseReverb { .. } => "reverse reverb",
                Kind::Riser { .. } => "riser",
                Kind::Impact { .. } => "impact",
            };
            let post = match &job.kind {
                Kind::Rumble { params, hits, .. } => Some(Post::Rumble {
                    hits: hits.clone(),
                    params: params.clone(),
                }),
                Kind::ReverseReverb { seconds, anchor }
                | Kind::Riser {
                    seconds, anchor, ..
                } => Some(Post::ReverseReverb {
                    seconds: *seconds,
                    anchor: *anchor,
                }),
                Kind::Impact { decay, .. } => Some(Post::Impact { decay: *decay }),
                _ => None,
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
                post,
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
                        strip: job.request.strip,
                    },
                }));
                if applied.is_ok() {
                    let later = if self.restart_deferred {
                        ": it plays when playback stops"
                    } else {
                        ""
                    };
                    self.reject(format!(
                        "Froze {}{}{later}",
                        job.name,
                        tail.unwrap_or_default()
                    ));
                }
            }
            Kind::Refreeze => {
                let effects = self
                    .doc()
                    .song
                    .track(&device)
                    .and_then(|t| t.frozen.as_ref())
                    .map(|f| f.effects.clone())
                    .unwrap_or_default();
                let applied = self.apply_command(Command::Song(SongCommand::RefreezeTrack {
                    device,
                    frozen: Frozen {
                        start: done.start,
                        length: done.length,
                        audio,
                        effects,
                        strip: job.request.strip,
                    },
                }));
                if applied.is_ok() {
                    let later = if self.restart_deferred {
                        ": it plays when playback stops"
                    } else {
                        ""
                    };
                    self.reject(format!(
                        "Rendered {} again{}{later}",
                        job.name,
                        tail.unwrap_or_default()
                    ));
                }
            }
            Kind::Loop { clip } => {
                let applied = self.apply_command(Command::Workflow(WorkflowCommand::RenderLoop {
                    id: clip,
                    audio,
                }));
                if applied.is_ok() {
                    self.reject(format!("Converted the loop of {} to one clip", job.name));
                }
            }
            Kind::FlattenComp { group } => {
                let applied =
                    self.apply_command(Command::Song(SongCommand::FlattenComp { group, audio }));
                if applied.is_ok() {
                    self.reject(format!("Rendered the comp of {} to one file", job.name));
                }
            }
            Kind::Join { clips } => {
                let count = clips.len();
                let applied = self.apply_command(Command::Workflow(WorkflowCommand::RenderJoin {
                    ids: clips,
                    audio,
                }));
                if applied.is_ok() {
                    self.reject(format!(
                        "Joined {count} clips of {} by rendering them",
                        job.name
                    ));
                }
            }
            Kind::Rumble {
                options, params, ..
            } => {
                let applied = self.apply_command(Command::Workflow(WorkflowCommand::AddRumble {
                    kick: device,
                    start: done.start,
                    length: done.length,
                    audio,
                    name: None,
                    options,
                }));
                if applied.is_ok() {
                    if let Some(id) = self.document.last_clip_id() {
                        self.arrangement.set_selected_clips(&[id]);
                    }
                    let how = match params.mode {
                        atomik_dsp::rumble::RumbleMode::TailRepeat => "tail repeat".to_owned(),
                        atomik_dsp::rumble::RumbleMode::Reverb => {
                            format!("reverb, decay {:.1} s", params.decay)
                        }
                    };
                    self.reject(format!(
                        "Rumble from {} ({how}) on a new track, ducked by the kick and low-passed",
                        job.name
                    ));
                }
            }
            Kind::ReverseReverb { seconds, .. } => {
                let applied = self.apply_command(Command::Song(SongCommand::AddRenderedTrack {
                    name: Some(format!("{} (reverse reverb)", job.name)),
                    after: Some(device),
                    start: done.start,
                    length: done.length,
                    audio,
                    mute: Vec::new(),
                }));
                if applied.is_ok() {
                    if let Some(id) = self.document.last_clip_id() {
                        self.arrangement.set_selected_clips(&[id]);
                    }
                    self.reject(format!(
                        "Printed a reverse reverb of {} ({seconds} s) on a new track,                          ending where the source starts",
                        job.name
                    ));
                }
            }
            Kind::Riser {
                seconds,
                anchor,
                bars,
            } => {
                let applied = self.apply_command(Command::Song(SongCommand::AddRenderedTrack {
                    name: Some(format!("{} (riser)", job.name)),
                    after: Some(device),
                    start: done.start,
                    length: done.length,
                    audio,
                    mute: Vec::new(),
                }));
                if applied.is_ok() {
                    if let Some(id) = self.document.last_clip_id() {
                        self.arrangement.set_selected_clips(&[id]);
                    }
                    self.transition_done(
                        anchor,
                        atomik_model::transition::TransitionKind::Riser,
                        &format!(
                            "Printed a riser from {} ({bars} {}, {seconds:.1} s) on a new track, \
                             rising into bar {}",
                            job.name,
                            if bars == 1 { "bar" } else { "bars" },
                            self.transition_bar(anchor),
                        ),
                    );
                }
            }
            Kind::Impact { decay, at } => {
                let applied = self.apply_command(Command::Song(SongCommand::AddRenderedTrack {
                    name: Some(format!("{} (impact)", job.name)),
                    after: Some(device),
                    start: done.start,
                    length: done.length,
                    audio,
                    mute: Vec::new(),
                }));
                if applied.is_ok() {
                    if let Some(id) = self.document.last_clip_id() {
                        self.arrangement.set_selected_clips(&[id]);
                    }
                    self.transition_done(
                        at,
                        atomik_model::transition::TransitionKind::Impact,
                        &format!(
                            "Printed an impact from {} (saturated, {decay:.0} s reverb) on a new \
                             track, landing on bar {}",
                            job.name,
                            self.transition_bar(at),
                        ),
                    );
                }
            }
            Kind::Bounce { mute, dry } => {
                let applied = self.apply_command(Command::Song(SongCommand::AddRenderedTrack {
                    name: Some(format!(
                        "{} ({})",
                        job.name,
                        if dry { "dry bounce" } else { "bounce" }
                    )),
                    after: Some(device),
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

#[path = "app_core_rumble.rs"]
mod rumble;

#[path = "app_core_transition.rs"]
mod transition;
