//! `atomik.modulation`: chorus, flanger and phaser.
//!
//! # Modes
//!
//! * **Chorus** (`mode` 0): three Hermite-interpolated modulated delays per side. Voice `v` has
//!   a base delay `delay * [1.0, 1.27, 0.79][v]` and an LFO phase offset of `v / 3` cycle; the
//!   instantaneous delay is `base * (1 + 0.5 * depth * lfo)`. The three voices are averaged
//!   (gain 1/3 each) into the wet signal. `feedback` feeds the averaged wet back into the line.
//! * **Flanger** (`mode` 1): one modulated delay per side, `delay * (1 + 0.9 * depth * lfo)`
//!   (never below 0.1 `delay`), the delayed signal is the wet output and `feedback` (positive or
//!   negative) feeds it back into the line.
//! * **Phaser** (`mode` 2): `stages` (2 to 12, even) cascaded first-order allpasses
//!   `H(z) = (a + z^-1) / (1 + a z^-1)` with `a = (t - 1) / (t + 1)`, `t = tan(pi f / sr)`. The
//!   corner frequency sweeps exponentially around 1 kHz: `f = 1000 * 2^(3 * depth * lfo)`
//!   (125 Hz to 8 kHz at full depth). `feedback` feeds the last stage's previous output back to
//!   the input (one sample of delay in the loop, so the loop gain is `|feedback| < 1`).
//!   `delay` is not used by the phaser. `tan` is a deterministic Pade approximation (the corner
//!   is limited to 0.7 rad, where the error is below 1e-5).
//!
//! # LFO, rate and tempo sync
//!
//! The LFO is the parabolic sine-like wave `4u(1 - |u|)`, `u = 2 phase - 1` (as
//! [`atomik_dsp::ParabolicLfo`]; no libm). The right channel's phase is the left's plus
//! `spread / 360` cycles (0 to 0.5).
//!
//! * `sync` off: the LFO free-runs at `rate` Hz (a phase accumulator in `f64`).
//! * `sync` on (1/16 to 4 bars, 4/4, `0.25, 0.5, 1, 2, 4, 8, 16` beats per cycle): while the
//!   transport is **playing** the phase is derived from the absolute frame,
//!   `phase = frac(frame / (beats * sr * 60 / bpm))` in `f64`, so it depends only on the
//!   frame index and the tempo, never on how the host splits the audio into batches, and the
//!   period is exact. A tempo change moves the phase (it is recomputed from the frame). While
//!   the transport is **stopped** the LFO free-runs at the synced rate `1 / period`, continuing
//!   from the last phase.
//!
//! # Mix, output and smoothing
//!
//! `mix_cv_in` (batch mean, clamped to -1..+1) times `mix_cv_trim` (-1..+1, default +1) is added
//! to `mix` and clamped to 0..1 before the smoother; unconnected, nothing changes.
//!
//! `out = (dry * (1 - mix) + wet * mix) * output`. `mix`, `output`, `depth`, `feedback` and the
//! delay time are smoothed per batch (20 ms, linear ramps). Switching `mode` clears the delay
//! lines and allpass states. Latency 0. The feedback signal is clamped to +-8 (a hard bound that
//! only matters if an interpolator overshoots at the maximum feedback), and all state is flushed
//! below 1e-20, so the output is finite and bounded for any parameters.

use crate::cv_in::{add_norm, batch_mean};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{BlockSmoother, ModDelay, exp2_det, flush_denormal};

#[cfg(test)]
mod tests;

const P_MODE: u32 = 0;
const P_RATE: u32 = 1;
const P_SYNC: u32 = 2;
const P_DEPTH: u32 = 3;
const P_FEEDBACK: u32 = 4;
const P_DELAY: u32 = 5;
const P_STAGES: u32 = 6;
const P_SPREAD: u32 = 7;
const P_MIX: u32 = 8;
const P_OUTPUT: u32 = 9;
const P_MIX_TRIM: u32 = 10;

/// Longest base delay in ms.
pub const MAX_DELAY_MS: f32 = 30.0;
/// Largest instantaneous delay as a multiple of the base delay (chorus voice 1.27 x 1.5).
const MAX_DELAY_FACTOR: f32 = 1.27 * 1.5;
/// Beats per LFO cycle of the sync choices 1 to 7 (4/4).
pub const SYNC_BEATS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
/// Chorus voice base-delay factors.
const VOICE_FACTOR: [f32; 3] = [1.0, 1.27, 0.79];
/// Maximum phaser stages.
const MAX_STAGES: usize = 12;
/// Phaser centre frequency in Hz.
const PHASER_CENTRE_HZ: f32 = 1_000.0;
/// Phaser sweep in octaves at full depth.
const PHASER_OCTAVES: f32 = 3.0;
/// Feedback clamp.
const FB_LIMIT: f32 = 8.0;

static PARAMS: [ParamDescriptor; 11] = [
    ParamDescriptor {
        id: ParamId(P_MODE),
        key: "mode",
        name: "Mode",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["chorus", "flanger", "phaser"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_RATE),
        key: "rate",
        name: "Rate",
        min: 0.02,
        max: 20.0,
        default: 0.5,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_SYNC),
        key: "sync",
        name: "Sync",
        min: 0.0,
        max: 7.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&[
            "off", "1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars",
        ]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_DEPTH),
        key: "depth",
        name: "Depth",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_FEEDBACK),
        key: "feedback",
        name: "Feedback",
        min: -0.95,
        max: 0.95,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_DELAY),
        key: "delay",
        name: "Delay",
        min: 0.1,
        max: MAX_DELAY_MS,
        default: 7.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_STAGES),
        key: "stages",
        name: "Stages",
        min: 2.0,
        max: 12.0,
        default: 4.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_SPREAD),
        key: "spread",
        name: "Stereo spread (degrees)",
        min: 0.0,
        max: 180.0,
        default: 90.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MIX),
        key: "mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_OUTPUT),
        key: "output",
        name: "Output",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MIX_TRIM),
        key: "mix_cv_trim",
        name: "Mix CV trim",
        min: -1.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const MIX_CV: SocketId = SocketId(4);

static SOCKETS: [SocketDescriptor; 5] = [
    SocketDescriptor {
        id: IN_L,
        key: "in_l",
        name: "In L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: IN_R,
        key: "in_r",
        name: "In R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: MIX_CV,
        key: "mix_cv_in",
        name: "Mix CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the modulation effect.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.modulation",
    name: "Modulation",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Modulation`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Modulation::new()),
    }
}

/// `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;

/// The parabolic LFO wave for a phase in cycles (any value; only the fractional part counts).
#[inline]
pub(crate) fn lfo_shape(phase: f64) -> f32 {
    let p = (phase - phase.floor()) as f32;
    let u = 2.0 * p - 1.0;
    4.0 * u * (1.0 - u.abs())
}

/// Chorus voice delay in samples (before the line clamp) for LFO value `lfo` in -1..=1.
#[inline]
pub(crate) fn chorus_delay(base_samples: f32, voice: usize, depth: f32, lfo: f32) -> f32 {
    base_samples * VOICE_FACTOR[voice] * (1.0 + 0.5 * depth * lfo)
}

/// Flanger delay in samples (before the line clamp).
#[inline]
pub(crate) fn flanger_delay(base_samples: f32, depth: f32, lfo: f32) -> f32 {
    base_samples * (1.0 + 0.9 * depth * lfo)
}

/// `tan(x)` by the [5/4] Pade approximation for `0 <= x <= 0.7` (`+ - * /` only).
#[inline]
fn tan_pade(x: f32) -> f32 {
    let x2 = x * x;
    x * (945.0 - 105.0 * x2 + x2 * x2) / (945.0 - 420.0 * x2 + 15.0 * x2 * x2)
}

/// First-order allpass coefficient for corner `hz`.
#[inline]
fn allpass_coeff(hz: f32, sr: f32) -> f32 {
    let x = (std::f32::consts::PI * hz / sr).clamp(0.001, 0.7);
    let t = tan_pade(x);
    (t - 1.0) / (t + 1.0)
}

/// Even stage count 2 to 12 from a raw value.
fn even_stages(v: f32) -> usize {
    (((v / 2.0).round() as i32) * 2).clamp(2, MAX_STAGES as i32) as usize
}

/// The chorus / flanger / phaser device.
#[derive(Debug, Clone)]
pub struct Modulation {
    sample_rate: f32,
    mode: u8,
    rate: f32,
    sync: usize,
    depth: f32,
    feedback: f32,
    delay_ms: f32,
    stages: usize,
    spread: f32,
    mix: f32,
    mix_trim: f32,
    output_db: f32,
    // State.
    fresh: bool,
    /// Free-running / last LFO phase (cycles, left channel, `0..1`).
    phase: f64,
    lines: [ModDelay; 2],
    ap: [[f64; MAX_STAGES]; 2],
    fb_state: [f64; 2],
    depth_sm: BlockSmoother,
    fb_sm: BlockSmoother,
    delay_sm: BlockSmoother,
    mix_sm: BlockSmoother,
    out_sm: BlockSmoother,
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulation {
    /// A modulation effect with default params; its delay lines are sized by `prepare`.
    pub fn new() -> Self {
        let mut m = Self {
            sample_rate: 48_000.0,
            mode: 0,
            rate: 0.5,
            sync: 0,
            depth: 0.5,
            feedback: 0.0,
            delay_ms: 7.0,
            stages: 4,
            spread: 90.0,
            mix: 0.5,
            mix_trim: 1.0,
            output_db: 0.0,
            fresh: true,
            phase: 0.0,
            lines: [ModDelay::new(4), ModDelay::new(4)],
            ap: [[0.0; MAX_STAGES]; 2],
            fb_state: [0.0; 2],
            depth_sm: BlockSmoother::new(0.5),
            fb_sm: BlockSmoother::new(0.0),
            delay_sm: BlockSmoother::new(7.0),
            mix_sm: BlockSmoother::new(0.5),
            out_sm: BlockSmoother::new(1.0),
        };
        m.alloc_lines();
        m
    }

    fn alloc_lines(&mut self) {
        let max = (MAX_DELAY_MS * MAX_DELAY_FACTOR * 0.001 * self.sample_rate).ceil() as usize + 8;
        self.lines = [ModDelay::new(max), ModDelay::new(max)];
    }

    fn clear_state(&mut self) {
        self.lines[0].clear();
        self.lines[1].clear();
        self.ap = [[0.0; MAX_STAGES]; 2];
        self.fb_state = [0.0; 2];
    }

    /// The LFO phase (left channel, cycles) of the last processed sample (test hook).
    #[cfg(test)]
    pub(crate) fn lfo_phase(&self) -> f64 {
        self.phase
    }
}

impl Device for Modulation {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.alloc_lines();
        for s in [
            &mut self.depth_sm,
            &mut self.fb_sm,
            &mut self.delay_sm,
            &mut self.mix_sm,
            &mut self.out_sm,
        ] {
            s.set_time(20.0, self.sample_rate, BATCH);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.clear_state();
        self.phase = 0.0;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_MODE => {
                let m = (v.round() as i32).clamp(0, 2) as u8;
                if m != self.mode {
                    self.mode = m;
                    self.clear_state();
                }
            }
            P_RATE => self.rate = v.clamp(0.02, 20.0),
            P_SYNC => self.sync = (v.round() as i32).clamp(0, 7) as usize,
            P_DEPTH => self.depth = v.clamp(0.0, 1.0),
            P_FEEDBACK => self.feedback = v.clamp(-0.95, 0.95),
            P_DELAY => self.delay_ms = v.clamp(0.1, MAX_DELAY_MS),
            P_STAGES => self.stages = even_stages(v),
            P_SPREAD => self.spread = v.clamp(0.0, 180.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_MIX_TRIM => self.mix_trim = v.clamp(-1.0, 1.0),
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let time = ctx.time;
        let sr = self.sample_rate;
        let sr64 = f64::from(sr);

        let out_target = exp2_det(self.output_db * LOG2_10_20);
        let mix_t = if ctx.is_connected(MIX_CV) {
            add_norm(self.mix, self.mix_trim, batch_mean(ctx.input(MIX_CV)))
        } else {
            self.mix
        };
        if self.fresh {
            self.depth_sm.set_immediate(self.depth);
            self.fb_sm.set_immediate(self.feedback);
            self.delay_sm.set_immediate(self.delay_ms);
            self.mix_sm.set_immediate(mix_t);
            self.out_sm.set_immediate(out_target);
            self.fresh = false;
        }
        let depth = self.depth_sm.advance(self.depth, BATCH);
        let fb = self.fb_sm.advance(self.feedback, BATCH);
        let delay = self.delay_sm.advance(self.delay_ms, BATCH);
        let mix = self.mix_sm.advance(mix_t, BATCH);
        let out_g = self.out_sm.advance(out_target, BATCH);

        // LFO timing for this batch.
        let synced = self.sync > 0;
        let period = if synced {
            SYNC_BEATS[self.sync - 1] * sr64 * 60.0 / time.tempo.bpm()
        } else {
            0.0
        };
        let inc = if synced {
            1.0 / period
        } else {
            f64::from(self.rate) / sr64
        };
        let locked = synced && time.playing;
        let spread = f64::from(self.spread) / 360.0;
        let mode = self.mode;
        let stages = self.stages;
        let ms_to_samples = 0.001 * sr;

        let mut out = [[0.0f32; BATCH]; 2];
        let mut ph = self.phase;
        for n in 0..BATCH {
            if locked {
                let x = (time.frame + n as u64) as f64 / period;
                ph = x - x.floor();
            } else {
                ph += inc;
                ph -= ph.floor();
            }
            let dpt = depth.at(n);
            let fbk = fb.at(n);
            let base = delay.at(n) * ms_to_samples;
            let m = mix.at(n);
            let g = out_g.at(n);
            for (c, o) in out.iter_mut().enumerate() {
                let dry = {
                    let v = if c == 0 { in_l[n] } else { in_r[n] };
                    if v.is_finite() { v } else { 0.0 }
                };
                let lp = if c == 0 { ph } else { ph + spread };
                let wet = match mode {
                    0 => {
                        let line = &mut self.lines[c];
                        let mut sum = 0.0f32;
                        for v in 0..3 {
                            let l = lfo_shape(lp + v as f64 / 3.0);
                            sum += line.read(chorus_delay(base, v, dpt, l));
                        }
                        let wet = sum * (1.0 / 3.0);
                        line.write(dry + fbk * wet.clamp(-FB_LIMIT, FB_LIMIT));
                        wet
                    }
                    1 => {
                        let line = &mut self.lines[c];
                        let wet = line.read(flanger_delay(base, dpt, lfo_shape(lp)));
                        line.write(dry + fbk * wet.clamp(-FB_LIMIT, FB_LIMIT));
                        wet
                    }
                    _ => {
                        let l = lfo_shape(lp);
                        let hz = PHASER_CENTRE_HZ * exp2_det(PHASER_OCTAVES * dpt * l);
                        let a = f64::from(allpass_coeff(hz, sr));
                        let fbin = f64::from(fbk) * self.fb_state[c].clamp(-8.0, 8.0);
                        let mut x = f64::from(dry) + fbin;
                        let st = &mut self.ap[c];
                        for s in st.iter_mut().take(stages) {
                            let y = a * x + *s;
                            *s = flush(x - a * y);
                            x = y;
                        }
                        self.fb_state[c] = flush(x);
                        x as f32
                    }
                };
                let wet = flush_denormal(wet);
                o[n] = (dry * (1.0 - m) + wet * m) * g;
            }
        }
        self.phase = ph;
        *ctx.output(OUT_L) = out[0];
        *ctx.output(OUT_R) = out[1];
    }

    fn latency_frames(&self) -> u32 {
        0
    }
}

#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}
