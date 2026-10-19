//! Row 2 of the preset matrix: `wide`, `pocket-radio`, `camcorder`, `dusty-groove` (indices 4 to
//! 7). `wide` is hand-wired (mid/side with a decorrelator, which the generic rack has no stage
//! for); the other three are rack presets.
//!
//! | Idx | Preset | Signal flow |
//! |---|---|---|
//! | 4 | `wide` | mid/side split; the side is high-passed and gets a decorrelated copy of the mid (four all-passes) added, then scaled by the width; a gentle high-shelf tilt after the recombine |
//! | 5 | `pocket-radio` | mono sum, high-pass, mid peak, a high low-pass as band limit, saturation (oversampled), low-pass, compressor; faint band-limited noise enters before the filters |
//! | 6 | `camcorder` | low-pass, wow and flutter, automatic gain control (upward leveller), narrowed image; hiss enters ahead of the AGC, a faint head-switch click train at the end |
//! | 7 | `dusty-groove` | high-pass, mono below 150 Hz, low-pass, slow warp; surface hiss, rumble, crackle and pops added after the filtering |

use super::chain::{
    Activity, ChainCommon, Ctx, PresetChain, Routing, SUB, StereoDelay, chain_stage_spec,
    short_latency,
};
use super::rack::{Kind, RackChain, RackDef, StageDef};
use super::voicing::{ChainSpec, Morph, ParamSpec, PresetVoicing, StageSpec, Val};
use atomik_dsp::rng::hash64;
use atomik_dsp::{
    Allpass, Biquad, BiquadCoeffs, BiquadShape, ShaperKind, SmoothedBiquad, Smoother2,
    SmootherClass,
};

const fn st(name: &'static str, kind: Kind) -> StageDef {
    StageDef { name, kind }
}

/// `pocket-radio`: a small transistor radio.
pub static POCKET_RADIO: RackDef = RackDef {
    stages: &[
        st("hp", Kind::HighPass),
        st("pk", Kind::Peak),
        st("bl", Kind::LowPass),
        st("sat", Kind::Sat(ShaperKind::SoftClip)),
        st("lp", Kind::LowPass),
        st("comp", Kind::Comp),
        st("hiss", Kind::Hiss),
    ],
    routing: Routing::MonoSum,
};

/// `camcorder`: consumer camcorder audio.
pub static CAMCORDER: RackDef = RackDef {
    stages: &[
        st("lp", Kind::LowPass),
        st("wobble", Kind::Wobble),
        st("lev", Kind::Leveller),
        st("width", Kind::Width),
        st("hiss", Kind::Hiss),
        st("clk", Kind::Clicks),
    ],
    routing: Routing::Stereo,
};

/// `dusty-groove`: a worn record.
pub static DUSTY_GROOVE: RackDef = RackDef {
    stages: &[
        st("hp", Kind::HighPass),
        st("mono", Kind::MonoLow),
        st("lp", Kind::LowPass),
        st("wobble", Kind::Wobble),
        st("hiss", Kind::Hiss),
        st("rum", Kind::Rumble),
        st("crackle", Kind::Crackle),
    ],
    routing: Routing::Stereo,
};

/// The rack definitions of this row with their preset indices (`wide` is hand-wired).
#[cfg(test)]
pub fn racks() -> [(usize, &'static RackDef); 3] {
    [(5, &POCKET_RADIO), (6, &CAMCORDER), (7, &DUSTY_GROOVE)]
}

/// The chain of preset `idx` (4 to 7).
pub fn build(idx: usize) -> Box<dyn PresetChain> {
    match idx {
        4 => Box::new(WideChain::new()),
        5 => Box::new(RackChain::new(&POCKET_RADIO)),
        6 => Box::new(RackChain::new(&CAMCORDER)),
        _ => Box::new(RackChain::new(&DUSTY_GROOVE)),
    }
}

// --- wide ---------------------------------------------------------------------------------------

const WIDE_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "side_gain",
        min: 0.0,
        max: 3.0,
    },
    ParamSpec {
        name: "decorr",
        min: 0.0,
        max: 1.0,
    },
    ParamSpec {
        name: "tilt_db",
        min: -6.0,
        max: 6.0,
    },
];

/// Delays of the four decorrelating all-passes in ms and their coefficients.
const DECORR_MS: [f32; 4] = [0.9, 1.5, 2.3, 3.4];
const DECORR_G: [f32; 4] = [0.55, -0.5, 0.5, -0.45];
/// The side high-pass corner.
const SIDE_HP_HZ: f64 = 200.0;
/// The tilt shelf corner.
const TILT_HZ: f32 = 5_000.0;

struct WideRt {
    side: Val,
    decorr: Val,
    tilt: Val,
    common: ChainCommon,
    trim: f32,
    hp_coeffs: BiquadCoeffs,
    hp: Biquad,
    aps: [Allpass; 4],
    tilt_bq: SmoothedBiquad,
    side_sm: Smoother2,
    dec_sm: Smoother2,
    pad: StereoDelay,
    latency: u32,
    fresh: bool,
    activity: Activity,
}

/// The hand-wired `wide` chain.
#[derive(Default)]
pub struct WideChain {
    rt: Option<WideRt>,
}

impl WideChain {
    /// A chain; `prepare` builds its state.
    pub fn new() -> Self {
        Self::default()
    }
}

impl PresetChain for WideChain {
    fn spec(&self) -> ChainSpec {
        ChainSpec {
            stages: vec![
                StageSpec {
                    name: "ms",
                    params: WIDE_PARAMS,
                },
                chain_stage_spec(),
            ],
            implemented: true,
        }
    }

    fn prepare(&mut self, sr: f32, seed: u64, v: &PresetVoicing) {
        let common = ChainCommon::read(v);
        let latency = short_latency(sr);
        let aps = std::array::from_fn(|i| {
            let jitter = (hash64(seed, 0x57DE, i as u64) % 3) as usize;
            Allpass::new((DECORR_MS[i] * 0.001 * sr).round() as usize + jitter)
        });
        self.rt = Some(WideRt {
            side: v.val("ms", "side_gain"),
            decorr: v.val("ms", "decorr"),
            tilt: v.val("ms", "tilt_db"),
            trim: common.trim(),
            common,
            hp_coeffs: BiquadCoeffs::design(
                BiquadShape::Highpass,
                SIDE_HP_HZ,
                std::f64::consts::FRAC_1_SQRT_2,
                0.0,
                f64::from(sr),
            ),
            hp: Biquad::default(),
            aps,
            tilt_bq: SmoothedBiquad::new(BiquadShape::HighShelf, TILT_HZ, 0.707, 0.0, sr),
            side_sm: Smoother2::with_class(1.0, SmootherClass::Fast, sr),
            dec_sm: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            pad: StereoDelay::new(latency as usize),
            latency,
            fresh: true,
            activity: Activity::new(sr),
        });
    }

    fn reset(&mut self) {
        if let Some(rt) = self.rt.as_mut() {
            rt.hp.reset();
            for a in &mut rt.aps {
                a.clear();
            }
            rt.tilt_bq.reset();
            rt.pad.reset();
            rt.fresh = true;
            rt.activity.reset();
        }
    }

    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], m: &Morph, _ctx: &Ctx) {
        let Some(rt) = self.rt.as_mut() else { return };
        let (side, dec, tilt) = (rt.side.at(m), rt.decorr.at(m), rt.tilt.at(m));
        rt.side_sm.set_target(side);
        rt.dec_sm.set_target(dec);
        rt.tilt_bq.set_gain_db(tilt.clamp(-6.0, 6.0));
        if rt.fresh {
            rt.side_sm.set_immediate(side);
            rt.dec_sm.set_immediate(dec);
            rt.tilt_bq.snap();
            rt.fresh = false;
        }
        for n in 0..SUB {
            let mid = 0.5 * (l[n] + r[n]);
            let s = 0.5 * (l[n] - r[n]);
            let s_hp = rt.hp.process(&rt.hp_coeffs, f64::from(s)) as f32;
            let mut d = mid;
            for (ap, g) in rt.aps.iter_mut().zip(DECORR_G) {
                d = ap.process(d, g);
            }
            let side_total = (s_hp + rt.dec_sm.next_value() * d) * rt.side_sm.next_value();
            let (a, b) = rt.tilt_bq.process(mid + side_total, mid - side_total);
            let (a, b) = rt.pad.process(a, b);
            let (a, b) = (a * rt.trim, b * rt.trim);
            let a = if a.is_finite() { a } else { 0.0 };
            let b = if b.is_finite() { b } else { 0.0 };
            l[n] = a;
            r[n] = b;
            rt.activity.update(a, b);
        }
    }

    fn latency(&self) -> u32 {
        self.rt.as_ref().map_or(0, |r| r.latency)
    }

    fn routing(&self) -> Routing {
        Routing::Stereo
    }

    fn activity(&self) -> f32 {
        self.rt.as_ref().map_or(0.0, |r| r.activity.value())
    }

    fn presence(&self) -> (f32, f32) {
        self.rt.as_ref().map_or((3.0, 2.0), |r| r.common.presence())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::voicing::Voicing;
    use crate::character::{ROUTES_CSV, VOICING_CSV, build_chain};
    use atomik_dsp::Rng;

    fn wide(sr: f32) -> (WideChain, Voicing) {
        let chains: Vec<Box<dyn PresetChain>> = (0..16).map(build_chain).collect();
        let specs: Vec<ChainSpec> = chains.iter().map(|c| c.spec()).collect();
        let v = Voicing::load(VOICING_CSV, ROUTES_CSV, &specs).unwrap();
        let mut c = WideChain::new();
        c.prepare(sr, 5, v.preset(4));
        (c, v)
    }

    fn render(
        c: &mut WideChain,
        v: &Voicing,
        t: f32,
        l: &[f32],
        r: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let presence = [1.0f32; SUB];
        let gain = [1.0f32; SUB];
        let ctx = Ctx {
            presence: &presence,
            gain: &gain,
        };
        let mut m = Morph::default();
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for (cl, cr) in l.chunks(SUB).zip(r.chunks(SUB)) {
            v.preset(4).resolve(t, &mut m);
            let mut a: [f32; SUB] = cl.try_into().unwrap();
            let mut b: [f32; SUB] = cr.try_into().unwrap();
            c.process(&mut a, &mut b, &m, &ctx);
            ol.extend_from_slice(&a);
            or.extend_from_slice(&b);
        }
        (ol, or)
    }

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..n).map(|_| r.next_bipolar() * 0.5).collect()
    }

    #[test]
    fn the_mid_survives_untouched_and_a_mono_source_becomes_pseudo_stereo() {
        let (mut c, v) = wide(48_000.0);
        let x = noise(SUB * 600, 1);
        let (l, r) = render(&mut c, &v, 0.0, &x, &x);
        let lat = c.latency() as usize;
        // Transform 0: the tilt is 0 dB, so (L + R) / 2 is the input delayed (times the trim).
        let trim = ChainCommon::read(v.preset(4)).trim();
        for n in lat + 2000..x.len() {
            let mid = 0.5 * (l[n] + r[n]);
            assert!((mid - x[n - lat] * trim).abs() < 1e-4, "n {n}");
        }
        // The side holds a decorrelated copy: L and R differ although the input is mono.
        let d: f32 = l
            .iter()
            .zip(&r)
            .skip(4000)
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(d > 1.0, "{d}");
    }

    #[test]
    fn width_grows_with_transform() {
        let side_energy = |t: f32| {
            let (mut c, v) = wide(48_000.0);
            let x = noise(SUB * 600, 2);
            let y = noise(SUB * 600, 3);
            let (l, r) = render(&mut c, &v, t, &x, &y);
            let s: f64 = l
                .iter()
                .zip(&r)
                .skip(4000)
                .map(|(a, b)| f64::from((a - b) * 0.5).powi(2))
                .sum();
            let m: f64 = l
                .iter()
                .zip(&r)
                .skip(4000)
                .map(|(a, b)| f64::from((a + b) * 0.5).powi(2))
                .sum();
            s / m
        };
        assert!(side_energy(1.0) > side_energy(0.0) * 1.5);
    }

    #[test]
    fn the_declared_latency_is_the_short_class_and_the_stage_set_is_the_table() {
        let (c, _) = wide(44_100.0);
        assert_eq!(c.latency(), short_latency(44_100.0));
        let names: Vec<_> = c.spec().stages.iter().map(|s| s.name).collect();
        assert_eq!(names, ["ms", "chain"]);
    }

    #[test]
    fn reset_clears_the_state() {
        let (mut c, v) = wide(48_000.0);
        let x = noise(SUB * 200, 4);
        let a = render(&mut c, &v, 0.7, &x, &x);
        c.reset();
        let b = render(&mut c, &v, 0.7, &x, &x);
        assert_eq!(a, b);
    }
}
