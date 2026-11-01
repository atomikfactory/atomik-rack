//! MIDI learn and mappings in the app core.
//!
//! The project's mappings (`RackDoc::midi_maps`, see [`atomik_model::midi_map`]) turn the messages
//! of the live MIDI input into commands. Everything here runs on the UI thread: the MIDI thread
//! only parses a message, queues it in the [`ControlHub`] and decides, from the hub's consume
//! list, whether the message also plays a note ([`crate::midi`]). [`AppCore::midi_control_frame`]
//! drains the hub once per frame.
//!
//! * **Applying.** A message goes through every mapping whose source matches it
//!   ([`MidiMapping::feed`]: range, invert, relative encoders, soft takeover, buttons) and the
//!   result is an ordinary command: `Command::SetParam` (continuous moves carry one merge id per
//!   gesture, so a sweep is one undo step like a knob drag), `SetVcaLevel`, `SetTempo` or a
//!   transport request. The engine therefore sees an ordinary param change and its smoothers
//!   apply it, and automation recording captures it like a mouse edit: **a mapped move is a
//!   touch**. A gesture ends when the control has been still for [`GESTURE_TIMEOUT`] seconds (a
//!   knob sends no release), which ends the merge and lets a Touch lane go.
//! * **Learning.** *MIDI Learn* mode swallows the first change made to a control in the views
//!   (a `SetParam`, a VCA fader move, a tempo edit or a transport button) and arms that target
//!   instead; the right-click menu of a control arms it directly. While a target is armed the
//!   first control moved on the hardware becomes its source (a 14 bit pair is recognised by its
//!   two halves, [`Learner`]); the mapping is one undo step, and every message is consumed while
//!   learning, so the keyboard plays nothing.
//! * **What is consumed.** A message a mapping with `consume` listens to never reaches the notes
//!   of the armed instrument; the list lives in the hub and is rebuilt when the mappings
//!   change.
//! * **Gaps.** A mapping whose target is gone does nothing (it is listed as missing).

use super::*;
use crate::midi_control::{ControlHub, Inbound};
use atomik_core::{ChannelMessage, VCA_MAX_DB, VCA_MIN_DB};
use atomik_model::{
    Effect, Learner, MapMode, MapState, MidiMapCommand, MidiMapping, MidiPreset, MidiSource,
    MidiTarget, RemoteCommand, TEMPO_MAX, TEMPO_MIN, TransportTarget, load_midi_preset,
    save_midi_preset,
};
use atomik_ui::{MidiMapRow, MidiMarks};

/// Seconds without a message after which a mapped move counts as let go.
pub const GESTURE_TIMEOUT: f64 = 0.5;

/// MIDI gesture merge ids start here, clear of the views' (multiples of 2^40 up to 11 * 2^40).
const MIDI_MERGE_BASE: u64 = 12 << 40;

/// What a held gesture is about: a mapping or a surface control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GestureKey {
    /// The mapping with this id.
    Map(u32),
    /// A control of a surface (`strip:3`, `master`, `pan:3`).
    Surface(String),
}

/// A held control: its merge id, when it last moved and whether a finger holds it (a touch
/// does not time out; it ends with the release).
#[derive(Debug, Clone)]
struct Gesture {
    key: GestureKey,
    merge: u64,
    last_at: f64,
    held: bool,
}

/// What the app keeps for MIDI learn and mappings (none of it is saved).
#[derive(Debug)]
pub(super) struct MidiControl {
    /// The queue from the MIDI thread, shared with the live session's input.
    pub(super) hub: Arc<ControlHub>,
    /// The learn mode: the next change made to a control arms it.
    learn_mode: bool,
    /// The target waiting for a hardware control.
    armed: Option<MidiTarget>,
    learner: Learner,
    /// The soft takeover and the like, by mapping id.
    states: BTreeMap<u32, MapState>,
    gestures: Vec<Gesture>,
    next_merge: u64,
    /// The time of the last frame, in seconds.
    now: f64,
    inbox: Vec<Inbound>,
    /// The last message seen and the mappings it hit, for the status line.
    last: Option<(Arc<str>, ChannelMessage)>,
    /// The messages received in all.
    received: u64,
    /// The device each control surface is locked to (runtime only, never saved), by surface name.
    pub(super) locks: BTreeMap<String, DeviceKey>,
    /// The surface whose message is being handled, for the mappings that follow a lock.
    origin: Option<String>,
    /// The surface of the last message the learner was fed, for the source it finds.
    learn_surface: Option<String>,
}

impl MidiControl {
    pub(super) fn new(hub: Arc<ControlHub>) -> Self {
        Self {
            hub,
            learn_mode: false,
            armed: None,
            learner: Learner::new(),
            states: BTreeMap::new(),
            gestures: Vec::new(),
            next_merge: MIDI_MERGE_BASE,
            now: 0.0,
            inbox: Vec::new(),
            last: None,
            received: 0,
            locks: BTreeMap::new(),
            origin: None,
            learn_surface: None,
        }
    }

    pub(super) fn learning(&self) -> bool {
        self.learn_mode || self.armed.is_some()
    }

    /// Opens or refreshes the gesture of `key` and returns its merge id.
    pub(super) fn gesture(&mut self, key: GestureKey, held: bool) -> u64 {
        let now = self.now;
        if let Some(g) = self.gestures.iter_mut().find(|g| g.key == key) {
            g.last_at = now;
            g.held |= held;
            return g.merge;
        }
        self.next_merge += 1;
        let merge = self.next_merge;
        self.gestures.push(Gesture {
            key,
            merge,
            last_at: now,
            held,
        });
        merge
    }

    /// Sets the time of the frame being handled.
    pub(super) fn set_now(&mut self, now: f64) {
        self.now = now;
    }

    /// True while a finger holds the gesture of `key`.
    pub(super) fn is_held(&self, key: &GestureKey) -> bool {
        self.gestures.iter().any(|g| &g.key == key && g.held)
    }

    /// Closes the gesture of `key`; true if it was open.
    pub(super) fn close_gesture(&mut self, key: &GestureKey) -> bool {
        let before = self.gestures.len();
        self.gestures.retain(|g| &g.key != key);
        self.gestures.len() != before
    }

    /// Closes the gestures that have been still for [`GESTURE_TIMEOUT`] (a held one never
    /// times out); true if any closed.
    fn expire(&mut self) -> bool {
        let now = self.now;
        let before = self.gestures.len();
        self.gestures
            .retain(|g| g.held || now - g.last_at < GESTURE_TIMEOUT);
        self.gestures.len() != before
    }
}

/// A target resolved against the project and the registry.
pub(super) enum Resolved {
    /// A param of a device.
    Param {
        key: DeviceKey,
        desc: &'static atomik_core::ParamDescriptor,
    },
    /// A VCA group's level.
    Vca(u32),
    /// The song tempo.
    Tempo,
    /// A transport key.
    Transport(TransportTarget),
    /// A command (undo, redo, select a track or a preset).
    Command(RemoteCommand),
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ learn state

    /// True while MIDI Learn mode is on.
    pub fn midi_learn_mode(&self) -> bool {
        self.midi.learn_mode
    }

    /// The target waiting for a hardware control, if any.
    pub fn midi_armed(&self) -> Option<&MidiTarget> {
        self.midi.armed.as_ref()
    }

    /// Switches MIDI Learn mode on or off; leaving it drops an armed target.
    pub fn set_midi_learn(&mut self, on: bool) {
        if self.midi.learn_mode == on {
            return;
        }
        self.midi.learn_mode = on;
        if on {
            self.report("MIDI Learn: change a control, then move a hardware control");
        } else {
            self.midi.armed = None;
            self.midi.learner.reset();
            self.report("MIDI Learn off");
        }
        self.sync_learning();
    }

    /// Flips MIDI Learn mode.
    pub fn toggle_midi_learn(&mut self) {
        self.set_midi_learn(!self.midi.learn_mode);
    }

    /// Arms `target`: the next hardware control moved is mapped to it. False, with a message,
    /// when the target cannot be mapped.
    pub fn midi_arm(&mut self, target: MidiTarget) -> bool {
        if let Some(problem) = self.doc().midi_target_problem(&self.registry, &target) {
            self.report(problem);
            return false;
        }
        self.midi.learner.reset();
        self.report(format!(
            "Move a hardware control to map {}",
            target.describe()
        ));
        self.midi.armed = Some(target);
        self.sync_learning();
        true
    }

    /// Drops the armed target (Esc).
    pub fn midi_cancel_arm(&mut self) {
        if self.midi.armed.take().is_some() {
            self.midi.learner.reset();
            self.sync_learning();
        }
    }

    fn sync_learning(&mut self) {
        self.midi.hub.set_learning(self.midi.learning());
        self.sync_midi_marks();
    }

    /// Removes every mapping of `target` (Forget MIDI); false, with a message, when it has none.
    pub fn midi_forget(&mut self, target: MidiTarget) -> bool {
        let text = target.describe();
        let done = self
            .apply_command(Command::MidiMap(MidiMapCommand::Forget { target }))
            .is_ok();
        if done {
            self.report(format!("Forgot the MIDI mapping of {text}"));
        }
        done
    }

    /// True when `device`'s `param` has a mapping (the views mark it).
    pub fn is_midi_mapped(&self, device: &DeviceKey, param: &str) -> bool {
        self.doc().midi_maps.iter().any(|m| {
            matches!(&m.target, MidiTarget::Param { device: d, param: p } if d == device && p == param)
        })
    }

    /// The target a view command stands for, when learning can arm it.
    fn learn_target_of(&self, command: &Command) -> Option<MidiTarget> {
        match command {
            Command::SetParam { key, param, .. } => Some(MidiTarget::Param {
                device: key.clone(),
                param: param.clone(),
            }),
            Command::Console(atomik_model::ConsoleCommand::SetVcaLevel { vca, .. }) => {
                Some(MidiTarget::Vca { id: *vca })
            }
            Command::SetTempo(_) => Some(MidiTarget::Tempo),
            Command::Batch(commands) => commands.iter().find_map(|c| self.learn_target_of(c)),
            _ => None,
        }
    }

    /// In MIDI Learn mode a command from a view arms its control instead of running; true when
    /// it was swallowed.
    pub(super) fn learn_swallows(&mut self, command: &Command) -> bool {
        if !self.midi.learn_mode {
            return false;
        }
        let Some(target) = self.learn_target_of(command) else {
            return false;
        };
        if self.midi.armed.as_ref() != Some(&target) {
            self.midi_arm(target);
        }
        true
    }

    /// In MIDI Learn mode a transport button arms its target instead of running.
    pub(super) fn learn_swallows_transport(&mut self, action: TransportAction) -> bool {
        if !self.midi.learn_mode {
            return false;
        }
        let target = match action {
            TransportAction::Play => TransportTarget::Play,
            TransportAction::Stop => TransportTarget::Stop,
            TransportAction::TogglePlay => TransportTarget::TogglePlay,
            TransportAction::ToggleRecord => TransportTarget::Record,
            TransportAction::ToggleLoop => TransportTarget::Loop,
            TransportAction::ReturnToZero => TransportTarget::ReturnToZero,
            _ => return false,
        };
        self.midi_arm(MidiTarget::Transport(target));
        true
    }

    // ------------------------------------------------------------------ the message path

    /// The hub the MIDI input fills.
    pub fn control_hub(&self) -> &Arc<ControlHub> {
        &self.midi.hub
    }

    /// Feeds a message as if `port` had sent it (the MCP server, tests); it is handled by the
    /// next [`Self::midi_control_frame`].
    pub fn inject_midi(&self, port: &str, message: ChannelMessage) {
        self.midi.hub.push(&Arc::from(port), message);
    }

    /// [`Self::inject_midi`] for raw bytes: one note, controller or pitch bend message (the MCP
    /// server and the hardware-free tests).
    ///
    /// # Errors
    /// A message naming the bytes when they are not such a message.
    pub fn inject_midi_bytes(&self, port: &str, bytes: &[u8]) -> Result<ChannelMessage, String> {
        let message = atomik_midi_io::parse_message(bytes, &mut atomik_midi_io::RunningStatus::default())
            .ok_or_else(|| {
                format!(
                    "{bytes:02X?} is not a note, controller or pitch bend message (a channel message of three bytes)"
                )
            })?;
        self.inject_midi(port, message);
        Ok(message)
    }

    /// Handles what the MIDI input queued since the last frame, learns, and lets held moves go:
    /// call it once per UI frame with a monotonic time in seconds.
    pub fn midi_control_frame(&mut self, now: f64) {
        self.midi.now = now;
        let mut inbox = std::mem::take(&mut self.midi.inbox);
        self.midi.hub.drain(&mut inbox);
        for Inbound { port, message } in inbox.drain(..) {
            self.midi.received += 1;
            self.midi.last = Some((Arc::clone(&port), message));
            self.midi_message(&port, message);
        }
        self.midi.inbox = inbox;
        if self.midi.armed.is_some()
            && let Some(source) = self.midi.learner.tick(now)
        {
            self.finish_learn(source);
        }
        self.end_expired_gestures();
    }

    /// Ends the gestures whose control has been still for the timeout (merge, automation touch).
    pub(super) fn end_expired_gestures(&mut self) {
        if self.midi.expire() {
            self.document.end_merge();
            self.release_automation();
        }
    }

    /// The last message the input received, as text (`Pad: CC 7 ch 1 = 64`), for a status line.
    pub fn midi_last_message(&self) -> Option<String> {
        let (port, m) = self.midi.last.as_ref()?;
        Some(format!("{port}: {}", describe_message(m)))
    }

    /// Messages received in all (tests and the status line).
    pub fn midi_received(&self) -> u64 {
        self.midi.received
    }

    fn midi_message(&mut self, port: &Arc<str>, msg: ChannelMessage) {
        self.midi_message_from(port, None, msg);
    }

    /// Handles the message of a generic controller surface: counted and shown in the status line
    /// like a message of a port, then run through the mappings tagged with the surface.
    pub(super) fn surface_message(&mut self, port: &Arc<str>, surface: &str, msg: ChannelMessage) {
        self.midi.received += 1;
        self.midi.last = Some((Arc::clone(port), msg));
        self.midi_message_from(port, Some(surface), msg);
    }

    /// Handles one message from `port`, sent by the control surface `surface` when it came from
    /// one. Learning takes its source (tied to the surface when there is one); otherwise every
    /// mapping that hears it runs.
    fn midi_message_from(&mut self, port: &Arc<str>, surface: Option<&str>, msg: ChannelMessage) {
        if self.midi.learning() {
            // Learning: nothing runs; with a target armed the first control becomes its source.
            if self.midi.armed.is_some() {
                self.midi.learn_surface = surface.map(str::to_owned);
                if let Some(source) = self.midi.learner.feed(port, msg, self.midi.now) {
                    self.finish_learn(source);
                }
            }
            return;
        }
        let hits: Vec<MidiMapping> = self
            .doc()
            .midi_maps
            .iter()
            .filter(|m| m.source.matches_from(port, surface, &msg))
            .cloned()
            .collect();
        self.midi.origin = surface.map(str::to_owned);
        for mapping in hits {
            self.apply_mapping(&mapping, &msg);
        }
        self.midi.origin = None;
    }

    fn finish_learn(&mut self, mut source: MidiSource) {
        source.surface = self.midi.learn_surface.take();
        let Some(target) = self.midi.armed.take() else {
            return;
        };
        self.midi.learner.reset();
        self.sync_learning();
        let text = format!("{} to {}", source.describe(), target.describe());
        if self
            .apply_command(Command::MidiMap(MidiMapCommand::Learn { source, target }))
            .is_ok()
        {
            self.report(format!("Mapped {text}"));
        }
    }

    // ------------------------------------------------------------------ resolving and applying

    fn resolve(&self, target: &MidiTarget) -> Option<Resolved> {
        match target {
            MidiTarget::Param { device, param } => self.resolve_param(device, param),
            MidiTarget::Vca { id } => self.doc().console.vca(*id).map(|_| Resolved::Vca(*id)),
            MidiTarget::Tempo => Some(Resolved::Tempo),
            MidiTarget::Transport(t) => Some(Resolved::Transport(*t)),
            MidiTarget::SelectedStrip { param } => {
                let strip = self.strips_to_route(true).into_iter().next()?;
                self.resolve_param(&strip, param)
            }
            MidiTarget::Focused { param } => {
                let device = self.focused_device(Some(param))?;
                self.resolve_param(&device, param)
            }
            MidiTarget::Command(c) => Some(Resolved::Command(*c)),
        }
    }

    pub(super) fn resolve_param(&self, device: &DeviceKey, param: &str) -> Option<Resolved> {
        let d = self.doc().resolve(device)?;
        let desc = self
            .registry
            .get(&d.type_id)?
            .descriptor
            .param_by_key(param)?;
        Some(Resolved::Param {
            key: device.clone(),
            desc,
        })
    }

    /// The target's value now, as a fraction of its range.
    pub(super) fn target_value(&self, r: &Resolved) -> Option<f32> {
        match r {
            Resolved::Param { key, desc } => {
                let plain = self.doc().param_value(key, desc.key, &self.registry)?;
                Some(desc.to_normalized(plain))
            }
            Resolved::Vca(id) => {
                let db = self.doc().console.vca(*id)?.level_db;
                Some(((db - VCA_MIN_DB) / (VCA_MAX_DB - VCA_MIN_DB)).clamp(0.0, 1.0))
            }
            Resolved::Tempo => Some(
                ((self.doc().tempo_bpm - TEMPO_MIN) / (TEMPO_MAX - TEMPO_MIN)).clamp(0.0, 1.0)
                    as f32,
            ),
            Resolved::Transport(_) | Resolved::Command(_) => None,
        }
    }

    fn apply_mapping(&mut self, mapping: &MidiMapping, msg: &ChannelMessage) {
        let Some(resolved) = self.resolve(&mapping.target) else {
            return;
        };
        let current = self.target_value(&resolved);
        let mut m = mapping.clone();
        // A stepped param has no use for the takeover (its value jumps by steps anyway).
        if matches!(&resolved, Resolved::Param { desc, .. } if desc.discrete) {
            m.options.pickup = false;
        }
        let mut state = self.midi.states.remove(&m.id).unwrap_or_default();
        let effect = m.feed(&mut state, msg, current);
        self.midi.states.insert(m.id, state);
        let continuous = matches!(m.options.mode, MapMode::Absolute | MapMode::Relative(_));
        match effect {
            Some(Effect::Trigger) => match resolved {
                Resolved::Transport(t) => self.transport_action(match t {
                    TransportTarget::Play => TransportAction::Play,
                    TransportTarget::Stop => TransportAction::Stop,
                    TransportTarget::TogglePlay => TransportAction::TogglePlay,
                    TransportTarget::Record => TransportAction::ToggleRecord,
                    TransportTarget::Loop => TransportAction::ToggleLoop,
                    TransportTarget::ReturnToZero => TransportAction::ReturnToZero,
                }),
                Resolved::Command(c) => self.run_remote(c),
                _ => {}
            },
            Some(Effect::Set(norm)) => {
                let merge = continuous.then(|| self.midi.gesture(GestureKey::Map(m.id), false));
                self.set_resolved(&resolved, norm, merge);
            }
            None => {
                // A control held still (a repeated value, a knob waiting to catch the target)
                // keeps its gesture open while it keeps talking.
                if continuous {
                    let _ = self.midi.gesture(GestureKey::Map(m.id), false);
                }
            }
        }
    }

    /// The device the mappings that address "the focused device" act on: the device the surface
    /// of the message being handled is locked to (whatever is selected), else the device selected
    /// in the rack (the primary one first). With `param`, a device that has no such param does
    /// not count (a locked surface then does nothing rather than act on another device).
    pub fn focused_device(&self, param: Option<&str>) -> Option<DeviceKey> {
        let fits = |key: &DeviceKey| match param {
            Some(p) => self.resolve_param(key, p).is_some(),
            None => self.doc().resolve(key).is_some(),
        };
        if let Some(lock) = self
            .midi
            .origin
            .as_ref()
            .and_then(|surface| self.midi.locks.get(surface))
        {
            return fits(lock).then(|| lock.clone());
        }
        self.view
            .primary()
            .into_iter()
            .chain(self.view.selection())
            .find(|k| fits(k))
    }

    /// Runs a command a mapping asked for. A mapped move that is still open ends first, so the
    /// command is a step of its own (undo takes back the whole sweep, not part of it).
    fn run_remote(&mut self, command: RemoteCommand) {
        self.midi.gestures.clear();
        self.document.end_merge();
        self.release_automation();
        match command {
            RemoteCommand::Undo => {
                if !self.undo() && self.rejection.is_none() {
                    self.report("Nothing to undo");
                }
            }
            RemoteCommand::Redo => {
                if !self.redo() && self.rejection.is_none() {
                    self.report("Nothing to redo");
                }
            }
            RemoteCommand::SelectTrack { delta } => {
                let id = if delta < 0 {
                    "sequencer.select_prev_track"
                } else {
                    "sequencer.select_next_track"
                };
                for _ in 0..delta.unsigned_abs() {
                    self.pane_command(Pane::Arrangement, id, None);
                }
                // The track's device is the focused one from now on.
                if let Some(device) = self.arrangement.selected_track().cloned() {
                    self.select(Some(device));
                }
            }
            RemoteCommand::SelectPreset { delta } => {
                let Some(device) = self.focused_device(None) else {
                    self.report("Select a device to change its preset");
                    return;
                };
                if self.plugins.instances.contains_key(&device) {
                    self.step_plugin_preset(&device, i32::from(delta));
                    self.feed_preset_bar();
                } else {
                    self.step_patch(&device, i32::from(delta));
                }
            }
        }
    }

    /// Sets a resolved target to `norm` (a fraction of its range) as one ordinary command; a
    /// value the target already has is not a command.
    pub(super) fn set_resolved(&mut self, r: &Resolved, norm: f32, merge: Option<u64>) {
        let command = match r {
            Resolved::Param { key, desc } => {
                let plain = desc.from_normalized(norm);
                let old = self.doc().param_value(key, desc.key, &self.registry);
                if old.is_some_and(|o| o.to_bits() == plain.to_bits()) {
                    return;
                }
                Command::SetParam {
                    key: key.clone(),
                    param: desc.key.to_owned(),
                    value: plain,
                    merge,
                }
            }
            Resolved::Vca(id) => {
                let db = VCA_MIN_DB + norm.clamp(0.0, 1.0) * (VCA_MAX_DB - VCA_MIN_DB);
                let Some(old) = self.doc().console.vca(*id).map(|v| v.level_db) else {
                    return;
                };
                let db = atomik_model::clamp_vca_db(db);
                if old.to_bits() == db.to_bits() {
                    return;
                }
                Command::Console(atomik_model::ConsoleCommand::SetVcaLevel {
                    vca: *id,
                    db,
                    merge,
                })
            }
            Resolved::Tempo => {
                let bpm = (TEMPO_MIN + f64::from(norm.clamp(0.0, 1.0)) * (TEMPO_MAX - TEMPO_MIN))
                    .mul_add(10.0, 0.0)
                    .round()
                    / 10.0;
                if (bpm - self.doc().tempo_bpm).abs() < 1e-9 {
                    return;
                }
                Command::SetTempo(bpm)
            }
            Resolved::Transport(_) | Resolved::Command(_) => return,
        };
        // A rejection (a param a macro drives, a frozen track) is shown like any other.
        let _ = self.apply_command(command);
    }

    /// A new project replaces the old one: no armed target, no held move, no takeover state.
    pub(super) fn midi_replaced(&mut self) {
        self.midi.armed = None;
        self.midi.learner.reset();
        self.midi.gestures.clear();
        self.sync_learning();
        self.midi_maps_changed();
    }

    /// Rebuilds what depends on the mappings: the consume list of the hub and the per-mapping
    /// state (the soft takeover starts again). Called when the mappings change and the project
    /// is replaced.
    pub(super) fn midi_maps_changed(&mut self) {
        self.midi.states.clear();
        let sources: Vec<MidiSource> = self
            .doc()
            .midi_maps
            .iter()
            .filter(|m| m.options.consume)
            .map(|m| m.source.clone())
            .collect();
        self.midi.hub.set_consumed(sources);
        self.sync_midi_marks();
    }

    // ------------------------------------------------------------------ views

    /// What the Mixer menu's MIDI items can do now.
    pub fn midi_menu(&self) -> crate::commands::MidiMenu {
        crate::commands::MidiMenu {
            learning: self.midi.learn_mode,
            has_maps: !self.doc().midi_maps.is_empty(),
            surfaces: !self.surfaces.is_empty(),
            locked: !self.midi.locks.is_empty(),
        }
    }

    /// The mapped controls and the learn state, as the rack and the console mark them.
    pub fn midi_marks(&self) -> MidiMarks {
        let mut marks = MidiMarks {
            learning: self.midi.learn_mode || self.midi.armed.is_some(),
            ..MidiMarks::default()
        };
        for m in &self.doc().midi_maps {
            match &m.target {
                MidiTarget::Param { device, param } => {
                    marks.mapped.insert((device.clone(), param.clone()));
                }
                MidiTarget::Vca { id } => {
                    marks.vcas.insert(*id);
                }
                _ => {}
            }
        }
        if let Some(MidiTarget::Param { device, param }) = &self.midi.armed {
            marks.armed = Some((device.clone(), param.clone()));
        }
        marks
    }

    /// Hands the rack and the console the current marks (a no-op while nothing is marked).
    pub(super) fn sync_midi_marks(&mut self) {
        let marks = self.midi_marks();
        self.view.set_midi_marks(marks.clone());
        self.console.set_midi_marks(marks);
    }

    /// The text of the status bar while MIDI Learn is on, or `None`.
    pub fn midi_status(&self) -> Option<String> {
        match (&self.midi.armed, self.midi.learn_mode) {
            (Some(t), _) => Some(format!(
                "MIDI LEARN: move a hardware control for {}",
                self.target_text(t)
            )),
            (None, true) => {
                Some("MIDI LEARN: change a control, then move a hardware control".to_owned())
            }
            (None, false) => None,
        }
    }

    /// A target as text for lists: the device's name and the param's name, `VCA Drums`, `Tempo`.
    pub fn target_text(&self, t: &MidiTarget) -> String {
        let doc = self.doc();
        let param_name = |device: &DeviceKey, param: &str| -> String {
            doc.resolve(device)
                .and_then(|d| self.registry.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(param))
                .map_or_else(|| param.to_owned(), |p| p.name.to_owned())
        };
        match t {
            MidiTarget::Param { device, param } => {
                let name = doc
                    .device(device)
                    .map_or_else(|| device.0.clone(), |d| d.name.clone());
                format!("{name} {}", param_name(device, param))
            }
            MidiTarget::Vca { id } => doc
                .console
                .vca(*id)
                .map_or_else(|| format!("VCA {id}"), |v| format!("VCA {}", v.name)),
            MidiTarget::SelectedStrip { param } => format!("Selected strip {param}"),
            other => other.describe(),
        }
    }

    /// The rows of the MIDI Mappings panel.
    pub fn midi_map_rows(&self) -> Vec<MidiMapRow> {
        self.doc()
            .midi_maps
            .iter()
            .map(|m| MidiMapRow {
                mapping: m.clone(),
                target: self.target_text(&m.target),
                missing: self
                    .doc()
                    .midi_target_problem(&self.registry, &m.target)
                    .is_some(),
            })
            .collect()
    }

    /// Replaces a mapping with an edited one (the panel's cells); one undo step.
    pub fn edit_midi_map(&mut self, mapping: MidiMapping) -> bool {
        self.apply_command(Command::MidiMap(MidiMapCommand::Set(mapping)))
            .is_ok()
    }

    /// Deletes a mapping; one undo step.
    pub fn remove_midi_map(&mut self, id: u32) -> bool {
        self.apply_command(Command::MidiMap(MidiMapCommand::Remove { id }))
            .is_ok()
    }

    /// Arms the target of mapping `id`: the next hardware control moved becomes its source.
    pub fn relearn_midi_map(&mut self, id: u32) -> bool {
        let Some(target) = self.doc().midi_map(id).map(|m| m.target.clone()) else {
            return false;
        };
        self.midi_arm(target)
    }

    // ------------------------------------------------------------------ presets

    /// Writes the project's mappings (`ids`, all when empty) as a preset file named after the
    /// file; says how many were saved and what could not be addressed by role.
    pub fn export_midi_preset(&mut self, path: &Path, ids: &[u32]) -> bool {
        let name = path
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (preset, notes) = self.doc().midi_preset(&name, ids);
        if preset.maps.is_empty() {
            self.report("There are no MIDI mappings to export");
            return false;
        }
        if let Err(e) = std::fs::write(path, save_midi_preset(&preset)) {
            self.report(format!("{}: {e}", path.display()));
            return false;
        }
        let left_out = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({} left out: {})", notes.len(), notes.join("; "))
        };
        self.report(format!(
            "Exported {} MIDI mapping{}{left_out}",
            preset.maps.len(),
            if preset.maps.len() == 1 { "" } else { "s" }
        ));
        true
    }

    /// Reads a preset file and adds its mappings, resolved against this project's devices, in
    /// one undo step.
    pub fn import_midi_preset(&mut self, path: &Path) -> bool {
        let preset = match read_preset(path) {
            Ok(p) => p,
            Err(e) => {
                self.report(format!("{}: {e}", path.display()));
                return false;
            }
        };
        self.apply_midi_preset(&preset)
    }

    /// Adds a preset's mappings, resolved against this project's devices, in one undo step.
    pub fn apply_midi_preset(&mut self, preset: &MidiPreset) -> bool {
        let (maps, notes) = self.doc().resolve_preset(&self.registry, preset);
        if maps.is_empty() {
            self.report(match notes.first() {
                Some(n) => format!("Nothing to map: {n}"),
                None => "The preset has no mappings".to_owned(),
            });
            return false;
        }
        let count = maps.len();
        if self
            .apply_command(Command::MidiMap(MidiMapCommand::AddMany(maps)))
            .is_err()
        {
            return false;
        }
        let skipped = if notes.is_empty() {
            String::new()
        } else {
            format!(", {} skipped: {}", notes.len(), notes.join("; "))
        };
        self.report(format!(
            "Added {count} MIDI mapping{} from {}{skipped}",
            if count == 1 { "" } else { "s" },
            if preset.name.is_empty() {
                "the preset"
            } else {
                &preset.name
            }
        ));
        true
    }

    /// Removes every mapping (one undo step).
    pub fn clear_midi_maps(&mut self) -> bool {
        self.apply_command(Command::MidiMap(MidiMapCommand::Clear))
            .is_ok()
    }
}

fn read_preset(path: &Path) -> Result<MidiPreset, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > 4 * 1024 * 1024 {
        return Err(format!("the file is {size} bytes, too large for a preset"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    load_midi_preset(&text).map_err(|e| e.to_string())
}

/// A message as text: `CC 7 ch 1 = 64`, `Note C3 ch 10 on 100`, `Bend ch 3 = 8192`.
pub fn describe_message(m: &ChannelMessage) -> String {
    use atomik_core::MessageKind;
    let ch = u16::from(m.channel) + 1;
    match m.kind {
        MessageKind::Cc { number, value } => format!("CC {number} ch {ch} = {value}"),
        MessageKind::NoteOn { key, velocity } => {
            format!(
                "Note {} ch {ch} on {velocity}",
                atomik_model::note_name(key)
            )
        }
        MessageKind::NoteOff { key } => {
            format!("Note {} ch {ch} off", atomik_model::note_name(key))
        }
        MessageKind::PitchBend { value } => format!("Bend ch {ch} = {value}"),
    }
}

#[cfg(test)]
#[path = "app_core_midimap_tests.rs"]
mod tests;
