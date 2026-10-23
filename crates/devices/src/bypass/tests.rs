//! Tests of the shared bypass wrapper, run against every built-in effect.

use super::*;
use crate::test_support::SR;
use atomik_core::{ParamDescriptor, ParamId};
use atomik_device_sdk::{
    DeviceDescriptor, DeviceKind, Direction, Registry, SocketDescriptor, SocketKind,
};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

const FADE_FRAMES: usize = 240;

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register_all(&mut r).unwrap();
    r
}

fn effect_ids() -> Vec<&'static str> {
    let r = registry();
    let ids: Vec<_> = r
        .type_ids()
        .filter(|t| r.descriptor(t).unwrap().kind == DeviceKind::Effect)
        .collect();
    assert_eq!(ids.len(), 8, "{ids:?}");
    ids
}

fn wrapped(type_id: &str) -> DeviceHarness {
    DeviceHarness::new(registry().create(type_id).unwrap(), SR)
}

fn raw(type_id: &str) -> DeviceHarness {
    use crate::stereo_delay;
    use crate::{character, compressor, distortion, eq, mastering, modulation, reverb};
    let d: Box<dyn Device> = match type_id {
        "atomik.reverb" => Box::new(reverb::Reverb::new()),
        "atomik.compressor" => Box::new(compressor::Compressor::new()),
        "atomik.eq" => Box::new(eq::Equalizer::new()),
        "atomik.distortion" => Box::new(distortion::Distortion::new()),
        "atomik.modulation" => Box::new(modulation::Modulation::new()),
        "atomik.mastering" => Box::new(mastering::Mastering::new()),
        "atomik.character" => Box::new(character::Character::new()),
        "atomik.stereo-delay" => Box::new(stereo_delay::StereoDelay::new()),
        other => panic!("{other}"),
    };
    DeviceHarness::new(d, SR)
}

/// Deterministic noise input per channel.
struct Noise(Rng, Rng);

impl Noise {
    fn new() -> Self {
        Self(Rng::new(11), Rng::new(12))
    }
}

/// One batch step: feeds noise, processes, returns (in, out) per channel.
fn step(h: &mut DeviceHarness, n: &mut Noise, amp: f32) -> ([Batch; 2], [Batch; 2]) {
    let mut x = [[0.0f32; BATCH]; 2];
    let [l, r] = &mut x;
    for (l, r) in l.iter_mut().zip(r.iter_mut()) {
        *l = n.0.next_bipolar() * amp;
        *r = n.1.next_bipolar() * amp;
    }
    *h.input_mut("in_l") = x[0];
    *h.input_mut("in_r") = x[1];
    h.process();
    (x, [*h.output("out_l"), *h.output("out_r")])
}

type Pair = [Vec<f32>; 2];

fn run(h: &mut DeviceHarness, batches: usize, amp: f32) -> (Pair, Pair) {
    let mut n = Noise::new();
    let (mut i, mut o): (Pair, Pair) = ([vec![], vec![]], [vec![], vec![]]);
    for _ in 0..batches {
        let (x, y) = step(h, &mut n, amp);
        for c in 0..2 {
            i[c].extend_from_slice(&x[c]);
            o[c].extend_from_slice(&y[c]);
        }
    }
    (i, o)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn bypass_off_is_bit_identical_to_the_bare_effect() {
    for id in effect_ids() {
        let (mut a, mut b) = (wrapped(id), raw(id));
        for h in [&mut a, &mut b] {
            if id == "atomik.compressor" {
                h.set_param("lookahead", 5.0);
                h.set_param("threshold", -30.0);
            }
            if id == "atomik.reverb" {
                h.set_param("mix", 0.5);
            }
        }
        let (_, oa) = run(&mut a, 60, 0.5);
        let (_, ob) = run(&mut b, 60, 0.5);
        for c in 0..2 {
            assert_eq!(bits(&oa[c]), bits(&ob[c]), "{id} ch {c}");
        }
    }
}

#[test]
fn bypassed_output_is_the_input_delayed_by_the_latency_bit_for_bit() {
    for id in effect_ids() {
        let mut h = wrapped(id);
        if id == "atomik.compressor" {
            h.set_param("lookahead", 5.0);
        }
        h.set_param("bypass", 1.0);
        let (i, o) = run(&mut h, 80, 0.5);
        let lat = h.device().latency_frames() as usize;
        if matches!(
            id,
            "atomik.compressor" | "atomik.mastering" | "atomik.character"
        ) {
            assert!(lat > 0, "{id} should be latent");
        }
        for c in 0..2 {
            for n in 0..i[c].len() {
                let want = if n >= lat { i[c][n - lat] } else { 0.0 };
                assert_eq!(o[c][n].to_bits(), want.to_bits(), "{id} ch {c} n {n}");
            }
        }
    }
}

#[test]
fn cv_outputs_and_meters_come_from_the_effect_while_bypassed() {
    let mut a = wrapped("atomik.compressor");
    let mut b = raw("atomik.compressor");
    for h in [&mut a, &mut b] {
        h.set_param("threshold", -40.0);
        h.set_param("ratio", 8.0);
    }
    a.set_param("bypass", 1.0);
    let mut n = Noise::new();
    let mut m = Noise::new();
    let mut max_gr = 0.0f32;
    for _ in 0..40 {
        step(&mut a, &mut n, 0.8);
        step(&mut b, &mut m, 0.8);
        assert_eq!(bits(a.output("gr_cv_out")), bits(b.output("gr_cv_out")));
        max_gr = a
            .output("gr_cv_out")
            .iter()
            .fold(max_gr, |p, x| p.max(x.abs()));
    }
    assert!(max_gr > 0.0, "the compressor must be working underneath");
    let (mut ma, mut mb) = ([0.0f32; 8], [0.0f32; 8]);
    assert!(a.device().has_meter_values());
    assert_eq!(
        a.device().meter_values(&mut ma),
        b.device().meter_values(&mut mb)
    );
    assert_eq!(ma, mb);
}

// ----- crossfade, against a trivial inner effect -----

static FX_PARAMS: [ParamDescriptor; 1] = [bypass_param(0)];
const fn sock(id: u16, key: &'static str, dir: Direction) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind: SocketKind::Audio,
        direction: dir,
    }
}
static FX_SOCKETS: [SocketDescriptor; 4] = [
    sock(0, "in_l", Direction::Input),
    sock(1, "in_r", Direction::Input),
    sock(2, "out_l", Direction::Output),
    sock(3, "out_r", Direction::Output),
];
static FX: DeviceDescriptor = DeviceDescriptor {
    type_id: "test.invert",
    name: "Invert",
    kind: DeviceKind::Effect,
    version: 1,
    params: &FX_PARAMS,
    sockets: &FX_SOCKETS,
    stages: &[],
};

/// Writes the inverted input.
struct Invert;

impl Device for Invert {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &FX
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {
        panic!("the bypass param must not reach the effect");
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for (i, o) in [(0, 2), (1, 3)] {
            let x = *ctx.input(SocketId(i));
            for (d, s) in ctx.output(SocketId(o)).iter_mut().zip(x) {
                *d = -s;
            }
        }
    }
}

fn constant(h: &mut DeviceHarness, batches: usize, v: f32) -> Vec<f32> {
    let mut out = vec![];
    for _ in 0..batches {
        *h.input_mut("in_l") = [v; BATCH];
        *h.input_mut("in_r") = [v; BATCH];
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    out
}

#[test]
fn switching_ramps_linearly_over_five_ms_and_snaps_exactly() {
    let mut h = DeviceHarness::new(Box::new(Bypassable::new(Invert)), SR);
    let pre = constant(&mut h, 4, 1.0);
    assert!(pre.iter().all(|s| s.to_bits() == (-1.0f32).to_bits()));
    h.set_param("bypass", 1.0);
    let y = constant(&mut h, 8, 1.0);
    // Sample i has m = (i + 1) / 240: wet -1 -> dry +1.
    let step = 2.0 / FADE_FRAMES as f32;
    for (i, s) in y.iter().enumerate().take(FADE_FRAMES - 1) {
        let want = -1.0 + step * (i + 1) as f32;
        assert!((s - want).abs() < 1e-4, "i {i}: {s} vs {want}");
    }
    let done = y
        .iter()
        .position(|s| s.to_bits() == 1.0f32.to_bits())
        .unwrap();
    assert!(
        (FADE_FRAMES - 1..=FADE_FRAMES).contains(&done),
        "done at {done}"
    );
    assert!(y[done..].iter().all(|s| s.to_bits() == 1.0f32.to_bits()));
    assert!(y.windows(2).all(|w| (w[1] - w[0]).abs() <= step * 1.01));
    assert!(y.windows(2).all(|w| w[1] >= w[0]));
    // And back.
    h.set_param("bypass", 0.0);
    let z = constant(&mut h, 8, 1.0);
    assert!(z.windows(2).all(|w| (w[1] - w[0]).abs() <= step * 1.01));
    let done = z
        .iter()
        .position(|s| s.to_bits() == (-1.0f32).to_bits())
        .unwrap();
    assert!((FADE_FRAMES - 1..=FADE_FRAMES).contains(&done));
    assert!(z[done..].iter().all(|s| s.to_bits() == (-1.0f32).to_bits()));
}

#[test]
fn a_device_loaded_bypassed_starts_bypassed_without_a_fade() {
    let mut h = DeviceHarness::new(Box::new(Bypassable::new(Invert)), SR);
    h.set_param("bypass", 1.0);
    assert!(
        constant(&mut h, 1, 1.0)
            .iter()
            .all(|s| s.to_bits() == 1.0f32.to_bits())
    );
    h.device_mut().reset();
    assert!(
        constant(&mut h, 1, 1.0)
            .iter()
            .all(|s| s.to_bits() == 1.0f32.to_bits())
    );
}

#[test]
fn the_effect_keeps_running_while_bypassed() {
    let mut a = wrapped("atomik.reverb");
    let mut b = wrapped("atomik.reverb");
    for h in [&mut a, &mut b] {
        h.set_param("mix", 1.0);
        h.set_param("decay", 3.0);
    }
    let mut na = Noise::new();
    let mut nb = Noise::new();
    // A burst, then silence; `a` is bypassed through the middle of the tail.
    for k in 0..10 {
        let amp = if k < 3 { 0.7 } else { 0.0 };
        step(&mut a, &mut na, amp);
        step(&mut b, &mut nb, amp);
    }
    a.set_param("bypass", 1.0);
    for _ in 0..60 {
        let (_, ya) = step(&mut a, &mut na, 0.0);
        assert!(ya[0].iter().all(|s| *s == 0.0), "dry of silence");
        step(&mut b, &mut nb, 0.0);
    }
    a.set_param("bypass", 0.0);
    // Skip the fade, then the tails must match the never-bypassed reference bit for bit.
    for _ in 0..4 {
        step(&mut a, &mut na, 0.0);
        step(&mut b, &mut nb, 0.0);
    }
    let mut energy = 0.0f32;
    for _ in 0..40 {
        let (_, ya) = step(&mut a, &mut na, 0.0);
        let (_, yb) = step(&mut b, &mut nb, 0.0);
        assert_eq!(bits(&ya[0]), bits(&yb[0]));
        assert_eq!(bits(&ya[1]), bits(&yb[1]));
        energy += ya[0].iter().map(|s| s * s).sum::<f32>();
    }
    assert!(energy > 1e-6, "the tail is still there: {energy}");
}

#[test]
fn every_effect_descriptor_ends_with_the_host_style_bypass_param() {
    let r = registry();
    for id in effect_ids() {
        let d = r.descriptor(id).unwrap();
        let p = d.params.last().unwrap();
        assert_eq!(p.key, "bypass", "{id}");
        assert_eq!(p.name, "Bypass");
        assert_eq!(p.id.0 as usize, d.params.len() - 1, "{id}");
        assert_eq!((p.min, p.max, p.default), (0.0, 1.0, 0.0));
        assert_eq!(p.unit, atomik_core::Unit::Toggle);
        assert!(p.discrete);
        assert_eq!(p.taper, atomik_core::Taper::Linear);
        assert_eq!(d.params.iter().filter(|q| q.key == "bypass").count(), 1);
        assert_eq!(d.validate(), Ok(()));
        for k in ["in_l", "in_r", "out_l", "out_r"] {
            assert!(d.socket_by_key(k).is_some(), "{id} {k}");
        }
        assert!(d.stages.is_empty(), "{id}");
        // The factory builds the wrapper: bypass takes effect.
        let mut h = wrapped(id);
        h.set_param("bypass", 1.0);
        let (i, o) = run(&mut h, 40, 0.5);
        let lat = h.device().latency_frames() as usize;
        assert_eq!(o[0][100].to_bits(), i[0][100 - lat].to_bits(), "{id}");
    }
}

#[test]
fn toggling_bypass_and_processing_never_allocate() {
    for id in effect_ids() {
        let mut h = wrapped(id);
        let mut n = Noise::new();
        for k in 0..40 {
            assert_no_alloc(|| h.set_param("bypass", (k / 5 % 2) as f32));
            step(&mut h, &mut n, 0.5);
        }
        assert_no_alloc(|| h.device_mut().reset());
        step(&mut h, &mut n, 0.5);
    }
}

#[test]
fn a_latency_class_change_while_bypassed_realigns_the_dry() {
    let mut h = wrapped("atomik.character");
    h.set_param("bypass", 1.0);
    let mut n = Noise::new();
    for _ in 0..20 {
        step(&mut h, &mut n, 0.5);
    }
    let short = h.device().latency_frames();
    h.set_param("preset", 8.0);
    let (mut i, mut o) = (vec![], vec![]);
    for _ in 0..300 {
        let (x, y) = step(&mut h, &mut n, 0.5);
        i.extend_from_slice(&x[0]);
        o.extend_from_slice(&y[0]);
    }
    let long = h.device().latency_frames() as usize;
    assert!(long > short as usize + 1000, "{short} -> {long}");
    let from = o.len() - 40 * BATCH;
    for k in from..o.len() {
        assert_eq!(o[k].to_bits(), i[k - long].to_bits(), "k {k}");
    }
}

#[test]
fn the_ring_covers_the_largest_latency_at_every_rate() {
    for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let mut h = DeviceHarness::new(registry().create("atomik.character").unwrap(), sr);
        h.set_param("preset", 8.0);
        h.process();
        let lat = h.device().latency_frames() as usize;
        let need = (sr * RING_SECONDS) as usize;
        assert!(lat * 4 < need.max(MIN_RING), "{sr}: {lat}");
    }
}
