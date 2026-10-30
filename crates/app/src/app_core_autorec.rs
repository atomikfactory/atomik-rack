//! Automation recording: knob and parameter moves into automation lanes while a take runs.
//!
//! Everything here runs on the UI thread; the audio thread only sees the lane hold
//! ([`EngineSink::hold_automation`]: a few atomics, no queue, no allocation).
//!
//! * **Arming.** A lane is armed for writing with its arm button (`AppCore::toggle_automation_arm`);
//!   the set is session state (never saved, so a reopened project cannot overwrite a lane by
//!   surprise) and survives New and Open no more than the rest of the view state. A take starts
//!   (`ToggleRecord`) when a track or a lane is armed; the lanes armed at that moment are the
//!   ones the take writes, whatever is armed later.
//! * **Capture.** While a take runs, every `ParamChanged` of a command that goes through
//!   `AppCore::apply_command` (a knob drag, a plugin window, a text entry, the MCP server) on
//!   an armed lane's parameter is offered to that lane's [`PassRecorder`] with the transport
//!   tick of the engine's latest batch. A command with a drag-merge id is a held control: Touch
//!   writes while it is held and lets go on the pointer release (`UiAction::EndMerge`, or the
//!   plugin gesture's end); a command without one touches for an instant. Undo and redo are not
//!   captured. Controller (MIDI CC) input has no path to a parameter yet, so it cannot be
//!   recorded; see the roadmap.
//! * **Hold.** While a lane is being written (Overwrite from the start, Latch from the first
//!   touch, Touch while held) it is held in the engine, so the old points do not fight the
//!   value being moved. Between touches in Touch the lane plays as it was; a container's macro
//!   lane holds every target it expands to.
//! * **Passes.** A loop wrap ends a pass at the loop end and starts the next at the loop start
//!   (the knob's current value is its first value in Overwrite); a locate ends the take. Each
//!   pass writes its own clips, so the later laps lie on top of the earlier ones.
//! * **Stop.** The take ends at the transport position and every written stretch becomes an
//!   automation clip on its lane (ticks relative to the clip start, thinned by
//!   [`thin_points`] with `AppCore::automation_thinning` of the parameter's range). All the
//!   clips of all the lanes are one `Command::Batch`, so a take is **one undo step**. A later
//!   clip hides the older points under it, which is how Overwrite and Touch replace what was
//!   there, and undo brings the old lane back whole.
//! * **Auto-create lanes.** With the transport bar's AUTO LANES switch on (session state, never
//!   saved) a take also records the parameters that have no lane yet: the first move of one
//!   while the take runs (not a macro-driven, internal or audio-track parameter; a macro-driven
//!   one is reported) starts a Touch recorder for it, and when the take ends the new lanes and
//!   their clips are part of the same `Command::Batch`, so one undo removes both. A parameter
//!   whose lane already exists records only when that lane is armed.
//! * **Override.** Moving a control with a gesture while the song plays and no take runs would
//!   be undone at once by the lane that plays the parameter. The first such move puts the lane in
//!   the *overridden* set, which joins the lanes a take writes in [`EngineSink::hold_automation`]
//!   (at most [`HOLD_CAPACITY`] engine lanes; beyond that the move is refused with a message),
//!   so the engine leaves the parameter at the value the user set. The OVERRIDE lamp of the
//!   transport bar clears the set, and so do Stop and a locate. It is session state: nothing in
//!   the document changes and an offline render never sets a hold.
//! * An engine restart cancels the take without a clip, like a MIDI take.

use super::*;
use atomik_engine::HOLD_CAPACITY;
use atomik_model::automation_rec::{PassRecorder, Segment, WriteMode, segment_clips};
use atomik_model::song::{automation_value_at, flatten_automation};
use atomik_model::{
    AutomationPoint, Clip, ClipContent, Document, LaneKind, expand_bank_lane, expand_macro_lane,
};
use std::collections::BTreeSet;

/// The default thinning tolerance: a recorded point goes when the line between its kept
/// neighbours is within this fraction of the parameter's range.
pub const DEFAULT_AUTOMATION_THINNING: f32 = 0.005;

/// An automation lane: its track's device and its param key.
pub type LaneId = (DeviceKey, String);

/// What the app keeps for automation recording.
#[derive(Debug)]
pub(super) struct AutoRec {
    /// The lanes armed for writing.
    pub(super) armed: BTreeSet<LaneId>,
    /// Thinning tolerance as a fraction of a parameter's range.
    thinning: f32,
    /// The running take.
    take: Option<AutoTake>,
    /// A take also records the parameters that have no lane, adding the lanes when it ends.
    pub(super) auto_create: bool,
    /// The lanes the user moved by hand while the song plays: the engine holds them.
    pub(super) overridden: BTreeSet<LaneId>,
    /// The hold last sent to the engine.
    sent: Vec<(DeviceInstanceId, ParamId)>,
}

impl AutoRec {
    pub(super) fn new() -> Self {
        Self {
            armed: BTreeSet::new(),
            thinning: DEFAULT_AUTOMATION_THINNING,
            take: None,
            auto_create: false,
            overridden: BTreeSet::new(),
            sent: Vec::new(),
        }
    }
}

/// A running automation take.
#[derive(Debug)]
struct AutoTake {
    lanes: BTreeMap<LaneId, LaneTake>,
    /// The loop count of the last transport state seen (a change is a wrap).
    loop_count: u64,
    /// Parameters without a lane that were reported as not recordable (said once each).
    refused: BTreeSet<LaneId>,
}

/// One lane of a running take.
#[derive(Debug)]
struct LaneTake {
    mode: WriteMode,
    /// The current pass.
    pass: PassRecorder,
    /// What earlier passes wrote.
    done: Vec<Segment>,
    /// The value the param had last.
    value: f32,
    /// The size of the param's range (the thinning tolerance is a fraction of it).
    range: f32,
    /// The lane does not exist yet: the take adds it when it ends.
    created: bool,
}

impl<S: EngineSink> AppCore<S> {
    /// The lanes armed for automation writing, in order.
    pub fn armed_automation_lanes(&self) -> &BTreeSet<LaneId> {
        &self.autorec.armed
    }

    /// Whether the automation lane of `param` on `device`'s track is armed for writing.
    pub fn automation_armed(&self, device: &DeviceKey, param: &str) -> bool {
        self.autorec
            .armed
            .contains(&(device.clone(), param.to_owned()))
    }

    /// Arms or disarms an automation lane (a lane that does not exist is refused). It takes
    /// effect at the next take; a running take keeps the lanes it started with.
    pub fn toggle_automation_arm(&mut self, device: &DeviceKey, param: &str) {
        let lane = (device.clone(), param.to_owned());
        let exists = self
            .doc()
            .song
            .track(device)
            .is_some_and(|t| t.automation_lane(param).is_some());
        if !exists {
            self.reject(format!("{device} has no automation lane for {param}"));
            return;
        }
        if !self.autorec.armed.remove(&lane) {
            self.autorec.armed.insert(lane);
        }
        self.show_transport();
    }

    /// The thinning tolerance of recorded automation, as a fraction of a parameter's range.
    pub fn automation_thinning(&self) -> f32 {
        self.autorec.thinning
    }

    /// Sets the thinning tolerance (a fraction of the range, `0..=0.25`; 0 keeps every point).
    pub fn set_automation_thinning(&mut self, fraction: f32) {
        self.autorec.thinning = if fraction.is_finite() {
            fraction.clamp(0.0, 0.25)
        } else {
            DEFAULT_AUTOMATION_THINNING
        };
    }

    /// Whether the lane of `param` on `device` is being written by the running take (the
    /// views draw it red).
    pub fn automation_writing(&self, device: &DeviceKey, param: &str) -> bool {
        self.autorec
            .take
            .as_ref()
            .is_some_and(|t| t.lanes.contains_key(&(device.clone(), param.to_owned())))
    }

    /// Armed lanes that still exist (an armed lane whose track or lane was removed is dropped).
    fn live_armed_lanes(&mut self) -> Vec<LaneId> {
        let song = &self.document.doc().song;
        self.autorec.armed.retain(|(d, p)| {
            song.track(d)
                .is_some_and(|t| t.automation_lane(p).is_some())
        });
        self.autorec.armed.iter().cloned().collect()
    }

    /// True when a take would write automation: some lane is armed, or new lanes are created.
    pub(super) fn has_armed_lanes(&mut self) -> bool {
        self.autorec.auto_create || !self.live_armed_lanes().is_empty()
    }

    /// Muting an automation lane holds what its param plays: the mute and a `SetParam` with the
    /// value the lane gives at the playhead are one batch (one undo step), so the param keeps
    /// that value once the lane stops driving it. Any other command comes back unchanged, and so
    /// does the mute of a lane that plays nothing.
    pub(super) fn hold_on_mute(&self, command: Command) -> Command {
        let Command::Song(SongCommand::SetLaneMuted {
            device,
            lane,
            muted: true,
        }) = &command
        else {
            return command;
        };
        let Some(track) = self.doc().song.track(device) else {
            return command;
        };
        let Some(l) = track.lanes.get(*lane).filter(|l| !l.muted) else {
            return command;
        };
        let LaneKind::Automation(param) = &l.kind else {
            return command;
        };
        let points = flatten_automation(track, *lane);
        if points.is_empty() {
            return command;
        }
        let value = automation_value_at(&points, self.position as f64);
        let hold = Command::SetParam {
            key: device.clone(),
            param: param.clone(),
            value,
            merge: None,
        };
        Command::Batch(vec![command, hold])
    }

    /// Removes the automation lane of `param` on `device`'s track (one undo step).
    pub fn delete_automation_lane(&mut self, device: &DeviceKey, param: &str) {
        // A rejection is already reported.
        let _ = self.apply_command(Command::Song(SongCommand::RemoveAutomationLane {
            device: device.clone(),
            param: param.to_owned(),
        }));
    }

    /// Removes the lane that shows `param` of `device` (the control menu's Clear Automation: a
    /// param a macro drives has the macro's lane). False, with the reason shown, when there is
    /// none.
    pub fn clear_automation_of(&mut self, device: &DeviceKey, param: &str) -> bool {
        let Some((host, lane)) = self.lane_host(device, param) else {
            self.report(format!("{device} {param} has no automation lane"));
            return false;
        };
        if self
            .doc()
            .song
            .track(&host)
            .and_then(|t| t.automation_lane(&lane))
            .is_none()
        {
            self.report(format!("{device} {param} has no automation lane to clear"));
            return false;
        }
        self.delete_automation_lane(&host, &lane);
        true
    }

    /// Mutes or unmutes the automation lane of `param` on `device`'s track (a mute holds the
    /// value the lane gives at the playhead, see [`Self::hold_on_mute`]).
    pub fn toggle_automation_lane_mute(&mut self, device: &DeviceKey, param: &str) {
        let Some(t) = self.doc().song.track(device) else {
            return;
        };
        let Some(lane) = t.automation_lane(param) else {
            return;
        };
        let muted = !t.lanes[lane].muted;
        // A rejection is already reported.
        let _ = self.apply_command(Command::Song(SongCommand::SetLaneMuted {
            device: device.clone(),
            lane,
            muted,
        }));
    }

    /// Gives the lane that shows `param` of `device` the value the control has now as its static
    /// value, adding the lane when it is missing (one undo step). False, with the reason shown,
    /// when the param cannot have a lane.
    pub fn set_static_from_control(&mut self, device: &DeviceKey, param: &str) -> bool {
        let Some((host, lane)) = self.lane_host(device, param) else {
            self.report(format!(
                "{device} {param} has no automation lane: it is an internal param, an audio track's or a container's inner device"
            ));
            return false;
        };
        let Some(value) = self.doc().param_value(&host, &lane, &self.registry) else {
            return false;
        };
        let exists = self
            .doc()
            .song
            .track(&host)
            .is_some_and(|t| t.automation_lane(&lane).is_some());
        let set = Command::Song(SongCommand::SetLaneStatic {
            device: host.clone(),
            param: lane.clone(),
            value: Some(value),
        });
        let command = if exists {
            set
        } else {
            Command::Batch(vec![
                Command::Song(SongCommand::AddAutomationLane {
                    device: host.clone(),
                    param: lane.clone(),
                }),
                set,
            ])
        };
        // A rejection is already reported (a lane that has that value already is one).
        self.apply_command(command).is_ok()
    }

    /// Clears the static value of the lane of `param` on `device`'s track.
    pub fn clear_lane_static(&mut self, device: &DeviceKey, param: &str) {
        // A rejection is already reported.
        let _ = self.apply_command(Command::Song(SongCommand::SetLaneStatic {
            device: device.clone(),
            param: param.to_owned(),
            value: None,
        }));
    }

    /// Whether a take also records the parameters that have no lane yet.
    pub fn auto_create_lanes(&self) -> bool {
        self.autorec.auto_create
    }

    /// Turns the creation of lanes during a take on or off (session state, applied to the next
    /// take: a running take keeps what it started with).
    pub fn set_auto_create_lanes(&mut self, on: bool) {
        if self.autorec.auto_create != on {
            self.autorec.auto_create = on;
            self.show_transport();
        }
    }

    /// How many lanes the user holds with a hand-moved value (the OVERRIDE lamp).
    pub fn overridden_lanes(&self) -> usize {
        self.autorec.overridden.len()
    }

    /// Lets the overridden lanes play again (the OVERRIDE lamp, Stop, a locate).
    pub fn clear_override(&mut self) {
        if self.autorec.overridden.is_empty() {
            return;
        }
        self.autorec.overridden.clear();
        self.update_automation_hold();
        self.show_transport();
    }

    /// Drops the overridden lanes whose lane is gone from the document.
    pub(super) fn prune_override(&mut self) {
        if self.autorec.overridden.is_empty() {
            return;
        }
        let song = &self.document.doc().song;
        let before = self.autorec.overridden.len();
        self.autorec.overridden.retain(|(d, p)| {
            song.track(d)
                .is_some_and(|t| t.automation_lane(p).is_some())
        });
        if self.autorec.overridden.len() != before {
            self.update_automation_hold();
            self.show_transport();
        }
    }

    /// Starts the automation part of a take at the transport position: one recorder per armed
    /// lane. Returns whether any lane records.
    pub(super) fn start_automation_take(&mut self) -> bool {
        let lanes = self.live_armed_lanes();
        let tick = self.position;
        let mut map = BTreeMap::new();
        for lane in lanes {
            let (device, param) = &lane;
            let Some(value) = self.doc().param_value(device, param, &self.registry) else {
                continue;
            };
            let range = self.lane_range(device, param);
            let mode = self.doc().song.write_mode(device, param);
            map.insert(
                lane,
                LaneTake {
                    mode,
                    pass: PassRecorder::new(mode, tick, value),
                    done: Vec::new(),
                    value,
                    range,
                    created: false,
                },
            );
        }
        if map.is_empty() && !self.autorec.auto_create {
            return false;
        }
        let loop_count = self.sink.transport_state().loop_count;
        self.autorec.take = Some(AutoTake {
            lanes: map,
            loop_count,
            refused: BTreeSet::new(),
        });
        self.update_automation_hold();
        true
    }

    /// The size of a lane's parameter range (1 for a parameter without a descriptor).
    fn lane_range(&self, device: &DeviceKey, param: &str) -> f32 {
        self.doc()
            .resolve(device)
            .and_then(|d| self.registry.get(&d.type_id))
            .and_then(|f| f.descriptor.param_by_key(param))
            .map_or(1.0, |p| (p.max - p.min).abs().max(f32::EPSILON))
    }

    /// The tick a parameter move happens at: the latest the engine published while playing,
    /// else the last position shown.
    fn automation_tick(&self, state: &TransportState) -> i64 {
        if state.playing || state.engine_frame > 0 {
            state.tick
        } else {
            self.position
        }
    }

    /// Offers the `ParamChanged` ops of a command to the running take. `merge` is the command's
    /// drag-merge id: `Some` is a held control.
    pub(super) fn capture_automation(&mut self, ops: &[SyncOp], merge: Option<u64>) {
        if self.autorec.take.is_none() {
            self.override_lanes(ops, merge);
            return;
        }
        let state = self.sink.transport_state();
        self.advance_automation(&state);
        let tick = self.automation_tick(&state);
        let create = self.autorec.auto_create;
        let Some(take) = self.autorec.take.as_mut() else {
            return;
        };
        let mut touched = false;
        let mut fresh = Vec::new();
        for op in ops {
            let SyncOp::ParamChanged { key, param, value } = op else {
                continue;
            };
            if let Some(lane) = take.lanes.get_mut(&(key.clone(), param.clone())) {
                lane.pass.change(tick, *value, merge.is_some());
                lane.value = *value;
                touched = true;
            } else if create {
                fresh.push((key.clone(), param.clone(), *value));
            }
        }
        for (device, param, value) in fresh {
            touched |= self.start_created_lane(device, param, value, tick, merge.is_some());
        }
        if touched {
            self.update_automation_hold();
        }
    }

    /// The first move of a parameter that has no lane during a take with lane creation on:
    /// starts a Touch recorder for the lane the take will add. A parameter that cannot have its
    /// own lane (internal, an audio track's, inside a container) is left alone; one a macro drives
    /// is reported once. True when a recorder started.
    fn start_created_lane(
        &mut self,
        device: DeviceKey,
        param: String,
        value: f32,
        tick: i64,
        held: bool,
    ) -> bool {
        let id = (device.clone(), param.clone());
        if self
            .doc()
            .song
            .track(&device)
            .is_some_and(|t| t.automation_lane(&param).is_some())
        {
            // A lane that exists records when it is armed, like any lane.
            return false;
        }
        if self.lane_host(&device, &param).as_ref() != Some(&id) {
            if self.doc().bank_driver(&device, &param).is_some()
                && let Some(take) = self.autorec.take.as_mut()
                && take.refused.insert(id)
            {
                self.report(format!(
                    "{device} {param} is driven by a macro, so no lane is created for it: automate the macro instead"
                ));
            }
            return false;
        }
        let range = self.lane_range(&device, &param);
        let Some(take) = self.autorec.take.as_mut() else {
            return false;
        };
        let mut pass = PassRecorder::new(WriteMode::Touch, tick, value);
        pass.change(tick, value, held);
        take.lanes.insert(
            id,
            LaneTake {
                mode: WriteMode::Touch,
                pass,
                done: Vec::new(),
                value,
                range,
                created: true,
            },
        );
        true
    }

    /// Without a take: a control moved by hand while the song plays holds its lane, so the lane
    /// does not take the value back at the next batch (see the module docs).
    fn override_lanes(&mut self, ops: &[SyncOp], merge: Option<u64>) {
        if merge.is_none() || !(self.playing || self.sink.transport_state().playing) {
            return;
        }
        let mut added = false;
        for op in ops {
            let SyncOp::ParamChanged { key, param, .. } = op else {
                continue;
            };
            let id = (key.clone(), param.clone());
            if self.autorec.overridden.contains(&id) {
                continue;
            }
            let has_lane = self
                .doc()
                .song
                .track(key)
                .is_some_and(|t| t.automation_lane(param).is_some());
            if !has_lane
                || !self
                    .doc()
                    .automation_values_at(self.position as f64)
                    .contains_key(&id)
            {
                continue;
            }
            // One more lane may not push the engine's hold past its capacity.
            let mut engine = Vec::new();
            for (d, p) in self.autorec.overridden.iter().chain(std::iter::once(&id)) {
                self.engine_lanes(d, p, &mut engine);
            }
            engine.sort();
            engine.dedup();
            if engine.len() > HOLD_CAPACITY {
                self.report(format!(
                    "At most {HOLD_CAPACITY} lanes can hold a value you moved: press OVERRIDE to let them play again"
                ));
                continue;
            }
            self.autorec.overridden.insert(id);
            added = true;
        }
        if added {
            self.update_automation_hold();
            self.show_transport();
        }
    }

    /// The pointer was released (a drag ended): Touch lets go.
    pub(super) fn release_automation(&mut self) {
        let tick = self.automation_tick(&self.sink.transport_state());
        let Some(take) = self.autorec.take.as_mut() else {
            return;
        };
        for lane in take.lanes.values_mut() {
            lane.pass.release(tick);
        }
        self.update_automation_hold();
    }

    /// A loop wrap since the last look ends the pass at the loop end and starts the next at the
    /// loop start. Called with every transport state the take sees.
    pub(super) fn advance_automation(&mut self, state: &TransportState) {
        let Some(take) = self.autorec.take.as_mut() else {
            return;
        };
        if state.loop_count == take.loop_count {
            return;
        }
        take.loop_count = state.loop_count;
        for lane in take.lanes.values_mut() {
            let next = PassRecorder::new(lane.mode, state.loop_start, lane.value);
            let ended = std::mem::replace(&mut lane.pass, next);
            lane.done.extend(ended.finish(state.loop_end));
        }
        self.update_automation_hold();
    }

    /// Sends the lanes the engine must leave alone, the ones a take is writing and the ones
    /// the user overrides, when the set changed. A lane a take will add has no lane in the
    /// engine yet, so it holds nothing.
    pub(super) fn update_automation_hold(&mut self) {
        let mut lanes: Vec<LaneId> = self
            .autorec
            .take
            .as_ref()
            .map(|take| {
                take.lanes
                    .iter()
                    .filter(|(_, l)| l.pass.is_writing() && !l.created)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        lanes.extend(self.autorec.overridden.iter().cloned());
        let mut held = Vec::new();
        for (device, param) in &lanes {
            self.engine_lanes(device, param, &mut held);
        }
        held.sort();
        held.dedup();
        if self.autorec.sent != held {
            self.sink.hold_automation(&held);
            self.autorec.sent = held;
        }
    }

    /// The lanes a running take is writing now, and the ones the user overrides (they show the
    /// user's value, not the lane's).
    pub(super) fn writing_automation_lanes(&self) -> Vec<LaneId> {
        let mut out: Vec<LaneId> = self.autorec.take.as_ref().map_or_else(Vec::new, |take| {
            take.lanes
                .iter()
                .filter(|(_, l)| l.pass.is_writing())
                .map(|(id, _)| id.clone())
                .collect()
        });
        out.extend(self.autorec.overridden.iter().cloned());
        out
    }

    /// True while an automation take is being recorded.
    pub(super) fn automation_take_running(&self) -> bool {
        self.autorec.take.is_some()
    }

    /// The engine `(device, param)` pairs that play the lane of `param` on `device`: itself,
    /// or for a container's macro every target it expands to.
    fn engine_lanes(
        &self,
        device: &DeviceKey,
        param: &str,
        out: &mut Vec<(DeviceInstanceId, ParamId)>,
    ) {
        let doc = self.doc();
        let one = |key: &DeviceKey, param: &str| {
            let id = self.ids.get(key).copied()?;
            let pid = containers::type_of(doc, key)
                .and_then(|t| self.registry.get(&t))
                .and_then(|f| f.descriptor.param_by_key(param))
                .map(|p| p.id)?;
            Some((id, pid))
        };
        if containers::is_container(doc, device) {
            let targets = expand_macro_lane(doc, &self.registry, device, param, &[]);
            for (target, target_param, _) in targets.into_iter().flatten() {
                out.extend(one(&target, &target_param));
            }
        } else if let Some(targets) = expand_bank_lane(doc, &self.registry, device, param, &[]) {
            // A macro bank's lane holds every param its macro drives.
            for (target, target_param, _) in targets {
                out.extend(one(&target, &target_param));
            }
        } else {
            out.extend(one(device, param));
        }
    }

    /// Ends the automation take at the transport position and writes it: one
    /// `Command::Batch` of `AddClip`s (one undo step). A take that wrote nothing changes
    /// nothing.
    pub(super) fn finish_automation_take(&mut self, state: &TransportState) {
        self.advance_automation(state);
        let Some(take) = self.autorec.take.take() else {
            return;
        };
        self.update_automation_hold();
        let end = self.automation_tick(state);
        let doc = self.document.doc();
        // The lanes the take adds come first, then every clip: one batch, one undo step. A
        // scratch copy tells which index each new lane gets.
        let mut scratch: Option<Document> = None;
        let mut adds = Vec::new();
        let mut commands = Vec::new();
        for ((device, param), lane) in take.lanes {
            let mut segments = lane.done;
            segments.extend(lane.pass.finish(end));
            let tolerance = self.autorec.thinning * lane.range;
            let clips = segment_clips(&segments, tolerance);
            if clips.is_empty() {
                continue;
            }
            let index = if lane.created {
                let add = Command::Song(SongCommand::AddAutomationLane {
                    device: device.clone(),
                    param: param.clone(),
                });
                let scratch = scratch.get_or_insert_with(|| Document::new(doc.clone()));
                // A lane that cannot be added (its device is gone, its track frozen) is dropped.
                if scratch.apply(add.clone(), &self.registry).is_err() {
                    continue;
                }
                adds.push(add);
                scratch
                    .doc()
                    .song
                    .track(&device)
                    .and_then(|t| t.automation_lane(&param))
            } else {
                doc.song
                    .track(&device)
                    .and_then(|t| t.automation_lane(&param))
            };
            let Some(index) = index else {
                continue;
            };
            for (start, length, points) in clips {
                commands.push(Command::Song(SongCommand::AddClip {
                    device: device.clone(),
                    lane: index,
                    clip: automation_clip(start, length, points),
                }));
            }
        }
        if commands.is_empty() {
            return;
        }
        adds.extend(commands);
        let commands = adds;
        // A rejection is already reported.
        let _ = self.apply_command(Command::Batch(commands));
        self.document.end_merge();
    }

    /// Throws a running automation take away without a clip, and the overrides with it (engine
    /// restart: the new engine holds nothing).
    pub(super) fn cancel_automation_take(&mut self) {
        let had = self.autorec.take.take().is_some() || !self.autorec.overridden.is_empty();
        self.autorec.overridden.clear();
        self.autorec.sent.clear();
        if had {
            self.sink.hold_automation(&[]);
            self.show_transport();
        }
    }
}

/// A clip for a written stretch (the id is allocated by the command).
fn automation_clip(start: i64, length: i64, points: Vec<AutomationPoint>) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        content: ClipContent::Automation(points),
    }
}
