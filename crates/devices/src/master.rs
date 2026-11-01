//! `atomik.master`: the console master section.
//!
//! ## Signal flow
//! `bus_*` (summing input: every channel output) + the 8 FX returns (`return{k}_*` × level,
//! constant-power pan, mute) → master insert and bus compressor (insert after the compressor by
//! default, before it with `insert.pre_comp`) → fader → `main_*`.
//! * `fx_send{k}_*` = `send_bus{k}_*` (summing input: every channel's send `k`) × `send{k}.level`
//!   (dB, default 0 dB = bit-exact pass-through), to the return effect.
//! * `monitor_*` (control room) = the selected source × `monitor.level`, × −20 dB when
//!   `monitor.dim` is on. `monitor.source` 0 = *Main* (the post-fader main mix),
//!   1 = *FX returns* (the returns sum alone, pre-insert/compressor/fader), then (appended so stored values keep their meaning) 2..=9 = *Send 1..8* (the `fx_send{k}` signal,
//!   after the send level) and 10..=17 = *Return 1..8* (the return after its fader and pan,
//!   before it joins the main sum). Dim and monitor level never touch `main_*`. The taps are
//!   copies kept in device state in stages 0 and 1 and read in stage 2, so the plan, the stages
//!   and the cables are unchanged.
//!
//! ## Processing stages
//! * Stage 0: `send_bus{k}_*` → `fx_send{k}_*`.
//! * Stage 1: `bus_*` + returns (→ compressor, unless `insert.pre_comp`) → `insert_send_*`.
//! * Stage 2: `insert_return_*` (pass-through when unconnected or bypassed) (→ compressor with
//!   `insert.pre_comp`) → fader → `main_*`, `monitor_*`, `gr_cv_out`.
//! * `key_in` is a stage-1 input, so the keyed compressor works without lag in either stage.
//! * [`Device::process`] runs the three stages in order (single-call hosts and tests).
//!
//! ## Design choices
//! * **Returns:** `return{k}.level` in dB, decibel taper −60 (= off) .. +6, default 0 dB;
//!   `return{k}.pan` uses the console's constant-power law (−3.01 dB at centre, `L·cos θ`,
//!   `R·sin θ`); a single connected return side feeds both sides.
//! * **Bus compressor:** [`atomik_dsp::Compressor`], sine-calibrated RMS detector, fixed
//!   [`atomik_dsp::KNEE_DB`] dB soft knee, stereo-linked. `comp.makeup` is applied after the gain
//!   reduction, only while the compressor is on. `comp.key` switches the side-chain to `key_in`
//!   (mono, used on both sides) when it is connected. `gr_cv_out` = `1 − g` of the compressor
//!   gain `g` (make-up excluded).
//! * **Insert:** `insert_send_*` carry the signal at the insert point; a connected
//!   `insert_return_*` replaces it unless `insert.bypass`.
//! * **Fader:** the same cubic position taper as the channel ([`atomik_dsp::fader_gain`]).
//! * Every gain and switch is smoothed over [`SMOOTH_MS`] (crossfades for on/off and inserts).
//!   The first batch after `prepare`/`reset` snaps them all to their targets.
//! * **Analyser tap** (`internal.tap_live`, set by the app while a window shows the master, never
//!   stored): stage 2 pushes the signal before the fader into a [`ScopeTap`] for the UI to
//!   analyse; off, nothing is written and the output is bit-identical.

use crate::channel::{
    SMOOTH_MS, Section, Step, fader, is_silent, pan, send_gain, send_level, smoother, snap, socket,
    stereo_in,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit, db_to_gain};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{Compressor, Detector, Smoother, fader_gain, pan_constant_power};
use std::sync::Arc;

/// Number of FX returns (and send buses).
pub const RETURNS: usize = 8;
/// Monitor attenuation while `monitor.dim` is on.
pub const DIM_DB: f32 = -20.0;

type Block = [f32; BATCH];

// Const builder mirroring every `ParamDescriptor` field (const fns cannot take a struct
// update with defaults), so it needs one argument per field.
#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    taper: Taper,
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete: false,
    }
}

const fn toggle(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        discrete: true,
        ..param(id, key, name, 0.0, 1.0, 0.0, Taper::Linear, Unit::Toggle)
    }
}

macro_rules! return_params {
    ($k:literal, $base:expr) => {
        [
            send_level(
                $base,
                concat!("return", $k, ".level"),
                concat!("Return ", $k, " level"),
            ),
            toggle(
                $base + 1,
                concat!("return", $k, ".mute"),
                concat!("Return ", $k, " mute"),
            ),
            pan(
                $base + 2,
                concat!("return", $k, ".pan"),
                concat!("Return ", $k, " pan"),
            ),
        ]
    };
}

const P_INSERT_PRE_COMP: usize = 0;
const P_INSERT_BYPASS: usize = 1;
const P_COMP_ON: usize = 2;
const P_COMP_THRESHOLD: usize = 3;
const P_COMP_RATIO: usize = 4;
const P_COMP_ATTACK: usize = 5;
const P_COMP_RELEASE: usize = 6;
const P_COMP_MAKEUP: usize = 7;
const P_COMP_KEY: usize = 8;
const P_RETURN_FIRST: usize = 9;
const P_FADER: usize = P_RETURN_FIRST + 3 * RETURNS;
const P_MONITOR_LEVEL: usize = P_FADER + 1;
const P_MONITOR_DIM: usize = P_FADER + 2;
const P_MONITOR_SOURCE: usize = P_FADER + 3;
/// First of the eight `send{k}.level` params (appended after `monitor.source`).
const P_SEND_FIRST: usize = P_MONITOR_SOURCE + 1;
/// The analyser tap switch (`internal.tap_live`: set by the app while a window shows the master,
/// never stored), appended after the sends.
const P_TAP_LIVE: usize = P_SEND_FIRST + RETURNS;
const PARAM_COUNT: usize = P_TAP_LIVE + 1;

/// `monitor.source` index of *FX returns*.
const SRC_RETURNS: usize = 1;
/// `monitor.source` index of *Send 1*.
const SRC_SEND_FIRST: usize = 2;
/// `monitor.source` index of *Return 1*.
const SRC_RETURN_FIRST: usize = SRC_SEND_FIRST + RETURNS;
/// Highest `monitor.source` index.
const SRC_LAST: usize = SRC_RETURN_FIRST + RETURNS - 1;

/// `monitor.source` choices: `Main`, `FX returns`, `Send 1..8`, `Return 1..8` (the last sixteen
/// were appended later).
pub const MONITOR_SOURCES: &[&str] = &[
    "Main",
    "FX returns",
    "Send 1",
    "Send 2",
    "Send 3",
    "Send 4",
    "Send 5",
    "Send 6",
    "Send 7",
    "Send 8",
    "Return 1",
    "Return 2",
    "Return 3",
    "Return 4",
    "Return 5",
    "Return 6",
    "Return 7",
    "Return 8",
];

static PARAMS: [ParamDescriptor; PARAM_COUNT] = {
    let head = [
        toggle(0, "insert.pre_comp", "Insert before compressor"),
        toggle(1, "insert.bypass", "Insert bypass"),
        toggle(2, "comp.on", "Bus comp on"),
        param(
            3,
            "comp.threshold",
            "Bus comp threshold",
            -40.0,
            0.0,
            -10.0,
            Taper::Linear,
            Unit::Db,
        ),
        param(
            4,
            "comp.ratio",
            "Bus comp ratio",
            1.0,
            10.0,
            2.0,
            Taper::Exponential,
            Unit::None,
        ),
        param(
            5,
            "comp.attack",
            "Bus comp attack",
            0.1,
            100.0,
            10.0,
            Taper::Exponential,
            Unit::Ms,
        ),
        param(
            6,
            "comp.release",
            "Bus comp release",
            50.0,
            2_000.0,
            300.0,
            Taper::Exponential,
            Unit::Ms,
        ),
        param(
            7,
            "comp.makeup",
            "Bus comp make-up",
            0.0,
            24.0,
            0.0,
            Taper::Linear,
            Unit::Db,
        ),
        toggle(8, "comp.key", "Bus comp key"),
    ];
    let returns = [
        return_params!("1", 9),
        return_params!("2", 12),
        return_params!("3", 15),
        return_params!("4", 18),
        return_params!("5", 21),
        return_params!("6", 24),
        return_params!("7", 27),
        return_params!("8", 30),
    ];
    let tail = [
        fader(P_FADER as u32),
        send_level(
            P_MONITOR_LEVEL as u32,
            "monitor.level",
            "Control room level",
        ),
        toggle(P_MONITOR_DIM as u32, "monitor.dim", "Dim"),
        ParamDescriptor {
            discrete: true,
            ..param(
                P_MONITOR_SOURCE as u32,
                "monitor.source",
                "Monitor source",
                0.0,
                SRC_LAST as f32,
                0.0,
                Taper::Linear,
                Unit::Choice(MONITOR_SOURCES),
            )
        },
    ];
    let sends = [
        send_level(P_SEND_FIRST as u32, "send1.level", "Send 1 level"),
        send_level(P_SEND_FIRST as u32 + 1, "send2.level", "Send 2 level"),
        send_level(P_SEND_FIRST as u32 + 2, "send3.level", "Send 3 level"),
        send_level(P_SEND_FIRST as u32 + 3, "send4.level", "Send 4 level"),
        send_level(P_SEND_FIRST as u32 + 4, "send5.level", "Send 5 level"),
        send_level(P_SEND_FIRST as u32 + 5, "send6.level", "Send 6 level"),
        send_level(P_SEND_FIRST as u32 + 6, "send7.level", "Send 7 level"),
        send_level(P_SEND_FIRST as u32 + 7, "send8.level", "Send 8 level"),
    ];
    let mut out = [tail[0]; PARAM_COUNT];
    let mut i = 0;
    while i < P_RETURN_FIRST {
        out[i] = head[i];
        i += 1;
    }
    while i < P_FADER {
        out[i] = returns[(i - P_RETURN_FIRST) / 3][(i - P_RETURN_FIRST) % 3];
        i += 1;
    }
    while i < P_SEND_FIRST {
        out[i] = tail[i - P_FADER];
        i += 1;
    }
    out[P_TAP_LIVE] = toggle(P_TAP_LIVE as u32, "internal.tap_live", "Analyser tap");
    while i < P_TAP_LIVE {
        out[i] = sends[i - P_SEND_FIRST];
        i += 1;
    }
    out
};

const fn sum_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Audio, Direction::SummingInput)
}
const fn audio_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Audio, Direction::Input)
}
const fn audio_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Audio, Direction::Output)
}

macro_rules! pair {
    ($f:ident, $pre:literal, $k:literal, $base:expr, $name:literal) => {
        [
            $f(
                $base,
                concat!($pre, $k, "_l"),
                concat!($name, " ", $k, " L"),
            ),
            $f(
                $base + 1,
                concat!($pre, $k, "_r"),
                concat!($name, " ", $k, " R"),
            ),
        ]
    };
}

macro_rules! eight {
    ($f:ident, $pre:literal, $base:expr, $name:literal) => {
        [
            pair!($f, $pre, "1", $base, $name),
            pair!($f, $pre, "2", $base + 2, $name),
            pair!($f, $pre, "3", $base + 4, $name),
            pair!($f, $pre, "4", $base + 6, $name),
            pair!($f, $pre, "5", $base + 8, $name),
            pair!($f, $pre, "6", $base + 10, $name),
            pair!($f, $pre, "7", $base + 12, $name),
            pair!($f, $pre, "8", $base + 14, $name),
        ]
    };
}

const S_BUS_L: u16 = 0;
const S_BUS_R: u16 = 1;
const S_SEND_BUS_FIRST: u16 = 2;
const S_FX_SEND_FIRST: u16 = S_SEND_BUS_FIRST + 2 * RETURNS as u16;
const S_RETURN_FIRST: u16 = S_FX_SEND_FIRST + 2 * RETURNS as u16;
const S_INSERT_SEND_L: u16 = S_RETURN_FIRST + 2 * RETURNS as u16;
const S_INSERT_SEND_R: u16 = S_INSERT_SEND_L + 1;
const S_INSERT_RETURN_L: u16 = S_INSERT_SEND_L + 2;
const S_INSERT_RETURN_R: u16 = S_INSERT_SEND_L + 3;
const S_KEY: u16 = S_INSERT_SEND_L + 4;
const S_MAIN_L: u16 = S_KEY + 1;
const S_MAIN_R: u16 = S_KEY + 2;
const S_MONITOR_L: u16 = S_KEY + 3;
const S_MONITOR_R: u16 = S_KEY + 4;
const S_GR: u16 = S_KEY + 5;
const SOCKET_COUNT: usize = S_GR as usize + 1;

static SOCKETS: [SocketDescriptor; SOCKET_COUNT] = {
    let bus = [
        sum_in(S_BUS_L, "bus_l", "Bus L"),
        sum_in(S_BUS_R, "bus_r", "Bus R"),
    ];
    let send_bus = eight!(sum_in, "send_bus", 2, "Send bus");
    let fx_send = eight!(audio_out, "fx_send", 18, "FX send");
    let returns = eight!(audio_in, "return", 34, "Return");
    let tail = [
        audio_out(S_INSERT_SEND_L, "insert_send_l", "Insert send L"),
        audio_out(S_INSERT_SEND_R, "insert_send_r", "Insert send R"),
        audio_in(S_INSERT_RETURN_L, "insert_return_l", "Insert return L"),
        audio_in(S_INSERT_RETURN_R, "insert_return_r", "Insert return R"),
        audio_in(S_KEY, "key_in", "Key in"),
        audio_out(S_MAIN_L, "main_l", "Main L"),
        audio_out(S_MAIN_R, "main_r", "Main R"),
        audio_out(S_MONITOR_L, "monitor_l", "Monitor L"),
        audio_out(S_MONITOR_R, "monitor_r", "Monitor R"),
        socket(
            S_GR,
            "gr_cv_out",
            "Gain reduction CV",
            SocketKind::Cv,
            Direction::Output,
        ),
    ];
    let mut out = [tail[0]; SOCKET_COUNT];
    out[0] = bus[0];
    out[1] = bus[1];
    let mut i = 0;
    while i < 2 * RETURNS {
        out[S_SEND_BUS_FIRST as usize + i] = send_bus[i / 2][i % 2];
        out[S_FX_SEND_FIRST as usize + i] = fx_send[i / 2][i % 2];
        out[S_RETURN_FIRST as usize + i] = returns[i / 2][i % 2];
        i += 1;
    }
    let mut j = 0;
    while j < tail.len() {
        out[S_INSERT_SEND_L as usize + j] = tail[j];
        j += 1;
    }
    out
};

/// Processing stage of each socket: stage 0 = send buses → FX sends, stage 1 = bus +
/// returns (and `key_in`) → insert send, stage 2 = insert return → main/monitor.
static STAGES: [u8; SOCKET_COUNT] = {
    let mut st = [2u8; SOCKET_COUNT];
    let mut i = 0;
    while i < 2 * RETURNS {
        st[S_SEND_BUS_FIRST as usize + i] = 0;
        st[S_FX_SEND_FIRST as usize + i] = 0;
        st[S_RETURN_FIRST as usize + i] = 1;
        i += 1;
    }
    st[S_BUS_L as usize] = 1;
    st[S_BUS_R as usize] = 1;
    st[S_INSERT_SEND_L as usize] = 1;
    st[S_INSERT_SEND_R as usize] = 1;
    st[S_KEY as usize] = 1;
    st
};

/// Static description of the master section.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &STAGES,
    type_id: "atomik.master",
    name: "Console Master",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Master`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Master::new()),
    }
}

/// The master section device.
#[derive(Debug, Clone)]
pub struct Master {
    values: [f32; PARAM_COUNT],
    ret_l: [Smoother; RETURNS],
    ret_r: [Smoother; RETURNS],
    comp: Compressor,
    comp_on: Section,
    makeup: Smoother,
    insert: Smoother,
    fader: Smoother,
    monitor: Smoother,
    /// `send{k}.level` gain, both sides of bus `k`.
    send: [Smoother; RETURNS],
    /// 0 = Main, 1 = the tap `sel` (the control-room source blend).
    source_blend: Smoother,
    /// Crossfade between two taps: 1 = still the previous tap `prev_sel`, 0 = the tap `sel`.
    source_swap: Smoother,
    /// The last non-Main `monitor.source` index (starts at *FX returns*).
    sel: usize,
    /// The tap that was selected before `sel` (while `source_swap` ramps).
    prev_sel: usize,
    /// Stage-0 taps: `fx_send{k}` after the send level.
    send_tap: [(Block, Block); RETURNS],
    /// Stage-1 taps: return `k` after its fader and pan (zero when not connected).
    ret_tap: [(Block, Block); RETURNS],
    /// Stage-1 → stage-2 hand-over: the signal at the insert point.
    mid: (Block, Block),
    /// The returns sum (pre-fader), for `monitor.source` = FX returns.
    rets: (Block, Block),
    /// `key_in` copied in stage 1 (stage 2 runs the compressor with `insert.pre_comp`).
    key: Block,
    key_active: bool,
    /// Insert routing latched in stage 1 for stage 2.
    pre_comp: bool,
    gr: Block,
    /// Set by `prepare`/`reset`: the next stage 0 snaps every smoother to its target.
    fresh: bool,
    /// Like `fresh`, for the insert crossfade: its target (return connected or not) is only
    /// known in stage 2, so the first insert return after `prepare`/`reset` snaps it there.
    fresh_insert: bool,
    /// The analyser ring (allocated in `prepare`): written with the signal before the fader
    /// while `internal.tap_live` is on.
    tap: Option<Arc<ScopeTap>>,
}

impl Default for Master {
    fn default() -> Self {
        Self::new()
    }
}

impl Master {
    /// A master section with default params (call `prepare` before processing).
    pub fn new() -> Self {
        let sr = 48_000.0;
        let mut values = [0.0; PARAM_COUNT];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        let sm = smoother(0.0, sr);
        let mut comp = Compressor::new(sr);
        comp.set_detector(Detector::Rms);
        let mut m = Self {
            values,
            ret_l: [sm; RETURNS],
            ret_r: [sm; RETURNS],
            comp,
            comp_on: Section::new(sr),
            makeup: sm,
            insert: sm,
            fader: sm,
            monitor: sm,
            send: [smoother(1.0, sr); RETURNS],
            source_blend: sm,
            source_swap: sm,
            sel: SRC_RETURNS,
            prev_sel: SRC_RETURNS,
            send_tap: [([0.0; BATCH], [0.0; BATCH]); RETURNS],
            ret_tap: [([0.0; BATCH], [0.0; BATCH]); RETURNS],
            mid: ([0.0; BATCH], [0.0; BATCH]),
            rets: ([0.0; BATCH], [0.0; BATCH]),
            key: [0.0; BATCH],
            key_active: false,
            pre_comp: false,
            gr: [0.0; BATCH],
            fresh: true,
            fresh_insert: true,
            tap: None,
        };
        for i in 0..PARAM_COUNT {
            m.apply(i);
        }
        m.reset();
        m
    }

    fn on(&self, i: usize) -> bool {
        self.values[i] >= 0.5
    }

    fn apply(&mut self, i: usize) {
        let v = self.values[i];
        match i {
            P_COMP_ON => self.comp_on.set_on(self.on(i)),
            P_COMP_THRESHOLD => self.comp.set_threshold_db(v),
            P_COMP_RATIO => self.comp.set_ratio(v),
            P_COMP_ATTACK => self.comp.set_attack_ms(v),
            P_COMP_RELEASE => self.comp.set_release_ms(v),
            P_COMP_MAKEUP => self.makeup.set_target(db_to_gain(v)),
            _ if (P_RETURN_FIRST..P_FADER).contains(&i) => {
                let k = (i - P_RETURN_FIRST) / 3;
                let base = P_RETURN_FIRST + 3 * k;
                let g = if self.on(base + 1) {
                    0.0
                } else {
                    send_gain(self.values[base])
                };
                let (pl, pr) = pan_constant_power(self.values[base + 2]);
                self.ret_l[k].set_target(g * pl);
                self.ret_r[k].set_target(g * pr);
            }
            P_FADER => self.fader.set_target(fader_gain(v)),
            P_MONITOR_LEVEL | P_MONITOR_DIM => {
                let dim = if self.on(P_MONITOR_DIM) {
                    db_to_gain(DIM_DB)
                } else {
                    1.0
                };
                self.monitor
                    .set_target(send_gain(self.values[P_MONITOR_LEVEL]) * dim);
            }
            P_MONITOR_SOURCE => self.select_source(v),
            _ if (P_SEND_FIRST..P_TAP_LIVE).contains(&i) => {
                self.send[i - P_SEND_FIRST].set_target(send_gain(v));
            }
            // Insert routing and the key switch are read in `process`.
            _ => {}
        }
    }

    /// Applies `monitor.source`: Main fades the tap out, any other choice fades it in; a change
    /// from one tap to another crossfades (so no click) and settles exactly on the new tap.
    fn select_source(&mut self, v: f32) {
        let idx = v.round().clamp(0.0, SRC_LAST as f32) as usize;
        if idx == 0 {
            self.source_blend.set_target(0.0);
            return;
        }
        if idx != self.sel {
            if self.source_blend.target() > 0.5 {
                self.prev_sel = self.sel;
                self.source_swap.set_immediate(1.0);
                self.source_swap.set_target(0.0);
            } else {
                // Coming from Main: the blend itself fades the new tap in.
                self.prev_sel = idx;
            }
            self.sel = idx;
        }
        self.source_blend.set_target(1.0);
    }

    /// The tap of `monitor.source` index `idx` (not Main).
    fn tap(&self, idx: usize) -> (Block, Block) {
        match idx {
            SRC_RETURNS => self.rets,
            SRC_SEND_FIRST..SRC_RETURN_FIRST => self.send_tap[idx - SRC_SEND_FIRST],
            _ => self.ret_tap[idx - SRC_RETURN_FIRST],
        }
    }

    fn smoothers_mut(&mut self) -> impl Iterator<Item = &mut Smoother> {
        [
            &mut self.makeup,
            &mut self.insert,
            &mut self.fader,
            &mut self.monitor,
            &mut self.source_blend,
            &mut self.source_swap,
        ]
        .into_iter()
        .chain(self.ret_l.iter_mut())
        .chain(self.ret_r.iter_mut())
        .chain(self.send.iter_mut())
    }

    fn comp_stage(&mut self, l: &mut Block, r: &mut Block) {
        let key_block = self.key;
        let key = self.key_active.then_some(&key_block);
        match self.comp_on.begin() {
            Step::Skip => return,
            Step::Fresh => self.comp.reset(),
            Step::Active => {}
        }
        for n in 0..BATCH {
            let (sl, sr) = match key {
                Some(k) => (k[n], k[n]),
                None => (l[n], r[n]),
            };
            let g = self.comp.process(sl, sr);
            let m = self.comp_on.next();
            let total = 1.0 + (g * self.makeup.next_value() - 1.0) * m;
            l[n] *= total;
            r[n] *= total;
            self.gr[n] = ((1.0 - g) * m).clamp(0.0, 1.0);
        }
    }

    /// Insert return: crossfades into the return when one is connected (and not bypassed).
    fn insert_return(&mut self, l: &mut Block, r: &mut Block, ret: Option<(&Block, &Block)>) {
        let use_return = ret.is_some() && !self.on(P_INSERT_BYPASS);
        self.insert.set_target(if use_return { 1.0 } else { 0.0 });
        if self.fresh_insert {
            snap(&mut self.insert);
            self.fresh_insert = false;
        }
        if is_silent(&self.insert) {
            return;
        }
        let zero = [0.0; BATCH];
        let (rl, rr) = ret.unwrap_or((&zero, &zero));
        for n in 0..BATCH {
            let m = self.insert.next_value();
            l[n] += (rl[n] - l[n]) * m;
            r[n] += (rr[n] - r[n]) * m;
        }
    }

    /// Stage 0: the send buses, times their level, go to the FX sends.
    fn stage0(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            for s in self.smoothers_mut() {
                snap(s);
            }
            self.comp_on.snap();
            self.fresh = false;
        }
        for k in 0..RETURNS {
            let base = 2 * k as u16;
            let mut l = *ctx.input(SocketId(S_SEND_BUS_FIRST + base));
            let mut r = *ctx.input(SocketId(S_SEND_BUS_FIRST + base + 1));
            let g = &mut self.send[k];
            // Exactly 0 dB (the default) is a plain copy: bit-exact, as before the level existed.
            if !(g.is_settled() && g.value().to_bits() == 1.0f32.to_bits()) {
                for n in 0..BATCH {
                    let gain = g.next_value();
                    l[n] *= gain;
                    r[n] *= gain;
                }
            }
            *ctx.output(SocketId(S_FX_SEND_FIRST + base)) = l;
            *ctx.output(SocketId(S_FX_SEND_FIRST + base + 1)) = r;
            self.send_tap[k] = (l, r);
        }
    }

    /// Stage 1: bus + returns (→ compressor unless `insert.pre_comp`) → `insert_send_*`.
    fn stage1(&mut self, ctx: &mut ProcessContext<'_>) {
        let mut l = *ctx.input(SocketId(S_BUS_L));
        let mut r = *ctx.input(SocketId(S_BUS_R));
        let mut rets_l = [0.0f32; BATCH];
        let mut rets_r = [0.0f32; BATCH];
        for k in 0..RETURNS {
            let base = S_RETURN_FIRST + 2 * k as u16;
            let (a, b, connected) = stereo_in(ctx, base, base + 1);
            let (gl, gr) = (&mut self.ret_l[k], &mut self.ret_r[k]);
            if !connected || (is_silent(gl) && is_silent(gr)) {
                snap(gl);
                snap(gr);
                self.ret_tap[k] = ([0.0; BATCH], [0.0; BATCH]);
                continue;
            }
            let (tl, tr) = &mut self.ret_tap[k];
            for n in 0..BATCH {
                tl[n] = a[n] * gl.next_value();
                tr[n] = b[n] * gr.next_value();
                rets_l[n] += tl[n];
                rets_r[n] += tr[n];
            }
        }
        for n in 0..BATCH {
            l[n] += rets_l[n];
            r[n] += rets_r[n];
        }
        self.rets = (rets_l, rets_r);
        self.key = *ctx.input(SocketId(S_KEY));
        self.key_active = self.on(P_COMP_KEY) && ctx.is_connected(SocketId(S_KEY));
        self.gr = [0.0; BATCH];
        self.pre_comp = self.on(P_INSERT_PRE_COMP);
        if !self.pre_comp {
            self.comp_stage(&mut l, &mut r);
        }
        *ctx.output(SocketId(S_INSERT_SEND_L)) = l;
        *ctx.output(SocketId(S_INSERT_SEND_R)) = r;
        self.mid = (l, r);
    }

    /// Stage 2: insert return (→ compressor with `insert.pre_comp`) → fader → `main_*`,
    /// `monitor_*`, `gr_cv_out`.
    fn stage2(&mut self, ctx: &mut ProcessContext<'_>) {
        let (mut l, mut r) = self.mid;
        let (ins_l, ins_r, ins_on) = stereo_in(ctx, S_INSERT_RETURN_L, S_INSERT_RETURN_R);
        self.insert_return(&mut l, &mut r, ins_on.then_some((&ins_l, &ins_r)));
        if self.pre_comp {
            self.comp_stage(&mut l, &mut r);
        }
        *ctx.output(SocketId(S_GR)) = self.gr;
        if let (true, Some(tap)) = (self.on(P_TAP_LIVE), &self.tap) {
            tap.push(&l, &r);
        }
        let (mut rets_l, mut rets_r) = self.tap(self.sel);
        // Mid-crossfade between two taps; settled, the new tap is used as it is (bit-exact).
        let crossfading =
            !(self.source_swap.is_settled() && self.source_swap.value().to_bits() == 0);
        let (old_l, old_r) = if crossfading {
            self.tap(self.prev_sel)
        } else {
            (rets_l, rets_r)
        };
        let mut mon_l = [0.0f32; BATCH];
        let mut mon_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let f = self.fader.next_value();
            l[n] *= f;
            r[n] *= f;
            let s = self.source_blend.next_value();
            let w = self.source_swap.next_value();
            let g = self.monitor.next_value();
            if crossfading {
                rets_l[n] += (old_l[n] - rets_l[n]) * w;
                rets_r[n] += (old_r[n] - rets_r[n]) * w;
            }
            mon_l[n] = (l[n] + (rets_l[n] - l[n]) * s) * g;
            mon_r[n] = (r[n] + (rets_r[n] - r[n]) * s) * g;
        }
        *ctx.output(SocketId(S_MAIN_L)) = l;
        *ctx.output(SocketId(S_MAIN_R)) = r;
        *ctx.output(SocketId(S_MONITOR_L)) = mon_l;
        *ctx.output(SocketId(S_MONITOR_R)) = mon_r;
    }
}

impl Device for Master {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        if self.tap.is_none() {
            self.tap = Some(Arc::new(ScopeTap::new()));
        }
        for s in self.smoothers_mut() {
            s.set_time(SMOOTH_MS, sr);
        }
        self.comp_on.set_time(sr);
        self.comp.set_sample_rate(sr);
        for i in 0..PARAM_COUNT {
            self.apply(i);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for s in self.smoothers_mut() {
            snap(s);
        }
        self.comp_on.snap();
        self.comp.reset();
        self.fresh = true;
        self.fresh_insert = true;
        self.mid = ([0.0; BATCH], [0.0; BATCH]);
        self.rets = ([0.0; BATCH], [0.0; BATCH]);
        self.send_tap = [([0.0; BATCH], [0.0; BATCH]); RETURNS];
        self.ret_tap = [([0.0; BATCH], [0.0; BATCH]); RETURNS];
        self.gr = [0.0; BATCH];
        self.key = [0.0; BATCH];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i < PARAM_COUNT {
            self.values[i] = plain;
            self.apply(i);
        }
    }

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        self.tap.clone()
    }

    /// Runs all three stages (for hosts that do not split stages, e.g. the test harness).
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.stage0(ctx);
        self.stage1(ctx);
        self.stage2(ctx);
    }

    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        match stage {
            0 => self.stage0(ctx),
            1 => self.stage1(ctx),
            2 => self.stage2(ctx),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
