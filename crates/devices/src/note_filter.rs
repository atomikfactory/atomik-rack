//! `atomik.note-filter`: transpose and the note filters in one device (a per-event note map).
//!
//! Stages, in order: event filter, key range, velocity range, transpose, velocity shaping, output
//! range.
//!
//! * **Key range** tests the *input* key: `inside` passes `key.low..=key.high`, `outside` passes the
//!   rest, `fold` passes every key and folds the *output* key by octaves into the range (a range
//!   narrower than an octave drops the keys no octave lands in).
//! * **Transpose** adds `transpose + 12 * octave` semitones. A result outside 0..=127 is dropped,
//!   not clamped: clamping would merge keys.
//! * **Velocity range** tests the input velocity (1..=127); outside notes and their Offs vanish.
//!   The velocity then goes through `curve` (`f = v / (v + (1 - v) * e)`, `e = 2^(-2 curve)`, so a
//!   positive curve lifts soft notes), `scale` and `offset`, clamped to 1..=127; `velocity.fixed`
//!   above 0 replaces the result. With all four at their defaults the velocity passes unrounded.
//! * **Events.** Bend, CC and pressure pass unless their toggle is off. Polyphonic pressure and
//!   per-note expressions follow the output key of their input key and drop when it sounds nothing.
//! * **Note safety.** A `NoteRouter` remembers what every input key emitted, so a param change
//!   while keys are held never leaves a stuck or doubled note: the Off releases the pitch the On
//!   sent. `AllOff` closes everything and passes on; a `reset` leaves an `AllOff` for the next
//!   batch when a note sounded.
//! * **Bypass** (`bypass`, last param): see `crate::note_fx::PlayerBypass`.

use atomik_core::{NoteBuffer, NoteEvent, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::exp2_det;

use crate::bypass::bypass_param;
use crate::note_fx::{NoteRouter, PlayerBypass, push_sorted};

const NOTES_IN: SocketId = SocketId(0);
const NOTES_OUT: SocketId = SocketId(1);

static SOCKETS: [SocketDescriptor; 2] = [
    SocketDescriptor {
        id: NOTES_IN,
        key: "notes_in",
        name: "Notes in",
        kind: SocketKind::Notes,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: NOTES_OUT,
        key: "notes_out",
        name: "Notes out",
        kind: SocketKind::Notes,
        direction: Direction::Output,
    },
];

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32, f32),
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: range.0,
        max: range.1,
        default: range.2,
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

const KEY_MODES: Unit = Unit::Choice(&["inside", "outside", "fold"]);

const P_TRANSPOSE: u32 = 0;
const P_OCTAVE: u32 = 1;
const P_KEY_LOW: u32 = 2;
const P_KEY_HIGH: u32 = 3;
const P_KEY_MODE: u32 = 4;
const P_VEL_LOW: u32 = 5;
const P_VEL_HIGH: u32 = 6;
const P_VEL_SCALE: u32 = 7;
const P_VEL_OFFSET: u32 = 8;
const P_VEL_CURVE: u32 = 9;
const P_VEL_FIXED: u32 = 10;
const P_PASS_BEND: u32 = 11;
const P_PASS_CC: u32 = 12;
const P_PASS_PRESSURE: u32 = 13;
const P_BYPASS: u32 = 14;

static PARAMS: [ParamDescriptor; 15] = [
    param(
        P_TRANSPOSE,
        "transpose",
        "Transpose",
        (-48.0, 48.0, 0.0),
        Unit::Semitones,
        true,
    ),
    param(
        P_OCTAVE,
        "octave",
        "Octave",
        (-4.0, 4.0, 0.0),
        Unit::Octaves,
        true,
    ),
    param(
        P_KEY_LOW,
        "key.low",
        "Key low",
        (0.0, 127.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_KEY_HIGH,
        "key.high",
        "Key high",
        (0.0, 127.0, 127.0),
        Unit::None,
        true,
    ),
    param(
        P_KEY_MODE,
        "key.mode",
        "Key mode",
        (0.0, 2.0, 0.0),
        KEY_MODES,
        true,
    ),
    param(
        P_VEL_LOW,
        "velocity.low",
        "Velocity low",
        (1.0, 127.0, 1.0),
        Unit::None,
        true,
    ),
    param(
        P_VEL_HIGH,
        "velocity.high",
        "Velocity high",
        (1.0, 127.0, 127.0),
        Unit::None,
        true,
    ),
    param(
        P_VEL_SCALE,
        "velocity.scale",
        "Velocity scale",
        (0.0, 2.0, 1.0),
        Unit::Percent,
        false,
    ),
    param(
        P_VEL_OFFSET,
        "velocity.offset",
        "Velocity offset",
        (-127.0, 127.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_VEL_CURVE,
        "velocity.curve",
        "Velocity curve",
        (-1.0, 1.0, 0.0),
        Unit::None,
        false,
    ),
    param(
        P_VEL_FIXED,
        "velocity.fixed",
        "Fixed velocity",
        (0.0, 127.0, 0.0),
        Unit::None,
        true,
    ),
    param(
        P_PASS_BEND,
        "pass.bend",
        "Pass bend",
        (0.0, 1.0, 1.0),
        Unit::Toggle,
        true,
    ),
    param(
        P_PASS_CC,
        "pass.cc",
        "Pass CC",
        (0.0, 1.0, 1.0),
        Unit::Toggle,
        true,
    ),
    param(
        P_PASS_PRESSURE,
        "pass.pressure",
        "Pass pressure",
        (0.0, 1.0, 1.0),
        Unit::Toggle,
        true,
    ),
    bypass_param(P_BYPASS),
];

/// Static description of the note filter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.note-filter",
    name: "Note Filter",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`NoteFilter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(NoteFilter::new()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyMode {
    Inside,
    Outside,
    Fold,
}

/// The note filter device.
#[derive(Debug, Clone)]
pub struct NoteFilter {
    router: NoteRouter,
    bypass: PlayerBypass,
    /// Emit an `AllOff` at the start of the next batch (a `reset` while something sounded).
    flush_alloff: bool,
    transpose: i32,
    octave: i32,
    key_low: i32,
    key_high: i32,
    key_mode: KeyMode,
    vel_low: i32,
    vel_high: i32,
    scale: f32,
    offset: f32,
    curve: f32,
    /// `2^(-2 curve)`.
    curve_e: f32,
    fixed: i32,
    pass_bend: bool,
    pass_cc: bool,
    pass_pressure: bool,
}

impl NoteFilter {
    fn new() -> Self {
        Self {
            router: NoteRouter::new(),
            bypass: PlayerBypass::new(),
            flush_alloff: false,
            transpose: 0,
            octave: 0,
            key_low: 0,
            key_high: 127,
            key_mode: KeyMode::Inside,
            vel_low: 1,
            vel_high: 127,
            scale: 1.0,
            offset: 0.0,
            curve: 0.0,
            curve_e: 1.0,
            fixed: 0,
            pass_bend: true,
            pass_cc: true,
            pass_pressure: true,
        }
    }

    // The exact comparisons detect the untouched defaults.
    #[allow(clippy::float_cmp)]
    /// The output note of an input `On`, or `None` when a filter drops it.
    fn map_on(&self, key: u8, velocity: f32) -> Option<(u8, f32)> {
        let k = i32::from(key);
        let low = self.key_low.min(self.key_high);
        let high = self.key_low.max(self.key_high);
        let inside = (low..=high).contains(&k);
        match self.key_mode {
            KeyMode::Inside if !inside => return None,
            KeyMode::Outside if inside => return None,
            _ => {}
        }
        let vi = ((velocity * 127.0).round() as i32).clamp(1, 127);
        if vi < self.vel_low.min(self.vel_high) || vi > self.vel_low.max(self.vel_high) {
            return None;
        }
        let mut out = k + self.transpose + 12 * self.octave;
        if self.key_mode == KeyMode::Fold {
            while out < low {
                out += 12;
            }
            while out > high {
                out -= 12;
            }
            if out < low {
                return None;
            }
        }
        if !(0..=127).contains(&out) {
            return None;
        }
        if self.fixed == 0 && self.scale == 1.0 && self.offset == 0.0 && self.curve == 0.0 {
            // Untouched velocity keeps its exact value (no round trip through 1..=127).
            return Some((out as u8, velocity));
        }
        let n = if self.fixed > 0 {
            self.fixed
        } else {
            let v = velocity.clamp(0.0, 1.0);
            let f = v / (v + (1.0 - v) * self.curve_e);
            ((127.0 * f * self.scale + self.offset).round() as i32).clamp(1, 127)
        };
        Some((out as u8, n as f32 / 127.0))
    }

    fn apply(&mut self, out: &mut NoteBuffer, e: NoteEvent) {
        let frame = usize::from(e.offset);
        let pass = |out: &mut NoteBuffer, kind: NoteKind| {
            push_sorted(
                out,
                NoteEvent {
                    offset: e.offset,
                    kind,
                },
            );
        };
        match e.kind {
            NoteKind::On { key, velocity } => match self.map_on(key, velocity) {
                Some(o) => self.router.on(out, frame, key, &[o]),
                None => self.router.on(out, frame, key, &[]),
            },
            NoteKind::Off { key } => self.router.off(out, frame, key),
            NoteKind::AllOff => {
                self.router.all_off(out, frame);
                pass(out, NoteKind::AllOff);
            }
            NoteKind::PitchBend { .. } if self.pass_bend => pass(out, e.kind),
            NoteKind::Cc { .. } if self.pass_cc => pass(out, e.kind),
            NoteKind::ChannelPressure { .. } if self.pass_pressure => pass(out, e.kind),
            NoteKind::PolyPressure { key, value } if self.pass_pressure => {
                if let Some(k) = self.router.follow(key) {
                    pass(out, NoteKind::PolyPressure { key: k, value });
                }
            }
            NoteKind::NoteExpr { key, kind, value } => {
                if let Some(k) = self.router.follow(key) {
                    pass(
                        out,
                        NoteKind::NoteExpr {
                            key: k,
                            kind,
                            value,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

impl Device for NoteFilter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {
        self.reset();
        self.flush_alloff = false;
    }

    fn reset(&mut self) {
        if self.router.any_sounding() {
            self.flush_alloff = true;
        }
        self.router.clear();
        if self.bypass.reset() {
            self.flush_alloff = true;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let int = |lo: f32, hi: f32| plain.round().clamp(lo, hi) as i32;
        match id.0 {
            P_TRANSPOSE => self.transpose = int(-48.0, 48.0),
            P_OCTAVE => self.octave = int(-4.0, 4.0),
            P_KEY_LOW => self.key_low = int(0.0, 127.0),
            P_KEY_HIGH => self.key_high = int(0.0, 127.0),
            P_KEY_MODE => {
                self.key_mode = match int(0.0, 2.0) {
                    0 => KeyMode::Inside,
                    1 => KeyMode::Outside,
                    _ => KeyMode::Fold,
                }
            }
            P_VEL_LOW => self.vel_low = int(1.0, 127.0),
            P_VEL_HIGH => self.vel_high = int(1.0, 127.0),
            P_VEL_SCALE => self.scale = plain.clamp(0.0, 2.0),
            P_VEL_OFFSET => self.offset = plain.round().clamp(-127.0, 127.0),
            P_VEL_CURVE => {
                self.curve = plain.clamp(-1.0, 1.0);
                self.curve_e = exp2_det(-2.0 * self.curve);
            }
            P_VEL_FIXED => self.fixed = int(0.0, 127.0),
            P_PASS_BEND => self.pass_bend = plain >= 0.5,
            P_PASS_CC => self.pass_cc = plain >= 0.5,
            P_PASS_PRESSURE => self.pass_pressure = plain >= 0.5,
            P_BYPASS => self.bypass.set_param(plain),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.flush_alloff {
            self.flush_alloff = false;
            push_sorted(
                ctx.notes_out(NOTES_OUT),
                NoteEvent {
                    offset: 0,
                    kind: NoteKind::AllOff,
                },
            );
        }
        if self.bypass.begin_batch(ctx.notes_out(NOTES_OUT)) {
            // The switch closes what the filter sent and starts it from nothing.
            self.router.close_all(ctx.notes_out(NOTES_OUT), 0);
            self.router.clear();
        }
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let e = ctx.notes_in(NOTES_IN)[i];
            if let Some(e) = self.bypass.route(ctx.notes_out(NOTES_OUT), e) {
                self.apply(ctx.notes_out(NOTES_OUT), e);
            }
        }
    }
}

#[cfg(test)]
mod tests;
