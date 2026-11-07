//! `atomik.duck`, the Sidechain Tool: pumping and ducking, from the song position, from a key
//! signal's hits, or from the key's level, with a CV output for ducking other things.
//!
//! The audio is multiplied by `gain = 1 - depth * dip`, where `dip` (0..1) is a curve that falls
//! to 1 over `attack` milliseconds and recovers to 0 over `time` milliseconds:
//!
//! ```text
//! t <  attack            dip = t / attack
//! t >= attack            dip = (1 - (t - attack) / time) ^ curve     (0 once t - attack >= time)
//! ```
//!
//! `curve` (0.25..4) bends the recovery: 1 is a straight line back, above 1 it hangs low and
//! comes back late, below 1 it comes back at once. What starts the curve depends on `mode`:
//!
//! * **pump.** The curve restarts every `rate` of the song (a half, quarter, eighth or sixteenth
//!   note): `t` is the time since the last multiple of the rate, computed from the transport
//!   position of the batch (`TimeInfo::tick`, plus the loop wrap), so it needs no history: playing
//!   from a locate gives the same gain as playing through to it, and a bounce is repeatable. The
//!   gain is 1 while the transport is stopped. No key is read.
//! * **trigger.** A hit on the key restarts the curve. The key is `sc_in_l/r` when connected,
//!   else the main input; its peak (instant rise, 10 ms fall) crossing `threshold` triggers, and
//!   it re-arms 6 dB below. A hit during a curve continues from the current dip instead of
//!   jumping, so retriggering does not click.
//! * **classic.** `dip` follows the key's level: the peak follower with `attack` and `time` as its
//!   rise and fall times, `dip = (clamp((level_dB - threshold) / 12, 0, 1)) ^ curve` (12 dB above
//!   the threshold ducks the full depth).
//!
//! * **curve.** A hit on the key (as in trigger mode) or any note-on at `notes_in`, as `trigger`
//!   says (the key, the notes, or either), starts a
//!   hand-drawn curve over `length` (a sixteenth, an eighth, a quarter or a half note, or one bar
//!   of the song's meter, in song time, so it follows tempo changes). The curve is up to eight
//!   points with bends ([`atomik_dsp::duck_curve`]) kept in hidden params (`hidden.curve_n`, then
//!   `hidden.curve_pN_x`, `_y` and `_bend`) and read from the stored points as the song plays; with
//!   no points it is a built-in fast fall and smooth recovery. A hit during a curve enters the new
//!   one where it first reaches the dip that is playing, so a double kick never lifts the gain; a
//!   curve that never gets that high fades the difference out over the rest of its length. After
//!   its end the curve holds its last value until the next hit. With `trigger` on notes the key is
//!   not read at all (the ducked signal never triggers itself).
//! * **band.** With `band` on only the part of the signal under `band_freq` is ducked: the signal
//!   is split into a low and a high band by a Linkwitz-Riley pair (two cascaded Butterworth
//!   sections each, 24 dB per octave), the low band is multiplied by `1 - duck` and the bands are
//!   added again. Added back unducked the pair is an allpass: the level is the input's at every
//!   frequency and only the phase turns around `band_freq`, so with nothing ducking the output is
//!   a phase-shifted copy of the input, not the input bit for bit (with `band` off it is). A tone
//!   several octaves above the split is not touched by the ducking: the difference between
//!   ducked and unducked is the leak of the low-pass (under -80 dB at five octaves).
//! * **send.** With `send` on the audio passes untouched and only `duck_cv_out` moves, to duck
//!   something else (cable it to another device's level CV with a negative trim).
//! * **`duck_cv_out`** is `depth * dip` (= `1 - gain`): 0 at rest, up to `depth` when fully ducked.
//! * `depth` is smoothed per batch (10 ms). The meter values are the dip (0 to 1) and the position
//!   along the curve in curve mode (1 when idle or in another mode). The maths is the deterministic `log2_det` /
//!   `exp2_det`. No latency. Non-finite input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{
    audio_in, audio_out, choice, count, cv_out, expo, finite, linear, sock, toggle,
};
use atomik_core::{BATCH, NoteKind, PPQ, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::duck_curve::{COUNT_KEY, Curve, MAX_POINTS, POINT_KEYS, Point, points_from_raw};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, exp2_det, exp2_det_f64, log2_det,
};

const P_MODE: u32 = 0;
const P_RATE: u32 = 1;
const P_DEPTH: u32 = 2;
const P_ATTACK: u32 = 3;
const P_TIME: u32 = 4;
const P_CURVE: u32 = 5;
const P_THRESHOLD: u32 = 6;
const P_SEND: u32 = 7;
const P_LENGTH: u32 = 8;
const P_TRIGGER: u32 = 9;
const P_BAND: u32 = 10;
const P_BAND_FREQ: u32 = 11;
const P_BYPASS: u32 = 12;
/// The hidden param that holds the number of points of the drawn curve.
const P_CURVE_N: u32 = 13;
/// The first of the hidden point params (`x`, `y`, `bend` for each point in turn).
const P_POINTS: u32 = 14;

static MODES: [&str; 4] = ["pump", "trigger", "classic", "curve"];
static RATES: [&str; 4] = ["1/2", "1/4", "1/8", "1/16"];
static TRIGGERS: [&str; 3] = ["key", "notes", "key or notes"];
static LENGTHS: [&str; 5] = ["1/16", "1/8", "1/4", "1/2", "1 bar"];

/// Length of one pump cycle in ticks for each entry of [`RATES`].
const RATE_TICKS: [f64; 4] = [
    2.0 * PPQ as f64,
    PPQ as f64,
    PPQ as f64 / 2.0,
    PPQ as f64 / 4.0,
];

/// Length of the curve in ticks for each entry of [`LENGTHS`] but the last (one bar follows the
/// meter of the song).
const LENGTH_TICKS: [f64; 4] = [
    PPQ as f64 / 4.0,
    PPQ as f64 / 2.0,
    PPQ as f64,
    2.0 * PPQ as f64,
];
/// The length of the drawn curve in ticks for the `length` param value `length` (0 to 4) in a
/// song whose bar is `bar_ticks` long.
pub fn curve_length_ticks(length: u8, bar_ticks: f64) -> f64 {
    match length {
        0..=3 => LENGTH_TICKS[usize::from(length)],
        _ => bar_ticks,
    }
}

/// The Butterworth Q of each section of the band split.
const BAND_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// The longest a curve may keep counting (in curve lengths), so the count stays small.
const MAX_ELAPSED: f64 = 4.0;
/// The shortest span (share of the curve) over which a curve that starts too low fades out the
/// difference to the dip it replaces.
const MIN_FADE_SPAN: f64 = 0.05;

/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`: dB per octave of amplitude.
const DB_PER_LOG2: f32 = 6.020_6;
/// The key follower's fall in trigger mode, ms.
const TRIGGER_FALL_MS: f32 = 10.0;
/// The level range above the threshold that ducks the full depth in classic mode, dB.
const CLASSIC_RANGE_DB: f32 = 12.0;
/// A trigger re-arms once the key is this many dB under the threshold.
const REARM_DB: f32 = 6.0;

/// The three hidden rows of curve point `$n` (0-based): `x`, `y` and the bend of its segment.
macro_rules! point_rows {
    ($($n:literal),*) => {
        [$(
            linear(
                P_POINTS + 3 * $n,
                POINT_KEYS[$n][0],
                "Curve point x",
                0.0,
                1.0,
                0.0,
                Unit::None,
            ),
            linear(
                P_POINTS + 3 * $n + 1,
                POINT_KEYS[$n][1],
                "Curve point y",
                0.0,
                1.0,
                0.0,
                Unit::None,
            ),
            count(
                P_POINTS + 3 * $n + 2,
                POINT_KEYS[$n][2],
                "Curve point bend",
                -64.0,
                64.0,
                0.0,
            ),
        )*]
    };
}

const POINT_ROWS: [ParamDescriptor; 3 * MAX_POINTS] = point_rows!(0, 1, 2, 3, 4, 5, 6, 7);

const PARAM_COUNT: usize = P_POINTS as usize + 3 * MAX_POINTS;

const fn params() -> [ParamDescriptor; PARAM_COUNT] {
    let head = [
        choice(P_MODE, "mode", "Mode", &MODES, 0.0),
        choice(P_RATE, "rate", "Rate", &RATES, 1.0),
        linear(P_DEPTH, "depth", "Depth", 0.0, 1.0, 0.8, Unit::Percent),
        expo(P_ATTACK, "attack", "Attack", 0.1, 50.0, 2.0, Unit::Ms),
        expo(P_TIME, "time", "Time", 10.0, 2_000.0, 250.0, Unit::Ms),
        expo(P_CURVE, "curve", "Curve", 0.25, 4.0, 1.0, Unit::None),
        linear(
            P_THRESHOLD,
            "threshold",
            "Threshold",
            -60.0,
            0.0,
            -30.0,
            Unit::Db,
        ),
        toggle(P_SEND, "send", "Send (CV only)", 0.0),
        choice(P_LENGTH, "length", "Length", &LENGTHS, 2.0),
        choice(P_TRIGGER, "trigger", "Curve trigger", &TRIGGERS, 2.0),
        toggle(P_BAND, "band", "Band only", 0.0),
        expo(
            P_BAND_FREQ,
            "band_freq",
            "Band freq",
            30.0,
            600.0,
            120.0,
            Unit::Hz,
        ),
        bypass_param(P_BYPASS),
        count(
            P_CURVE_N,
            COUNT_KEY,
            "Curve points",
            0.0,
            MAX_POINTS as f32,
            0.0,
        ),
    ];
    let mut out = [head[0]; PARAM_COUNT];
    let mut i = 0;
    while i < head.len() {
        out[i] = head[i];
        i += 1;
    }
    let mut j = 0;
    while j < POINT_ROWS.len() {
        out[head.len() + j] = POINT_ROWS[j];
        j += 1;
    }
    out
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] = params();

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SC_L: SocketId = SocketId(2);
const SC_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const DUCK_CV: SocketId = SocketId(6);
const NOTES: SocketId = SocketId(7);

static SOCKETS: [SocketDescriptor; 8] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "sc_in_l", "Sidechain L"),
    audio_in(3, "sc_in_r", "Sidechain R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_out(6, "duck_cv_out", "Duck CV"),
    sock(
        7,
        "notes_in",
        "Notes in (curve trigger)",
        SocketKind::Notes,
        Direction::Input,
    ),
];

/// Static description of the sidechain tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.duck",
    name: "Sidechain Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Duck`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Duck::new())),
    }
}

/// The dip (0..1) `t_ms` after the start of a curve: a fall over `attack_ms`, then a recovery
/// over `time_ms` bent by `curve`.
#[inline]
pub fn dip_at(t_ms: f64, attack_ms: f32, time_ms: f32, curve: f32) -> f32 {
    if t_ms < 0.0 {
        return 0.0;
    }
    let t = t_ms as f32;
    if t < attack_ms {
        return t / attack_ms;
    }
    let y = (t - attack_ms) / time_ms;
    if y >= 1.0 {
        0.0
    } else {
        exp2_det(curve * log2_det(1.0 - y))
    }
}

/// The curve time at which the fall has reached `dip` (a retrigger continues from there).
#[inline]
fn fall_time_for(dip: f32, attack_ms: f32) -> f32 {
    dip.clamp(0.0, 1.0) * attack_ms
}

/// The duck device.
#[derive(Debug, Clone)]
pub struct Duck {
    sample_rate: f32,
    mode: u8,
    rate: u8,
    depth: f32,
    attack_ms: f32,
    time_ms: f32,
    curve: f32,
    threshold_db: f32,
    send: bool,
    length: u8,
    trigger: u8,
    band: bool,
    band_freq: f32,
    /// The stored curve: how many points and their `x`, `y`, bend.
    curve_n: f32,
    curve_raw: [[f32; 3]; MAX_POINTS],
    // Derived.
    thr_lin: f32,
    fall_alpha: f64,
    attack_alpha: f64,
    release_alpha: f64,
    shape: Curve,
    shape_dirty: bool,
    low_coeffs: BiquadCoeffs,
    high_coeffs: BiquadCoeffs,
    // State.
    fresh: bool,
    depth_sm: BlockSmoother,
    /// Peak follower of the key.
    env: f32,
    armed: bool,
    /// Samples since the current curve began (trigger mode); `None` before the first hit.
    since: Option<u64>,
    /// Offset in ms added to `since` (a retrigger that continues from a partial dip).
    since_base_ms: f32,
    /// The curve mode: a curve is running (it holds its last value after its end).
    curve_active: bool,
    /// Where the running curve was entered (0 to 1).
    curve_start: f64,
    /// Ticks since the running curve was triggered.
    curve_elapsed: f64,
    /// The dip still to fade out after a retrigger into a curve that does not reach the old dip.
    curve_carry: f32,
    /// The dip the curve gave at the last sample.
    curve_dip: f32,
    /// The band split's low-pass and high-pass sections, per channel: `[channel][section]`.
    low: [[Biquad; 2]; 2],
    high: [[Biquad; 2]; 2],
    meters: [f32; METERS],
}

/// The meter values: the dip and the position along the curve.
pub const METERS: usize = 2;
/// Indices into the meter values.
pub mod meter {
    /// The dip (0 to 1) at the end of the batch.
    pub const DIP: usize = 0;
    /// The position along the curve (0 to 1; 1 when idle or not in curve mode).
    pub const POSITION: usize = 1;
}

impl Default for Duck {
    fn default() -> Self {
        Self::new()
    }
}

impl Duck {
    /// A sidechain tool with default params.
    pub fn new() -> Self {
        let mut d = Self {
            sample_rate: 48_000.0,
            mode: 0,
            rate: 1,
            depth: 0.8,
            attack_ms: 2.0,
            time_ms: 250.0,
            curve: 1.0,
            threshold_db: -30.0,
            send: false,
            length: 2,
            trigger: 2,
            band: false,
            band_freq: 120.0,
            curve_n: 0.0,
            curve_raw: [[0.0; 3]; MAX_POINTS],
            thr_lin: 0.0,
            fall_alpha: 0.0,
            attack_alpha: 0.0,
            release_alpha: 0.0,
            shape: Curve::default(),
            shape_dirty: false,
            low_coeffs: BiquadCoeffs::IDENTITY,
            high_coeffs: BiquadCoeffs::IDENTITY,
            fresh: true,
            depth_sm: BlockSmoother::new(0.8),
            env: 0.0,
            armed: true,
            since: None,
            since_base_ms: 0.0,
            curve_active: false,
            curve_start: 0.0,
            curve_elapsed: 0.0,
            curve_carry: 0.0,
            curve_dip: 0.0,
            low: [[Biquad::default(); 2]; 2],
            high: [[Biquad::default(); 2]; 2],
            meters: [0.0, 1.0],
        };
        d.derive();
        d
    }

    fn alpha(&self, ms: f32) -> f64 {
        let n = f64::from(ms) * 0.001 * f64::from(self.sample_rate);
        if n > 0.0 {
            exp2_det_f64(-std::f64::consts::LOG2_E / n)
        } else {
            0.0
        }
    }

    fn derive(&mut self) {
        self.thr_lin = exp2_det(self.threshold_db * LOG2_10_20);
        self.fall_alpha = self.alpha(TRIGGER_FALL_MS);
        self.attack_alpha = self.alpha(self.attack_ms);
        self.release_alpha = self.alpha(self.time_ms);
        let sr = f64::from(self.sample_rate);
        let freq = f64::from(self.band_freq).min(sr * 0.45);
        self.low_coeffs = BiquadCoeffs::design(BiquadShape::Lowpass, freq, BAND_Q, 0.0, sr);
        self.high_coeffs = BiquadCoeffs::design(BiquadShape::Highpass, freq, BAND_Q, 0.0, sr);
    }

    /// Milliseconds since the current trigger curve began (`None` before any hit).
    fn trigger_ms(&self) -> Option<f64> {
        self.since.map(|s| {
            f64::from(self.since_base_ms) + s as f64 * 1000.0 / f64::from(self.sample_rate)
        })
    }

    /// Rebuilds the curve from the stored points.
    fn rebuild_shape(&mut self) {
        let mut points = [Point::new(0.0, 0.0, 0); MAX_POINTS];
        let n = points_from_raw(self.curve_n, &self.curve_raw, &mut points);
        self.shape.set(&points[..n]);
        self.shape_dirty = false;
    }

    /// Whether the key crossed the threshold at this sample: the follower of `k` (instant rise,
    /// 10 ms fall) rising through it while armed. It re-arms 6 dB below.
    fn key_hit(&mut self, k: f32) -> bool {
        self.env = k.max((f64::from(self.env) * self.fall_alpha) as f32);
        if self.env < 1e-20 {
            self.env = 0.0;
        }
        if self.armed && self.env >= self.thr_lin {
            self.armed = false;
            true
        } else {
            if !self.armed && self.env < self.thr_lin * exp2_det(-REARM_DB * LOG2_10_20) {
                self.armed = true;
            }
            false
        }
    }

    /// Starts the drawn curve: from its start, or from where it first reaches the dip that is
    /// playing.
    fn curve_trigger(&mut self) {
        let playing = if self.curve_active {
            self.curve_dip
        } else {
            0.0
        };
        let (start, short) = self.shape.first_reach(playing);
        self.curve_start = start;
        self.curve_carry = short;
        self.curve_elapsed = 0.0;
        self.curve_active = true;
    }

    /// The dip of the drawn curve now and the position along it; advances the curve by `tpf`
    /// ticks of a curve `len` ticks long.
    fn curve_step(&mut self, tpf: f64, len: f64) -> (f32, f64) {
        if !self.curve_active {
            return (0.0, 1.0);
        }
        let pos = self.curve_start + self.curve_elapsed / len;
        let mut dip = self.shape.read(pos);
        if self.curve_carry > 0.0 {
            let span = (1.0 - self.curve_start).max(MIN_FADE_SPAN);
            let gone = ((pos - self.curve_start) / span).clamp(0.0, 1.0) as f32;
            dip = (dip + self.curve_carry * (1.0 - gone)).min(1.0);
        }
        self.curve_dip = dip;
        self.curve_elapsed = (self.curve_elapsed + tpf).min(len * MAX_ELAPSED);
        (dip, pos.min(1.0))
    }
}

impl Device for Duck {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.depth_sm.set_time(10.0, self.sample_rate, BATCH);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.armed = true;
        self.since = None;
        self.since_base_ms = 0.0;
        self.curve_active = false;
        self.curve_start = 0.0;
        self.curve_elapsed = 0.0;
        self.curve_carry = 0.0;
        self.curve_dip = 0.0;
        self.low = [[Biquad::default(); 2]; 2];
        self.high = [[Biquad::default(); 2]; 2];
        self.meters = [0.0, 1.0];
        self.shape_dirty = true;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 3.0).round() as u8,
            P_RATE => self.rate = v.clamp(0.0, 3.0).round() as u8,
            P_DEPTH => self.depth = v.clamp(0.0, 1.0),
            P_ATTACK => self.attack_ms = v.clamp(0.1, 50.0),
            P_TIME => self.time_ms = v.clamp(10.0, 2_000.0),
            P_CURVE => self.curve = v.clamp(0.25, 4.0),
            P_THRESHOLD => self.threshold_db = v.clamp(-60.0, 0.0),
            P_SEND => self.send = v >= 0.5,
            P_LENGTH => self.length = v.clamp(0.0, 4.0).round() as u8,
            P_TRIGGER => self.trigger = v.clamp(0.0, 2.0).round() as u8,
            P_BAND => {
                let on = v >= 0.5;
                if on && !self.band {
                    self.low = [[Biquad::default(); 2]; 2];
                    self.high = [[Biquad::default(); 2]; 2];
                }
                self.band = on;
            }
            P_BAND_FREQ => self.band_freq = v.clamp(30.0, 600.0),
            P_CURVE_N => {
                self.curve_n = v.clamp(0.0, MAX_POINTS as f32).round();
                self.shape_dirty = true;
                return;
            }
            n if (P_POINTS..P_POINTS + 3 * MAX_POINTS as u32).contains(&n) => {
                let at = (n - P_POINTS) as usize;
                self.curve_raw[at / 3][at % 3] = v;
                self.shape_dirty = true;
                return;
            }
            _ => return,
        }
        self.derive();
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let (sc_l_conn, sc_r_conn) = (ctx.is_connected(SC_L), ctx.is_connected(SC_R));
        let sc_l = *ctx.input(SC_L);
        let sc_r = *ctx.input(SC_R);
        let time = ctx.time;
        if self.fresh {
            self.depth_sm.set_immediate(self.depth);
            self.fresh = false;
        }
        if self.shape_dirty {
            self.rebuild_shape();
        }
        let depth = self.depth_sm.advance(self.depth, BATCH);

        let cycle = RATE_TICKS[usize::from(self.rate)];
        let tpf = time.ticks_per_frame.max(1e-12);
        let sr = f64::from(self.sample_rate);
        let (num, den) = time.time_signature;
        let bar = f64::from(num.max(1)) * 4.0 * PPQ as f64 / f64::from(den.max(1));
        let curve_len = curve_length_ticks(self.length, bar);
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        let mut position = 1.0f64;
        let mut next_event = 0usize;
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            let dip = match self.mode {
                0 => {
                    if time.playing {
                        let tick = match time.loop_wrap {
                            Some(w) if n >= usize::from(w.offset) => {
                                w.to_tick + (n - usize::from(w.offset)) as f64 * tpf
                            }
                            _ => time.tick + n as f64 * tpf,
                        };
                        let phase = tick.rem_euclid(cycle);
                        let t_ms = phase / tpf / sr * 1000.0;
                        dip_at(t_ms, self.attack_ms, self.time_ms, self.curve)
                    } else {
                        0.0
                    }
                }
                _ => {
                    // The key: a connected sidechain (a lone side feeds both), else the input.
                    let k = if sc_l_conn || sc_r_conn {
                        let (a, b) = (finite(sc_l[n]), finite(sc_r[n]));
                        match (sc_l_conn, sc_r_conn) {
                            (true, false) => a.abs(),
                            (false, true) => b.abs(),
                            _ => a.abs().max(b.abs()),
                        }
                    } else if self.mode == 3 && self.trigger == 1 {
                        // Notes only: the ducked signal must not trigger itself.
                        0.0
                    } else {
                        x[0].abs().max(x[1].abs())
                    };
                    if self.mode == 1 {
                        if self.key_hit(k) {
                            let current = self.trigger_ms().map_or(0.0, |t| {
                                dip_at(t, self.attack_ms, self.time_ms, self.curve)
                            });
                            self.since = Some(0);
                            self.since_base_ms = fall_time_for(current, self.attack_ms);
                        }
                        let dip = self
                            .trigger_ms()
                            .map_or(0.0, |t| dip_at(t, self.attack_ms, self.time_ms, self.curve));
                        if let Some(s) = self.since.as_mut() {
                            *s = s.saturating_add(1);
                        }
                        dip
                    } else if self.mode == 3 {
                        let mut hit = self.key_hit(k) && self.trigger != 1;
                        let events = ctx.notes_in(NOTES);
                        while next_event < events.len()
                            && usize::from(events[next_event].offset) <= n
                        {
                            if self.trigger != 0
                                && matches!(events[next_event].kind, NoteKind::On { .. })
                            {
                                hit = true;
                            }
                            next_event += 1;
                        }
                        if hit {
                            self.curve_trigger();
                        }
                        let (dip, pos) = self.curve_step(tpf, curve_len);
                        position = pos;
                        dip
                    } else {
                        let a = if k > self.env {
                            self.attack_alpha
                        } else {
                            self.release_alpha
                        };
                        self.env = (f64::from(k) + a * (f64::from(self.env) - f64::from(k))) as f32;
                        if self.env < 1e-20 {
                            self.env = 0.0;
                        }
                        let db = DB_PER_LOG2 * log2_det(self.env);
                        let over = ((db - self.threshold_db) / CLASSIC_RANGE_DB).clamp(0.0, 1.0);
                        if over <= 0.0 {
                            0.0
                        } else {
                            exp2_det(self.curve * log2_det(over))
                        }
                    }
                }
            };
            let duck = depth.at(n) * dip;
            cv[n] = duck;
            if self.send {
                out_l[n] = x[0];
                out_r[n] = x[1];
            } else if self.band {
                // Only the low band is ducked; the bands add up to an allpass of the input.
                let mut y = [0.0f32; 2];
                for (ch, v) in x.iter().enumerate() {
                    let v = f64::from(*v);
                    let [l1, l2] = &mut self.low[ch];
                    let [h1, h2] = &mut self.high[ch];
                    let low = l2.process(&self.low_coeffs, l1.process(&self.low_coeffs, v));
                    let high = h2.process(&self.high_coeffs, h1.process(&self.high_coeffs, v));
                    y[ch] = (high + (1.0 - f64::from(duck)) * low) as f32;
                }
                out_l[n] = y[0];
                out_r[n] = y[1];
            } else {
                let g = 1.0 - duck;
                out_l[n] = x[0] * g;
                out_r[n] = x[1] * g;
            }
        }
        self.meters[meter::DIP] = cv[BATCH - 1];
        self.meters[meter::POSITION] = position as f32;
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(DUCK_CV) = cv;
    }
}

#[cfg(test)]
mod tests;
