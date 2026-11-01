//! `atomik.dual-layer`: two poly synth layers played together.
//!
//! Each layer is a whole poly synth voice engine ([`crate::poly_synth`]) with its own oscillators,
//! filters, envelopes and LFOs (the params `a.*` and `b.*`, the poly synth's keys under a layer
//! prefix); both hear the same notes, pitch bend, wheel and pedal. Shared params: `voices`
//! (per layer), `master.level`, `bend.range`, `glide`, `legato` and `spread`.
//!
//! * **Layer mode and morph.** `layer.morph` goes from layer A alone (0) to layer B alone (1).
//!   In `morph` mode the layers cross-fade linearly (`1 - m`, `m`). In `stack` mode both play at
//!   full level from the middle: the gains are `min(1, 2(1 - m))` and `min(1, 2m)`. Either way
//!   the ends are exactly one layer, bit for bit as that layer sounds alone.
//! * **Age.** `layer.age` sets the `drift` of both layers: every voice is detuned by a seeded
//!   random amount (up to 20 cents) that wanders slowly. The layers draw from different seeds,
//!   so they drift apart.
//! * **Output.** The layers are summed and limited once by the poly synth's soft limiter (every
//!   output is bounded by 4). Sockets: notes in, the cutoff, pitch, resonance, level and pan CV
//!   (which reach both layers) and the stereo output.
//!
//! The descriptor is built once, on first use, from the poly synth's: the layer params are its
//! params with `a.`/`b.` key prefixes (the strings are leaked once and live for the process).

use std::sync::OnceLock;

use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
};

use crate::poly_synth::{
    BASE_PARAMS, BatchIn, BatchOut, CUTOFF_CV_IN, LEVEL_CV_IN, NOTES_IN, OUT_L, OUT_R,
    P_BEND_RANGE, P_CHORUS_DEPTH, P_CHORUS_MIX, P_CHORUS_RATE, P_CV_LEVEL, P_CV_PAN, P_CV_PITCH,
    P_CV_RESONANCE, P_DELAY_FEEDBACK, P_DELAY_MIX, P_DELAY_TIME, P_DRIFT, P_GLIDE, P_LEGATO,
    P_MASTER, P_OSC1_WAVE, P_OSC2_FINE, P_OSC2_WAVE, P_SPREAD, P_VOICES, PAN_CV_IN, PITCH_CV_IN,
    PolySynth, RESONANCE_CV_IN, SOCKETS, derive_seed, soft_limit,
};

const LAYER_MODES: Unit = Unit::Choice(&["stack", "morph"]);

/// Where a param of the dual layer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Mode,
    Morph,
    Age,
    /// A poly synth param set on both layers.
    Both(u32),
    /// A poly synth param of one layer (0 is A, 1 is B).
    Layer(u32, usize),
}

/// Poly synth params both layers share.
const SHARED: [u32; 6] = [
    P_VOICES,
    P_MASTER,
    P_BEND_RANGE,
    P_GLIDE,
    P_LEGATO,
    P_SPREAD,
];

/// Poly synth params the dual layer does not expose (the CV trims keep their defaults, `drift`
/// is `layer.age` and the effects are the poly synth's).
const HIDDEN: [u32; 11] = [
    P_CV_PITCH,
    P_CV_RESONANCE,
    P_CV_LEVEL,
    P_CV_PAN,
    P_DRIFT,
    P_CHORUS_MIX,
    P_CHORUS_RATE,
    P_CHORUS_DEPTH,
    P_DELAY_MIX,
    P_DELAY_TIME,
    P_DELAY_FEEDBACK,
];

struct Spec {
    descriptor: DeviceDescriptor,
    routes: Vec<Route>,
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    SPEC.get_or_init(|| {
        let mut params = Vec::new();
        let mut routes = Vec::new();
        let mut push = |mut p: ParamDescriptor, route: Route| {
            p.id = ParamId(params.len() as u32);
            params.push(p);
            routes.push(route);
        };
        let own = |key, name, max, default, unit, discrete| ParamDescriptor {
            id: ParamId(0),
            key,
            name,
            min: 0.0,
            max,
            default,
            taper: Taper::Linear,
            unit,
            discrete,
        };
        push(
            own("layer.mode", "Layer mode", 1.0, 1.0, LAYER_MODES, true),
            Route::Mode,
        );
        push(
            own("layer.morph", "Layer morph", 1.0, 0.5, Unit::Percent, false),
            Route::Morph,
        );
        push(
            own("layer.age", "Layer age", 1.0, 0.0, Unit::Percent, false),
            Route::Age,
        );
        for id in SHARED {
            push(BASE_PARAMS[id as usize], Route::Both(id));
        }
        for (layer, prefix) in [(0usize, "a"), (1, "b")] {
            for p in BASE_PARAMS {
                if SHARED.contains(&p.id.0) || HIDDEN.contains(&p.id.0) {
                    continue;
                }
                let mut q = p;
                if layer == 1 {
                    // Layer B starts as a different sound: a triangle against a sine.
                    match p.id.0 {
                        P_OSC1_WAVE => q.default = 2.0,
                        P_OSC2_WAVE => q.default = 3.0,
                        P_OSC2_FINE => q.default = -7.0,
                        _ => {}
                    }
                }
                q.key = leak(format!("{prefix}.{}", p.key));
                q.name = leak(format!("{} {}", prefix.to_uppercase(), p.name));
                push(q, Route::Layer(p.id.0, layer));
            }
        }
        let params: &'static [ParamDescriptor] = Box::leak(params.into_boxed_slice());
        Spec {
            descriptor: DeviceDescriptor {
                stages: &[],
                type_id: "atomik.dual-layer",
                name: "Dual Layer",
                kind: DeviceKind::Instrument,
                version: 1,
                params,
                sockets: &SOCKETS[..8],
            },
            routes,
        }
    })
}

/// Static description of the dual layer (built on first use).
pub fn descriptor() -> &'static DeviceDescriptor {
    &spec().descriptor
}

/// Factory registering [`DualLayer`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: descriptor(),
        create: || Box::new(DualLayer::new()),
    }
}

/// The layer gains for a mode and a morph position `0..=1`.
fn gains(morph_mode: bool, m: f32) -> (f32, f32) {
    let m = m.clamp(0.0, 1.0);
    if morph_mode {
        (1.0 - m, m)
    } else {
        ((2.0 * (1.0 - m)).min(1.0), (2.0 * m).min(1.0))
    }
}

/// The dual layer device.
pub struct DualLayer {
    spec: &'static Spec,
    layers: [PolySynth; 2],
    morph_mode: bool,
    morph: f32,
    seed: u64,
}

impl std::fmt::Debug for DualLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DualLayer")
            .field("morph_mode", &self.morph_mode)
            .field("morph", &self.morph)
            .finish_non_exhaustive()
    }
}

impl Default for DualLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl DualLayer {
    /// A dual layer with default params (call `prepare` before processing). Builds the shared
    /// wave bank on the first call in the process, so create it off the audio thread.
    pub fn new() -> Self {
        let spec = spec();
        let mut d = Self {
            spec,
            layers: [PolySynth::new(), PolySynth::new()],
            morph_mode: true,
            morph: 0.5,
            seed: 0,
        };
        for (i, p) in spec.descriptor.params.iter().enumerate() {
            d.set_param(ParamId(i as u32), p.default);
        }
        d
    }
}

impl Device for DualLayer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &self.spec.descriptor
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.seed = info.seed;
        let sr = info.sample_rate.hz() as f32;
        self.layers[0].prepare_with(sr, info.seed);
        self.layers[1].prepare_with(sr, derive_seed(info.seed, 7_001));
    }

    fn reset(&mut self) {
        self.layers.iter_mut().for_each(Device::reset);
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let Some(route) = self.spec.routes.get(id.0 as usize).copied() else {
            return;
        };
        match route {
            Route::Mode => self.morph_mode = plain >= 0.5,
            Route::Morph => self.morph = plain,
            Route::Age => {
                for l in &mut self.layers {
                    l.set_param(ParamId(P_DRIFT), plain);
                }
            }
            Route::Both(p) => {
                for l in &mut self.layers {
                    l.set_param(ParamId(p), plain);
                }
            }
            Route::Layer(p, layer) => self.layers[layer].set_param(ParamId(p), plain),
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let cutoff = *ctx.input(CUTOFF_CV_IN);
        let pitch = *ctx.input(PITCH_CV_IN);
        let resonance = *ctx.input(RESONANCE_CV_IN);
        let level = *ctx.input(LEVEL_CV_IN);
        let pan = *ctx.input(PAN_CV_IN);
        let (mut a, mut b) = (BatchOut::default(), BatchOut::default());
        {
            let inp = BatchIn {
                events: ctx.notes_in(NOTES_IN),
                cutoff: &cutoff,
                pitch: &pitch,
                resonance: &resonance,
                level: &level,
                pan: &pan,
                resonance_on: ctx.is_connected(RESONANCE_CV_IN),
                level_on: ctx.is_connected(LEVEL_CV_IN),
                pan_on: ctx.is_connected(PAN_CV_IN),
            };
            self.layers[0].render_batch(&inp, false, &mut a);
            self.layers[1].render_batch(&inp, false, &mut b);
        }
        let (ga, gb) = gains(self.morph_mode, self.morph);
        let mix = |x: &[f32; BATCH], y: &[f32; BATCH]| {
            let mut out = [0.0f32; BATCH];
            for ((o, p), q) in out.iter_mut().zip(x).zip(y) {
                *o = soft_limit(p * ga + q * gb);
            }
            out
        };
        *ctx.output(OUT_L) = mix(&a.l, &b.l);
        *ctx.output(OUT_R) = mix(&a.r, &b.r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SR, harness, off, on, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::signal::{peak, rms};

    const OUT: [&str; 2] = ["out_l", "out_r"];

    fn seconds(s: f64) -> usize {
        (s * SR / BATCH as f64).ceil() as usize
    }

    fn dual() -> DeviceHarness {
        harness(Box::new(DualLayer::new()))
    }

    #[test]
    fn the_descriptor_validates_and_prefixes_the_layers() {
        let d = descriptor();
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(d.type_id, "atomik.dual-layer");
        assert_eq!(d.kind, DeviceKind::Instrument);
        for key in [
            "layer.mode",
            "layer.morph",
            "layer.age",
            "voices",
            "master.level",
            "a.osc1.wave",
            "b.osc1.wave",
            "a.filter.cutoff",
            "b.env.mod.dest",
            "a.filter2.mode",
            "b.lfo2.rate",
        ] {
            assert!(d.param_by_key(key).is_some(), "{key}");
        }
        for key in [
            "drift",
            "chorus.mix",
            "pitch_cv_trim",
            "a.drift",
            "b.voices",
        ] {
            assert!(d.param_by_key(key).is_none(), "{key}");
        }
        let layer = |p: &str| d.params.iter().filter(|x| x.key.starts_with(p)).count();
        assert_eq!(layer("a."), layer("b."));
        assert_eq!(
            layer("a."),
            crate::poly_synth::PARAM_COUNT - SHARED.len() - HIDDEN.len()
        );
        assert_eq!(d.sockets.len(), 8);
        // Same descriptor on every call.
        assert!(std::ptr::eq(descriptor(), descriptor()));
    }

    #[test]
    fn morph_at_the_ends_is_each_layer_alone_bit_for_bit() {
        let tune = [
            ("osc1.wave", 0.0),
            ("osc2.wave", 1.0),
            ("filter.cutoff", 1_200.0),
            ("filter.resonance", 0.5),
            ("filter.env_amount", 0.6),
            ("lfo.to_cutoff", 0.5),
            ("lfo.wave", 4.0),
            ("master.level", 3.0),
        ];
        let poly = &crate::poly_synth::DESCRIPTOR;
        for mode in [0.0f32, 1.0] {
            for (layer, morph, seed) in [("a", 0.0f32, 0u64), ("b", 1.0, derive_seed(0, 7_001))] {
                let mut d = dual();
                d.set_param("layer.mode", mode);
                d.set_param("layer.morph", morph);
                // The layer alone is a poly synth with the same params and the layer's seed.
                let mut alone =
                    DeviceHarness::with_options(Box::new(PolySynth::new()), SR, 120.0, seed);
                for p in poly.params {
                    let id = p.id.0;
                    if SHARED.contains(&id) || HIDDEN.contains(&id) {
                        continue;
                    }
                    d.set_param(&format!("{layer}.{}", p.key), p.default);
                }
                for (k, v) in tune {
                    if k == "master.level" {
                        d.set_param(k, v);
                    } else {
                        d.set_param(&format!("{layer}.{k}"), v);
                    }
                    alone.set_param(k, v);
                }
                // The other layer plays something else entirely.
                let against = if layer == "a" { "b" } else { "a" };
                d.set_param(&format!("{against}.osc1.wave"), 3.0);
                d.set_param(&format!("{against}.osc.mix"), 0.0);
                for h in [&mut d, &mut alone] {
                    for (i, key) in [40u8, 47, 52, 59].iter().enumerate() {
                        h.push_note("notes_in", on(i as u16 * 7, *key, 0.9));
                    }
                }
                let (x, y) = (
                    d.render(seconds(0.6), &OUT),
                    alone.render(seconds(0.6), &OUT),
                );
                assert!(rms(&y[0]) > 0.01);
                for ch in 0..2 {
                    assert_eq!(x[ch], y[ch], "mode {mode} layer {layer}, channel {ch}");
                }
            }
        }
    }

    #[test]
    fn stack_and_morph_differ_in_the_middle() {
        assert_eq!(gains(true, 0.5), (0.5, 0.5));
        assert_eq!(gains(false, 0.5), (1.0, 1.0));
        assert_eq!(gains(true, 0.0), (1.0, 0.0));
        assert_eq!(gains(false, 0.0), (1.0, 0.0));
        assert_eq!(gains(true, 1.0), (0.0, 1.0));
        assert_eq!(gains(false, 1.0), (0.0, 1.0));
        assert_eq!(gains(false, 0.25), (1.0, 0.5));
        assert_eq!(gains(true, 3.0), (0.0, 1.0), "clamped");
        let level = |mode: f32| {
            let mut d = dual();
            d.set_param("layer.mode", mode);
            d.set_param("layer.morph", 0.5);
            d.push_note("notes_in", on(0, 52, 1.0));
            rms(&d.render(seconds(0.5), &OUT)[0][9_600..])
        };
        let (stack, morph) = (level(0.0), level(1.0));
        assert!(stack > morph * 1.3, "{stack} vs {morph}");
    }

    #[test]
    fn age_drifts_the_layers_apart_and_zero_is_exact() {
        let render = |age: f32| {
            let mut d = harness(Box::new(DualLayer::new()));
            d.set_param("layer.age", age);
            for (k, v) in [("osc1.wave", 3.0), ("osc.mix", 0.0)] {
                d.set_param(&format!("a.{k}"), v);
                d.set_param(&format!("b.{k}"), v);
            }
            d.set_param("layer.mode", 0.0);
            d.push_note("notes_in", on(0, 57, 1.0));
            d.render(seconds(1.0), &OUT).remove(0)
        };
        let (still, aged) = (render(0.0), render(1.0));
        assert_ne!(still, aged);
        // With age 0 and equal layers both play the same pitch: a pure sine of full level.
        assert_eq!(render(0.0), render(0.0));
        assert!(peak(&aged) < 4.0 && aged.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn it_sweeps_every_param_and_stays_allocation_free() {
        let mut h = dual();
        for k in [24u8, 60, 96] {
            h.push_note("notes_in", on(0, k, 1.0));
        }
        sweep_params_min_max(&mut h, descriptor());
        h.push_note("notes_in", off(0, 60));
        h.render(8, &OUT);
    }

    #[test]
    fn both_layers_keep_pressure_as_a_modulation_source() {
        use atomik_core::{NoteBuffer, NoteEvent, NoteKind, SampleRate, Tempo, TimeInfo};
        let mut d = DualLayer::new();
        let n = descriptor().sockets.len();
        let mut signals = vec![[0.0f32; BATCH]; n];
        let mut notes: Vec<NoteBuffer> = (0..n).map(|_| NoteBuffer::with_capacity(8)).collect();
        for kind in [
            NoteKind::ChannelPressure { value: 0.25 },
            NoteKind::PolyPressure {
                key: 64,
                value: 0.75,
            },
        ] {
            notes[0].push(NoteEvent { offset: 0, kind }).unwrap();
        }
        let connected = vec![false; n];
        let time = TimeInfo::at(
            0,
            SampleRate::new(SR).unwrap(),
            Tempo::new(120.0).unwrap(),
            true,
        );
        let mut ctx = ProcessContext::new(time, &mut signals, &mut notes, &connected);
        d.process(&mut ctx);
        for layer in &d.layers {
            assert_eq!((layer.pressure_on(60), layer.pressure_on(64)), (0.25, 0.75));
        }
    }

    #[test]
    fn the_layers_are_deterministic_for_a_seed() {
        let run = |seed: u64| {
            let mut h = DeviceHarness::with_options(Box::new(DualLayer::new()), SR, 120.0, seed);
            h.set_param("layer.age", 0.8);
            h.set_param("a.noise.level", 0.2);
            h.push_note("notes_in", on(0, 60, 1.0));
            h.render(40, &OUT).remove(0)
        };
        assert_eq!(run(4), run(4));
        assert_ne!(run(4), run(5));
    }
}
