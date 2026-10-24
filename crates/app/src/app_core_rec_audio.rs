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
//!   exactly like MIDI laps. Each is one undo step per track. Undo removes the clip or the
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
use crate::audio_record::{TAP_SECONDS, place_lap, plan_tap, track_input};
use crate::audio_writer::{Ctl, FinishInfo, TakeResult, WriterConfig, WriterTrack, spawn};
use atomik_model::song::AUDIO_TRACK_TYPE;
use atomik_model::{AssetMeta, AudioClipContent, Clip, ClipContent, ClipId};
use std::sync::mpsc::Sender;

/// Frames after the last transport state seen that a finished take still records (on top of the
/// compensation), for the delay between the UI's reading and the engine's stop.
const END_SLACK_FRAMES: u64 = 2_048;

/// UI frames Play waits for the streamer's read-ahead at most (about 2 s).
pub(super) const PRIME_WAIT_FRAMES: u32 = 120;

/// A running audio take.
pub(super) struct AudioRecording {
    ctl: Sender<Ctl>,
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
        let Some(tap) = self.sink.start_audio_record(&plan.channels, capacity) else {
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
            writer,
            start_song_frame,
            start_loop_count: state.loop_count,
            origin: None,
            last: None,
        });
        true
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

    /// Adds the placed laps of one track: a single lap is one `AddAudioClip`; several laps (a
    /// loop recording) become the stacked takes of one `NewTakeGroup`, later laps on top, so
    /// the last lap plays until the user picks or comps another. Both are one undo step.
    fn add_lap_clips(&mut self, device: DeviceKey, laps: Vec<PlacedLap>) {
        let content = |asset: String, source_offset: u64| AudioClipContent {
            asset,
            source_offset,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: atomik_model::FadeCurve::Linear,
        };
        let command = if let [(start, length, asset, source_offset)] = laps.as_slice() {
            SongCommand::AddAudioClip {
                device,
                start: *start,
                length: *length,
                asset: asset.clone(),
                source_offset: *source_offset,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: atomik_model::FadeCurve::Linear,
            }
        } else {
            let takes = laps
                .into_iter()
                .map(|(start, length, asset, source_offset)| Clip {
                    id: ClipId(0),
                    start,
                    length,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    grid: None,
                    scale: None,
                    controllers: Vec::new(),
                    content: ClipContent::Audio(content(asset, source_offset)),
                })
                .collect();
            SongCommand::NewTakeGroup {
                device,
                lane: 0,
                takes,
            }
        };
        // A rejection is already reported.
        let _ = self.apply_command(Command::Song(command));
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
            self.send_transport(TransportCommand::Play);
        } else {
            self.pending_play = Some(waited + 1);
        }
    }
}

/// The audio track's param the app sets (not stored): 1 while the track is armed.
const MONITOR_LIVE_PARAM: &str = "internal.monitor_live";
