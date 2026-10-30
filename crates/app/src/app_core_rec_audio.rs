//! Audio recording, input monitoring and the play-after-prime rule in the app core.
//! The pure rules are in [`crate::audio_record`], the writer
//! thread in [`crate::audio_writer`]; this is the glue that follows the transport.
//!
//! # Recording flow
//!
//! * **Start** (`ToggleRecord`, [`AppCore::start_recording`]): each armed audio track's input
//!   comes from its cables ([`track_input`]); a track without input cables records nothing and
//!   the message says so (the take still runs for the others, and with no recordable track
//!   nothing starts). One engine record tap covers the union of the needed channels (4 s of
//!   audio), and a writer thread drains it into per-track files in the bundle's `Audio/`
//!   (the session scratch bundle until the project is saved).
//! * **Input gain and clip-safe** ([`crate::audio_record`]): each armed track's `rec_gain` and
//!   `rec_clip_safe` params go to the tap's strip of that track when the take starts and every
//!   UI frame after ([`AppCore::feed_take_inputs`]), so the file holds the gained (and, with
//!   clip-safe, limited) signal and a change is heard in the take from the next batch on.
//! * **Input meter** ([`AppCore::feed_rec_input`]): every frame the transport bar gets the
//!   post-gain peak of each armed track's input (the host input's peak, which the Audio I/O
//!   meters already read, times the track's gain; the peak of a scaled signal is the scaled
//!   peak) and the track it shows: the selected track when it is armed, else the first armed
//!   audio track. It works while armed and not recording, so the level can be set before a take.
//! * **Track header meters** (`feed_track_meters`, called at the end of [`AppCore::feed_rec_input`]):
//!   each audio track's header in the arrangement shows the same input level, per channel, while
//!   the track is armed, and otherwise the playback level: the output of the track's console
//!   channel (`out_l` / `out_r`, after the fader, the mute and the solo), which is what you hear
//!   from the track before it joins the master. The console's meters are read once per frame
//!   (a read resets the peak): by [`AppCore::update_console_meters`] while the console shows, else
//!   by the header feed, and both use that one reading. The clip lamp of an armed track is shared
//!   with the transport bar: a click on either clears both. A project with no audio track reads
//!   nothing.
//! * **While recording** every UI frame notes the first and the latest *playing* transport
//!   state (as MIDI recording does), which anchor the engine-frame to song-position conversion.
//! * **Stop or locate** ([`AppCore::finish_take`]): the writer is told where the take ends and
//!   how to convert frames ([`crate::audio_writer::FinishInfo`]): the take ends
//!   `END_SLACK_FRAMES` (2048) plus the compensation after the last state seen, so the
//!   performer's last notes (which arrive a round trip late) are kept; the audio after the
//!   transport stopped is placed by extrapolating the last state. The tap keeps running until
//!   the writer has delivered its result, then it is stopped.
//! * **Result** ([`Msg::Recorded`]): per lap and track one asset (stored by the writer) is
//!   registered (`RegisterAsset`, not an undo step). A track with one lap gets one
//!   `AddAudioClip`; a track with several laps (a loop recording) gets **one `NewTakeGroup`**,
//!   the laps stacked as takes with the last lap playing (pick or comp them in the arrangement),
//!   exactly like MIDI laps. Laps that land on clips already on the lane never hide them or get
//!   hidden: they join the take group whose compiled clips they overlap (`AddTake`) or make a
//!   group with the overlapped plain clips (`GroupAsTakes`). Each is one undo step per track. Undo removes the clip or the
//!   group; the asset stays in the pool until a save prunes it (the
//!   orphan rule). A clip starts on the first tick at or after its audio ([`place_lap`]) with a
//!   sub-tick `source_offset`, so every sample plays on its exact song frame.
//! * **Problems** (`overflow_frames`, gaps) are reported as a message; the take still lands,
//!   aligned (gaps are silence).
//! * An engine restart ([`AppCore::rebuild_engine`]) cancels a running take without a clip.
//!
//! # Monitoring
//!
//! `internal.monitor_live` of every audio track device is 1 while the track is armed and 0
//! otherwise ([`AppCore::push_monitor_live`]); the device decides with its `monitor` choice.
//! It goes through the engine only (like `internal.solo_silence`): never a model command, so it
//! is not stored or undoable. It is sent on arm changes, after every engine restart (load,
//! audio restart) and when a device is (re)inserted; only changes are sent.
//!
//! # Playing streamed audio
//!
//! The engine only prefetches streamed audio while it renders, so Play waits (a few UI frames)
//! until [`EngineSink::stream_primed`] says every streamed track has its read-ahead, but never
//! longer than `PRIME_WAIT_FRAMES` frames.

use super::*;
use crate::audio_import::{Loaded, ticks_for_frames};
use crate::audio_record::{
    TAP_SECONDS, input_peak, input_peaks, input_settings, place_lap, plan_tap, track_input,
};
use crate::audio_writer::{Ctl, FinishInfo, TakeResult, WriterConfig, WriterTrack, spawn};
use atomik_engine::TapControls;
use atomik_model::song::AUDIO_TRACK_TYPE;
use atomik_model::{AssetMeta, AudioClipContent, Clip, ClipContent, ClipId};
use atomik_ui::{ChannelMeters, MeterSource, RecInput, TrackLevel};
use std::sync::mpsc::Sender;

/// Frames after the last transport state seen that a finished take still records (on top of the
/// compensation), for the delay between the UI's reading and the engine's stop.
const END_SLACK_FRAMES: u64 = 2_048;

/// UI frames Play waits for the streamer's read-ahead at most (about 2 s).
pub(super) const PRIME_WAIT_FRAMES: u32 = 120;

/// A running audio take.
pub(super) struct AudioRecording {
    ctl: Sender<Ctl>,
    /// The tap's strips: input gain and clip-safe of each recorded track.
    controls: Arc<TapControls>,
    /// The track of each strip.
    strips: Vec<DeviceKey>,
    /// The writer thread.
    writer: std::thread::JoinHandle<()>,
    /// Song frame the transport started from, when recording started from a stopped transport.
    start_song_frame: Option<u64>,
    /// The transport's loop count when recording started.
    start_loop_count: u64,
    /// The first playing transport state seen.
    origin: Option<TransportState>,
    /// The latest playing transport state seen.
    last: Option<TransportState>,
}

/// A lap of a finished take placed on the tick grid: start tick, length in ticks, asset id and
/// source offset.
type PlacedLap = (i64, i64, String, u64);

impl<S: EngineSink> AppCore<S> {
    /// The armed audio tracks, in song order.
    pub(super) fn armed_audio_tracks(&self) -> Vec<DeviceKey> {
        let doc = self.doc();
        doc.song
            .tracks
            .iter()
            .filter(|t| t.armed && doc.is_audio_track(&t.device, &self.registry))
            .map(|t| t.device.clone())
            .collect()
    }

    /// Starts the audio part of a take for the armed audio `tracks` (module docs). False when
    /// nothing records (the reason is reported).
    pub(super) fn start_audio_recording(&mut self, tracks: &[DeviceKey]) -> bool {
        let mut planned = Vec::new();
        let mut names = Vec::new();
        let mut missing = Vec::new();
        for key in tracks {
            let name = self
                .doc()
                .device(key)
                .map_or_else(|| key.0.clone(), |d| d.name.clone());
            match track_input(self.doc(), key) {
                Some(input) => {
                    planned.push((key.clone(), input));
                    names.push(name);
                }
                None => missing.push(name),
            }
        }
        if !missing.is_empty() {
            self.reject(format!(
                "no input on armed audio track {}: it records nothing (cable the Audio I/O inputs to its in_l / in_r)",
                missing.join(", ")
            ));
        }
        if planned.is_empty() {
            return false;
        }
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => {
                self.reject(format!("audio recording: {e}"));
                return false;
            }
        };
        let rate = self.doc().sample_rate;
        let plan = plan_tap(&planned);
        let capacity = TAP_SECONDS * rate as usize;
        let strips: Vec<DeviceKey> = plan.tracks.iter().map(|t| t.device.clone()).collect();
        let controls = Arc::new(TapControls::new(strips.len()));
        self.set_strip_controls(&controls, &strips);
        let Some(tap) =
            self.sink
                .start_audio_record(&plan.channels, capacity, Arc::clone(&controls))
        else {
            self.reject(
                "audio recording needs a running audio engine (audio is off or its stream did not open)"
                    .to_owned(),
            );
            return false;
        };
        let take = self.audio.next_take;
        self.audio.next_take += 1;
        let tracks = plan
            .tracks
            .iter()
            .zip(names)
            .enumerate()
            .map(|(n, (planned, name))| WriterTrack {
                planned: planned.clone(),
                name,
                path: bundle
                    .audio_dir()
                    .join(format!("rec-{}-{take}-{n}.wav", std::process::id())),
            })
            .collect();
        let config = WriterConfig {
            tracks,
            bundle,
            sample_rate: rate,
            stream_threshold: self.stream_threshold(),
            generation: self.audio.generation,
            take,
        };
        let (ctl, writer) = match spawn(tap, config, self.audio.tx.clone()) {
            Ok(w) => w,
            Err(e) => {
                self.sink.stop_audio_record();
                self.reject(format!("audio recording: cannot start the writer: {e}"));
                return false;
            }
        };
        self.audio.rec_pending += 1;
        let state = self.sink.transport_state();
        let start_song_frame =
            (!self.playing).then(|| self.doc().song.frame_at(self.position, rate));
        self.audio.recording = Some(AudioRecording {
            ctl,
            controls,
            strips,
            writer,
            start_song_frame,
            start_loop_count: state.loop_count,
            origin: None,
            last: None,
        });
        true
    }

    /// Copies the input gain and clip-safe params of `strips` into `controls`.
    fn set_strip_controls(&self, controls: &TapControls, strips: &[DeviceKey]) {
        for (strip, key) in strips.iter().enumerate() {
            let s = input_settings(self.doc(), key, &self.registry);
            controls.set(strip, s.gain(), s.clip_safe);
        }
    }

    /// Hands the running take the tracks' current input settings (module docs); one wait-free
    /// store per strip.
    pub(super) fn feed_take_inputs(&self) {
        if let Some(rec) = &self.audio.recording {
            self.set_strip_controls(&rec.controls, &rec.strips);
        }
    }

    /// Feeds the transport bar's recording input (module docs): the focus track with its
    /// settings and the post-gain peaks of every armed audio track, at `now_seconds` (any
    /// monotonic clock). Call once per frame, after [`Self::feed_io_meters`].
    pub fn feed_rec_input(&mut self, now_seconds: f64) {
        let armed = self.armed_audio_tracks();
        let focus = self
            .arrangement
            .selected_track()
            .filter(|t| armed.contains(t))
            .or_else(|| armed.first())
            .cloned();
        let source = focus.map(|device| {
            let settings = input_settings(self.doc(), &device, &self.registry);
            RecInput {
                name: self
                    .doc()
                    .device(&device)
                    .map_or_else(|| device.0.clone(), |d| d.name.clone()),
                device,
                gain_db: settings.gain_db,
                clip_safe: settings.clip_safe,
            }
        });
        let host = self.host_input_peaks();
        let peaks: Vec<(DeviceKey, f32)> = armed
            .iter()
            .map(|key| {
                let peak = track_input(self.doc(), key).map_or(0.0, |input| {
                    input_peak(host, input, input_settings(self.doc(), key, &self.registry))
                });
                (key.clone(), peak)
            })
            .collect();
        // A click on either meter of a track clears the lamp of the other, so they agree.
        for device in self.arrangement.take_meter_clears() {
            if device.1 == MeterSource::Input {
                self.transport_bar.clear_rec_latch(&device.0);
            }
        }
        for device in self.transport_bar.take_rec_clears() {
            self.arrangement
                .clear_meter_latch(&device, MeterSource::Input);
        }
        self.transport_bar.set_rec_input(source);
        self.transport_bar.feed_rec_peaks(&peaks, now_seconds);
        self.feed_track_meters(&armed, now_seconds);
    }

    /// Feeds the meters in the arrangement's audio track headers (module docs): the input of an
    /// armed track, per channel, else the output of the track's console channel (post-fader),
    /// from the console readings of this frame. A project with no audio track reads nothing.
    fn feed_track_meters(&mut self, armed: &[DeviceKey], now_seconds: f64) {
        let tracks: Vec<DeviceKey> = self
            .doc()
            .song
            .tracks
            .iter()
            .filter(|t| self.doc().is_audio_track(&t.device, &self.registry))
            .map(|t| t.device.clone())
            .collect();
        if tracks.is_empty() {
            self.readings_fresh = false;
            self.arrangement.feed_track_levels(&[], now_seconds);
            return;
        }
        // The console reads the meters only while it shows (and then, first, this frame): else
        // read them here. A read resets the peak, so each frame is read once.
        if !std::mem::take(&mut self.readings_fresh) {
            let shared = std::mem::take(&mut self.io_meters.shared);
            self.frame_readings = self.console_readings_with(&shared);
            self.io_meters.shared = shared;
        }
        let host = self.host_input_peaks();
        let levels: Vec<TrackLevel> = tracks
            .into_iter()
            .map(|device| {
                if armed.contains(&device) {
                    let settings = input_settings(self.doc(), &device, &self.registry);
                    let (peaks, stereo) = track_input(self.doc(), &device)
                        .map_or(([0.0; 2], true), |input| input_peaks(host, input, settings));
                    return TrackLevel {
                        device,
                        source: MeterSource::Input,
                        peaks,
                        stereo,
                    };
                }
                let reading = self
                    .doc()
                    .console
                    .channel_of(&device)
                    .and_then(|c| self.frame_readings.iter().find(|(k, _)| *k == c.channel))
                    .map_or(ChannelMeters::SILENT, |(_, m)| *m);
                TrackLevel {
                    device,
                    source: MeterSource::Playback,
                    peaks: [reading.out_peak_l, reading.out_peak_r],
                    stereo: true,
                }
            })
            .collect();
        self.arrangement.feed_track_levels(&levels, now_seconds);
    }

    /// Notes the transport states of a running audio take.
    pub(super) fn feed_audio_take(&mut self, state: &TransportState) {
        if !state.playing {
            return;
        }
        if let Some(rec) = &mut self.audio.recording {
            rec.origin.get_or_insert(*state);
            rec.last = Some(*state);
        }
    }

    /// Hands the running audio take to its writer to finish (module docs).
    pub(super) fn finish_audio_take(&mut self) {
        let Some(rec) = self.audio.recording.take() else {
            return;
        };
        let (Some(origin), Some(last), Some(snapshot)) =
            (rec.origin, rec.last, self.snapshot.clone())
        else {
            // The transport never played: nothing to place.
            let _ = rec.ctl.send(Ctl::Cancel);
            // Joined by `wait_background` (the writer deletes its partial file before exiting).
            self.audio.cancelled.retain(|w| !w.is_finished());
            self.audio.cancelled.push(rec.writer);
            self.audio.rec_pending = self.audio.rec_pending.saturating_sub(1);
            self.stop_tap_when_idle();
            return;
        };
        let rate = f64::from(self.doc().sample_rate);
        let compensation = self.sink.record_offset_frames();
        let valid_from = match rec.start_song_frame {
            Some(start)
                if origin.song_frame >= start && origin.loop_count == rec.start_loop_count =>
            {
                origin
                    .engine_frame
                    .saturating_sub(origin.song_frame - start)
            }
            Some(_) => origin.engine_frame,
            None => 0,
        };
        let end =
            last.engine_frame + END_SLACK_FRAMES + u64::try_from(compensation.max(0)).unwrap_or(0);
        let finish = FinishInfo {
            end_engine_frame: end,
            compensation,
            valid_from,
            position: Box::new(move |frame| snapshot.position_at(&last, frame, rate)),
        };
        if rec.ctl.send(Ctl::Finish(Box::new(finish))).is_err() {
            self.audio.rec_pending = self.audio.rec_pending.saturating_sub(1);
            self.reject("audio recording: the writer is gone, the take is lost".to_owned());
            self.stop_tap_when_idle();
        }
    }

    /// Throws a running audio take away (engine restart).
    pub(super) fn cancel_audio_recording(&mut self) {
        if let Some(rec) = self.audio.recording.take() {
            let _ = rec.ctl.send(Ctl::Cancel);
            // Joined by `wait_background` (the writer deletes its partial file before exiting).
            self.audio.cancelled.push(rec.writer);
            self.audio.rec_pending = self.audio.rec_pending.saturating_sub(1);
        }
        self.stop_tap_when_idle();
    }

    fn stop_tap_when_idle(&mut self) {
        if self.audio.recording.is_none() && self.audio.rec_pending == 0 {
            self.sink.stop_audio_record();
        }
    }

    /// Applies a finished take: one `RegisterAsset` per lap, then per track one `AddAudioClip`
    /// for a single lap (one undo step), or one `NewTakeGroup` when the loop made several laps
    /// (the laps are stacked takes, one undo step; module docs).
    pub(super) fn apply_recorded(&mut self, result: Result<TakeResult, String>) {
        match result {
            Err(e) => self.reject(format!("audio recording: {e}")),
            Ok(take) => {
                let mut placed: Vec<(DeviceKey, Vec<PlacedLap>)> = Vec::new();
                for lap in &take.laps {
                    let Some((start, length, source_offset)) = self.place_lap_clip(lap) else {
                        continue;
                    };
                    let entry = (start, length, lap.info.hash.clone(), source_offset);
                    match placed.iter_mut().find(|(d, _)| *d == lap.device) {
                        Some((_, laps)) => laps.push(entry),
                        None => placed.push((lap.device.clone(), vec![entry])),
                    }
                }
                for (device, laps) in placed {
                    self.add_lap_clips(device, laps);
                }
                if take.overflow_frames > 0 || take.gap_frames > 0 {
                    self.reject(format!(
                        "audio recording dropped {} input frames (the disk could not keep up): the take is aligned and the gaps are silent",
                        take.overflow_frames.max(take.gap_frames)
                    ));
                }
            }
        }
        self.stop_tap_when_idle();
    }

    /// Registers the lap's asset and places it on the tick grid: `(start tick, length in ticks,
    /// source offset)`, or `None` (reported) when it cannot be placed.
    fn place_lap_clip(
        &mut self,
        lap: &crate::audio_writer::RecordedLap,
    ) -> Option<(i64, i64, u64)> {
        let loaded = Loaded {
            hash: lap.info.hash.clone(),
            info: Some(lap.info.clone()),
            asset: lap.asset.clone(),
            peaks: Arc::clone(&lap.peaks),
        };
        self.insert_loaded(&loaded);
        let rate = self.doc().sample_rate;
        let register = Command::RegisterAsset {
            hash: lap.info.hash.clone(),
            meta: AssetMeta {
                channels: lap.info.channels,
                sample_rate: lap.info.sample_rate,
                frames: lap.info.frames,
                original_name: lap.info.original_name.clone(),
            },
        };
        self.apply_command(register).ok()?;
        if !self.doc().is_audio_track(&lap.device, &self.registry) {
            self.reject(format!("{} is not an audio track any more", lap.device));
            return None;
        }
        let song = &self.doc().song;
        let placement = place_lap(lap.song_frame, lap.info.frames, &|t| song.frame_at(t, rate))?;
        let length = ticks_for_frames(song, placement.start_tick, placement.frames, rate);
        Some((placement.start_tick, length, placement.source_offset))
    }

    /// Adds the placed laps of one track. On a lane with nothing under the take: a single lap is
    /// one `AddAudioClip`; several laps (a loop recording) become the stacked takes of one
    /// `NewTakeGroup`, later laps on top, so the last lap plays until the user picks or comps
    /// another. A take over existing clips never hides, or is hidden by, them: the laps join the
    /// take group whose comp clips they overlap (`AddTake`, one per lap), or the overlapped plain
    /// clips and the laps become a new group (`GroupAsTakes`); the new takes play over the overlap
    /// and the old clips everywhere else. Every case is one undo step.
    fn add_lap_clips(&mut self, device: DeviceKey, laps: Vec<PlacedLap>) {
        let content = |asset: String, source_offset: u64| AudioClipContent {
            asset,
            source_offset,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: atomik_model::FadeCurve::Linear,
            reverse: false,
            envelope: Vec::new(),
            xfade: None,
            warp: Default::default(),
        };
        let clips: Vec<Clip> = laps
            .iter()
            .map(|(start, length, asset, source_offset)| Clip {
                id: ClipId(0),
                start: *start,
                length: *length,
                offset: 0,
                loop_length: None,
                muted: false,
                grid: None,
                scale: None,
                controllers: Vec::new(),
                content: ClipContent::Audio(content(asset.clone(), *source_offset)),
            })
            .collect();
        let from = clips.iter().map(|c| c.start).min().unwrap_or(0);
        let to = clips.iter().map(Clip::end).max().unwrap_or(0);
        let lane = 0;
        // What the lane holds under the take: the take group whose comp clips overlap it, else
        // the plain clips that do.
        let (group, loose) = self
            .doc()
            .song
            .track(&device)
            .and_then(|t| t.lanes.get(lane))
            .map_or((None, Vec::new()), |l| {
                let overlapping: Vec<&Clip> = l
                    .clips
                    .iter()
                    .filter(|c| !c.muted && c.start < to && c.end() > from)
                    .collect();
                let group = l
                    .take_groups
                    .iter()
                    .find(|g| overlapping.iter().any(|c| g.clips.contains(&c.id)))
                    .map(|g| g.id);
                let loose = overlapping
                    .iter()
                    .filter(|c| !l.take_groups.iter().any(|g| g.clips.contains(&c.id)))
                    .map(|c| c.id)
                    .collect();
                (group, loose)
            });
        let commands: Vec<SongCommand> = if let Some(group) = group {
            clips
                .into_iter()
                .map(|clip| SongCommand::AddTake { group, clip })
                .collect()
        } else if !loose.is_empty() {
            vec![SongCommand::GroupAsTakes {
                device,
                lane,
                clips: loose,
                extra: clips,
            }]
        } else if let [(start, length, asset, source_offset)] = laps.as_slice() {
            vec![SongCommand::AddAudioClip {
                device,
                start: *start,
                length: *length,
                asset: asset.clone(),
                source_offset: *source_offset,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: atomik_model::FadeCurve::Linear,
            }]
        } else {
            vec![SongCommand::NewTakeGroup {
                device,
                lane,
                takes: clips,
            }]
        };
        for (i, command) in commands.into_iter().enumerate() {
            // The takes after the first fold into its undo step. A rejection is already reported.
            self.join_next = i > 0;
            let _ = self.apply_command(Command::Song(command));
            self.join_next = false;
        }
        self.document.end_merge();
    }

    /// Sends `internal.monitor_live` (1 while armed) to the audio tracks whose value the engine
    /// does not have yet (module docs).
    pub(super) fn push_monitor_live(&mut self) {
        let Some(pid) = self
            .registry
            .get(AUDIO_TRACK_TYPE)
            .and_then(|f| f.descriptor.param_by_key(MONITOR_LIVE_PARAM))
            .map(|p| p.id)
        else {
            return;
        };
        let want: Vec<(DeviceKey, bool)> = self
            .doc()
            .song
            .tracks
            .iter()
            .filter(|t| self.doc().is_audio_track(&t.device, &self.registry))
            .map(|t| (t.device.clone(), t.armed))
            .collect();
        self.audio
            .monitor_sent
            .retain(|k, _| want.iter().any(|(w, _)| w == k));
        for (key, armed) in want {
            if self.audio.monitor_sent.get(&key) == Some(&armed) {
                continue;
            }
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let result = self.sink.set_param(id, pid, if armed { 1.0 } else { 0.0 });
            self.engine_result(result);
            self.audio.monitor_sent.insert(key, armed);
        }
    }

    /// Sends a Play that waited for the streamer's read-ahead (module docs).
    pub(super) fn drive_pending_play(&mut self) {
        let Some(waited) = self.pending_play else {
            return;
        };
        if self.sink.stream_primed() || waited >= PRIME_WAIT_FRAMES {
            self.pending_play = None;
            let command = super::play_command(self.pending_pre_roll.take());
            self.send_transport(command);
        } else {
            self.pending_play = Some(waited + 1);
        }
    }
}

/// The audio track's param the app sets (not stored): 1 while the track is armed.
const MONITOR_LIVE_PARAM: &str = "internal.monitor_live";
