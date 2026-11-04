//! `atomik.de-esser`: turns down harsh sibilance, either in its own band or across the whole
//! signal.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in -> band-pass at `freq`, width `q`, unity gain at the centre   (b)
//! level = follower of max(|b_l|, |b_r|)  (`attack`, `release`)  -> dB
//! reduction = min(range, max(0, level - threshold) * (1 - 1/ratio))      g = 10^(-reduction/20)
//! split:     out = in - (1 - g) * b        (the band alone is turned down)
//! wideband:  out = in * g                  (everything is turned down)
//! listen:    out = b                       (hear what the detector hears)
//! ```
//!
//! * **Split mode.** The band-pass of the detector is also the band that is turned down: its
//!   output is subtracted by `1 - g`, so with no reduction the signal passes bit for bit (the
//!   band-pass part cancels exactly when `g` is 1), and at full reduction the band becomes a notch.
//!   The filter is the zero-delay state-variable filter of [`atomik_dsp::Svf`], so there is no
//!   latency and no look-ahead.
//! * **Detector.** The two channels are linked (the louder band wins) so the image does not move.
//!   The follower rises with `attack` and falls with `release`.
//! * **`freq_cv_in`** moves the centre by `freq_cv_trim * cv * 2` octaves (the batch mean), clamped
//!   to 500 Hz and 0.45 of the sample rate.
//! * **`gr_cv_out`** is `1 - g`: 0 with no reduction, towards 1 as the reduction grows.
//! * **Meter values** (`has_meter_values`): `[input peak, output peak, reduction in dB]`.
//! * Deterministic maths (`log2_det`, `exp2_det`, the `tan` of the filter), no latency, non-finite
//!   input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{
    audio_in, audio_out, choice, cv_in, cv_out, expo, finite, linear, toggle, trim,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{Svf, exp2_det, exp2_det_f64, log2_det};

const P_FREQ: u32 = 0;
const P_Q: u32 = 1;
const P_THRESHOLD: u32 = 2;
const P_RATIO: u32 = 3;
const P_RANGE: u32 = 4;
const P_ATTACK: u32 = 5;
const P_RELEASE: u32 = 6;
const P_MODE: u32 = 7;
const P_LISTEN: u32 = 8;
const P_FREQ_TRIM: u32 = 9;

static MODES: [&str; 2] = ["split", "wideband"];

/// `20 * log10(2)`: an amplitude ratio of `2^x` is `x * DB_PER_OCTAVE` dB.
const DB_PER_OCTAVE: f32 = 6.020_6;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The level below which the detector reads silence, dB.
const FLOOR_DB: f32 = -120.0;
/// How far the CV moves the centre at full trim, octaves.
const CV_OCTAVES: f32 = 2.0;

static PARAMS: [ParamDescriptor; 11] = [
    expo(
        P_FREQ,
        "freq",
        "Frequency",
        1_000.0,
        16_000.0,
        6_500.0,
        Unit::Hz,
    ),
    expo(P_Q, "q", "Width (Q)", 0.5, 8.0, 2.0, Unit::None),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -60.0,
        0.0,
        -30.0,
        Unit::Db,
    ),
    expo(P_RATIO, "ratio", "Ratio", 1.5, 20.0, 6.0, Unit::None),
    linear(P_RANGE, "range", "Range", 0.0, 24.0, 12.0, Unit::Db),
    expo(P_ATTACK, "attack", "Attack", 0.05, 20.0, 0.5, Unit::Ms),
    expo(P_RELEASE, "release", "Release", 5.0, 500.0, 40.0, Unit::Ms),
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    toggle(P_LISTEN, "listen", "Listen", 0.0),
    trim(P_FREQ_TRIM, "freq_cv_trim", "Frequency CV trim"),
    bypass_param(10),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const FREQ_CV: SocketId = SocketId(2);
const OUT_L: SocketId = SocketId(3);
const OUT_R: SocketId = SocketId(4);
const GR_CV: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    cv_in(2, "freq_cv_in", "Frequency CV"),
    audio_out(3, "out_l", "Out L"),
    audio_out(4, "out_r", "Out R"),
    cv_out(5, "gr_cv_out", "GR CV"),
];

/// Static description of the de-esser.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.de-esser",
    name: "De-esser",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`DeEsser`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(DeEsser::new())),
    }
}

/// The de-esser device.
#[derive(Debug, Clone)]
pub struct DeEsser {
    sample_rate: f32,
    freq: f32,
    q: f32,
    threshold_db: f32,
    slope: f32,
    range_db: f32,
    attack_ms: f32,
    release_ms: f32,
    wideband: bool,
    listen: bool,
    freq_trim: f32,
    // Derived.
    attack_c: f32,
    release_c: f32,
    // State.
    filters: [Svf; 2],
    level: f32,
    moved: bool,
    meters: [f32; 3],
}

impl Default for DeEsser {
    fn default() -> Self {
        Self::new()
    }
}

impl DeEsser {
    /// A de-esser with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut d = Self {
            sample_rate: 48_000.0,
            freq: 6_500.0,
            q: 2.0,
            threshold_db: -30.0,
            slope: 1.0 - 1.0 / 6.0,
            range_db: 12.0,
            attack_ms: 0.5,
            release_ms: 40.0,
            wideband: false,
            listen: false,
            freq_trim: 1.0,
            attack_c: 0.0,
            release_c: 0.0,
            filters: [Svf::new(); 2],
            level: 0.0,
            moved: false,
            meters: [0.0; 3],
        };
        d.derive();
        d.set_filters(6_500.0);
        d
    }

    fn coefficient(ms: f32, sample_rate: f32) -> f32 {
        let n = f64::from(ms) * 0.001 * f64::from(sample_rate);
        (1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)) as f32
    }

    fn derive(&mut self) {
        self.attack_c = Self::coefficient(self.attack_ms, self.sample_rate);
        self.release_c = Self::coefficient(self.release_ms, self.sample_rate);
    }

    fn set_filters(&mut self, hz: f32) {
        let hz = hz.clamp(500.0, 0.45 * self.sample_rate);
        let g = Svf::cutoff_to_g_det(hz, self.sample_rate);
        let k = 1.0 / self.q;
        for f in &mut self.filters {
            f.set_coefficients(g, k);
        }
    }
}

impl Device for DeEsser {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.derive();
        self.set_filters(self.freq);
        self.reset();
    }

    fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
        self.level = 0.0;
        self.moved = false;
        self.set_filters(self.freq);
        self.meters = [0.0; 3];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_FREQ => {
                self.freq = v.clamp(1_000.0, 16_000.0);
                self.set_filters(self.freq);
            }
            P_Q => {
                self.q = v.clamp(0.5, 8.0);
                self.set_filters(self.freq);
            }
            P_THRESHOLD => self.threshold_db = v.clamp(-60.0, 0.0),
            P_RATIO => self.slope = 1.0 - 1.0 / v.clamp(1.5, 20.0),
            P_RANGE => self.range_db = v.clamp(0.0, 24.0),
            P_ATTACK => {
                self.attack_ms = v.clamp(0.05, 20.0);
                self.derive();
            }
            P_RELEASE => {
                self.release_ms = v.clamp(5.0, 500.0);
                self.derive();
            }
            P_MODE => self.wideband = v >= 0.5,
            P_LISTEN => self.listen = v >= 0.5,
            P_FREQ_TRIM => self.freq_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let cv = clean(batch_mean(ctx.input(FREQ_CV)));
        if cv != 0.0 {
            self.set_filters(self.freq * exp2_det(self.freq_trim * cv * CV_OCTAVES));
            self.moved = true;
        } else if self.moved {
            self.set_filters(self.freq);
            self.moved = false;
        }
        let k = 1.0 / self.q;

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut gr = [0.0f32; BATCH];
        let (mut pk_in, mut pk_out) = (0.0f32, 0.0f32);
        let mut worst_db = 0.0f32;
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            pk_in = pk_in.max(x[0].abs()).max(x[1].abs());
            // The unity-gain band of each channel.
            let band = [
                self.filters[0].process(x[0]).bp * k,
                self.filters[1].process(x[1]).bp * k,
            ];
            let rect = band[0].abs().max(band[1].abs());
            let c = if rect > self.level {
                self.attack_c
            } else {
                self.release_c
            };
            self.level += c * (rect - self.level);
            if self.level < 1e-20 {
                self.level = 0.0;
            }
            let level_db = if self.level > 0.0 {
                (log2_det(self.level) * DB_PER_OCTAVE).max(FLOOR_DB)
            } else {
                FLOOR_DB
            };
            let reduction =
                ((level_db - self.threshold_db).max(0.0) * self.slope).min(self.range_db);
            let g = if reduction > 0.0 {
                exp2_det(-reduction * LOG2_10_20)
            } else {
                1.0
            };
            worst_db = worst_db.max(reduction);
            gr[n] = 1.0 - g;
            for (c, o) in [&mut out_l, &mut out_r].into_iter().enumerate() {
                o[n] = if self.listen {
                    band[c]
                } else if self.wideband {
                    x[c] * g
                } else if g < 1.0 {
                    x[c] - (1.0 - g) * band[c]
                } else {
                    x[c]
                };
            }
            pk_out = pk_out.max(out_l[n].abs()).max(out_r[n].abs());
        }
        self.meters = [pk_in, pk_out, worst_db];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(GR_CV) = gr;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }
}

#[cfg(test)]
mod tests;
