//! `atomik.rotary-speaker`: a rotating-speaker cabinet. A horn rotor and a drum rotor, each with
//! its own slow and fast speed and its own acceleration, turn in front of two microphones and
//! give the swirl of Doppler shift and level movement.
//!
//! # Signal flow
//!
//! ```text
//! in (L + R) / 2 -> drive -> 24 dB/oct crossover --> high band -> horn rotor --\
//!                                                \-> low band  -> drum rotor ---+-> mic L, mic R
//! out = dry * (1 - mix) + (horn + drum at each microphone) * mix
//! ```
//!
//! * **Rotors.** Each rotor has a phase (turns) and a speed (Hz) that follows its target with a
//!   one-pole approach of `*_ramp` milliseconds, so a change between slow and fast speeds up or slows
//!   down like a heavy rotor does (the drum is slower to move than the horn). `speed` picks Stop
//!   (0 Hz), Slow or Fast; a high `speed_cv_in` (the batch mean above 0.5, back to the knob
//!   below 0.3) forces Fast, so a gate or a button can be the speed switch. The first batch
//!   after a reset starts at the target speed, with no run-up. The horn starts at phase 0 and the
//!   drum a quarter turn later.
//! * **Doppler.** Per microphone, the rotor's distance gives a delay of `1 + 0.4 ms - doppler
//!   span cos(a) / 2` (`a` the rotor angle seen by the microphone, `span` 0.6 ms for the horn
//!   and 0.8 ms for the drum): a read of the rotor's delay line, so the pitch moves up while the
//!   rotor comes toward the microphone and down while it leaves. `doppler` 0 keeps the fixed
//!   delay of 0.4 ms (a whole number of samples), the same for both rotors, so the two bands still add up flat.
//! * **Level.** The gain at a microphone is `1 - tremolo k (1 - cos a) / 2` (`k` 1 for the horn,
//!   0.6 for the drum): full while the rotor faces the microphone and `1 - k tremolo` while it
//!   faces away.
//! * **Microphones.** The two microphones see the rotors at angles `-spread / 4` and
//!   `+spread / 4` turns from each other's centre: 0 puts both at the same point (both outputs
//!   equal), 1 puts them 180 degrees apart.
//! * **Drive.** A rational soft clipper (`drive` 0 is bit-exact off) with a gain of up to 18 dB
//!   in front of the crossover and a square-root makeup.
//! * **CV out.** `horn_cv_out` is the sine of the horn angle, -1..+1, one value per sample.
//! * The wet path has a mean delay of 0.4 ms plus one sample (no latency is reported; the dry
//!   signal is not delayed, so a mix between them has a slight comb). Deterministic maths
//!   (`sin_cycles`, `exp2_det_f64`); the output stays finite for any parameters; non-finite
//!   input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::batch_mean;
use crate::tool_support::{
    GateIn, audio_in, audio_out, choice, cv_in, cv_out, expo, finite, linear,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{Crossover, DelayLine, exp2_det_f64, sin_cycles};

const P_SPEED: u32 = 0;
const P_HORN_SLOW: u32 = 1;
const P_HORN_FAST: u32 = 2;
const P_DRUM_SLOW: u32 = 3;
const P_DRUM_FAST: u32 = 4;
const P_HORN_RAMP: u32 = 5;
const P_DRUM_RAMP: u32 = 6;
const P_CROSSOVER: u32 = 7;
const P_DRIVE: u32 = 8;
const P_DOPPLER: u32 = 9;
const P_TREMOLO: u32 = 10;
const P_SPREAD: u32 = 11;
const P_MIX: u32 = 12;

/// The `speed` choices.
const SPEEDS: &[&str] = &["Stop", "Slow", "Fast"];
const STOP: u8 = 0;
const FAST: u8 = 2;

/// The fixed part of the rotor delay, milliseconds (half the drum span).
const CENTRE_MS: f64 = 0.4;
/// Doppler delay span of the horn and of the drum, milliseconds.
const HORN_SPAN_MS: f64 = 0.6;
const DRUM_SPAN_MS: f64 = 0.8;
/// Level movement of the drum relative to the horn.
const DRUM_TREMOLO: f32 = 0.6;
/// The drive's most gain, as a linear factor (18 dB).
const MAX_DRIVE_GAIN: f32 = 7.943_282;

static PARAMS: [ParamDescriptor; 14] = [
    choice(P_SPEED, "speed", "Speed", SPEEDS, 1.0),
    linear(
        P_HORN_SLOW,
        "horn_slow",
        "Horn slow",
        0.3,
        2.0,
        0.8,
        Unit::Hz,
    ),
    linear(
        P_HORN_FAST,
        "horn_fast",
        "Horn fast",
        3.0,
        10.0,
        6.7,
        Unit::Hz,
    ),
    linear(
        P_DRUM_SLOW,
        "drum_slow",
        "Drum slow",
        0.2,
        1.5,
        0.7,
        Unit::Hz,
    ),
    linear(
        P_DRUM_FAST,
        "drum_fast",
        "Drum fast",
        3.0,
        8.0,
        5.8,
        Unit::Hz,
    ),
    expo(
        P_HORN_RAMP,
        "horn_ramp",
        "Horn ramp",
        50.0,
        3_000.0,
        600.0,
        Unit::Ms,
    ),
    expo(
        P_DRUM_RAMP,
        "drum_ramp",
        "Drum ramp",
        200.0,
        8_000.0,
        2_500.0,
        Unit::Ms,
    ),
    expo(
        P_CROSSOVER,
        "crossover",
        "Crossover",
        200.0,
        1_600.0,
        800.0,
        Unit::Hz,
    ),
    linear(P_DRIVE, "drive", "Drive", 0.0, 1.0, 0.2, Unit::Percent),
    linear(
        P_DOPPLER,
        "doppler",
        "Doppler",
        0.0,
        1.0,
        0.6,
        Unit::Percent,
    ),
    linear(
        P_TREMOLO,
        "tremolo",
        "Tremolo",
        0.0,
        1.0,
        0.5,
        Unit::Percent,
    ),
    linear(
        P_SPREAD,
        "spread",
        "Mic spread",
        0.0,
        1.0,
        1.0,
        Unit::Percent,
    ),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    bypass_param(13),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SPEED_CV: SocketId = SocketId(2);
const OUT_L: SocketId = SocketId(3);
const OUT_R: SocketId = SocketId(4);
const HORN_CV: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    cv_in(2, "speed_cv_in", "Speed CV"),
    audio_out(3, "out_l", "Out L"),
    audio_out(4, "out_r", "Out R"),
    cv_out(5, "horn_cv_out", "Horn CV"),
];

/// Static description of the rotary speaker.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.rotary-speaker",
    name: "Rotary Speaker",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`RotarySpeaker`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(RotarySpeaker::new())),
    }
}

/// One rotor: its angle and its current speed.
#[derive(Debug, Clone, Copy)]
struct Rotor {
    /// Turns, 0..1.
    phase: f64,
    /// Hz.
    speed: f64,
}

impl Rotor {
    fn at(phase: f64) -> Self {
        Self { phase, speed: 0.0 }
    }

    /// One sample: the speed approaches `target` by `coefficient`, then the angle advances.
    #[inline]
    fn step(&mut self, target: f64, coefficient: f64, sample_rate: f64) {
        self.speed += coefficient * (target - self.speed);
        let p = self.phase + self.speed / sample_rate;
        self.phase = p - p.floor();
    }
}

/// Longest delay line needed at `sample_rate`: the centre plus half the larger span, plus a
/// few samples.
fn line_len(sample_rate: f32) -> usize {
    (DRUM_SPAN_MS * 0.001 * f64::from(sample_rate)).ceil() as usize + 4
}

/// The rotary speaker device.
#[derive(Debug, Clone)]
pub struct RotarySpeaker {
    sample_rate: f32,
    speed: u8,
    horn_slow: f32,
    horn_fast: f32,
    drum_slow: f32,
    drum_fast: f32,
    horn_ramp: f32,
    drum_ramp: f32,
    crossover: f32,
    drive: f32,
    doppler: f32,
    tremolo: f32,
    spread: f32,
    mix: f32,
    // Derived.
    horn_a: f64,
    drum_a: f64,
    centre: f32,
    horn_span: f32,
    drum_span: f32,
    drive_gain: f32,
    drive_makeup: f32,
    // State.
    fresh: bool,
    horn: Rotor,
    drum: Rotor,
    gate: GateIn,
    split: Crossover,
    horn_line: DelayLine,
    drum_line: DelayLine,
}

impl Default for RotarySpeaker {
    fn default() -> Self {
        Self::new()
    }
}

impl RotarySpeaker {
    /// A rotary speaker with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut s = Self {
            sample_rate: 48_000.0,
            speed: 1,
            horn_slow: 0.8,
            horn_fast: 6.7,
            drum_slow: 0.7,
            drum_fast: 5.8,
            horn_ramp: 600.0,
            drum_ramp: 2_500.0,
            crossover: 800.0,
            drive: 0.2,
            doppler: 0.6,
            tremolo: 0.5,
            spread: 1.0,
            mix: 1.0,
            horn_a: 0.0,
            drum_a: 0.0,
            centre: 0.0,
            horn_span: 0.0,
            drum_span: 0.0,
            drive_gain: 1.0,
            drive_makeup: 1.0,
            fresh: true,
            horn: Rotor::at(0.0),
            drum: Rotor::at(0.25),
            gate: GateIn::default(),
            split: Crossover::new(48_000.0),
            horn_line: DelayLine::new(line_len(48_000.0)),
            drum_line: DelayLine::new(line_len(48_000.0)),
        };
        s.derive();
        s.split.set_freq(s.crossover);
        s.reset();
        s
    }

    /// The per-sample coefficient of a one-pole approach with time constant `ms`.
    fn coefficient(ms: f32, sample_rate: f32) -> f64 {
        let n = f64::from(ms) * 0.001 * f64::from(sample_rate);
        1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)
    }

    fn derive(&mut self) {
        self.horn_a = Self::coefficient(self.horn_ramp, self.sample_rate);
        self.drum_a = Self::coefficient(self.drum_ramp, self.sample_rate);
        let per_second = f64::from(self.sample_rate) * 0.001;
        self.centre = (CENTRE_MS * per_second).round() as f32;
        self.horn_span = (HORN_SPAN_MS * per_second) as f32;
        self.drum_span = (DRUM_SPAN_MS * per_second) as f32;
        self.drive_gain = 1.0 + self.drive * (MAX_DRIVE_GAIN - 1.0);
        self.drive_makeup = 1.0 / self.drive_gain.sqrt();
    }

    /// The target speeds, Hz: (horn, drum).
    fn targets(&self, forced_fast: bool) -> (f64, f64) {
        let speed = if forced_fast { FAST } else { self.speed };
        match speed {
            STOP => (0.0, 0.0),
            FAST => (f64::from(self.horn_fast), f64::from(self.drum_fast)),
            _ => (f64::from(self.horn_slow), f64::from(self.drum_slow)),
        }
    }

    /// The soft clipper: a rational `tanh` (exact enough, and deterministic).
    #[inline]
    fn saturate(&self, x: f32) -> f32 {
        if self.drive <= 0.0 {
            return x;
        }
        let u = (x * self.drive_gain).clamp(-3.0, 3.0);
        let y = u * (27.0 + u * u) / (27.0 + 9.0 * u * u);
        y * self.drive_makeup
    }
}

/// What one rotor gives at the two microphones: a read of its delay line each, delayed by the
/// Doppler of that microphone's angle and weighted by the level of it.
#[inline]
fn at_mics(
    line: &DelayLine,
    rotor: &Rotor,
    centre: f32,
    span: f32,
    doppler: f32,
    tremolo: f32,
    offset: f64,
) -> [f32; 2] {
    let mut out = [0.0f32; 2];
    for (c, o) in out.iter_mut().enumerate() {
        let side = if c == 0 { -offset } else { offset };
        let cos = sin_cycles(rotor.phase + side + 0.25) as f32;
        let delay = 1.0 + centre - 0.5 * span * doppler * cos;
        let gain = 1.0 - tremolo * 0.5 * (1.0 - cos);
        *o = line.read(delay) * gain;
    }
    out
}

impl Device for RotarySpeaker {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.derive();
        self.split = Crossover::new(self.sample_rate);
        self.split.set_freq(self.crossover);
        self.horn_line = DelayLine::new(line_len(self.sample_rate));
        self.drum_line = DelayLine::new(line_len(self.sample_rate));
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
        self.horn = Rotor::at(0.0);
        self.drum = Rotor::at(0.25);
        self.gate.reset();
        self.split.reset();
        self.horn_line.clear();
        self.drum_line.clear();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_SPEED => self.speed = v.round().clamp(0.0, 2.0) as u8,
            P_HORN_SLOW => self.horn_slow = v.clamp(0.3, 2.0),
            P_HORN_FAST => self.horn_fast = v.clamp(3.0, 10.0),
            P_DRUM_SLOW => self.drum_slow = v.clamp(0.2, 1.5),
            P_DRUM_FAST => self.drum_fast = v.clamp(3.0, 8.0),
            P_HORN_RAMP => {
                self.horn_ramp = v.clamp(50.0, 3_000.0);
                self.derive();
            }
            P_DRUM_RAMP => {
                self.drum_ramp = v.clamp(200.0, 8_000.0);
                self.derive();
            }
            P_CROSSOVER => {
                self.crossover = v.clamp(200.0, 1_600.0);
                self.split.set_freq(self.crossover);
            }
            P_DRIVE => {
                self.drive = v.clamp(0.0, 1.0);
                self.derive();
            }
            P_DOPPLER => self.doppler = v.clamp(0.0, 1.0),
            P_TREMOLO => self.tremolo = v.clamp(0.0, 1.0),
            P_SPREAD => self.spread = v.clamp(0.0, 1.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let forced_fast = self.gate.update(batch_mean(ctx.input(SPEED_CV)));
        let (horn_target, drum_target) = self.targets(forced_fast);
        if self.fresh {
            self.horn.speed = horn_target;
            self.drum.speed = drum_target;
            self.fresh = false;
        }
        let rate = f64::from(self.sample_rate);
        let offset = f64::from(self.spread) * 0.25;
        let drum_tremolo = self.tremolo * DRUM_TREMOLO;

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        for n in 0..BATCH {
            let dry = [finite(in_l[n]), finite(in_r[n])];
            let x = self.saturate((dry[0] + dry[1]) * 0.5);
            let (low, high) = self.split.process(x);
            let h = at_mics(
                &self.horn_line,
                &self.horn,
                self.centre,
                self.horn_span,
                self.doppler,
                self.tremolo,
                offset,
            );
            let d = at_mics(
                &self.drum_line,
                &self.drum,
                self.centre,
                self.drum_span,
                self.doppler,
                drum_tremolo,
                offset,
            );
            self.horn_line.write(finite(high));
            self.drum_line.write(finite(low));
            let wet = [h[0] + d[0], h[1] + d[1]];
            if self.mix >= 1.0 {
                out_l[n] = wet[0];
                out_r[n] = wet[1];
            } else {
                out_l[n] = dry[0] * (1.0 - self.mix) + wet[0] * self.mix;
                out_r[n] = dry[1] * (1.0 - self.mix) + wet[1] * self.mix;
            }
            cv[n] = sin_cycles(self.horn.phase) as f32;
            self.horn.step(horn_target, self.horn_a, rate);
            self.drum.step(drum_target, self.drum_a, rate);
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(HORN_CV) = cv;
    }
}

#[cfg(test)]
mod tests;
