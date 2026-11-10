//! The clip launcher in the core: launches and stops sent to the engine, what the engine
//! publishes shown in the Launcher view, and Record Launches.
//!
//! **Transport rule.** Launching a slot or a scene while the transport is stopped starts the
//! transport from where it stands; the launch waits for its boundary like any other (a position
//! on a bar line launches at once). The engine keeps a request that arrives while stopped until
//! play, so the order of the two messages does not matter.
//!
//! **Back to Arrangement** is immediate (the next tick); **Stop** and **Stop All** wait for the
//! launch quantize, like a launch. Stopping a slot hands its track back to the arrangement.
//!
//! **Record Launches.** With the switch on, pressing Record starts a launch pass (without an
//! armed track too) and Stop or Record again ends it. Each launch the engine reports (with the
//! song tick it happened at) opens a run on its track and the next launch or stop of that track
//! closes it; the runs become arrangement clips of the slots' content, looped to the length the
//! slot played (a slot whose clip has a loop of its own is written one clip per cycle). All of
//! them are one `Batch`, so the whole pass is one undo step.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_engine::{DeviceInstanceId, LaunchCommand, LaunchEvent, LaunchGrid, LaunchState};
use atomik_model::{
    Clip, ClipId, Command, DeviceKey, LaunchQuantize, LauncherClip, LauncherCommand, SongCommand,
};
use atomik_ui::{LauncherAction, LauncherFeed, LauncherView, TrackLaunch};
use std::collections::BTreeMap;

/// UI frames a slot that waits stays lit or dark while it blinks.
const BLINK_FRAMES: u32 = 18;

/// What the core keeps for the launcher.
#[derive(Default)]
pub(super) struct LauncherCore {
    /// Whether the launcher shows in the lower pane (this session only).
    on: bool,
    /// The launch tracks of the song last submitted: the model track and its engine device, in
    /// the order the engine publishes them.
    tracks: Vec<(DeviceKey, DeviceInstanceId)>,
    states: Vec<LaunchState>,
    /// Record Launches.
    pub(super) record: bool,
    /// The launch pass in progress.
    pub(super) rec: Option<LaunchRec>,
    frames: u32,
}

/// A launch pass: what is playing and what has played.
#[derive(Default)]
pub(super) struct LaunchRec {
    open: BTreeMap<DeviceKey, (LauncherClip, i64)>,
    done: Vec<(DeviceKey, LauncherClip, i64, i64)>,
}

/// The grid a launch quantize waits for.
fn grid_of(q: LaunchQuantize) -> LaunchGrid {
    match (q.fixed_ticks(), q.bars()) {
        (Some(n), _) => LaunchGrid::Ticks(n),
        (None, Some(n)) => LaunchGrid::Bars(n),
        (None, None) => LaunchGrid::Ticks(1),
    }
}

impl<S: EngineSink> AppCore<S> {
    fn lview(&mut self) -> &mut LauncherView {
        &mut self.launcher_view
    }

    /// The Launcher view.
    pub fn launcher(&self) -> &LauncherView {
        &self.launcher_view
    }

    /// The Launcher view, mutably (bounds, scale, focus).
    pub fn launcher_mut(&mut self) -> &mut LauncherView {
        &mut self.launcher_view
    }

    /// Whether the Launcher shows in the lower pane.
    pub fn launcher_visible(&self) -> bool {
        self.launch.on
    }

    /// Shows or hides the Launcher (not saved).
    pub fn set_launcher_visible(&mut self, visible: bool) {
        self.launch.on = visible;
        self.sync_launcher();
    }

    /// Whether Record Launches is on.
    pub fn record_launches(&self) -> bool {
        self.launch.record
    }

    /// Whether a launch pass is being recorded.
    pub fn recording_launches(&self) -> bool {
        self.launch.rec.is_some()
    }

    /// Switches Record Launches.
    pub fn set_record_launches(&mut self, on: bool) {
        self.launch.record = on;
        self.sync_launcher();
    }

    /// Hands the launch tracks of the song just submitted (model track, engine device).
    pub(super) fn set_launch_tracks(&mut self, tracks: Vec<(DeviceKey, DeviceInstanceId)>) {
        self.launch.tracks = tracks;
    }

    /// Reads the document and what the engine publishes into the Launcher view.
    pub(super) fn sync_launcher(&mut self) {
        let mut feed = LauncherFeed {
            record: self.launch.record,
            blink: (self.launch.frames / BLINK_FRAMES).is_multiple_of(2),
            ..LauncherFeed::default()
        };
        for ((key, _), s) in self.launch.tracks.iter().zip(&self.launch.states) {
            feed.tracks.insert(
                key.clone(),
                TrackLaunch {
                    playing: s.playing.map(usize::from),
                    queued: s.queued.map(usize::from),
                    queued_stop: s.queued_stop,
                    phase: s.phase,
                },
            );
        }
        let doc = self.document.doc();
        self.launcher_view.sync(doc);
        self.launcher_view.set_feed(feed);
    }

    /// Once per UI frame: reads the launcher's published state and the launches the engine made.
    pub(super) fn poll_launcher(&mut self) {
        self.launch.frames = self.launch.frames.wrapping_add(1);
        let mut states = std::mem::take(&mut self.launch.states);
        self.sink.launch_states(&mut states);
        self.launch.states = states;
        while let Some(event) = self.sink.next_launch_event() {
            self.on_launch_event(event);
        }
        if self.launch.on {
            self.sync_launcher();
        }
    }

    // ------------------------------------------------------------------ launching

    fn launch_device(&self, track: &DeviceKey) -> Option<DeviceInstanceId> {
        self.launch
            .tracks
            .iter()
            .find(|(k, _)| k == track)
            .map(|(_, id)| *id)
    }

    fn send_launch(&mut self, command: LaunchCommand) {
        self.send_transport(atomik_engine::TransportCommand::Launch(command));
    }

    /// Starts the transport when it is stopped (a launch with the transport stopped plays).
    fn play_for_launch(&mut self) {
        if !self.playing {
            self.play();
        }
    }

    /// Launches the slot of `track` at scene row `row` at the next boundary of its quantize.
    pub fn launch_slot(&mut self, track: &DeviceKey, row: usize) {
        let launcher = &self.document.doc().song.launcher;
        if launcher.slot(track, row).is_none() {
            return;
        }
        let grid = grid_of(launcher.quantize_of(track, row));
        let Some(device) = self.launch_device(track) else {
            self.report("that track does not play: unmute it or check its device".to_string());
            return;
        };
        let Ok(slot) = u16::try_from(row) else {
            return;
        };
        self.send_launch(LaunchCommand::Launch { device, slot, grid });
        self.play_for_launch();
    }

    /// Launches every slot of scene `row`; with "scene stops empty tracks" on, the tracks that
    /// play a slot and have none in the row stop.
    pub fn launch_scene(&mut self, row: usize) {
        let launcher = self.document.doc().song.launcher.clone();
        let tracks: Vec<DeviceKey> = self.launch.tracks.iter().map(|(k, _)| k.clone()).collect();
        let mut any = false;
        for (i, track) in tracks.iter().enumerate() {
            if launcher.slot(track, row).is_some() {
                self.launch_slot(track, row);
                any = true;
            } else if launcher.stop_empty
                && self
                    .launch
                    .states
                    .get(i)
                    .is_some_and(|s| s.playing.is_some())
            {
                self.stop_launch_track(track);
                any = true;
            }
        }
        if !any {
            self.report("that scene has no clips".to_string());
        }
    }

    /// Stops the slot of `track` at the next boundary of the song's launch quantize: the track
    /// plays its arrangement again.
    pub fn stop_launch_track(&mut self, track: &DeviceKey) {
        let grid = grid_of(self.document.doc().song.launcher.quantize);
        if let Some(device) = self.launch_device(track) {
            self.send_launch(LaunchCommand::Stop { device, grid });
        }
    }

    /// Stops every slot: at once ([`Self::launcher_back`]) or at the next boundary.
    pub fn stop_all_slots(&mut self, now: bool) {
        let grid = if now {
            LaunchGrid::Ticks(1)
        } else {
            grid_of(self.document.doc().song.launcher.quantize)
        };
        self.send_launch(LaunchCommand::StopAll { grid });
    }

    /// Every track plays its arrangement again, now.
    pub fn launcher_back(&mut self) {
        self.stop_all_slots(true);
    }

    fn launcher_edit(&mut self, command: LauncherCommand) -> bool {
        self.apply_command(Command::Song(SongCommand::Launcher(command)))
            .is_ok()
    }

    /// A launcher request of the view or a command of the Launcher menu.
    pub fn launcher_action(&mut self, action: LauncherAction) {
        match action {
            LauncherAction::Launch { device, row } => self.launch_slot(&device, row),
            LauncherAction::LaunchScene(row) => self.launch_scene(row),
            LauncherAction::Stop(device) => self.stop_launch_track(&device),
            LauncherAction::Back => self.launcher_back(),
            LauncherAction::StopAll => self.stop_all_slots(false),
            LauncherAction::ToggleRecord => {
                let on = !self.launch.record;
                self.set_record_launches(on);
            }
            LauncherAction::ToggleStopEmpty => {
                let on = !self.document.doc().song.launcher.stop_empty;
                self.launcher_edit(LauncherCommand::SetStopEmpty(on));
            }
            LauncherAction::SetQuantize(q) => {
                self.launcher_edit(LauncherCommand::SetQuantize(q));
            }
            LauncherAction::DragOut { device, row, pos } => {
                // A slot dropped on the arrangement: a clip on the lane's track at the tick.
                if let Some((lane, start)) = self.arrangement.drop_spot_at(pos) {
                    self.launcher_edit(LauncherCommand::ToArrangement {
                        device,
                        row,
                        start,
                        onto: lane.map(|(d, _)| d),
                    });
                }
            }
            LauncherAction::DropFile { device, row, path } => {
                if crate::midi_file::is_midi_path(&path) {
                    self.import_midi_slot(&path, device, row);
                } else {
                    self.import_slot(path, device, row);
                }
            }
            LauncherAction::DropClips { device, row, clips } => {
                if let Some(clip) = clips.first().copied() {
                    self.launcher_edit(LauncherCommand::PutClip { device, row, clip });
                }
            }
            LauncherAction::OpenClip(id) => {
                let audio = self
                    .doc()
                    .song
                    .clip_or_take(id)
                    .is_some_and(|(_, c)| c.content.is_audio());
                if audio {
                    self.open_audio_editor(id);
                } else {
                    self.open_piano_roll(id);
                }
            }
            LauncherAction::NewClip { device, row } => {
                let length = self.bar_ticks();
                let command = LauncherCommand::NewClip {
                    device: device.clone(),
                    row,
                    length,
                };
                if self.launcher_edit(command)
                    && let Some(slot) = self.document.doc().song.launcher.slot(&device, row)
                {
                    let id = slot.clip.id;
                    self.open_piano_roll(id);
                }
            }
        }
        self.sync_launcher();
    }

    /// The length of the first bar of the song, in ticks.
    fn bar_ticks(&self) -> i64 {
        self.document.doc().song.meter_table().map().bar_ticks_at(0)
    }

    /// Adds a scene at the end.
    pub fn launcher_new_scene(&mut self) {
        let at = self.document.doc().song.launcher.scenes.len();
        self.launcher_edit(LauncherCommand::InsertScene { at });
    }

    /// The selected arrangement clip goes into the slot of its track at the cursor row.
    pub fn launcher_from_arrangement(&mut self) {
        let Some(id) = self
            .arrangement
            .selected_clips()
            .into_iter()
            .find(|id| self.document.doc().song.clip(*id).is_some())
        else {
            self.report("select a note clip in the arrangement first".to_string());
            return;
        };
        let Some((track, _, _)) = self.document.doc().song.clip(id) else {
            return;
        };
        let device = track.device.clone();
        let row = self.lview().cursor().1;
        self.launcher_edit(LauncherCommand::PutClip {
            device,
            row,
            clip: id,
        });
        self.sync_launcher();
    }

    /// The slot under the cursor goes into the arrangement at the playhead.
    pub fn launcher_to_arrangement(&mut self) {
        let Some((device, row)) = self.lview().cursor_slot() else {
            return;
        };
        let start = self.position.max(0);
        self.launcher_edit(LauncherCommand::ToArrangement {
            device,
            row,
            start,
            onto: None,
        });
    }

    // ------------------------------------------------------------------ recording

    /// Starts a launch pass at the transport position (and the transport when it is stopped).
    pub(super) fn begin_launch_rec(&mut self) {
        self.launch.rec = Some(LaunchRec::default());
        self.report("recording launches".to_string());
    }

    fn on_launch_event(&mut self, event: LaunchEvent) {
        if self.launch.rec.is_none() {
            return;
        }
        let Some(track) = self
            .launch
            .tracks
            .iter()
            .find(|(_, id)| *id == event.device)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        let clip = event.slot.and_then(|s| {
            self.document
                .doc()
                .song
                .launcher
                .slot(&track, usize::from(s))
                .cloned()
        });
        let Some(rec) = self.launch.rec.as_mut() else {
            return;
        };
        if let Some((was, start)) = rec.open.remove(&track) {
            rec.done.push((track.clone(), was, start, event.tick));
        }
        if let Some(clip) = clip {
            rec.open.insert(track, (clip, event.tick));
        }
    }

    /// Ends the launch pass at song tick `end` and writes what was launched into the
    /// arrangement: one undo step.
    pub(super) fn finish_launch_rec(&mut self, end: i64) {
        // What the engine reported since the last frame belongs to the pass.
        while let Some(event) = self.sink.next_launch_event() {
            self.on_launch_event(event);
        }
        let Some(mut rec) = self.launch.rec.take() else {
            return;
        };
        for (track, (clip, start)) in std::mem::take(&mut rec.open) {
            rec.done.push((track, clip, start, end));
        }
        let commands = self.launch_clips(&rec.done);
        if commands.is_empty() {
            return;
        }
        let count = commands.len();
        if self.apply_command(Command::Batch(commands)).is_ok() {
            self.report(format!(
                "wrote {count} launched clip{} into the arrangement",
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    /// The `AddClip` commands for runs `(track, slot clip, start tick, end tick)`.
    fn launch_clips(&self, runs: &[(DeviceKey, LauncherClip, i64, i64)]) -> Vec<Command> {
        let song = &self.document.doc().song;
        let mut out = Vec::new();
        for (device, slot, start, end) in runs {
            let (start, end) = (*start, *end);
            if end <= start {
                continue;
            }
            let Some(track) = song.track(device) else {
                continue;
            };
            let lane = track
                .lanes
                .iter()
                .position(|l| l.kind.is_notes() || l.kind.is_audio())
                .unwrap_or(0);
            let len = slot.clip.length;
            let make = |s: i64, e: i64, loop_length: Option<i64>| -> Command {
                Command::Song(SongCommand::AddClip {
                    device: device.clone(),
                    lane,
                    clip: Clip {
                        id: ClipId(0),
                        start: s,
                        length: e - s,
                        loop_length,
                        muted: false,
                        ..slot.clip.clone()
                    },
                })
            };
            if slot.clip.loop_length.is_none() {
                // One looped clip: the content repeats every cycle of the slot.
                out.push(make(start, end, (end - start > len).then_some(len)));
            } else {
                // The slot's content loops inside its own cycle: one clip per cycle.
                let mut s = start;
                while s < end {
                    out.push(make(s, (s + len).min(end), slot.clip.loop_length));
                    s += len;
                }
            }
        }
        out
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Arrangement clips dragged out of the arrangement and dropped at `pos`: on a launcher slot
    /// the first of them is copied into it.
    pub(super) fn clips_dragged(&mut self, ids: &[ClipId], pos: atomik_ui::kurbo::Point) {
        if !self.launch.on {
            return;
        }
        let Some((device, row, _)) = self.launcher_view.slot_at(pos) else {
            return;
        };
        let Some(clip) = ids
            .iter()
            .copied()
            .find(|id| self.doc().song.clip(*id).is_some())
        else {
            return;
        };
        self.launcher_edit(LauncherCommand::PutClip { device, row, clip });
    }
}
