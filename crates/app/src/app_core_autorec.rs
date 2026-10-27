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
//! * An engine restart cancels the take without a clip, like a MIDI take.

use super::*;
use atomik_model::automation_rec::{PassRecorder, Segment, WriteMode, segment_clips};
use atomik_model::{AutomationPoint, Clip, ClipContent, expand_bank_lane, expand_macro_lane};
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
}

impl AutoRec {
    pub(super) fn new() -> Self {
        Self {
            armed: BTreeSet::new(),
            thinning: DEFAULT_AUTOMATION_THINNING,
            take: None,
        }
    }
}

/// A running automation take.
#[derive(Debug)]
struct AutoTake {
    lanes: BTreeMap<LaneId, LaneTake>,
    /// The loop count of the last transport state seen (a change is a wrap).
    loop_count: u64,
    /// The lanes held in the engine now.
    held: Vec<(DeviceInstanceId, ParamId)>,
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

    /// True when a take would write automation (some lane is armed).
    pub(super) fn has_armed_lanes(&mut self) -> bool {
        !self.live_armed_lanes().is_empty()
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
                },
            );
        }
        if map.is_empty() {
            return false;
        }
        let loop_count = self.sink.transport_state().loop_count;
        self.autorec.take = Some(AutoTake {
            lanes: map,
            loop_count,
            held: Vec::new(),
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
            return;
        }
        let state = self.sink.transport_state();
        self.advance_automation(&state);
        let tick = self.automation_tick(&state);
        let Some(take) = self.autorec.take.as_mut() else {
            return;
        };
        let mut touched = false;
        for op in ops {
            if let SyncOp::ParamChanged { key, param, value } = op
                && let Some(lane) = take.lanes.get_mut(&(key.clone(), param.clone()))
            {
                lane.pass.change(tick, *value, merge.is_some());
                lane.value = *value;
                touched = true;
            }
        }
        if touched {
            self.update_automation_hold();
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

    /// Sends the lanes being written to the engine when the set changed.
    fn update_automation_hold(&mut self) {
        let Some(take) = &self.autorec.take else {
            return;
        };
        let writing: Vec<LaneId> = take
            .lanes
            .iter()
            .filter(|(_, l)| l.pass.is_writing())
            .map(|(id, _)| id.clone())
            .collect();
        let mut held = Vec::new();
        for (device, param) in &writing {
            self.engine_lanes(device, param, &mut held);
        }
        held.sort();
        held.dedup();
        if let Some(take) = self.autorec.take.as_mut()
            && take.held != held
        {
            self.sink.hold_automation(&held);
            take.held = held;
        }
    }

    /// The lanes a running take is writing now (they show the user's value, not the lane's).
    pub(super) fn writing_automation_lanes(&self) -> Vec<LaneId> {
        self.autorec.take.as_ref().map_or_else(Vec::new, |take| {
            take.lanes
                .iter()
                .filter(|(_, l)| l.pass.is_writing())
                .map(|(id, _)| id.clone())
                .collect()
        })
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
        self.sink.hold_automation(&[]);
        let end = self.automation_tick(state);
        let doc = self.document.doc();
        let mut commands = Vec::new();
        for ((device, param), lane) in take.lanes {
            let Some(index) = doc
                .song
                .track(&device)
                .and_then(|t| t.automation_lane(&param))
            else {
                continue;
            };
            let mut segments = lane.done;
            segments.extend(lane.pass.finish(end));
            let tolerance = self.autorec.thinning * lane.range;
            for (start, length, points) in segment_clips(&segments, tolerance) {
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
        // A rejection is already reported.
        let _ = self.apply_command(Command::Batch(commands));
        self.document.end_merge();
    }

    /// Throws a running automation take away without a clip (engine restart).
    pub(super) fn cancel_automation_take(&mut self) {
        if self.autorec.take.take().is_some() {
            self.sink.hold_automation(&[]);
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
