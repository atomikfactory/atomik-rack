//! The profile driver: a surface driver whose layout is a [`ControllerProfile`], so any
//! controller set up with the wizard works the same way.
//!
//! * **Knobs** 1..=8 become [`SurfaceAction::Knob`]: the app applies them to eight parameters of
//!   the device the surface follows (the selected device, or the locked one). An absolute knob
//!   sets the position, an encoder turns by its detents. The eight are a **page** of the
//!   device's parameters: Page Down and Page Up (and the `controller.knob_page_*` commands) move
//!   to the previous and next eight, wrapping round. The page starts at 1 again when the
//!   followed device changes, unless the profile keeps a page per device.
//! * **Faders** 1..=8 are the levels of the eight strips of the current **bank**; Bank Left and
//!   Bank Right move the bank by eight strips (the driver keeps it, like the MCU driver). The
//!   master fader is the master.
//! * **Soft takeover**: when an absolute knob or fader is not where its parameter is (another
//!   device, bank or page, an edit with the mouse), moving it does nothing until it comes within
//!   a small distance of the value or sweeps through it, and then it takes over. The same rule
//!   as MIDI learn ([`MapState::take_over`]). A relative encoder and a motorised fader (one that
//!   receives the value as feedback) never wait. While a control waits, the driver says which
//!   way to turn it. The profile can switch it off.
//! * **Keys** are Play, Stop, Record and Loop, pressed (the release is ignored).
//! * **Feedback**: a control whose profile entry says `feedback` gets its value sent back on the
//!   output port: knobs show the parameter, faders the strip level, keys light with the
//!   transport. Only what changed is sent. A control the user is moving is left alone for a
//!   moment so a motorised fader never fights the hand.
//!
//! Messages are parsed here (the host hands over the raw bytes of a driver that does not feed
//! the mappings), 14 bit pairs and NRPN included.

use crate::controller_profile::{ControlMode, ControlSpec, ControllerProfile, Role};
use atomik_core::{ChannelMessage, MessageKind};
use atomik_midi_io::surface::{
    KnobMove, SurfaceAction, SurfaceDriver, SurfaceState, TransportButton,
};
use atomik_midi_io::{RunningStatus, parse_message};
use atomik_model::{
    DEFAULT_SENSITIVITY, MapState, ParamChange, ParamParser, SourceKind, is_param_controller,
};
use std::collections::BTreeMap;

/// The strips a bank holds (one per fader).
pub const BANK: usize = crate::controller_profile::FADERS as usize;
/// The knobs of a page.
const KNOBS: usize = crate::controller_profile::KNOBS as usize;
/// How many feedback passes a moved control is left alone for (about half a second).
const QUIET_PASSES: u32 = 15;
/// How close a control must come to the value of its parameter to take over, as a fraction of the
/// range.
const PICKUP_TOLERANCE: f32 = 0.02;

/// What one message of a control said.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reading {
    /// A position, 0..=1.
    Level(f32),
    /// A turn of this many detents.
    Detents(i32),
    /// A key went down.
    Press,
}

/// A surface driver for a controller profile.
#[derive(Debug)]
pub struct ProfileDriver {
    profile: ControllerProfile,
    running: RunningStatus,
    params: [ParamParser; 16],
    /// The two halves of every 14 bit controller seen: `(channel, msb controller)`.
    pairs: BTreeMap<(u8, u8), (u8, u8)>,
    bank: usize,
    /// The last raw value sent for each control.
    sent: Vec<Option<i32>>,
    /// The feedback passes each control is still left alone for.
    quiet: Vec<u32>,
    /// The soft takeover of each control.
    takeover: Vec<MapState>,
    /// The way each waiting control was last told to turn (`true` is up), so it is told once.
    told: Vec<Option<bool>>,
    /// The page of knobs now (0-based).
    page: usize,
    /// The device the knobs follow (the id the state gave last).
    followed: Option<String>,
    /// The page each device was left on, when the profile keeps a page per device.
    kept: BTreeMap<String, usize>,
}

impl ProfileDriver {
    /// A driver for `profile`.
    pub fn new(profile: ControllerProfile) -> Self {
        let n = profile.controls.len();
        Self {
            profile,
            running: RunningStatus::default(),
            params: [ParamParser::new(); 16],
            pairs: BTreeMap::new(),
            bank: 0,
            sent: vec![None; n],
            quiet: vec![0; n],
            takeover: vec![MapState::default(); n],
            told: vec![None; n],
            page: 0,
            followed: None,
            kept: BTreeMap::new(),
        }
    }

    /// The profile the driver runs.
    pub fn profile(&self) -> &ControllerProfile {
        &self.profile
    }

    /// The first strip of the bank (0-based).
    pub fn bank(&self) -> usize {
        self.bank
    }

    /// The page of knobs now (0-based).
    pub fn page(&self) -> usize {
        self.page
    }

    /// How many pages of knobs `state`'s device has (at least one).
    fn pages(state: &SurfaceState) -> usize {
        state.knobs.len().div_ceil(KNOBS).max(1)
    }

    /// Notices a change of the followed device: the page goes back to the first (or to the one
    /// the device was left on) and every knob starts again.
    fn sync(&mut self, state: &SurfaceState) {
        if self.followed != state.remote.id {
            if self.profile.options.keep_page
                && let Some(old) = self.followed.take()
            {
                self.kept.insert(old, self.page);
            }
            self.page = match (&state.remote.id, self.profile.options.keep_page) {
                (Some(id), true) => self.kept.get(id).copied().unwrap_or(0),
                _ => 0,
            };
            self.followed = state.remote.id.clone();
            self.knobs_moved();
        }
        let last = Self::pages(state) - 1;
        if self.page > last {
            self.page = last;
            self.knobs_moved();
        }
    }

    /// The knobs address other parameters now: they wait again and show the new values.
    fn knobs_moved(&mut self) {
        for (i, c) in self.profile.controls.iter().enumerate() {
            if matches!(c.role, Role::Knob(_)) {
                self.takeover[i].rearm();
                self.told[i] = None;
                self.sent[i] = None;
                self.quiet[i] = 0;
            }
        }
    }

    /// Moves the knobs `delta` pages on (wrapping) and says where they are.
    fn turn_page(&mut self, delta: i32, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
        self.sync(state);
        let name = &state.remote.name;
        if state.remote.id.is_none() || state.knobs.is_empty() {
            actions.push(SurfaceAction::Notice(
                "No device with knobs is selected".to_owned(),
            ));
            return;
        }
        let pages = Self::pages(state);
        let to = (self.page as i64 + i64::from(delta)).rem_euclid(pages as i64) as usize;
        if to != self.page {
            self.page = to;
            self.knobs_moved();
        }
        let (first, last) = (to * KNOBS + 1, ((to + 1) * KNOBS).min(state.knobs.len()));
        actions.push(SurfaceAction::Notice(format!(
            "Knobs: {name} {first}-{last}"
        )));
    }

    fn strip_of(&self, fader: u8, count: usize) -> Option<usize> {
        let i = self.bank + usize::from(fader.saturating_sub(1));
        (i < count).then_some(i)
    }

    /// What `msg` says to `control`, when it is for it.
    fn read(
        &mut self,
        control: &ControlSpec,
        msg: &ChannelMessage,
        event: Option<atomik_model::ParamEvent>,
    ) -> Option<Reading> {
        if msg.channel != control.channel {
            return None;
        }
        match (control.source, msg.kind) {
            (SourceKind::Cc(n), MessageKind::Cc { number, value }) if number == n => {
                Some(seven_bit(control.mode, value & 0x7F))
            }
            (SourceKind::Cc14(n), MessageKind::Cc { number, value })
                if number == n || number == n + 32 =>
            {
                let half = self.pairs.entry((msg.channel, n)).or_default();
                if number == n {
                    half.0 = value & 0x7F;
                } else {
                    half.1 = value & 0x7F;
                }
                let raw = (u16::from(half.0) << 7) | u16::from(half.1);
                Some(Reading::Level(f32::from(raw) / 16_383.0))
            }
            (SourceKind::Note(n), MessageKind::NoteOn { key, .. }) if key == n => {
                Some(Reading::Press)
            }
            (SourceKind::PitchBend, MessageKind::PitchBend { value }) => {
                Some(Reading::Level(f32::from(value & 0x3FFF) / 16_383.0))
            }
            (SourceKind::Nrpn(n), MessageKind::Cc { .. }) => {
                let e = event.filter(|e| e.number == n)?;
                match (e.change, control.mode) {
                    (ParamChange::Value(v), ControlMode::Absolute) => {
                        Some(Reading::Level(f32::from(v) / 16_383.0))
                    }
                    (ParamChange::Value(v), ControlMode::Button) => {
                        (v >= 8192).then_some(Reading::Press)
                    }
                    (ParamChange::Step(d), ControlMode::Relative(_)) => {
                        Some(Reading::Detents(i32::from(d)))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn quiet_for(&mut self, index: usize) {
        if let Some(q) = self.quiet.get_mut(index) {
            *q = QUIET_PASSES;
        }
    }

    fn act(
        &mut self,
        index: usize,
        control: &ControlSpec,
        reading: Reading,
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
    ) {
        let detent = |d: i32| d as f32 * DEFAULT_SENSITIVITY;
        match (control.role, reading) {
            (Role::Knob(i), Reading::Level(v)) => {
                let knob = self.page * KNOBS + usize::from(i) - 1;
                let current = state
                    .knobs
                    .get(knob)
                    .copied()
                    .flatten()
                    .filter(|_| !state.remote.stepped.get(knob).copied().unwrap_or(false));
                let target = || {
                    state
                        .remote
                        .params
                        .get(knob)
                        .cloned()
                        .unwrap_or_else(|| "its parameter".to_owned())
                };
                if self.takes(index, control, v, current, target, actions) {
                    actions.push(SurfaceAction::Knob {
                        knob,
                        change: KnobMove::To(v),
                    });
                }
            }
            (Role::Knob(i), Reading::Detents(d)) => {
                actions.push(SurfaceAction::Knob {
                    knob: self.page * KNOBS + usize::from(i) - 1,
                    change: KnobMove::By(detent(d)),
                });
            }
            (Role::Fader(i), reading) => {
                let Some(strip) = self.strip_of(i, state.strips.len()) else {
                    return;
                };
                let now = state.strips[strip].fader;
                let allowed = match reading {
                    Reading::Level(v) => {
                        let name = || state.strips[strip].name.clone();
                        self.takes(index, control, v, Some(now), name, actions)
                    }
                    _ => true,
                };
                if allowed && let Some(value) = fader_value(now, reading) {
                    actions.push(SurfaceAction::Fader { strip, value });
                }
            }
            (Role::MasterFader, reading) => {
                let now = state.master.as_ref().map_or(0.0, |m| m.fader);
                let allowed = match reading {
                    Reading::Level(v) => {
                        let name = || "the master".to_owned();
                        self.takes(index, control, v, Some(now), name, actions)
                    }
                    _ => true,
                };
                if allowed && let Some(value) = fader_value(now, reading) {
                    actions.push(SurfaceAction::MasterFader(value));
                }
            }
            (Role::Play, Reading::Press) => {
                actions.push(SurfaceAction::Transport(TransportButton::Play));
            }
            (Role::Stop, Reading::Press) => {
                actions.push(SurfaceAction::Transport(TransportButton::Stop));
            }
            (Role::Record, Reading::Press) => {
                actions.push(SurfaceAction::Transport(TransportButton::Record));
            }
            (Role::Loop, Reading::Press) => {
                actions.push(SurfaceAction::Transport(TransportButton::Cycle));
            }
            (Role::BankLeft, Reading::Press) => {
                self.move_bank(self.bank.saturating_sub(BANK), state)
            }
            (Role::BankRight, Reading::Press) => self.move_bank(self.bank + BANK, state),
            (Role::PageDown, Reading::Press) => self.turn_page(-1, state, actions),
            (Role::PageUp, Reading::Press) => self.turn_page(1, state, actions),
            _ => return,
        }
        if !matches!(reading, Reading::Press) {
            self.quiet_for(index);
        }
    }

    /// True when `control`, now at `level`, may set its target, which stands at `current`
    /// (`None` when it cannot be read: nothing to wait for). A control that has to catch the
    /// value tells the user once which way to turn it (`what` names the target).
    fn takes(
        &mut self,
        index: usize,
        control: &ControlSpec,
        level: f32,
        current: Option<f32>,
        what: impl FnOnce() -> String,
        actions: &mut Vec<SurfaceAction>,
    ) -> bool {
        // A motorised fader is brought to the value by the feedback; it never waits.
        let motorised = control.feedback && !matches!(control.role, Role::Knob(_));
        let Some(cur) = current.filter(|_| self.profile.options.pickup && !motorised) else {
            return true;
        };
        if self.takeover[index].take_over(level, cur, PICKUP_TOLERANCE) {
            self.told[index] = None;
            return true;
        }
        if let Some(up) = self.takeover[index].turn_to_catch(cur)
            && self.told[index] != Some(up)
        {
            self.told[index] = Some(up);
            actions.push(SurfaceAction::Notice(format!(
                "Turn {} {} to pick up {}",
                control.role.label().to_lowercase(),
                if up { "up" } else { "down" },
                what()
            )));
        }
        false
    }

    fn move_bank(&mut self, to: usize, state: &SurfaceState) {
        let last_page = state.strips.len().saturating_sub(1) / BANK * BANK;
        let to = to.min(last_page);
        if to != self.bank {
            self.bank = to;
            // The faders show other strips now: send them all again.
            for (i, c) in self.profile.controls.iter().enumerate() {
                if matches!(c.role, Role::Fader(_)) {
                    self.sent[i] = None;
                    self.takeover[i].rearm();
                    self.told[i] = None;
                }
            }
        }
    }

    /// The value to show on `control` for `state`, 0..=1.
    fn shown(&self, control: &ControlSpec, state: &SurfaceState) -> Option<f32> {
        let on = |b: bool| Some(if b { 1.0 } else { 0.0 });
        match control.role {
            Role::Knob(i) => Some(
                state
                    .knobs
                    .get(self.page * KNOBS + usize::from(i) - 1)
                    .copied()
                    .flatten()
                    .unwrap_or(0.0),
            ),
            Role::Fader(i) => Some(
                self.strip_of(i, state.strips.len())
                    .map_or(0.0, |s| state.strips[s].fader),
            ),
            Role::MasterFader => Some(state.master.as_ref().map_or(0.0, |m| m.fader)),
            Role::Play => on(state.transport.playing),
            Role::Stop => on(!state.transport.playing),
            Role::Record => on(state.transport.recording),
            Role::Loop => on(state.transport.looping),
            Role::BankLeft | Role::BankRight | Role::PageDown | Role::PageUp => None,
        }
    }
}

/// A 7 bit controller value as a reading in `mode`.
fn seven_bit(mode: ControlMode, value: u8) -> Reading {
    match mode {
        ControlMode::Absolute => Reading::Level(f32::from(value) / 127.0),
        ControlMode::Relative(kind) => Reading::Detents(kind.detents(value)),
        ControlMode::Button => {
            if value >= 64 {
                Reading::Press
            } else {
                Reading::Detents(0)
            }
        }
    }
}

/// The new fader position for a reading (a turn moves the fader from where it is).
fn fader_value(now: f32, reading: Reading) -> Option<f32> {
    match reading {
        Reading::Level(v) => Some(v),
        Reading::Detents(0) | Reading::Press => None,
        Reading::Detents(d) => Some((now + d as f32 * DEFAULT_SENSITIVITY).clamp(0.0, 1.0)),
    }
}

/// The messages that show `level` (0..=1) on `control`, and the raw number sent (to tell
/// whether it changed).
fn encode(control: &ControlSpec, level: f32) -> (i32, Vec<Vec<u8>>) {
    let level = level.clamp(0.0, 1.0);
    let ch = control.channel & 0x0F;
    let seven = (level * 127.0).round() as i32;
    let fourteen = (level * 16_383.0).round() as i32;
    let cc = |n: u8, v: i32| vec![0xB0 | ch, n, (v & 0x7F) as u8];
    match control.source {
        SourceKind::Cc(n) => (seven, vec![cc(n, seven)]),
        SourceKind::Cc14(n) => (
            fourteen,
            vec![cc(n, fourteen >> 7), cc(n + 32, fourteen & 0x7F)],
        ),
        SourceKind::PitchBend => (
            fourteen,
            vec![vec![
                0xE0 | ch,
                (fourteen & 0x7F) as u8,
                ((fourteen >> 7) & 0x7F) as u8,
            ]],
        ),
        SourceKind::Nrpn(n) => (
            fourteen,
            vec![
                cc(99, i32::from(n >> 7)),
                cc(98, i32::from(n & 0x7F)),
                cc(6, fourteen >> 7),
                cc(38, fourteen & 0x7F),
            ],
        ),
        SourceKind::Note(n) => (
            seven,
            vec![if seven > 0 {
                vec![0x90 | ch, n, seven as u8]
            } else {
                vec![0x80 | ch, n, 0]
            }],
        ),
        // A profile never holds an RPN control; nothing is sent for one.
        SourceKind::Rpn(_) => (0, Vec::new()),
    }
}

impl SurfaceDriver for ProfileDriver {
    fn name(&self) -> &'static str {
        "profile"
    }

    fn connect(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        self.sent.iter_mut().for_each(|s| *s = None);
        self.quiet.iter_mut().for_each(|q| *q = 0);
        self.takeover.iter_mut().for_each(MapState::rearm);
        self.told.iter_mut().for_each(|t| *t = None);
        self.followed = None;
        self.sync(state);
        self.feedback(state, out);
    }

    fn on_message(
        &mut self,
        bytes: &[u8],
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        _replies: &mut Vec<Vec<u8>>,
    ) {
        let Some(msg) = parse_message(bytes, &mut self.running) else {
            return;
        };
        self.sync(state);
        let event = match msg.kind {
            MessageKind::Cc { number, .. } if is_param_controller(number) => {
                self.params[usize::from(msg.channel & 0x0F)].feed(&msg)
            }
            _ => None,
        };
        let controls = self.profile.controls.clone();
        for (index, control) in controls.iter().enumerate() {
            if let Some(reading) = self.read(control, &msg, event) {
                self.act(index, control, reading, state, actions);
            }
        }
    }

    fn feedback(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        self.sync(state);
        if self.profile.output.is_none() {
            return;
        }
        for index in 0..self.profile.controls.len() {
            let control = self.profile.controls[index];
            if !control.feedback {
                continue;
            }
            if self.quiet[index] > 0 {
                self.quiet[index] -= 1;
                continue;
            }
            let Some(level) = self.shown(&control, state) else {
                continue;
            };
            let (raw, messages) = encode(&control, level);
            if self.sent[index] == Some(raw) {
                continue;
            }
            self.sent[index] = Some(raw);
            out.extend(messages);
        }
    }

    fn knob_page(&mut self, delta: i32, state: &SurfaceState, actions: &mut Vec<SurfaceAction>) {
        self.turn_page(delta, state, actions);
    }

    fn disconnect(&mut self, _out: &mut Vec<Vec<u8>>) {}
}

#[cfg(test)]
#[path = "controller_driver_tests.rs"]
mod tests;
