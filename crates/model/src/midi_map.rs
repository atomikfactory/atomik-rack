//! MIDI mappings: a hardware control (a controller, a pitch bend, a note) bound to a parameter,
//! a VCA group's level, the tempo or a transport key.
//!
//! The mappings are **project data** (`RackDoc::midi_maps`, project format 28, undoable), but
//! they never take part in the sound: they only decide which commands the live MIDI input
//! issues, so the routing, the plan and the bounce of a project are exactly what they were
//! (`crates/render/tests/midi_maps.rs` proves a project with mappings bounces bit for bit like
//! one without).
//!
//! # Sources
//!
//! A [`MidiSource`] is a kind of message ([`SourceKind`]: a 7 bit controller, a 14 bit
//! controller pair (the MSB controller 0..=31 with its LSB 32 higher), pitch bend or a note), on
//! one channel or any, from one input port or any. A learned source takes the channel and any
//! port.
//!
//! A source may also name the **control surface** it is on (`MidiSource::surface`): a generic
//! controller surface of the settings. A source with a surface hears that surface only; one
//! without hears every surface and every port, so older projects map as they did.
//!
//! The **camera** is an input too: a source whose port is [`crate::CAMERA_PORT`] hears the
//! webcam hand tracker only (see [`crate::camera_source`]), and hardware never drives it.
//!
//! # Options
//!
//! [`MapOptions`] holds the range (`min` and `max` as fractions of the target's own range, so a
//! cutoff knob can be confined to a stretch), `invert`, the [`MapMode`] (absolute, three
//! flavours of relative encoder, toggle and momentary buttons), the soft takeover (`pickup`) and
//! whether the mapped message is `consume`d (kept away from the notes of the armed instrument).
//! [`MidiMapping::feed`] is the whole of the maths, pure and testable: a message and the target's
//! current value in, the new value (or a trigger) out.
//!
//! # Addressing
//!
//! A project mapping addresses a **concrete** target ([`MidiTarget`]): a device key and a param
//! key, a VCA group id, the tempo, a transport key, or the **selected strip** (a role: the
//! console strip that is selected, resolved by the app at the time of each message). A
//! **preset** (`MidiPreset`, exported to a JSON file) addresses by role instead
//! ([`PresetTarget`]): a device **type** with its position among the devices of that type in the
//! rack (`nth`, from 0) and a param key, so a preset made for the first poly synth's cutoff
//! applies to the first poly synth of any project, and a preset for `atomik.channel` number 3's
//! fader to the third channel of any project. The input port and the surface are not part of a
//! preset.
//!
//! Two more targets follow the user instead of naming a device: [`MidiTarget::Focused`] is a
//! param of the **focused device** (the device a surface is locked to, else the device selected
//! in the rack) and [`MidiTarget::Command`] runs a [`RemoteCommand`] (select the previous or next
//! track, undo, redo, step through the presets of the selected device) when a button is pressed.

use crate::console::{CHANNEL_TYPE, is_hidden_param};
use crate::history::Edit;
use crate::midi_param::{
    MAX_PARAM_NUMBER, PARAM_BUTTON_THRESHOLD, ParamBank, ParamChange, ParamEvent, ParamSpace,
    is_param_selector,
};
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_core::{ChannelMessage, MessageKind};
use atomik_device_sdk::Registry;

/// Most mappings in a project.
pub const MAX_MIDI_MAPS: usize = 512;
/// Longest input port name kept in a source, in characters.
pub const MAX_PORT_NAME_CHARS: usize = 128;
/// How close the hardware value must come to the target for the soft takeover to pick it up
/// (a little over one 7 bit step).
pub const PICKUP_WINDOW: f32 = 0.012;
/// The default size of a relative encoder detent, as a fraction of the target's range.
pub const DEFAULT_SENSITIVITY: f32 = 1.0 / 64.0;
/// How long the learner waits for the second half of a 14 bit pair, in seconds.
pub const PAIR_WINDOW_SECONDS: f64 = 0.1;

/// What kind of message a source listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    /// A 7 bit controller, 0..=127.
    Cc(u8),
    /// A 14 bit controller pair: the MSB controller, 0..=31 (the LSB is that number plus 32).
    Cc14(u8),
    /// Pitch bend (14 bit).
    PitchBend,
    /// A note: its on message presses, its off message releases.
    Note(u8),
    /// A non-registered parameter number, 0..=16383 (see [`crate::midi_param`]): its data entry
    /// is the position, data increment and decrement step it.
    Nrpn(u16),
    /// A registered parameter number, 0..=16383.
    Rpn(u16),
}

/// A hardware control: what message, on which channel, from which port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSource {
    /// The input port by name, or any port.
    pub port: Option<String>,
    /// The MIDI channel, 0..=15, or any channel.
    pub channel: Option<u8>,
    /// The kind of message.
    pub kind: SourceKind,
    /// The control surface the control is on (a generic controller surface, by name), or any
    /// origin: a source with no surface hears every surface and every port; one with a surface
    /// hears that surface only.
    pub surface: Option<String>,
}

impl MidiSource {
    /// A source on any port.
    pub fn any_port(channel: Option<u8>, kind: SourceKind) -> Self {
        Self {
            port: None,
            channel,
            kind,
            surface: None,
        }
    }

    /// True when `msg` from `port` is for this source (a message of a plain MIDI input: a
    /// source tied to a surface does not hear it).
    pub fn matches(&self, port: &str, msg: &ChannelMessage) -> bool {
        self.matches_from(port, None, msg)
    }

    /// True when `msg` from `port`, sent by the control surface `surface` (`None` for a plain
    /// MIDI input), is for this source.
    pub fn matches_from(&self, port: &str, surface: Option<&str>, msg: &ChannelMessage) -> bool {
        if self.surface.as_deref().is_some_and(|s| surface != Some(s)) {
            return false;
        }
        if self.port.as_deref().is_some_and(|p| p != port) {
            return false;
        }
        if self.channel.is_some_and(|c| c != msg.channel) {
            return false;
        }
        match (self.kind, msg.kind) {
            (SourceKind::Cc(n), MessageKind::Cc { number, .. }) => n == number,
            (SourceKind::Cc14(n), MessageKind::Cc { number, .. }) => {
                n == number || n + 32 == number
            }
            (SourceKind::PitchBend, MessageKind::PitchBend { .. }) => true,
            (
                SourceKind::Note(k),
                MessageKind::NoteOn { key, .. } | MessageKind::NoteOff { key },
            ) => k == key,
            _ => false,
        }
    }

    /// True when `event` (a parameter run completed on `channel` of `port`, sent by `surface`
    /// when it came from a control surface) is for this source. The message-level
    /// [`Self::matches_from`] never hears an NRPN or RPN source: its controllers arrive as a run
    /// that the app parses first.
    pub fn matches_param(
        &self,
        port: &str,
        surface: Option<&str>,
        channel: u8,
        event: &ParamEvent,
    ) -> bool {
        if self.surface.as_deref().is_some_and(|s| surface != Some(s)) {
            return false;
        }
        if self.port.as_deref().is_some_and(|p| p != port) {
            return false;
        }
        if self.channel.is_some_and(|c| c != channel) {
            return false;
        }
        match self.kind {
            SourceKind::Nrpn(n) => event.space == ParamSpace::Nrpn && event.number == n,
            SourceKind::Rpn(n) => event.space == ParamSpace::Rpn && event.number == n,
            _ => false,
        }
    }

    /// The kind of message alone, without channel or port: `CC 7`, `NRPN 1234`, `Note C3`.
    pub fn what(&self) -> String {
        match self.kind {
            SourceKind::Cc(n) => format!("CC {n}"),
            SourceKind::Cc14(n) => format!("CC {n}/{} (14 bit)", n + 32),
            SourceKind::PitchBend => "Bend".to_owned(),
            SourceKind::Note(k) => format!("Note {}", note_name(k)),
            SourceKind::Nrpn(n) => format!("NRPN {n}"),
            SourceKind::Rpn(n) => format!("RPN {n}"),
        }
    }

    /// A short text for lists: `CC 7 ch 1`, `CC 1/33 (14 bit) any ch`, `Bend ch 3`, `Note C3 ch 10`.
    pub fn describe(&self) -> String {
        let what = self.what();
        let channel = self
            .channel
            .map_or_else(|| "any ch".to_owned(), |c| format!("ch {}", c + 1));
        if let Some((hand, control)) = self.camera_control() {
            return format!(
                "Camera, {} {} ({what} {channel})",
                hand.label(),
                control.label()
            );
        }
        match (&self.surface, &self.port) {
            (Some(s), _) => format!("{what} {channel} [{s}]"),
            (None, Some(p)) if p == crate::CAMERA_PORT => format!("{what} {channel} (Camera)"),
            (None, Some(p)) => format!("{what} {channel} ({p})"),
            (None, None) => format!("{what} {channel}"),
        }
    }

    /// True when the source is a 7 bit controller.
    pub fn is_cc(&self) -> bool {
        matches!(self.kind, SourceKind::Cc(_))
    }

    /// True when the source is a registered or non-registered parameter number.
    pub fn is_param(&self) -> bool {
        matches!(self.kind, SourceKind::Nrpn(_) | SourceKind::Rpn(_))
    }

    /// True when a relative encoder can use the source: a 7 bit controller, or a parameter
    /// number (whose data increment and decrement are steps).
    pub fn can_be_relative(&self) -> bool {
        self.is_cc() || self.is_param()
    }
}

/// The name of a MIDI key (`C3` is 60).
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key % 12)],
        i32::from(key / 12) - 2
    )
}

/// A transport key a mapping can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransportTarget {
    /// Play from the current position.
    Play,
    /// Stop.
    Stop,
    /// Play, or stop when playing.
    TogglePlay,
    /// Toggle recording.
    Record,
    /// Toggle the song loop.
    Loop,
    /// Go to the start of the song.
    ReturnToZero,
}

impl TransportTarget {
    /// Every transport target, in menu order.
    pub const ALL: [TransportTarget; 6] = [
        TransportTarget::Play,
        TransportTarget::Stop,
        TransportTarget::TogglePlay,
        TransportTarget::Record,
        TransportTarget::Loop,
        TransportTarget::ReturnToZero,
    ];

    /// The name saved in files.
    pub fn id(self) -> &'static str {
        match self {
            TransportTarget::Play => "play",
            TransportTarget::Stop => "stop",
            TransportTarget::TogglePlay => "toggle_play",
            TransportTarget::Record => "record",
            TransportTarget::Loop => "loop",
            TransportTarget::ReturnToZero => "return_to_zero",
        }
    }

    /// The target saved under `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The label for lists.
    pub fn label(self) -> &'static str {
        match self {
            TransportTarget::Play => "Play",
            TransportTarget::Stop => "Stop",
            TransportTarget::TogglePlay => "Play/Stop",
            TransportTarget::Record => "Record",
            TransportTarget::Loop => "Loop",
            TransportTarget::ReturnToZero => "Return to zero",
        }
    }
}

/// A command a mapping can run: what a hardware button does besides moving a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemoteCommand {
    /// Select another track: `delta` tracks down the arrangement (negative is up), clamped at the
    /// ends.
    SelectTrack {
        /// How many tracks to move, never 0.
        delta: i8,
    },
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
    /// Load another preset of the selected device: `delta` steps through its preset list
    /// (negative is back), wrapping at the ends.
    SelectPreset {
        /// How many presets to move, never 0.
        delta: i8,
    },
}

impl RemoteCommand {
    /// The most tracks or presets one press moves.
    pub const MAX_DELTA: i8 = 16;

    /// The commands the panel offers, in menu order.
    pub const ALL: [RemoteCommand; 6] = [
        RemoteCommand::SelectTrack { delta: -1 },
        RemoteCommand::SelectTrack { delta: 1 },
        RemoteCommand::Undo,
        RemoteCommand::Redo,
        RemoteCommand::SelectPreset { delta: -1 },
        RemoteCommand::SelectPreset { delta: 1 },
    ];

    /// The name saved in files (the step comes separately, see [`Self::delta`]).
    pub fn id(self) -> &'static str {
        match self {
            RemoteCommand::SelectTrack { .. } => "select_track",
            RemoteCommand::Undo => "undo",
            RemoteCommand::Redo => "redo",
            RemoteCommand::SelectPreset { .. } => "select_preset",
        }
    }

    /// The step of a stepping command.
    pub fn delta(self) -> Option<i8> {
        match self {
            RemoteCommand::SelectTrack { delta } | RemoteCommand::SelectPreset { delta } => {
                Some(delta)
            }
            _ => None,
        }
    }

    /// The command saved under `id` with `delta` (a stepping command needs a non-zero delta of
    /// at most [`Self::MAX_DELTA`]; the others take none).
    pub fn from_id(id: &str, delta: Option<i64>) -> Result<Self, String> {
        let step = || -> Result<i8, String> {
            let d = delta.ok_or_else(|| format!("`{id}` needs a delta"))?;
            if d == 0 || d.unsigned_abs() > RemoteCommand::MAX_DELTA as u64 {
                return Err(format!(
                    "the delta of `{id}` is 1 to {} either way, not {d}",
                    RemoteCommand::MAX_DELTA
                ));
            }
            Ok(d as i8)
        };
        let none = |c: RemoteCommand| -> Result<Self, String> {
            match delta {
                Some(_) => Err(format!("`{id}` has no delta")),
                None => Ok(c),
            }
        };
        match id {
            "select_track" => Ok(RemoteCommand::SelectTrack { delta: step()? }),
            "undo" => none(RemoteCommand::Undo),
            "redo" => none(RemoteCommand::Redo),
            "select_preset" => Ok(RemoteCommand::SelectPreset { delta: step()? }),
            other => Err(format!("unknown command {other:?}")),
        }
    }

    /// The label for lists: `Previous track`, `Undo`, `Next preset`, `Track +3`.
    pub fn label(self) -> String {
        let step = |delta: i8, one: &str, other: &str, noun: &str| match delta {
            -1 => one.to_owned(),
            1 => other.to_owned(),
            d => format!("{noun} {d:+}"),
        };
        match self {
            RemoteCommand::SelectTrack { delta } => {
                step(delta, "Previous track", "Next track", "Track")
            }
            RemoteCommand::Undo => "Undo".to_owned(),
            RemoteCommand::Redo => "Redo".to_owned(),
            RemoteCommand::SelectPreset { delta } => {
                step(delta, "Previous preset", "Next preset", "Preset")
            }
        }
    }
}

/// What a mapping drives.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MidiTarget {
    /// A param of a device (a path key inside a container is allowed).
    Param {
        /// The device.
        device: DeviceKey,
        /// The param key.
        param: String,
    },
    /// The level offset of a VCA group, by id.
    Vca {
        /// The group's id.
        id: u32,
    },
    /// The song tempo (20..=999 BPM over the range).
    Tempo,
    /// A transport key.
    Transport(TransportTarget),
    /// A param of the **selected console strip** (a channel or a bus), resolved by the app at
    /// each message: with it one fader controls whichever strip is selected.
    SelectedStrip {
        /// The strip param key (`fader`, `pan`, `mute`, `solo`, `send1`, ...).
        param: String,
    },
    /// A param of the **focused device**: the device the surface is locked to, else the device
    /// selected in the rack (resolved by the app at each message, so one control follows the
    /// selection). A device without the param is left alone.
    Focused {
        /// The param key.
        param: String,
    },
    /// A command (a button).
    Command(RemoteCommand),
}

impl MidiTarget {
    /// True for a target that is pressed rather than set (a transport key).
    pub fn is_trigger(&self) -> bool {
        matches!(self, MidiTarget::Transport(_) | MidiTarget::Command(_))
    }

    /// A short text for lists: `synth1 filter.cutoff`, `VCA 2`, `Tempo`, `Play`, `Selected strip fader`.
    pub fn describe(&self) -> String {
        match self {
            MidiTarget::Param { device, param } => format!("{device} {param}"),
            MidiTarget::Vca { id } => format!("VCA {id}"),
            MidiTarget::Tempo => "Tempo".to_owned(),
            MidiTarget::Transport(t) => t.label().to_owned(),
            MidiTarget::SelectedStrip { param } => format!("Selected strip {param}"),
            MidiTarget::Focused { param } => format!("Focused device {param}"),
            MidiTarget::Command(c) => c.label(),
        }
    }
}

/// How the three relative encoder encodings read a 7 bit value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeKind {
    /// 1..=63 clockwise, 127 down to 65 counter-clockwise (two's complement of 7 bits).
    TwosComplement,
    /// 65..=127 clockwise, 63 down to 1 counter-clockwise (64 is the centre).
    BinaryOffset,
    /// Bit 6 is the sign (set is counter-clockwise), bits 0..=5 the size.
    SignMagnitude,
}

impl RelativeKind {
    /// The detents of a 7 bit encoder value, positive clockwise (0 is nothing).
    pub fn detents(self, value: u8) -> i32 {
        let v = i32::from(value & 0x7F);
        match self {
            RelativeKind::TwosComplement => {
                if v >= 64 {
                    v - 128
                } else {
                    v
                }
            }
            RelativeKind::BinaryOffset => v - 64,
            RelativeKind::SignMagnitude => {
                let magnitude = v & 0x3F;
                if v & 0x40 == 0 { magnitude } else { -magnitude }
            }
        }
    }

    /// The name saved in files.
    pub fn id(self) -> &'static str {
        match self {
            RelativeKind::TwosComplement => "relative_twos",
            RelativeKind::BinaryOffset => "relative_offset",
            RelativeKind::SignMagnitude => "relative_sign",
        }
    }
}

/// How a control drives its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapMode {
    /// The control's position is the value.
    Absolute,
    /// An encoder: each message moves the value by some detents.
    Relative(RelativeKind),
    /// A button: each press flips the target between the range's ends.
    Toggle,
    /// A button: the target is at one end while it is held and at the other when released.
    Momentary,
}

impl MapMode {
    /// The name saved in files.
    pub fn id(self) -> &'static str {
        match self {
            MapMode::Absolute => "absolute",
            MapMode::Relative(k) => k.id(),
            MapMode::Toggle => "toggle",
            MapMode::Momentary => "momentary",
        }
    }

    /// The mode saved under `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "absolute" => MapMode::Absolute,
            "relative_twos" => MapMode::Relative(RelativeKind::TwosComplement),
            "relative_offset" => MapMode::Relative(RelativeKind::BinaryOffset),
            "relative_sign" => MapMode::Relative(RelativeKind::SignMagnitude),
            "toggle" => MapMode::Toggle,
            "momentary" => MapMode::Momentary,
            _ => return None,
        })
    }

    /// The label for lists.
    pub fn label(self) -> &'static str {
        match self {
            MapMode::Absolute => "Absolute",
            MapMode::Relative(RelativeKind::TwosComplement) => "Relative (two's complement)",
            MapMode::Relative(RelativeKind::BinaryOffset) => "Relative (binary offset)",
            MapMode::Relative(RelativeKind::SignMagnitude) => "Relative (sign magnitude)",
            MapMode::Toggle => "Toggle",
            MapMode::Momentary => "Momentary",
        }
    }
}

/// How a mapping turns a message into a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapOptions {
    /// The target value (as a fraction of its range) the control's bottom gives.
    pub min: f32,
    /// The target value the control's top gives. `min` may be above `max`.
    pub max: f32,
    /// Swap the two ends.
    pub invert: bool,
    /// How the control drives the target.
    pub mode: MapMode,
    /// A relative detent as a fraction of the target's range.
    pub sensitivity: f32,
    /// Soft takeover: an absolute control does nothing until it reaches the target's value, so
    /// the target never jumps.
    pub pickup: bool,
    /// Keep the mapped message away from the notes of the armed instrument.
    pub consume: bool,
}

impl Default for MapOptions {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            invert: false,
            mode: MapMode::Absolute,
            sensitivity: DEFAULT_SENSITIVITY,
            pickup: true,
            consume: true,
        }
    }
}

impl MapOptions {
    /// The options a learned mapping starts with: a button (a note) toggles, anything else is
    /// absolute with soft takeover.
    pub fn for_source(kind: SourceKind) -> Self {
        Self {
            mode: match kind {
                SourceKind::Note(_) => MapMode::Toggle,
                _ => MapMode::Absolute,
            },
            ..Self::default()
        }
    }

    /// The options a learned `source` starts with: those of [`Self::for_source`], except that the
    /// camera's twist is an endless encoder (two's complement), the way it sends.
    pub fn for_learned(source: &MidiSource) -> Self {
        match source.camera_control() {
            Some((_, crate::CameraControl::Twist)) => Self {
                mode: MapMode::Relative(RelativeKind::TwosComplement),
                ..Self::default()
            },
            _ => Self::for_source(source.kind),
        }
    }

    /// Why the options cannot be used with `source`, or `None`.
    pub fn problem(&self, source: &MidiSource) -> Option<String> {
        let unit = |x: f32| x.is_finite() && (0.0..=1.0).contains(&x);
        if !unit(self.min) || !unit(self.max) {
            return Some("the range must lie between 0 and 1".to_owned());
        }
        if !(self.sensitivity.is_finite() && self.sensitivity > 0.0 && self.sensitivity <= 1.0) {
            return Some("the sensitivity must be above 0 and at most 1".to_owned());
        }
        if matches!(self.mode, MapMode::Relative(_)) && !source.can_be_relative() {
            return Some(
                "only a 7 bit controller or an NRPN or RPN can be a relative encoder".to_owned(),
            );
        }
        None
    }
}

/// One mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiMapping {
    /// The mapping's id (from 1, unique in the project; ignored by the add commands).
    pub id: u32,
    /// The hardware control.
    pub source: MidiSource,
    /// What it drives.
    pub target: MidiTarget,
    /// How.
    pub options: MapOptions,
}

/// What a message does to a target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// Set the target to this value, a fraction of its range.
    Set(f32),
    /// A transport key was pressed.
    Trigger,
}

/// What a mapping remembers between messages (never saved).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MapState {
    /// The soft takeover has caught the target.
    picked_up: bool,
    /// The last value the control asked for, before the takeover.
    last_out: Option<f32>,
    /// The last value the mapping set.
    last_applied: Option<f32>,
    /// The two halves of a 14 bit controller.
    msb: u8,
    lsb: u8,
    /// The button is down.
    held: bool,
}

impl MapState {
    /// Forgets the takeover (the target was set by something else, or the mapping changed).
    pub fn rearm(&mut self) {
        self.picked_up = false;
        self.last_out = None;
    }

    /// True once the soft takeover has caught the target.
    pub fn is_picked_up(&self) -> bool {
        self.picked_up
    }

    /// The soft takeover for a control that now asks for `out` while the target stands at `cur`
    /// (both fractions of the range): true when the control may set the target. Until it comes
    /// within `window` of the target, or moves through it between two messages, nothing is set. A
    /// target that moved since the control last set it (a mouse drag, automation, a recall) has
    /// to be caught again. Every controller that reads absolute positions uses this one rule.
    pub fn take_over(&mut self, out: f32, cur: f32, window: f32) -> bool {
        if self.picked_up
            && self
                .last_applied
                .is_some_and(|applied| (applied - cur).abs() > window)
        {
            self.picked_up = false;
        }
        if !self.picked_up {
            let near = (out - cur).abs() <= window;
            let crossed = self
                .last_out
                .is_some_and(|prev| (prev - cur) * (out - cur) <= 0.0);
            self.last_out = Some(out);
            if !(near || crossed) {
                return false;
            }
            self.picked_up = true;
        }
        self.last_out = Some(out);
        self.last_applied = Some(out);
        true
    }

    /// Which way the control has to turn to catch a target at `cur`: `Some(true)` is up. `None`
    /// when the control has caught it, or has not been moved yet.
    pub fn turn_to_catch(&self, cur: f32) -> Option<bool> {
        if self.picked_up {
            return None;
        }
        self.last_out.map(|out| cur > out)
    }
}

impl MidiMapping {
    /// True when `msg` from `port` is for this mapping.
    pub fn matches(&self, port: &str, msg: &ChannelMessage) -> bool {
        self.source.matches(port, msg)
    }

    /// What `msg` does: `current` is the target's value now as a fraction of its range (`None`
    /// when it is not known, for a trigger or a target that cannot be read, which switches the
    /// soft takeover and the relative encoders off). `msg` must [`Self::matches`].
    pub fn feed(
        &self,
        state: &mut MapState,
        msg: &ChannelMessage,
        current: Option<f32>,
    ) -> Option<Effect> {
        let o = &self.options;
        if self.target.is_trigger() {
            return self.press(state, msg).then_some(Effect::Trigger);
        }
        match o.mode {
            MapMode::Absolute => {
                let level = self.level(state, msg)?;
                self.absolute(state, level, current)
            }
            MapMode::Relative(kind) => {
                let MessageKind::Cc { value, .. } = msg.kind else {
                    return None;
                };
                self.moved_by(state, kind.detents(value), current)
            }
            MapMode::Toggle => {
                let pressed = self.press(state, msg);
                self.flip(state, pressed, current)
            }
            MapMode::Momentary => {
                let down = self.button_down(msg)?;
                self.hold(state, down)
            }
        }
    }

    /// What a parameter run (an NRPN or RPN data entry, increment or decrement) does. `current`
    /// is as for [`Self::feed`]. The event must be one the source of the mapping
    /// [`MidiSource::matches_param`].
    ///
    /// A value is the position of the control (14 bit); a step moves the target by detents of the
    /// mapping's sensitivity in any mode that moves a value, so a pair of increment and decrement
    /// buttons works. A relative encoder reads the high half of a value like a 7 bit controller.
    /// A button is down from the middle of the range.
    pub fn feed_param(
        &self,
        state: &mut MapState,
        event: &ParamEvent,
        current: Option<f32>,
    ) -> Option<Effect> {
        let down = |v: u16| v >= PARAM_BUTTON_THRESHOLD;
        if self.target.is_trigger() {
            return match event.change {
                ParamChange::Value(v) => self.edge(state, down(v)).then_some(Effect::Trigger),
                ParamChange::Step(d) => (d > 0).then_some(Effect::Trigger),
            };
        }
        match (self.options.mode, event.change) {
            (MapMode::Absolute, ParamChange::Value(v)) => {
                let level = f32::from(v.min(MAX_PARAM_NUMBER)) / 16_383.0;
                self.absolute(state, level, current)
            }
            (MapMode::Absolute | MapMode::Relative(_), ParamChange::Step(d)) => {
                self.moved_by(state, i32::from(d), current)
            }
            (MapMode::Relative(kind), ParamChange::Value(v)) => {
                self.moved_by(state, kind.detents((v >> 7) as u8), current)
            }
            (MapMode::Toggle, ParamChange::Value(v)) => {
                let pressed = self.edge(state, down(v));
                self.flip(state, pressed, current)
            }
            (MapMode::Toggle, ParamChange::Step(d)) => self.flip(state, d > 0, current),
            (MapMode::Momentary, ParamChange::Value(v)) => self.hold(state, down(v)),
            (MapMode::Momentary, ParamChange::Step(_)) => None,
        }
    }

    /// A relative move of `detents` (positive is clockwise) from the value of the target.
    fn moved_by(&self, state: &mut MapState, detents: i32, current: Option<f32>) -> Option<Effect> {
        let o = &self.options;
        if detents == 0 {
            return None;
        }
        let sign = if o.invert { -1.0 } else { 1.0 };
        let (lo, hi) = (o.min.min(o.max), o.min.max(o.max));
        let moved = current? + detents as f32 * o.sensitivity * sign;
        let out = moved.clamp(lo, hi);
        state.last_applied = Some(out);
        Some(Effect::Set(out))
    }

    /// A toggle that was pressed flips the target against what it is now (it may have changed
    /// elsewhere).
    fn flip(&self, state: &mut MapState, pressed: bool, current: Option<f32>) -> Option<Effect> {
        if !pressed {
            return None;
        }
        let (off, on) = self.ends();
        let is_on = current.is_some_and(|c| (c - on).abs() < (c - off).abs());
        let out = if is_on { off } else { on };
        state.last_applied = Some(out);
        Some(Effect::Set(out))
    }

    /// A momentary button: the target is at one end while it is down, at the other when up.
    fn hold(&self, state: &mut MapState, down: bool) -> Option<Effect> {
        let was = state.held;
        state.held = down;
        if down == was {
            return None;
        }
        let (off, on) = self.ends();
        let out = if down { on } else { off };
        state.last_applied = Some(out);
        Some(Effect::Set(out))
    }

    /// The target values of a released and a pressed button (`invert` swaps them).
    fn ends(&self) -> (f32, f32) {
        let o = &self.options;
        if o.invert {
            (o.max, o.min)
        } else {
            (o.min, o.max)
        }
    }

    /// Whether the message says the button is down (`Some(true)`), up (`Some(false)`), or is not
    /// a button message for this source.
    fn button_down(&self, msg: &ChannelMessage) -> Option<bool> {
        match msg.kind {
            MessageKind::NoteOn { .. } => Some(true),
            MessageKind::NoteOff { .. } => Some(false),
            MessageKind::Cc { value, .. } => Some(value >= 64),
            MessageKind::PitchBend { value } => Some(value >= 8192),
        }
    }

    /// True for the message that presses a button (a release, or a repeat while it is down, is
    /// not a press).
    fn press(&self, state: &mut MapState, msg: &ChannelMessage) -> bool {
        self.button_down(msg)
            .is_some_and(|down| self.edge(state, down))
    }

    /// Records the button as `down` and says whether that is a press.
    fn edge(&self, state: &mut MapState, down: bool) -> bool {
        let pressed = down && !state.held;
        state.held = down;
        pressed
    }

    /// The control's position, 0..=1, or `None` for a message that carries none.
    fn level(&self, state: &mut MapState, msg: &ChannelMessage) -> Option<f32> {
        match (self.source.kind, msg.kind) {
            (SourceKind::Cc(_), MessageKind::Cc { value, .. }) => Some(f32::from(value) / 127.0),
            (SourceKind::Cc14(n), MessageKind::Cc { number, value }) => {
                if number == n {
                    state.msb = value;
                } else {
                    state.lsb = value;
                }
                let raw = (u16::from(state.msb) << 7) | u16::from(state.lsb);
                Some(f32::from(raw) / 16_383.0)
            }
            (SourceKind::PitchBend, MessageKind::PitchBend { value }) => {
                Some(f32::from(value.min(16_383)) / 16_383.0)
            }
            (SourceKind::Note(_), MessageKind::NoteOn { velocity, .. }) => {
                Some(f32::from(velocity) / 127.0)
            }
            _ => None,
        }
    }

    fn absolute(&self, state: &mut MapState, level: f32, current: Option<f32>) -> Option<Effect> {
        let o = &self.options;
        let r = if o.invert { 1.0 - level } else { level };
        let out = o.min + r * (o.max - o.min);
        if o.pickup
            && let Some(cur) = current
            && !state.take_over(out, cur, PICKUP_WINDOW)
        {
            return None;
        }
        state.last_out = Some(out);
        state.last_applied = Some(out);
        Some(Effect::Set(out))
    }
}

/// What a message is for the learner: the source it would make.
fn learned(msg: &ChannelMessage) -> Option<MidiSource> {
    let kind = match msg.kind {
        MessageKind::NoteOn { key, .. } => SourceKind::Note(key),
        // A note off and the channel-mode controllers are never a control.
        MessageKind::NoteOff { .. } => return None,
        MessageKind::Cc { number, .. } if number >= 120 || is_param_selector(number) => {
            return None;
        }
        MessageKind::Cc { number, .. } => SourceKind::Cc(number),
        MessageKind::PitchBend { .. } => SourceKind::PitchBend,
    };
    Some(MidiSource::any_port(Some(msg.channel), kind))
}

/// Finds the source of the first control the user moves: a note, a pitch bend or a controller,
/// and, for a controller below 64, waits [`PAIR_WINDOW_SECONDS`] for its other half so that an
/// MSB/LSB pair becomes one 14 bit source. Pure: time comes in as an argument.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Learner {
    pending: Option<Pending>,
    /// The runs of selecting and data controllers, so a knob that sends an NRPN or RPN is
    /// learned as that number and not as the controllers it is made of.
    params: ParamBank,
}

#[derive(Debug, Clone, PartialEq)]
struct Pending {
    port: String,
    channel: u8,
    /// The controller number first seen (0..=63).
    number: u8,
    at: f64,
}

impl Learner {
    /// A learner waiting for a message.
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the first half of a possible pair is held back.
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Forgets anything held back.
    pub fn reset(&mut self) {
        self.pending = None;
        self.params.reset();
    }

    /// Takes a message at time `now` (seconds): returns the source when it is decided.
    pub fn feed(&mut self, port: &str, msg: ChannelMessage, now: f64) -> Option<MidiSource> {
        if let Some(event) = self.params.feed(port, &msg) {
            // A completed parameter run decides at once and drops what was held back.
            self.pending = None;
            let kind = match event.space {
                ParamSpace::Nrpn => SourceKind::Nrpn(event.number),
                ParamSpace::Rpn => SourceKind::Rpn(event.number),
            };
            return Some(MidiSource::any_port(Some(msg.channel), kind));
        }
        if let MessageKind::Cc { number, .. } = msg.kind
            && number < 64
        {
            return self.feed_cc(port, msg.channel, number, now);
        }
        // Another kind of message decides at once and drops what was held back.
        let held = self.pending.take();
        match learned(&msg) {
            Some(source) => Some(source),
            None => held.map(|p| p.source()),
        }
    }

    fn feed_cc(&mut self, port: &str, channel: u8, number: u8, now: f64) -> Option<MidiSource> {
        let Some(p) = self.pending.take() else {
            self.pending = Some(Pending {
                port: port.to_owned(),
                channel,
                number,
                at: now,
            });
            return None;
        };
        let same_control = p.port == port && p.channel == channel;
        if same_control && number == p.number {
            // The same knob again: keep waiting from the first message's time.
            self.pending = Some(p);
            return None;
        }
        let pair = same_control
            && ((p.number < 32 && number == p.number + 32)
                || (number < 32 && p.number == number + 32));
        if pair {
            let msb = p.number.min(number);
            return Some(MidiSource::any_port(Some(channel), SourceKind::Cc14(msb)));
        }
        // Two different controls at once: the first one wins.
        Some(p.source())
    }

    /// Decides a held-back controller once the pair window has passed.
    pub fn tick(&mut self, now: f64) -> Option<MidiSource> {
        let p = self.pending.as_ref()?;
        if now - p.at < PAIR_WINDOW_SECONDS {
            return None;
        }
        self.pending.take().map(|p| p.source())
    }
}

impl Pending {
    fn source(&self) -> MidiSource {
        MidiSource::any_port(Some(self.channel), SourceKind::Cc(self.number))
    }
}

// ---------------------------------------------------------------------------- presets

/// What a preset mapping addresses: a role, not a concrete device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetTarget {
    /// A param of the `nth` (from 0, in rack order) device of a type.
    Device {
        /// The device type id (`atomik.channel`, `atomik.poly-synth`).
        type_id: String,
        /// The position among the top-level devices of that type.
        nth: usize,
        /// The param key.
        param: String,
    },
    /// A param of **a device of a type**, wherever it sits: the preset of one plugin. It is
    /// applied to the device the user picks (the selected one), else to the first device of the
    /// type, so the mappings follow the plugin and not its position in the rack.
    OfType {
        /// The device type id (a plugin's type id names the plugin).
        type_id: String,
        /// The param key.
        param: String,
    },
    /// The level of a VCA group, by id.
    Vca {
        /// The group's id.
        id: u32,
    },
    /// The tempo.
    Tempo,
    /// A transport key.
    Transport(TransportTarget),
    /// A param of the selected console strip.
    SelectedStrip {
        /// The strip param key.
        param: String,
    },
    /// A param of the focused device.
    Focused {
        /// The param key.
        param: String,
    },
    /// A command.
    Command(RemoteCommand),
}

/// One mapping of a preset.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetMap {
    /// The control (its port is never part of a preset).
    pub source: MidiSource,
    /// The role it drives.
    pub target: PresetTarget,
    /// How.
    pub options: MapOptions,
}

/// A set of mappings addressed by role, exported to and imported from a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiPreset {
    /// The preset's name.
    pub name: String,
    /// The mappings.
    pub maps: Vec<PresetMap>,
}

impl RackDoc {
    /// The mapping `id`.
    pub fn midi_map(&self, id: u32) -> Option<&MidiMapping> {
        self.midi_maps.iter().find(|m| m.id == id)
    }

    /// The mappings of `target`.
    pub fn midi_maps_for<'a>(
        &'a self,
        target: &'a MidiTarget,
    ) -> impl Iterator<Item = &'a MidiMapping> {
        self.midi_maps.iter().filter(move |m| &m.target == target)
    }

    /// Why `target` cannot be mapped in this project, or `None`.
    pub fn midi_target_problem(&self, registry: &Registry, target: &MidiTarget) -> Option<String> {
        match target {
            MidiTarget::Param { device, param } => {
                let Some(d) = self.resolve(device) else {
                    return Some(format!("no device `{device}`"));
                };
                if is_hidden_param(param) {
                    return Some(format!("`{param}` is not a parameter you can map"));
                }
                let desc = registry.get(&d.type_id).map(|f| f.descriptor)?;
                if desc.param_by_key(param).is_some()
                    && let Some(a) = self.bank_driver(device, param)
                {
                    // A macro owns the param: the controller belongs on the macro.
                    return Some(format!(
                        "`{device}` {param} is driven by macro {} of `{}`: map the macro instead",
                        a.index + 1,
                        a.bank
                    ));
                }
                desc.param_by_key(param)
                    .is_none()
                    .then(|| format!("`{device}` has no param `{param}`"))
            }
            MidiTarget::Vca { id } => self
                .console
                .vca(*id)
                .is_none()
                .then(|| format!("no VCA group {id}")),
            MidiTarget::Tempo | MidiTarget::Transport(_) | MidiTarget::Command(_) => None,
            MidiTarget::Focused { param } => (param.is_empty() || is_hidden_param(param))
                .then(|| format!("`{param}` is not a parameter you can map")),
            MidiTarget::SelectedStrip { param } => {
                let desc = registry.get(CHANNEL_TYPE).map(|f| f.descriptor)?;
                (desc.param_by_key(param).is_none() || is_hidden_param(param))
                    .then(|| format!("a channel strip has no param `{param}`"))
            }
        }
    }

    /// The position of the top-level device `key` among the devices of its type.
    fn nth_of_type(&self, key: &DeviceKey) -> Option<(String, usize)> {
        let d = self.devices.iter().find(|d| &d.key == key)?;
        let nth = self
            .devices
            .iter()
            .filter(|o| o.type_id == d.type_id)
            .position(|o| o.key == d.key)?;
        Some((d.type_id.clone(), nth))
    }

    /// Exports the mappings of one device as the preset of its type: every param mapping of
    /// `device` becomes a [`PresetTarget::OfType`], so the preset applies to any device of that
    /// type (a plugin, say) in any project, wherever it sits in the rack. `None` when the device
    /// is gone or has no mappings.
    pub fn midi_preset_for_device(&self, name: &str, device: &DeviceKey) -> Option<MidiPreset> {
        let type_id = self.resolve(device)?.type_id.clone();
        let mut preset = MidiPreset {
            name: name.to_owned(),
            maps: Vec::new(),
        };
        for m in &self.midi_maps {
            let MidiTarget::Param { device: key, param } = &m.target else {
                continue;
            };
            if key != device {
                continue;
            }
            let mut source = m.source.clone();
            source.port = None;
            source.surface = None;
            preset.maps.push(PresetMap {
                source,
                target: PresetTarget::OfType {
                    type_id: type_id.clone(),
                    param: param.clone(),
                },
                options: m.options,
            });
        }
        (!preset.maps.is_empty()).then_some(preset)
    }

    /// Exports the mappings `ids` (all when empty) as a preset by role. A mapping that cannot be
    /// addressed by role (a device inside a container, or one that is gone) is left out and
    /// named in the second value.
    pub fn midi_preset(&self, name: &str, ids: &[u32]) -> (MidiPreset, Vec<String>) {
        let mut preset = MidiPreset {
            name: name.to_owned(),
            maps: Vec::new(),
        };
        let mut notes = Vec::new();
        for m in &self.midi_maps {
            if !ids.is_empty() && !ids.contains(&m.id) {
                continue;
            }
            let target = match &m.target {
                MidiTarget::Param { device, param } => match self.nth_of_type(device) {
                    Some((type_id, nth)) => PresetTarget::Device {
                        type_id,
                        nth,
                        param: param.clone(),
                    },
                    None => {
                        notes.push(format!(
                            "{}: `{device}` is not a top-level device",
                            m.target.describe()
                        ));
                        continue;
                    }
                },
                MidiTarget::Vca { id } => PresetTarget::Vca { id: *id },
                MidiTarget::Tempo => PresetTarget::Tempo,
                MidiTarget::Transport(t) => PresetTarget::Transport(*t),
                MidiTarget::SelectedStrip { param } => PresetTarget::SelectedStrip {
                    param: param.clone(),
                },
                MidiTarget::Focused { param } => PresetTarget::Focused {
                    param: param.clone(),
                },
                MidiTarget::Command(c) => PresetTarget::Command(*c),
            };
            let mut source = m.source.clone();
            source.port = None;
            source.surface = None;
            preset.maps.push(PresetMap {
                source,
                target,
                options: m.options,
            });
        }
        (preset, notes)
    }

    /// Resolves a preset against this project: the mappings (ids 0, to be assigned) that found
    /// their target, and a note for each that did not (no such device, no such param, no such
    /// group).
    pub fn resolve_preset(
        &self,
        registry: &Registry,
        preset: &MidiPreset,
    ) -> (Vec<MidiMapping>, Vec<String>) {
        self.resolve_preset_on(registry, preset, None)
    }

    /// Like [`Self::resolve_preset`], with the device a preset made for one device (a
    /// [`PresetTarget::OfType`]) goes to: `on` when it is of the right type, else the first
    /// top-level device of the type.
    pub fn resolve_preset_on(
        &self,
        registry: &Registry,
        preset: &MidiPreset,
        on: Option<&DeviceKey>,
    ) -> (Vec<MidiMapping>, Vec<String>) {
        let mut maps = Vec::new();
        let mut notes = Vec::new();
        for p in &preset.maps {
            let target = match &p.target {
                PresetTarget::OfType { type_id, param } => {
                    let chosen = on
                        .and_then(|k| self.resolve(k).filter(|d| &d.type_id == type_id))
                        .map(|d| d.key.clone());
                    let found = chosen.or_else(|| {
                        self.devices
                            .iter()
                            .find(|d| &d.type_id == type_id)
                            .map(|d| d.key.clone())
                    });
                    match found {
                        Some(device) => MidiTarget::Param {
                            device,
                            param: param.clone(),
                        },
                        None => {
                            notes.push(format!("no {type_id} for `{param}`"));
                            continue;
                        }
                    }
                }
                PresetTarget::Device {
                    type_id,
                    nth,
                    param,
                } => {
                    let found = self
                        .devices
                        .iter()
                        .filter(|d| &d.type_id == type_id)
                        .nth(*nth);
                    match found {
                        Some(d) => MidiTarget::Param {
                            device: d.key.clone(),
                            param: param.clone(),
                        },
                        None => {
                            notes.push(format!("no {type_id} number {} for `{param}`", nth + 1));
                            continue;
                        }
                    }
                }
                PresetTarget::Vca { id } => MidiTarget::Vca { id: *id },
                PresetTarget::Tempo => MidiTarget::Tempo,
                PresetTarget::Transport(t) => MidiTarget::Transport(*t),
                PresetTarget::SelectedStrip { param } => MidiTarget::SelectedStrip {
                    param: param.clone(),
                },
                PresetTarget::Focused { param } => MidiTarget::Focused {
                    param: param.clone(),
                },
                PresetTarget::Command(c) => MidiTarget::Command(*c),
            };
            if let Some(problem) = self.midi_target_problem(registry, &target) {
                notes.push(problem);
                continue;
            }
            maps.push(MidiMapping {
                id: 0,
                source: p.source.clone(),
                target,
                options: p.options,
            });
        }
        (maps, notes)
    }
}

// ---------------------------------------------------------------------------- commands

/// An edit of the project's mappings; each is one undo step.
#[derive(Debug, Clone, PartialEq)]
pub enum MidiMapCommand {
    /// Adds a mapping (its `id` is ignored: the next free id is used).
    Add(MidiMapping),
    /// What learning does: `target` gets `source`. A target that already has a mapping keeps its
    /// options and takes the new source; otherwise a mapping is added with the options of
    /// [`MapOptions::for_source`].
    Learn {
        /// The control that was moved.
        source: MidiSource,
        /// What it drives.
        target: MidiTarget,
    },
    /// Replaces the mapping with this id (source, target and options).
    Set(MidiMapping),
    /// Removes the mapping `id`.
    Remove {
        /// The mapping.
        id: u32,
    },
    /// Removes every mapping of `target` ("Forget MIDI").
    Forget {
        /// The target.
        target: MidiTarget,
    },
    /// Adds several mappings (a preset), in order; ids are assigned.
    AddMany(Vec<MidiMapping>),
    /// Removes every mapping.
    Clear,
}

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadMidiMap(message.into())
}

fn check_source(source: &MidiSource) -> Result<(), ModelError> {
    if let Some(c) = source.channel
        && c > 15
    {
        return Err(bad(format!("channel {} does not exist", u16::from(c) + 1)));
    }
    if let Some(p) = &source.port
        && (p.is_empty() || p.chars().count() > MAX_PORT_NAME_CHARS)
    {
        return Err(bad(format!(
            "an input port name has 1 to {MAX_PORT_NAME_CHARS} characters"
        )));
    }
    match source.kind {
        SourceKind::Cc(n) if n > 127 => Err(bad(format!("controller {n} does not exist"))),
        SourceKind::Cc14(n) if n > 31 => Err(bad(format!(
            "a 14 bit controller pair starts at 0..=31, not {n}"
        ))),
        SourceKind::Note(k) if k > 127 => Err(bad(format!("key {k} does not exist"))),
        SourceKind::Nrpn(n) | SourceKind::Rpn(n) if n > MAX_PARAM_NUMBER => Err(bad(format!(
            "parameter number {n} does not exist (0..={MAX_PARAM_NUMBER})"
        ))),
        _ => Ok(()),
    }
}

fn check_mapping(
    doc: &RackDoc,
    registry: &Registry,
    m: &MidiMapping,
    target_known: bool,
) -> Result<(), ModelError> {
    check_source(&m.source)?;
    if let Some(problem) = m.options.problem(&m.source) {
        return Err(bad(problem));
    }
    if !target_known && let Some(problem) = doc.midi_target_problem(registry, &m.target) {
        return Err(bad(problem));
    }
    Ok(())
}

fn next_id(maps: &[MidiMapping]) -> u32 {
    maps.iter().map(|m| m.id).max().unwrap_or(0) + 1
}

/// Plans a mapping command as one [`Edit::MidiMaps`] (the whole list swaps).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: MidiMapCommand,
) -> Result<Edit, ModelError> {
    let old = doc.midi_maps.clone();
    let mut new = old.clone();
    match command {
        MidiMapCommand::Add(mut m) => {
            check_mapping(doc, registry, &m, false)?;
            m.id = next_id(&new);
            new.push(m);
        }
        MidiMapCommand::Learn { source, target } => {
            check_source(&source)?;
            if let Some(problem) = doc.midi_target_problem(registry, &target) {
                return Err(bad(problem));
            }
            match new.iter_mut().find(|m| m.target == target) {
                Some(existing) => {
                    if let Some(problem) = existing.options.problem(&source) {
                        // A relative encoder cannot take a button: the options start over.
                        let _ = problem;
                        existing.options = MapOptions::for_learned(&source);
                    }
                    existing.source = source;
                }
                None => {
                    let options = MapOptions::for_learned(&source);
                    new.push(MidiMapping {
                        id: next_id(&old),
                        source,
                        target,
                        options,
                    });
                }
            }
        }
        MidiMapCommand::Set(m) => {
            let Some(existing) = new.iter_mut().find(|x| x.id == m.id) else {
                return Err(bad(format!("no mapping {}", m.id)));
            };
            let target_known = existing.target == m.target;
            check_mapping(doc, registry, &m, target_known)?;
            *existing = m;
        }
        MidiMapCommand::Remove { id } => {
            let before = new.len();
            new.retain(|m| m.id != id);
            if new.len() == before {
                return Err(bad(format!("no mapping {id}")));
            }
        }
        MidiMapCommand::Forget { target } => {
            new.retain(|m| m.target != target);
            if new.len() == old.len() {
                return Err(bad(format!("{} has no MIDI mapping", target.describe())));
            }
        }
        MidiMapCommand::AddMany(maps) => {
            if maps.is_empty() {
                return Err(bad("there is nothing to add"));
            }
            for mut m in maps {
                check_mapping(doc, registry, &m, false)?;
                m.id = next_id(&new);
                new.push(m);
            }
        }
        MidiMapCommand::Clear => {
            if new.is_empty() {
                return Err(bad("there are no mappings"));
            }
            new.clear();
        }
    }
    if new.len() > MAX_MIDI_MAPS {
        return Err(bad(format!("at most {MAX_MIDI_MAPS} mappings")));
    }
    if new == old {
        return Err(bad("the mapping is already like that"));
    }
    Ok(Edit::MidiMaps { old, new })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(ch: u8, n: u8, v: u8) -> ChannelMessage {
        ChannelMessage::cc(ch, n, v)
    }

    fn mapping(kind: SourceKind, options: MapOptions) -> MidiMapping {
        MidiMapping {
            id: 1,
            source: MidiSource::any_port(Some(0), kind),
            target: MidiTarget::Param {
                device: DeviceKey("d".into()),
                param: "p".into(),
            },
            options,
        }
    }

    fn set(e: Option<Effect>) -> f32 {
        match e {
            Some(Effect::Set(v)) => v,
            other => panic!("expected a value, got {other:?}"),
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn source_matching_respects_port_channel_and_kind() {
        let s = MidiSource {
            port: Some("Pad".into()),
            channel: Some(2),
            kind: SourceKind::Cc(7),
            surface: None,
        };
        assert!(s.matches("Pad", &cc(2, 7, 1)));
        assert!(!s.matches("Other", &cc(2, 7, 1)));
        assert!(!s.matches("Pad", &cc(3, 7, 1)));
        assert!(!s.matches("Pad", &cc(2, 8, 1)));
        let any = MidiSource::any_port(None, SourceKind::Cc14(1));
        assert!(any.matches("x", &cc(9, 1, 0)));
        assert!(
            any.matches("x", &cc(9, 33, 0)),
            "the LSB belongs to the pair"
        );
        assert!(!any.matches("x", &cc(9, 2, 0)));
        let note = MidiSource::any_port(None, SourceKind::Note(60));
        assert!(note.matches("x", &ChannelMessage::note_on(0, 60, 90)));
        assert!(note.matches("x", &ChannelMessage::note_off(0, 60)));
        assert!(!note.matches("x", &ChannelMessage::note_on(0, 61, 90)));
        let bend = MidiSource::any_port(Some(1), SourceKind::PitchBend);
        assert!(bend.matches("x", &ChannelMessage::pitch_bend(1, 0)));
        assert!(!bend.matches("x", &ChannelMessage::pitch_bend(0, 0)));
    }

    #[test]
    fn absolute_maps_the_range_and_inverts() {
        let mut m = mapping(SourceKind::Cc(1), MapOptions::default());
        m.options.pickup = false;
        let mut st = MapState::default();
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 0), None)), 0.0));
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 127), None)), 1.0));
        assert!(near(
            set(m.feed(&mut st, &cc(0, 1, 64), None)),
            64.0 / 127.0
        ));
        m.options.min = 0.25;
        m.options.max = 0.75;
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 0), None)), 0.25));
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 127), None)), 0.75));
        m.options.invert = true;
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 0), None)), 0.75));
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 127), None)), 0.25));
        // A reversed range (min above max) runs the other way.
        m.options.invert = false;
        m.options.min = 1.0;
        m.options.max = 0.0;
        assert!(near(set(m.feed(&mut st, &cc(0, 1, 0), None)), 1.0));
    }

    #[test]
    fn fourteen_bit_pairs_combine_msb_and_lsb() {
        let mut m = mapping(SourceKind::Cc14(7), MapOptions::default());
        m.options.pickup = false;
        let mut st = MapState::default();
        // MSB 64, LSB 0 is 8192 / 16383.
        assert!(near(
            set(m.feed(&mut st, &cc(0, 7, 64), None)),
            8192.0 / 16_383.0
        ));
        assert!(near(
            set(m.feed(&mut st, &cc(0, 39, 1), None)),
            8193.0 / 16_383.0
        ));
        assert!(near(
            set(m.feed(&mut st, &cc(0, 7, 127), None)),
            16_257.0 / 16_383.0
        ));
        assert!(near(set(m.feed(&mut st, &cc(0, 39, 127), None)), 1.0));
        assert!(near(
            set(m.feed(&mut st, &cc(0, 7, 0), None)),
            127.0 / 16_383.0
        ));
        assert!(near(set(m.feed(&mut st, &cc(0, 39, 0), None)), 0.0));
    }

    #[test]
    fn pitch_bend_is_a_fourteen_bit_level() {
        let mut m = mapping(SourceKind::PitchBend, MapOptions::default());
        m.options.pickup = false;
        let mut st = MapState::default();
        let bend = |v| ChannelMessage::pitch_bend(0, v);
        assert!(near(set(m.feed(&mut st, &bend(0), None)), 0.0));
        assert!(near(set(m.feed(&mut st, &bend(16_383), None)), 1.0));
        assert!(near(
            set(m.feed(&mut st, &bend(8192), None)),
            8192.0 / 16_383.0
        ));
    }

    #[test]
    fn relative_encoders_decode_three_ways() {
        use RelativeKind::*;
        for (kind, value, want) in [
            (TwosComplement, 1, 1),
            (TwosComplement, 63, 63),
            (TwosComplement, 127, -1),
            (TwosComplement, 65, -63),
            (TwosComplement, 0, 0),
            (BinaryOffset, 65, 1),
            (BinaryOffset, 127, 63),
            (BinaryOffset, 63, -1),
            (BinaryOffset, 1, -63),
            (BinaryOffset, 64, 0),
            (SignMagnitude, 1, 1),
            (SignMagnitude, 63, 63),
            (SignMagnitude, 65, -1),
            (SignMagnitude, 127, -63),
            (SignMagnitude, 64, 0),
        ] {
            assert_eq!(kind.detents(value), want, "{kind:?} {value}");
        }
    }

    #[test]
    fn a_relative_encoder_moves_the_current_value_and_clamps_to_the_range() {
        let opts = MapOptions {
            mode: MapMode::Relative(RelativeKind::TwosComplement),
            sensitivity: 0.05,
            ..MapOptions::default()
        };
        let m = mapping(SourceKind::Cc(16), opts);
        let mut st = MapState::default();
        assert!(near(set(m.feed(&mut st, &cc(0, 16, 2), Some(0.5))), 0.6));
        assert!(near(set(m.feed(&mut st, &cc(0, 16, 126), Some(0.5))), 0.4));
        assert!(near(set(m.feed(&mut st, &cc(0, 16, 1), Some(0.99))), 1.0));
        assert!(near(set(m.feed(&mut st, &cc(0, 16, 127), Some(0.01))), 0.0));
        assert_eq!(m.feed(&mut st, &cc(0, 16, 0), Some(0.5)), None);
        assert_eq!(
            m.feed(&mut st, &cc(0, 16, 1), None),
            None,
            "an encoder needs the value it moves"
        );
        let mut inv = m.clone();
        inv.options.invert = true;
        assert!(near(set(inv.feed(&mut st, &cc(0, 16, 2), Some(0.5))), 0.4));
        let mut ranged = m;
        ranged.options.min = 0.2;
        ranged.options.max = 0.6;
        assert!(near(
            set(ranged.feed(&mut st, &cc(0, 16, 20), Some(0.5))),
            0.6
        ));
        assert!(near(
            set(ranged.feed(&mut st, &cc(0, 16, 100), Some(0.5))),
            0.2
        ));
    }

    #[test]
    fn soft_takeover_waits_for_the_hardware_to_reach_the_target() {
        let m = mapping(SourceKind::Cc(1), MapOptions::default());
        let mut st = MapState::default();
        // The target is at 0.5; the control starts at 0: nothing happens...
        assert_eq!(m.feed(&mut st, &cc(0, 1, 0), Some(0.5)), None);
        assert_eq!(m.feed(&mut st, &cc(0, 1, 30), Some(0.5)), None);
        assert!(!st.is_picked_up());
        // ...until it comes within a step of the target...
        let v = set(m.feed(&mut st, &cc(0, 1, 64), Some(0.5)));
        assert!(near(v, 64.0 / 127.0));
        assert!(st.is_picked_up());
        // ...and from then on follows without jumps, whatever the distance.
        assert!(near(
            set(m.feed(&mut st, &cc(0, 1, 100), Some(v))),
            100.0 / 127.0
        ));
        assert!(near(
            set(m.feed(&mut st, &cc(0, 1, 10), Some(100.0 / 127.0))),
            10.0 / 127.0
        ));
    }

    #[test]
    fn soft_takeover_also_catches_a_target_the_control_jumped_over() {
        let m = mapping(SourceKind::Cc(1), MapOptions::default());
        let mut st = MapState::default();
        assert_eq!(m.feed(&mut st, &cc(0, 1, 10), Some(0.5)), None);
        // A fast sweep from 10 to 120 skips the window: the crossing counts.
        assert!(near(
            set(m.feed(&mut st, &cc(0, 1, 120), Some(0.5))),
            120.0 / 127.0
        ));
    }

    #[test]
    fn take_over_is_the_rule_any_absolute_control_can_use() {
        let mut st = MapState::default();
        // Far below the target: nothing is taken, and the control must turn up.
        assert!(!st.take_over(0.1, 0.5, 0.02));
        assert_eq!(st.turn_to_catch(0.5), Some(true));
        // Within the window: taken.
        assert!(st.take_over(0.49, 0.5, 0.02));
        assert_eq!(st.turn_to_catch(0.5), None);
        // The target moved elsewhere: the control has to catch it again, from above this time.
        assert!(!st.take_over(0.9, 0.2, 0.02));
        assert_eq!(st.turn_to_catch(0.2), Some(false));
        // A forgotten takeover starts again.
        st.rearm();
        assert_eq!(st.turn_to_catch(0.2), None);
    }

    #[test]
    fn a_target_moved_elsewhere_makes_the_control_catch_it_again() {
        let m = mapping(SourceKind::Cc(1), MapOptions::default());
        let mut st = MapState::default();
        let v = set(m.feed(&mut st, &cc(0, 1, 64), Some(64.0 / 127.0)));
        // The mouse moves the target to 0.9; the hardware does not follow until it gets there.
        assert_eq!(m.feed(&mut st, &cc(0, 1, 65), Some(0.9)), None);
        assert!(!st.is_picked_up());
        assert!(near(
            set(m.feed(&mut st, &cc(0, 1, 114), Some(0.9))),
            114.0 / 127.0
        ));
        let _ = v;
        // Without pickup the control just takes over.
        let mut free = m;
        free.options.pickup = false;
        let mut st = MapState::default();
        assert!(near(set(free.feed(&mut st, &cc(0, 1, 0), Some(0.9))), 0.0));
    }

    #[test]
    fn a_toggle_flips_on_each_press_and_ignores_releases_and_repeats() {
        let mut opts = MapOptions::for_source(SourceKind::Note(36));
        assert_eq!(opts.mode, MapMode::Toggle);
        opts.pickup = false;
        let m = mapping(SourceKind::Note(36), opts);
        let mut st = MapState::default();
        let on = ChannelMessage::note_on(0, 36, 100);
        let off = ChannelMessage::note_off(0, 36);
        // Off now: a press turns it on.
        assert!(near(set(m.feed(&mut st, &on, Some(0.0))), 1.0));
        assert_eq!(m.feed(&mut st, &off, Some(1.0)), None);
        // On now: the next press turns it off.
        assert!(near(set(m.feed(&mut st, &on, Some(1.0))), 0.0));
        assert_eq!(
            m.feed(&mut st, &on, Some(0.0)),
            None,
            "a repeat is not a press"
        );
        assert_eq!(m.feed(&mut st, &off, Some(0.0)), None);
        // It follows what the target really is (it was changed elsewhere).
        assert!(near(set(m.feed(&mut st, &on, Some(0.0))), 1.0));
        assert_eq!(m.feed(&mut st, &off, Some(1.0)), None);
        assert!(near(set(m.feed(&mut st, &on, Some(1.0))), 0.0));
    }

    #[test]
    fn a_cc_button_toggles_at_64_and_above() {
        let opts = MapOptions {
            mode: MapMode::Toggle,
            ..MapOptions::default()
        };
        let m = mapping(SourceKind::Cc(80), opts);
        let mut st = MapState::default();
        assert!(near(set(m.feed(&mut st, &cc(0, 80, 127), Some(0.0))), 1.0));
        assert_eq!(m.feed(&mut st, &cc(0, 80, 127), Some(1.0)), None);
        assert_eq!(m.feed(&mut st, &cc(0, 80, 0), Some(1.0)), None);
        assert!(near(set(m.feed(&mut st, &cc(0, 80, 127), Some(1.0))), 0.0));
    }

    #[test]
    fn a_momentary_button_holds_one_end_while_down() {
        let opts = MapOptions {
            mode: MapMode::Momentary,
            min: 0.1,
            max: 0.9,
            ..MapOptions::default()
        };
        let m = mapping(SourceKind::Note(40), opts);
        let mut st = MapState::default();
        assert!(near(
            set(m.feed(&mut st, &ChannelMessage::note_on(0, 40, 1), None)),
            0.9
        ));
        assert_eq!(
            m.feed(&mut st, &ChannelMessage::note_on(0, 40, 1), None),
            None
        );
        assert!(near(
            set(m.feed(&mut st, &ChannelMessage::note_off(0, 40), None)),
            0.1
        ));
        let mut inv = m;
        inv.options.invert = true;
        let mut st = MapState::default();
        assert!(near(
            set(inv.feed(&mut st, &ChannelMessage::note_on(0, 40, 1), None)),
            0.1
        ));
    }

    #[test]
    fn a_transport_target_triggers_on_the_press_only() {
        let mut m = mapping(
            SourceKind::Note(50),
            MapOptions::for_source(SourceKind::Note(50)),
        );
        m.target = MidiTarget::Transport(TransportTarget::Play);
        let mut st = MapState::default();
        assert_eq!(
            m.feed(&mut st, &ChannelMessage::note_on(0, 50, 100), None),
            Some(Effect::Trigger)
        );
        assert_eq!(
            m.feed(&mut st, &ChannelMessage::note_off(0, 50), None),
            None
        );
        assert_eq!(
            m.feed(&mut st, &ChannelMessage::note_on(0, 50, 100), None),
            Some(Effect::Trigger)
        );
        // A controller button works the same, from 64 up.
        let mut c = mapping(SourceKind::Cc(115), MapOptions::default());
        c.target = MidiTarget::Transport(TransportTarget::Stop);
        let mut st = MapState::default();
        assert_eq!(
            c.feed(&mut st, &cc(0, 115, 127), None),
            Some(Effect::Trigger)
        );
        assert_eq!(c.feed(&mut st, &cc(0, 115, 127), None), None);
        assert_eq!(c.feed(&mut st, &cc(0, 115, 0), None), None);
        assert_eq!(
            c.feed(&mut st, &cc(0, 115, 127), None),
            Some(Effect::Trigger)
        );
    }

    #[test]
    fn the_learner_takes_notes_bends_and_controllers() {
        let mut l = Learner::new();
        assert_eq!(
            l.feed("p", ChannelMessage::note_on(4, 60, 100), 0.0),
            Some(MidiSource::any_port(Some(4), SourceKind::Note(60)))
        );
        assert_eq!(l.feed("p", ChannelMessage::note_off(4, 60), 0.0), None);
        assert_eq!(
            l.feed("p", ChannelMessage::pitch_bend(1, 5), 0.0),
            Some(MidiSource::any_port(Some(1), SourceKind::PitchBend))
        );
        // Channel mode messages are never a control.
        assert_eq!(l.feed("p", cc(0, 123, 0), 0.0), None);
        assert!(!l.is_waiting());
        // A controller above 63 decides at once.
        assert_eq!(
            l.feed("p", cc(2, 74, 10), 0.0),
            Some(MidiSource::any_port(Some(2), SourceKind::Cc(74)))
        );
    }

    #[test]
    fn the_learner_waits_for_the_other_half_of_a_fourteen_bit_pair() {
        let mut l = Learner::new();
        // MSB then LSB.
        assert_eq!(l.feed("p", cc(0, 1, 50), 0.00), None);
        assert!(l.is_waiting());
        assert_eq!(
            l.feed("p", cc(0, 33, 3), 0.01),
            Some(MidiSource::any_port(Some(0), SourceKind::Cc14(1)))
        );
        // LSB then MSB.
        assert_eq!(l.feed("p", cc(0, 34, 3), 0.0), None);
        assert_eq!(
            l.feed("p", cc(0, 2, 9), 0.01),
            Some(MidiSource::any_port(Some(0), SourceKind::Cc14(2)))
        );
        // A lone controller is decided when the window passes.
        assert_eq!(l.feed("p", cc(0, 7, 9), 1.0), None);
        assert_eq!(
            l.feed("p", cc(0, 7, 10), 1.05),
            None,
            "the same knob keeps waiting"
        );
        assert_eq!(l.tick(1.05), None);
        assert_eq!(
            l.tick(1.0 + PAIR_WINDOW_SECONDS),
            Some(MidiSource::any_port(Some(0), SourceKind::Cc(7)))
        );
        assert!(!l.is_waiting());
        // Another channel is not the other half.
        assert_eq!(l.feed("p", cc(0, 1, 9), 2.0), None);
        assert_eq!(
            l.feed("p", cc(1, 33, 9), 2.01),
            Some(MidiSource::any_port(Some(0), SourceKind::Cc(1)))
        );
        // A note decides at once and wins over what was held back.
        assert_eq!(l.feed("p", cc(0, 1, 9), 3.0), None);
        assert_eq!(
            l.feed("p", ChannelMessage::note_on(0, 5, 9), 3.01),
            Some(MidiSource::any_port(Some(0), SourceKind::Note(5)))
        );
        assert!(!l.is_waiting());
    }

    fn event(space: ParamSpace, number: u16, change: ParamChange) -> ParamEvent {
        ParamEvent {
            space,
            number,
            change,
        }
    }

    fn nrpn(number: u16, change: ParamChange) -> ParamEvent {
        event(ParamSpace::Nrpn, number, change)
    }

    #[test]
    fn a_parameter_source_hears_its_number_family_channel_and_port_only() {
        let s = MidiSource {
            port: Some("Pad".into()),
            channel: Some(2),
            kind: SourceKind::Nrpn(300),
            surface: None,
        };
        let e = nrpn(300, ParamChange::Value(1));
        assert!(s.matches_param("Pad", None, 2, &e));
        assert!(!s.matches_param("Other", None, 2, &e));
        assert!(!s.matches_param("Pad", None, 3, &e));
        assert!(!s.matches_param("Pad", None, 2, &nrpn(301, ParamChange::Value(1))));
        assert!(
            !s.matches_param(
                "Pad",
                None,
                2,
                &event(ParamSpace::Rpn, 300, ParamChange::Step(1))
            ),
            "an RPN is not an NRPN of the same number"
        );
        let on_surface = MidiSource {
            surface: Some("Desk".into()),
            ..s.clone()
        };
        assert!(!on_surface.matches_param("Pad", None, 2, &e));
        assert!(on_surface.matches_param("Pad", Some("Desk"), 2, &e));
        // The message-level match never hears a parameter source.
        assert!(!s.matches("Pad", &cc(2, 6, 10)));
        assert!(!MidiSource::any_port(None, SourceKind::Cc(7)).matches_param("Pad", None, 0, &e));
    }

    #[test]
    fn a_data_entry_is_a_14_bit_level() {
        let mut m = mapping(SourceKind::Nrpn(5), MapOptions::default());
        m.options.pickup = false;
        let mut st = MapState::default();
        let at = |m: &MidiMapping, st: &mut MapState, v| {
            set(m.feed_param(st, &nrpn(5, ParamChange::Value(v)), None))
        };
        assert!(near(at(&m, &mut st, 0), 0.0));
        assert!(near(at(&m, &mut st, 16_383), 1.0));
        assert!(near(at(&m, &mut st, 8_192), 8_192.0 / 16_383.0));
        m.options.invert = true;
        m.options.min = 0.2;
        m.options.max = 0.6;
        assert!(near(at(&m, &mut st, 0), 0.6));
        assert!(near(at(&m, &mut st, 16_383), 0.2));
    }

    #[test]
    fn increment_and_decrement_step_the_value_in_detents() {
        let mut m = mapping(SourceKind::Rpn(0), MapOptions::default());
        m.options.sensitivity = 0.1;
        let mut st = MapState::default();
        let step = |d| event(ParamSpace::Rpn, 0, ParamChange::Step(d));
        assert!(near(set(m.feed_param(&mut st, &step(1), Some(0.5))), 0.6));
        assert!(near(set(m.feed_param(&mut st, &step(-2), Some(0.5))), 0.3));
        assert!(near(set(m.feed_param(&mut st, &step(9), Some(0.5))), 1.0));
        assert_eq!(
            m.feed_param(&mut st, &step(1), None),
            None,
            "no known value, no relative move"
        );
        m.options.invert = true;
        assert!(near(set(m.feed_param(&mut st, &step(1), Some(0.5))), 0.4));
        // A relative mapping moves the same way on steps, and reads the high half of a value.
        m.options.invert = false;
        m.options.mode = MapMode::Relative(RelativeKind::TwosComplement);
        assert!(near(set(m.feed_param(&mut st, &step(1), Some(0.5))), 0.6));
        let up = nrpn_for(&m, ParamChange::Value(1 << 7));
        assert!(near(set(m.feed_param(&mut st, &up, Some(0.5))), 0.6));
        let down = nrpn_for(&m, ParamChange::Value(127 << 7));
        assert!(near(set(m.feed_param(&mut st, &down, Some(0.5))), 0.4));
    }

    fn nrpn_for(m: &MidiMapping, change: ParamChange) -> ParamEvent {
        match m.source.kind {
            SourceKind::Nrpn(n) => nrpn(n, change),
            SourceKind::Rpn(n) => event(ParamSpace::Rpn, n, change),
            other => panic!("not a parameter source: {other:?}"),
        }
    }

    #[test]
    fn a_parameter_button_is_down_from_the_middle_of_the_range() {
        let mut m = mapping(SourceKind::Nrpn(1), MapOptions::default());
        m.options.mode = MapMode::Toggle;
        let mut st = MapState::default();
        let value = |v| nrpn(1, ParamChange::Value(v));
        assert!(near(
            set(m.feed_param(&mut st, &value(16_383), Some(0.0))),
            1.0
        ));
        assert_eq!(
            m.feed_param(&mut st, &value(16_383), Some(1.0)),
            None,
            "a repeat"
        );
        assert_eq!(
            m.feed_param(&mut st, &value(0), Some(1.0)),
            None,
            "a release"
        );
        assert!(near(
            set(m.feed_param(&mut st, &value(9_000), Some(1.0))),
            0.0
        ));
        m.options.mode = MapMode::Momentary;
        let mut st = MapState::default();
        assert!(near(
            set(m.feed_param(&mut st, &value(16_000), Some(0.0))),
            1.0
        ));
        assert!(near(set(m.feed_param(&mut st, &value(10), Some(1.0))), 0.0));
        // A transport target presses on the data entry and on an increment.
        m.target = MidiTarget::Transport(TransportTarget::Play);
        let mut st = MapState::default();
        assert_eq!(
            m.feed_param(&mut st, &value(16_000), None),
            Some(Effect::Trigger)
        );
        assert_eq!(m.feed_param(&mut st, &value(16_100), None), None);
        assert_eq!(m.feed_param(&mut st, &value(0), None), None);
        assert_eq!(
            m.feed_param(&mut st, &nrpn(1, ParamChange::Step(1)), None),
            Some(Effect::Trigger)
        );
        assert_eq!(
            m.feed_param(&mut st, &nrpn(1, ParamChange::Step(-1)), None),
            None
        );
    }

    #[test]
    fn the_learner_learns_a_parameter_number_not_its_controllers() {
        let mut l = Learner::new();
        // The selecting controllers are never a control by themselves.
        assert_eq!(l.feed("p", cc(3, 99, 1), 0.0), None);
        assert_eq!(l.feed("p", cc(3, 98, 2), 0.0), None);
        assert!(!l.is_waiting());
        assert_eq!(
            l.feed("p", cc(3, 6, 64), 0.0),
            Some(MidiSource::any_port(Some(3), SourceKind::Nrpn(130)))
        );
        // A run on another channel does not borrow this one's selection.
        assert_eq!(l.feed("p", cc(4, 6, 64), 0.1), None);
        l.reset();
        assert_eq!(l.feed("p", cc(0, 101, 0), 0.0), None);
        assert_eq!(l.feed("p", cc(0, 100, 3), 0.0), None);
        assert_eq!(
            l.feed("p", cc(0, 96, 1), 0.0),
            Some(MidiSource::any_port(Some(0), SourceKind::Rpn(3)))
        );
        // Data entry with nothing selected is still the plain controller.
        l.reset();
        assert_eq!(l.feed("p", cc(0, 6, 10), 0.0), None);
        assert_eq!(
            l.tick(1.0),
            Some(MidiSource::any_port(Some(0), SourceKind::Cc(6)))
        );
    }

    #[test]
    fn a_parameter_source_is_checked_and_described() {
        let ok = MidiSource::any_port(Some(1), SourceKind::Nrpn(16_383));
        assert_eq!(ok.describe(), "NRPN 16383 ch 2");
        assert_eq!(ok.what(), "NRPN 16383");
        assert_eq!(
            MidiSource::any_port(None, SourceKind::Rpn(2)).describe(),
            "RPN 2 any ch"
        );
        assert!(check_source(&ok).is_ok());
        assert!(check_source(&MidiSource::any_port(None, SourceKind::Rpn(16_384))).is_err());
        // A parameter number can be a relative encoder, a pitch bend cannot.
        let relative = MapOptions {
            mode: MapMode::Relative(RelativeKind::SignMagnitude),
            ..MapOptions::default()
        };
        assert_eq!(relative.problem(&ok), None);
        assert!(
            relative
                .problem(&MidiSource::any_port(None, SourceKind::PitchBend))
                .is_some()
        );
        assert_eq!(MapOptions::for_source(ok.kind).mode, MapMode::Absolute);
    }

    #[test]
    fn options_are_checked() {
        let cc_src = MidiSource::any_port(None, SourceKind::Cc(1));
        assert!(MapOptions::default().problem(&cc_src).is_none());
        let o = MapOptions {
            min: -0.1,
            ..MapOptions::default()
        };
        assert!(o.problem(&cc_src).is_some());
        let o = MapOptions {
            sensitivity: 0.0,
            ..MapOptions::default()
        };
        assert!(o.problem(&cc_src).is_some());
        let mut o = MapOptions {
            mode: MapMode::Relative(RelativeKind::BinaryOffset),
            ..MapOptions::default()
        };
        assert!(o.problem(&cc_src).is_none());
        assert!(
            o.problem(&MidiSource::any_port(None, SourceKind::PitchBend))
                .is_some()
        );
        o.max = f32::NAN;
        assert!(o.problem(&cc_src).is_some());
    }

    #[test]
    fn sources_and_targets_describe_themselves() {
        assert_eq!(
            MidiSource::any_port(Some(0), SourceKind::Cc(7)).describe(),
            "CC 7 ch 1"
        );
        assert_eq!(
            MidiSource::any_port(None, SourceKind::Cc14(1)).describe(),
            "CC 1/33 (14 bit) any ch"
        );
        assert_eq!(
            MidiSource {
                port: Some("Pad".into()),
                channel: Some(9),
                kind: SourceKind::Note(60),
                surface: None,
            }
            .describe(),
            "Note C3 ch 10 (Pad)"
        );
        assert_eq!(MidiTarget::Vca { id: 2 }.describe(), "VCA 2");
        assert_eq!(
            MidiTarget::Transport(TransportTarget::Loop).describe(),
            "Loop"
        );
        assert_eq!(note_name(0), "C-2");
        assert_eq!(note_name(127), "G8");
    }

    #[test]
    fn transport_and_mode_ids_round_trip() {
        for t in TransportTarget::ALL {
            assert_eq!(TransportTarget::from_id(t.id()), Some(t));
        }
        for mode in [
            MapMode::Absolute,
            MapMode::Relative(RelativeKind::TwosComplement),
            MapMode::Relative(RelativeKind::BinaryOffset),
            MapMode::Relative(RelativeKind::SignMagnitude),
            MapMode::Toggle,
            MapMode::Momentary,
        ] {
            assert_eq!(MapMode::from_id(mode.id()), Some(mode));
            assert!(!mode.label().is_empty());
        }
        assert_eq!(MapMode::from_id("nope"), None);
    }
}
