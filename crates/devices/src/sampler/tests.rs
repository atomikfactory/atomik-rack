//! Sampler tests: (playback against analytic expectations) (crossfade)
//! (allocation freedom) (stealing, layering and swaps are deterministic).

use super::*;
use crate::test_support::{
    SR, assert_bend_range, assert_keys, assert_sustain_pedal, assert_volume_and_expression,
    harness, off, on, sweep_params_min_max,
};
use atomik_core::{NoteEvent, NoteKind};
use atomik_device_sdk::{SampleBuffer, velocity_fade};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::{max_step, peak, rms};
use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Test-only window into the voice allocator, updated at the end of `process` and of
/// `swap_samples` (atomic stores only: no allocation).
#[derive(Debug)]
pub struct Probe {
    /// Number of non-idle voices.
    pub active: AtomicUsize,
    /// Non-idle voices playing from the draining map.
    pub draining: AtomicUsize,
    /// Voices cut by a second swap.
    pub cuts: AtomicU64,
    /// Per voice: 0 when idle, else 1 + the key it is playing (or will play after a steal).
    pub keys: [AtomicU32; MAX_VOICES],
}

impl Probe {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            draining: AtomicUsize::new(0),
            cuts: AtomicU64::new(0),
            keys: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
    fn keys(&self) -> Vec<u8> {
        let mut k: Vec<u8> = self
            .keys
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .filter(|v| *v > 0)
            .map(|v| (v - 1) as u8)
            .collect();
        k.sort_unstable();
        k
    }
    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
    fn draining(&self) -> usize {
        self.draining.load(Ordering::Relaxed)
    }
    fn cuts(&self) -> u64 {
        self.cuts.load(Ordering::Relaxed)
    }
}

const OUT: [&str; 2] = ["out_l", "out_r"];
/// Samples after a note start when the 0.5 ms attack (24 samples) is certainly over.
const SETTLED: usize = 40;
const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn buffer(channels: Vec<Vec<f32>>) -> Arc<SampleBuffer> {
    Arc::new(SampleBuffer::new(channels.into()).unwrap())
}

/// `i / 4096`: exactly representable, so every comparison below can be bit-exact.
fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 / 4096.0).collect()
}

/// Deterministic noise in −0.9..0.9.
fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.9
        })
        .collect()
}

fn sine(n: usize, period: f64, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (std::f64::consts::TAU * i as f64 / period).sin() as f32)
        .collect()
}

/// A gate zone over the whole of buffer `buffer` (`frames` long) on every key, root 60.
fn zone(buffer: u32, frames: u64) -> Zone {
    Zone {
        buffer,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 60,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start: 0,
        end: frames,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: atomik_device_sdk::ZoneExt::NONE,
    }
}

fn map(buffers: Vec<Arc<SampleBuffer>>, zones: Vec<Zone>) -> SampleMapRef {
    Arc::new(SampleMap::new(buffers, zones).unwrap())
}

/// One mono ramp zone map.
fn ramp_map(frames: usize, edit: impl Fn(&mut Zone)) -> SampleMapRef {
    let mut z = zone(0, frames as u64);
    edit(&mut z);
    map(vec![buffer(vec![ramp(frames)])], vec![z])
}

/// One mono constant-level zone map (a fresh buffer each call).
fn flat_map(level: f32, frames: usize) -> SampleMapRef {
    map(
        vec![buffer(vec![vec![level; frames]])],
        vec![zone(0, frames as u64)],
    )
}

fn same(a: &SampleMapRef, b: &Option<SampleMapRef>) -> bool {
    b.as_ref().is_some_and(|b| Arc::ptr_eq(a, b))
}

/// A harness around a fresh sampler with the envelope, level and velocity neutralised: attack
/// 0.5 ms, sustain 1, master 0 dB, no velocity sensitivity.
fn sampler() -> (DeviceHarness, Arc<Probe>) {
    let probe = Arc::new(Probe::new());
    let mut s = Sampler::new();
    s.probe = Some(probe.clone());
    let mut h = harness(Box::new(s));
    h.set_param("env.amp.attack", 0.5);
    h.set_param("env.amp.sustain", 1.0);
    h.set_param("env.amp.release", 0.5);
    h.set_param("master.level", 0.0);
    h.set_param("velocity.amp", 0.0);
    (h, probe)
}

fn with_map(m: &SampleMapRef) -> (DeviceHarness, Arc<Probe>) {
    let (mut h, p) = sampler();
    assert!(h.device_mut().swap_samples(Some(m.clone())).is_none());
    (h, p)
}

fn render(h: &mut DeviceHarness, batches: usize) -> (Vec<f32>, Vec<f32>) {
    let mut o = h.render(batches, &OUT);
    let r = o.pop().unwrap();
    (o.pop().unwrap(), r)
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn all_off(offset: u16) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::AllOff,
    }
}

/// A long quiet sine on every key with a 5 ms release.
fn performance_synth() -> DeviceHarness {
    let (mut h, _) = with_map(&map(
        vec![buffer(vec![sine(96_000, 109.0, 0.5)])],
        vec![zone(0, 96_000)],
    ));
    h.set_param("env.amp.release", 5.0);
    h
}

#[test]
fn sustain_pedal_holds_the_release_until_pedal_up() {
    assert_sustain_pedal(performance_synth, &OUT, 60);
}

#[test]
fn volume_and_expression_scale_the_output() {
    assert_volume_and_expression(performance_synth, &OUT, 60);
}

#[test]
fn bend_range_param_and_rpn_zero() {
    assert_bend_range(performance_synth, &OUT, 60);
}

#[test]
fn sustain_pedal_leaves_one_shot_zones_alone() {
    let mut z = zone(0, 96_000);
    z.trigger = TriggerMode::OneShot;
    let make = || {
        let (mut h, _) = with_map(&map(vec![buffer(vec![sine(96_000, 109.0, 0.5)])], vec![z]));
        h.set_param("env.amp.release", 5.0);
        h
    };
    let run = |pedal: bool| {
        let mut h = make();
        if pedal {
            h.push_note(
                "notes_in",
                NoteEvent {
                    offset: 0,
                    kind: NoteKind::Cc {
                        controller: 64,
                        value: 1.0,
                    },
                },
            );
        }
        h.push_note("notes_in", on(1, 60, 1.0));
        h.push_note("notes_in", off(5, 60));
        bits(&h.render(100, &OUT).remove(0))
    };
    assert_eq!(
        run(true),
        run(false),
        "a one-shot ignores the note-off either way"
    );
}

#[test]
fn pressure_reaches_the_modulation_source() {
    let mut s = Sampler::new();
    s.handle(NoteKind::ChannelPressure { value: 0.5 });
    s.handle(NoteKind::PolyPressure {
        key: 60,
        value: 0.75,
    });
    assert_eq!((s.ctl.pressure(60), s.ctl.pressure(61)), (0.75, 0.5));
    s.reset();
    assert_eq!(s.ctl.pressure(60), 0.0);
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "voices",
            "master.level",
            "tune.semi",
            "tune.fine",
            "env.amp.attack",
            "env.amp.decay",
            "env.amp.sustain",
            "env.amp.release",
            "velocity.amp",
            "filter.enabled",
            "filter.mode",
            "filter.cutoff",
            "filter.resonance",
            "filter.env_amount",
            "filter.key_track",
            "filter.velocity",
            "env.filter.attack",
            "env.filter.decay",
            "env.filter.sustain",
            "env.filter.release",
            "lfo.wave",
            "lfo.rate",
            "lfo.to_pitch",
            "lfo.to_cutoff",
            "lfo.to_amp",
            "cv.pitch_amount",
            "cv.cutoff_amount",
            "filter.resonance_cv_trim",
            "master.level_cv_trim",
            "pan_cv_trim",
            "bend.range",
        ],
        &[
            "notes_in",
            "pitch_cv_in",
            "cutoff_cv_in",
            "resonance_cv_in",
            "level_cv_in",
            "pan_cv_in",
            "out_l",
            "out_r",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.sampler");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    let p = |k: &str| {
        let d = DESCRIPTOR.param_by_key(k).unwrap();
        (d.min, d.max, d.default)
    };
    assert_eq!(p("voices"), (1.0, 64.0, 32.0));
    assert_eq!(p("master.level"), (-60.0, 6.0, -6.0));
    assert_eq!(p("filter.enabled"), (0.0, 1.0, 0.0));
    assert_eq!(p("cv.pitch_amount").1, 24.0);
    assert_eq!(factory().descriptor.type_id, "atomik.sampler");
}

// ---- playback against analytic expectations -------------------------------------

#[test]
fn root_key_note_reproduces_the_asset_bit_exactly_from_start() {
    let (l, r) = (noise(3000, 1), noise(3000, 2));
    let mut z = zone(0, 3000);
    z.start = 100;
    let m = map(vec![buffer(vec![l.clone(), r.clone()])], vec![z]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(5, 60, 1.0));
    let (ol, or) = render(&mut h, 4);
    assert!(
        ol[..5].iter().chain(&or[..5]).all(|v| *v == 0.0),
        "silent before the offset"
    );
    assert_eq!(probe.active(), 1);
    // After the 0.5 ms attack the amp envelope is exactly 1: the output is the asset.
    let k = 5 + SETTLED;
    assert_eq!(bits(&ol[k..200]), bits(&l[100 + SETTLED..295]));
    assert_eq!(bits(&or[k..200]), bits(&r[100 + SETTLED..295]));
}

#[test]
fn octave_up_reads_every_second_frame() {
    let m = ramp_map(4000, |_| {});
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 72, 1.0));
    let (ol, or) = render(&mut h, 6);
    let want = ramp(4000);
    for n in SETTLED..300 {
        let mono = want[2 * n] * FRAC_1_SQRT_2;
        assert_eq!(ol[n].to_bits(), mono.to_bits(), "sample {n}");
        assert_eq!(
            ol[n].to_bits(),
            or[n].to_bits(),
            "a mono sample feeds both sides"
        );
    }
}

#[test]
fn transpose_and_tune_params_change_the_ratio_exactly_for_octaves() {
    let want = ramp(4000);
    // Zone transpose of 12, global tune of 12, and zone transpose −12 with tune.semi 24.
    for (zone_transpose, semi) in [(12i8, 0.0f32), (0, 12.0), (-12, 24.0)] {
        let m = ramp_map(4000, |z| z.transpose = zone_transpose);
        let (mut h, _) = with_map(&m);
        h.set_param("tune.semi", semi);
        h.push_note("notes_in", on(0, 60, 1.0));
        let (ol, _) = render(&mut h, 3);
        for n in SETTLED..150 {
            let w = want[2 * n] * FRAC_1_SQRT_2;
            assert_eq!(ol[n].to_bits(), w.to_bits(), "{zone_transpose}/{semi} @{n}");
        }
    }
    // Zone cents and fine tune add: +100 − 100 cents is the root pitch again.
    let m = ramp_map(4000, |z| z.tune_cents = 100.0);
    let (mut h, _) = with_map(&m);
    h.set_param("tune.fine", -100.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 2);
    for n in SETTLED..100 {
        assert_eq!(ol[n].to_bits(), (want[n] * FRAC_1_SQRT_2).to_bits());
    }
}

#[test]
fn pitch_cv_shifts_by_cv_pitch_amount_semitones() {
    let m = ramp_map(4000, |_| {});
    let (mut h, _) = with_map(&m);
    h.set_param("cv.pitch_amount", 12.0);
    *h.input_mut("pitch_cv_in") = [1.0; BATCH];
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 3);
    let want = ramp(4000);
    for n in SETTLED..150 {
        assert_eq!(ol[n].to_bits(), (want[2 * n] * FRAC_1_SQRT_2).to_bits());
    }
}

// ---- CV inputs: resonance, level, pan -------------------------------------------

/// Output after settling for a flat mono zone of `level` with an optional CV on `socket`.
fn flat_with_cv(
    m: &SampleMapRef,
    setup: impl Fn(&mut DeviceHarness),
    socket: &str,
    cv: Option<f32>,
) -> (f32, f32) {
    let (mut h, _) = with_map(m);
    setup(&mut h);
    if let Some(cv) = cv {
        *h.input_mut(socket) = [cv; BATCH];
    }
    h.push_note("notes_in", on(0, 60, 1.0));
    let (l, r) = render(&mut h, 4);
    (l[150], r[150])
}

#[test]
fn level_cv_scales_gain_by_amount_in_db() {
    let m = flat_map(0.1, 4000);
    let at = |cv: Option<f32>, amount: f32| {
        let (l, _) = flat_with_cv(
            &m,
            |h| h.set_param("master.level_cv_trim", amount / 24.0),
            "level_cv_in",
            cv,
        );
        l
    };
    let base = at(None, 12.0);
    assert!((base - 0.1 * FRAC_1_SQRT_2).abs() < 1e-6);
    let up = at(Some(0.5), 12.0); // +6 dB
    assert!((up / base - 10f32.powf(6.0 / 20.0)).abs() < 1e-4, "{up}");
    let down = at(Some(-1.0), 12.0);
    assert!((down / base - 10f32.powf(-12.0 / 20.0)).abs() < 1e-4);
    assert_eq!(
        at(Some(1.0), 0.0).to_bits(),
        base.to_bits(),
        "amount 0 cancels"
    );
    assert_eq!(
        at(Some(7.0), 12.0).to_bits(),
        at(Some(1.0), 12.0).to_bits(),
        "clamped to +1"
    );
    assert!((at(Some(1.0), -12.0) / base - 10f32.powf(-12.0 / 20.0)).abs() < 1e-4);
}

#[test]
fn pan_cv_adds_to_the_zone_pan_and_clamps() {
    let m = flat_map(0.5, 4000);
    let sides = |cv: Option<f32>, amount: f32| {
        let (l, r) = flat_with_cv(&m, |h| h.set_param("pan_cv_trim", amount), "pan_cv_in", cv);
        (l, r)
    };
    let (l0, r0) = sides(None, 1.0);
    assert!((l0 - 0.5 * FRAC_1_SQRT_2).abs() < 1e-6 && l0.to_bits() == r0.to_bits());
    let (l, r) = sides(Some(1.0), 1.0);
    assert!(l.abs() < 1e-6 && (r - 0.5).abs() < 1e-6, "{l} {r}");
    let (l, r) = sides(Some(-1.0), 1.0);
    assert!(r.abs() < 1e-6 && (l - 0.5).abs() < 1e-6, "{l} {r}");
    let (l, r) = sides(Some(0.0), 1.0);
    assert!((l - l0).abs() < 1e-6 && (r - r0).abs() < 1e-6, "centre");
    let (l, r) = sides(Some(1.0), 0.0);
    assert!(
        (l - l0).abs() < 1e-6 && (r - r0).abs() < 1e-6,
        "amount 0 cancels"
    );
    assert_eq!(sides(Some(5.0), 1.0), sides(Some(1.0), 1.0), "clamped");
    let (l, r) = sides(Some(1.0), -1.0);
    assert!(
        r.abs() < 1e-6 && (l - 0.5).abs() < 1e-6,
        "negative amount mirrors"
    );
    // Zone pan 0.5 plus cv -0.5 lands at the centre.
    let flat = map(
        vec![buffer(vec![vec![0.5; 4000]])],
        vec![{
            let mut z = zone(0, 4000);
            z.pan = 0.5;
            z
        }],
    );
    let (l, r) = flat_with_cv(
        &flat,
        |h| h.set_param("pan_cv_trim", 1.0),
        "pan_cv_in",
        Some(-0.5),
    );
    assert!((l - r).abs() < 1e-6, "{l} {r}");
    // A stereo sample is balanced (unity at the centre, the near side stays at 1).
    let st = map(
        vec![buffer(vec![vec![0.5; 4000], vec![0.25; 4000]])],
        vec![zone(0, 4000)],
    );
    let (l, r) = flat_with_cv(
        &st,
        |h| h.set_param("pan_cv_trim", 1.0),
        "pan_cv_in",
        Some(1.0),
    );
    assert!(l.abs() < 1e-6 && (r - 0.25).abs() < 1e-6, "{l} {r}");
    let (l, r) = flat_with_cv(
        &st,
        |h| h.set_param("pan_cv_trim", 1.0),
        "pan_cv_in",
        Some(0.0),
    );
    assert!((l - 0.5).abs() < 1e-6 && (r - 0.25).abs() < 1e-6);
}

#[test]
fn resonance_cv_adds_to_filter_resonance_and_clamps() {
    // A sine of 1200 Hz through a low-pass at 1200 Hz: the gain at the cutoff is Q.
    let m = map(
        vec![buffer(vec![sine(40_000, 40.0, 0.05)])],
        vec![zone(0, 40_000)],
    );
    let level = |cv: Option<f32>, amount: f32| {
        let (mut h, _) = with_map(&m);
        h.set_param("filter.enabled", 1.0);
        h.set_param("filter.cutoff", 1200.0);
        h.set_param("filter.resonance", 0.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("filter.resonance_cv_trim", amount);
        if let Some(cv) = cv {
            *h.input_mut("resonance_cv_in") = [cv; BATCH];
        }
        h.push_note("notes_in", on(0, 60, 1.0));
        let (l, _) = render(&mut h, seconds(0.3));
        rms(&l[6000..])
    };
    let base = level(None, 1.0);
    assert!(level(Some(1.0), 1.0) > base * 5.0);
    assert!(
        (level(Some(1.0), 0.0) - base).abs() < base * 1e-4,
        "amount 0"
    );
    assert!(
        (level(Some(-1.0), 1.0) - base).abs() < base * 1e-4,
        "floor at 0"
    );
    assert_eq!(
        level(Some(4.0), 1.0).to_bits(),
        level(Some(1.0), 1.0).to_bits(),
        "clamped"
    );
}

#[test]
fn new_cv_inputs_unconnected_leave_output_bit_identical() {
    let run = |amounts: Option<[f32; 3]>| {
        let m = map(
            vec![buffer(vec![noise(6000, 3), noise(6000, 4)])],
            vec![zone(0, 6000)],
        );
        let (mut h, _) = with_map(&m);
        h.set_param("filter.enabled", 1.0);
        h.set_param("filter.cutoff", 900.0);
        h.set_param("filter.resonance", 0.6);
        h.set_param("lfo.to_cutoff", 1.0);
        if let Some(a) = amounts {
            for (k, v) in [
                "filter.resonance_cv_trim",
                "master.level_cv_trim",
                "pan_cv_trim",
            ]
            .into_iter()
            .zip(a)
            {
                h.set_param(k, v);
            }
        }
        h.push_note("notes_in", on(0, 60, 0.8));
        h.push_note("notes_in", on(70, 64, 0.8));
        let (l, r) = render(&mut h, 12);
        (bits(&l), bits(&r))
    };
    assert_eq!(run(None), run(Some([-0.8, 20.0, -0.3])));
}

#[test]
fn new_cv_inputs_are_allocation_free() {
    let m = map(
        vec![buffer(vec![noise(6000, 3), noise(6000, 4)])],
        vec![zone(0, 6000)],
    );
    let (mut h, _) = with_map(&m);
    h.set_param("filter.enabled", 1.0);
    for s in ["resonance_cv_in", "level_cv_in", "pan_cv_in"] {
        *h.input_mut(s) = [0.6; BATCH];
    }
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(9, 67, 1.0));
    for _ in 0..30 {
        h.process();
    }
}

#[test]
fn key_distance_from_root_gives_the_equal_tempered_ratio() {
    // A sine of 1/40 cycle per frame at key 60 sounds 5 semitones up at key 65.
    let data = sine(8000, 40.0, 0.5);
    let m = map(vec![buffer(vec![data])], vec![zone(0, 8000)]);
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 65, 1.0));
    let (ol, _) = render(&mut h, seconds(0.1));
    let want = SR / 40.0 * 2f64.powf(5.0 / 12.0);
    let f = atomik_testkit::signal::dominant_frequency(&ol[100..4000], SR);
    assert!((f - want).abs() < 0.01 * want, "{f} Hz, wanted {want}");
}

#[test]
fn forward_loop_wraps_at_exact_frames() {
    let m = ramp_map(200, |z| {
        z.loop_mode = LoopMode::Forward;
        z.loop_start = 50;
        z.loop_end = 100;
    });
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 8);
    let want = ramp(200);
    let at = |k: usize| if k < 100 { k } else { 50 + (k - 50) % 50 };
    for k in SETTLED..400 {
        let w = want[at(k)] * FRAC_1_SQRT_2;
        assert_eq!(ol[k].to_bits(), w.to_bits(), "sample {k}");
    }
    assert_eq!(probe.active(), 1, "a held looped note keeps sounding");
}

#[test]
fn ping_pong_loop_turns_at_exact_frames() {
    let m = ramp_map(200, |z| {
        z.loop_mode = LoopMode::PingPong;
        z.loop_start = 50;
        z.loop_end = 100;
    });
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 8);
    let want = ramp(200);
    // Turning points are the last loop frame (99) and the loop start (50); period 2 * 49.
    let at = |k: usize| {
        if k < 50 {
            k
        } else {
            let n = (k - 50) % 98;
            if n < 49 { 50 + n } else { 50 + 98 - n }
        }
    };
    for k in SETTLED..400 {
        let w = want[at(k)] * FRAC_1_SQRT_2;
        assert_eq!(ol[k].to_bits(), w.to_bits(), "sample {k}");
    }
}

#[test]
fn a_loop_after_the_start_plays_the_run_in_once() {
    let m = ramp_map(300, |z| {
        z.start = 20;
        z.end = 280;
        z.loop_mode = LoopMode::Forward;
        z.loop_start = 100;
        z.loop_end = 120;
    });
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 6);
    let want = ramp(300);
    let at = |k: usize| {
        let p = 20 + k;
        if p < 120 { p } else { 100 + (p - 120) % 20 }
    };
    for k in SETTLED..300 {
        let w = want[at(k)] * FRAC_1_SQRT_2;
        assert_eq!(ol[k].to_bits(), w.to_bits(), "sample {k}");
    }
}

#[test]
fn one_shot_ignores_note_off_and_gate_releases() {
    for (trigger, sounds_after_off) in [(TriggerMode::OneShot, true), (TriggerMode::Gate, false)] {
        let m = ramp_map(4000, |z| z.trigger = trigger);
        let (mut h, probe) = with_map(&m);
        h.push_note("notes_in", on(0, 60, 1.0));
        render(&mut h, 2);
        h.push_note("notes_in", off(0, 60));
        render(&mut h, 2);
        let (ol, _) = render(&mut h, 4);
        assert_eq!(probe.active() == 1, sounds_after_off, "{trigger:?}");
        assert_eq!(
            ol.iter().any(|v| *v != 0.0),
            sounds_after_off,
            "{trigger:?}"
        );
    }
}

#[test]
fn all_off_releases_one_shots_too() {
    let m = ramp_map(40_000, |z| z.trigger = TriggerMode::OneShot);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 2);
    h.push_note("notes_in", all_off(0));
    render(&mut h, 4);
    assert_eq!(probe.active(), 0);
}

#[test]
fn a_voice_reaching_end_with_the_loop_off_goes_idle() {
    let m = ramp_map(100, |_| {});
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 1);
    assert_eq!(probe.active(), 1, "100 frames end inside the second batch");
    assert!(ol.iter().any(|v| *v != 0.0));
    render(&mut h, 1);
    assert_eq!(probe.active(), 0);
    let (ol, _) = render(&mut h, 1);
    assert!(ol.iter().all(|v| *v == 0.0));
}

#[test]
fn a_voice_reaching_end_with_the_loop_off_fades_to_zero_over_the_last_frames() {
    // 48 kHz: the fade is min(64 frames, 1.5 ms = 72 frames) = 64 frames, linear, zero at end.
    let level = 0.5f32;
    let m = map(
        vec![buffer(vec![vec![level; 600], vec![level; 600]])],
        vec![zone(0, 600)],
    );
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, or) = render(&mut h, 12);
    for (k, v) in ol.iter().enumerate().take(536).skip(SETTLED) {
        assert_eq!(
            v.to_bits(),
            level.to_bits(),
            "full level before the fade, {k}"
        );
    }
    for k in 536..600 {
        let want = level * (600 - k) as f32 / 64.0;
        assert!(
            (ol[k] - want).abs() < 1e-6,
            "frame {k}: {} vs {want}",
            ol[k]
        );
        assert_eq!(ol[k].to_bits(), or[k].to_bits());
    }
    assert!(ol[599] < level / 60.0, "the last frame is nearly silent");
    assert!(ol[600..].iter().all(|v| *v == 0.0), "silent from end");
    // The step into silence is at most one fade step, not the full level.
    assert!(max_step(&ol[536..640]) < level / 60.0);
}

#[test]
fn the_end_fade_is_short_capped_by_the_zone_and_the_rate() {
    assert_eq!(end_fade_len(0, 10_000, 48_000.0), 64);
    assert_eq!(end_fade_len(0, 10_000, 192_000.0), 64, "frames cap");
    assert_eq!(end_fade_len(0, 10_000, 22_050.0), 33, "1.5 ms cap");
    assert_eq!(
        end_fade_len(100, 120, 48_000.0),
        20,
        "never longer than the zone"
    );
    assert_eq!(end_fade_len(5, 5, 48_000.0), 1, "at least one frame");
    assert_eq!(end_fade_gain(0, 0.0, 100, 10), 1.0);
    assert_eq!(end_fade_gain(90, 0.0, 100, 10), 1.0);
    assert!((end_fade_gain(95, 0.5, 100, 10) - 0.45).abs() < 1e-6);
    assert_eq!(end_fade_gain(100, 0.0, 100, 10), 0.0);
    assert_eq!(end_fade_gain(120, 0.0, 100, 10), 0.0);
}

#[test]
fn a_looped_zone_has_no_end_fade() {
    let m = ramp_map(200, |z| {
        z.loop_mode = LoopMode::Forward;
        z.loop_start = 50;
        z.loop_end = 200;
    });
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 12);
    let want = ramp(200);
    // Frames 136..200 would be inside a fade; they are the plain asset.
    for k in 136..200 {
        assert_eq!(ol[k].to_bits(), (want[k] * FRAC_1_SQRT_2).to_bits(), "{k}");
    }
}

#[test]
fn no_map_and_no_matching_zone_are_silent() {
    let (mut h, probe) = sampler();
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 2);
    assert!(ol.iter().all(|v| *v == 0.0) && probe.active() == 0);
    let mut z = zone(0, 500);
    z.key_lo = 70;
    z.vel_lo = 100;
    let m = map(vec![buffer(vec![ramp(500)])], vec![z]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 72, 0.2));
    let (ol, _) = render(&mut h, 2);
    assert!(ol.iter().all(|v| *v == 0.0) && probe.active() == 0);
    let empty = map(vec![], vec![]);
    let (mut h, _) = with_map(&empty);
    h.push_note("notes_in", on(0, 60, 1.0));
    assert!(render(&mut h, 2).0.iter().all(|v| *v == 0.0));
}

// ---- pan, gain, velocity ------------------------------------------------------------------

#[test]
fn zone_gain_pan_and_velocity_shape_the_output() {
    let data = vec![0.5f32; 600];
    let mono = buffer(vec![data.clone()]);
    let stereo = buffer(vec![data.clone(), data]);
    let level = |buf: &Arc<SampleBuffer>, edit: &dyn Fn(&mut Zone), vel: f32, va: f32| {
        let mut z = zone(0, 600);
        edit(&mut z);
        let m = map(vec![buf.clone()], vec![z]);
        let (mut h, _) = with_map(&m);
        h.set_param("velocity.amp", va);
        h.push_note("notes_in", on(0, 60, vel));
        let (l, r) = render(&mut h, 3);
        (l[100], r[100])
    };
    let (l, r) = level(&mono, &|_| {}, 1.0, 0.0);
    assert!((l - 0.5 * FRAC_1_SQRT_2).abs() < 1e-6 && l.to_bits() == r.to_bits());
    let (l, r) = level(&mono, &|z| z.pan = -1.0, 1.0, 0.0);
    assert!((l - 0.5).abs() < 1e-6 && r.abs() < 1e-6);
    let (l, r) = level(&stereo, &|_| {}, 1.0, 0.0);
    assert_eq!((l, r), (0.5, 0.5));
    let (l, r) = level(&stereo, &|z| z.pan = 1.0, 1.0, 0.0);
    assert!(l.abs() < 1e-6 && (r - 0.5).abs() < 1e-6);
    let (l, _) = level(&stereo, &|z| z.gain_db = -6.0, 1.0, 0.0);
    assert!((l - 0.5 * db_to_gain(-6.0)).abs() < 1e-6);
    let vel = 64.0 / 127.0;
    let (l, _) = level(&stereo, &|_| {}, vel, 1.0);
    assert!(
        (l - 0.5 * vel).abs() < 1e-6,
        "velocity.amp = 1 follows the velocity"
    );
}

// ---- layering and stealing ----------------------------------------------------------

#[test]
fn every_matching_zone_sounds_and_layers_sum() {
    let mut z1 = zone(1, 500);
    z1.key_lo = 60;
    z1.key_hi = 60;
    let mut z2 = zone(2, 500);
    z2.vel_lo = 100;
    let m = map(
        vec![
            buffer(vec![vec![0.25f32; 500]]),
            buffer(vec![vec![0.125f32; 500]]),
            buffer(vec![vec![0.0625f32; 500]]),
        ],
        vec![zone(0, 500), z1, z2],
    );
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 3);
    assert_eq!(probe.active(), 3);
    let want = (0.25 + 0.125 + 0.0625) * FRAC_1_SQRT_2;
    assert!((ol[100] - want).abs() < 1e-6, "{} vs {want}", ol[100]);
    // A soft note on another key matches zone 0 only.
    h.push_note("notes_in", on(0, 61, 0.1));
    render(&mut h, 1);
    assert_eq!(probe.active(), 4);
}

#[test]
fn the_oldest_voice_is_stolen_with_a_two_ms_fade() {
    let m = flat_map(0.5, 40_000);
    let (mut h, probe) = with_map(&m);
    h.set_param("voices", 2.0);
    for (i, key) in [60u8, 62, 64].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.8));
    }
    let (ol, _) = render(&mut h, 4); // > 2 ms steal fade
    assert_eq!(probe.active(), 2);
    assert_eq!(probe.keys(), vec![62, 64], "key 60 was the oldest");
    assert!(ol.iter().all(|v| v.is_finite()));
    // The steal fade is a 2 ms ramp (96 frames at 48 kHz), not a cut; the largest step left is
    // the new note's own 0.5 ms attack (0.5 / 24 per sample).
    assert!(
        max_step(&ol[100..]) < 0.03,
        "steal click {}",
        max_step(&ol[100..])
    );
}

#[test]
fn a_note_with_more_zones_than_voices_steals_inside_itself_deterministically() {
    let zones: Vec<Zone> = (0..5).map(|_| zone(0, 20_000)).collect();
    let m = map(vec![buffer(vec![noise(20_000, 9)])], zones);
    let run = || {
        let (mut h, probe) = with_map(&m);
        h.set_param("voices", 3.0);
        h.push_note("notes_in", on(0, 60, 1.0));
        let (l, r) = render(&mut h, 10);
        assert_eq!(probe.active(), 3);
        (bits(&l), bits(&r))
    };
    assert_eq!(run(), run());
}

#[test]
fn identical_runs_are_bit_identical_with_everything_switched_on() {
    let mut z0 = zone(0, 6000);
    z0.loop_mode = LoopMode::Forward;
    z0.loop_start = 1000;
    z0.loop_end = 3000;
    z0.crossfade = 500;
    z0.pan = -0.4;
    let mut z1 = zone(1, 6000);
    z1.loop_mode = LoopMode::PingPong;
    z1.loop_start = 500;
    z1.loop_end = 4000;
    z1.tune_cents = 13.0;
    z1.pan = 0.6;
    let m = map(
        vec![
            buffer(vec![noise(6000, 1)]),
            buffer(vec![noise(6000, 2), noise(6000, 3)]),
        ],
        vec![z0, z1],
    );
    let run = || {
        let (mut h, _) = with_map(&m);
        h.set_param("voices", 4.0);
        h.set_param("filter.enabled", 1.0);
        h.set_param("filter.cutoff", 1500.0);
        h.set_param("filter.env_amount", 0.5);
        h.set_param("filter.key_track", 0.7);
        h.set_param("filter.velocity", 0.5);
        h.set_param("lfo.to_pitch", 0.4);
        h.set_param("lfo.to_cutoff", 1.0);
        h.set_param("lfo.to_amp", 0.3);
        h.set_param("env.amp.decay", 80.0);
        h.set_param("env.amp.sustain", 0.6);
        *h.input_mut("pitch_cv_in") = [0.1; BATCH];
        let mut out = Vec::new();
        for b in 0..60u16 {
            if b % 11 == 5 {
                h.push_note("notes_in", off(0, 48 + ((b - 5) % 20) as u8));
            }
            if b % 7 == 0 {
                let vel = 0.4 + f32::from(b % 5) * 0.1;
                h.push_note("notes_in", on(b % 50, 48 + (b % 20) as u8, vel));
            }
            let (l, r) = render(&mut h, 1);
            out.extend(bits(&l));
            out.extend(bits(&r));
        }
        out
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    assert!(
        a.iter().any(|v| f32::from_bits(*v).abs() > 0.01),
        "the run is audible"
    );
}

// ---- crossfade -------------------------------------------------------------------------

fn looped_sine_map(crossfade: u64) -> SampleMapRef {
    // Period 37 frames does not divide the 500-frame loop.
    let mut z = zone(0, 2000);
    z.loop_mode = LoopMode::Forward;
    z.loop_start = 600;
    z.loop_end = 1100;
    z.crossfade = crossfade;
    map(vec![buffer(vec![sine(2000, 37.0, 0.5)])], vec![z])
}

#[test]
fn a_crossfaded_loop_has_a_smaller_step_at_the_loop_point() {
    let run = |xf| {
        let (mut h, _) = with_map(&looped_sine_map(xf));
        h.push_note("notes_in", on(0, 60, 1.0));
        render(&mut h, 40).0 // 2560 samples: the loop wraps at samples 1100, 1600, 2100
    };
    let (plain, faded) = (run(0), run(200));
    let step = |x: &[f32], k: usize| (x[k] - x[k - 1]).abs();
    for wrap in [1100, 1600, 2100] {
        assert!(step(&plain, wrap) > 0.1, "premise: the hard loop clicks");
        assert!(step(&faded, wrap) < 0.5 * step(&plain, wrap), "wrap {wrap}");
    }
    assert!(max_step(&faded[700..]) < max_step(&plain[700..]));
    assert!(rms(&faded[700..]) > 0.2, "the faded loop is still loud");
}

// ---- swapping maps ----------------------------------------------------------------

#[test]
fn without_voices_the_replaced_map_comes_back_at_once() {
    let (mut h, probe) = sampler();
    let a = ramp_map(500, |_| {});
    let b = ramp_map(500, |_| {});
    assert!(h.device_mut().swap_samples(Some(a.clone())).is_none());
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(same(&a, &back), "nobody needs `a`: it is handed back");
    let back = h.device_mut().swap_samples(None);
    assert!(same(&b, &back));
    assert!(h.device_mut().swap_samples(None).is_none());
    assert_eq!(probe.cuts(), 0);
}

#[test]
fn voices_migrate_to_a_map_that_shares_their_buffer() {
    let buf = buffer(vec![ramp(8000)]);
    let a = map(vec![buf.clone()], vec![zone(0, 8000)]);
    let b = map(vec![buf.clone()], vec![zone(0, 8000), zone(0, 8000)]);
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (before, _) = render(&mut h, 3);
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(same(&a, &back), "the voice moved to `b`: `a` is not needed");
    assert_eq!(probe.draining(), 0);
    let (after, _) = render(&mut h, 3);
    // The note continues in place: sample 192 of the ramp, no jump.
    let want = ramp(8000);
    assert_eq!(before[191].to_bits(), (want[191] * FRAC_1_SQRT_2).to_bits());
    for n in 0..100 {
        assert_eq!(
            after[n].to_bits(),
            (want[192 + n] * FRAC_1_SQRT_2).to_bits()
        );
    }
    assert_eq!(probe.active(), 1);
}

#[test]
fn a_migrated_voice_adopts_the_new_zone_parameters() {
    let buf = buffer(vec![vec![0.5f32; 8000]]);
    let a = map(vec![buf.clone()], vec![zone(0, 8000)]);
    let mut z = zone(0, 8000);
    z.gain_db = -6.0;
    z.pan = -1.0;
    let b = map(vec![buf.clone()], vec![z]);
    let (mut h, _) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 3);
    assert!(h.device_mut().swap_samples(Some(b)).is_some());
    let (l, r) = render(&mut h, 1);
    assert!((l[10] - 0.5 * db_to_gain(-6.0)).abs() < 1e-6 && r[10].abs() < 1e-6);
}

#[test]
fn a_voice_whose_zone_vanished_drains_over_five_ms_from_the_old_map() {
    let a = flat_map(0.5, 20_000);
    let b = flat_map(0.25, 20_000);
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 3);
    assert!(
        h.device_mut().swap_samples(Some(b.clone())).is_none(),
        "`a` is draining"
    );
    assert_eq!(probe.draining(), 1);
    // 5 ms = 240 frames: still the old sample (0.5), fading out linearly.
    let (l, _) = render(&mut h, 3);
    assert!((l[0] - 0.5 * FRAC_1_SQRT_2).abs() < 1e-6, "{}", l[0]);
    assert!(
        (l[1] - 0.5 * FRAC_1_SQRT_2 * (1.0 - 1.0 / 240.0)).abs() < 1e-5,
        "{}",
        l[1]
    );
    assert!(l.windows(2).all(|w| w[1] <= w[0]), "monotone fade");
    assert!(l[191] > 0.0 && l[191] < l[0] * 0.3);
    let (l, _) = render(&mut h, 1);
    assert!(l[48..].iter().all(|v| *v == 0.0), "silent after 240 frames");
    assert_eq!((probe.draining(), probe.active()), (0, 0));
    // New notes play the new map.
    h.push_note("notes_in", on(0, 60, 1.0));
    let (l, _) = render(&mut h, 2);
    assert!((l[100] - 0.25 * FRAC_1_SQRT_2).abs() < 1e-6);
}

#[test]
fn a_second_swap_while_voices_drain_cuts_them_and_returns_the_draining_map() {
    let (a, b, c) = (
        flat_map(0.5, 20_000),
        flat_map(0.25, 20_000),
        flat_map(0.1, 20_000),
    );
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(0, 64, 1.0));
    render(&mut h, 3);
    assert!(h.device_mut().swap_samples(Some(b.clone())).is_none());
    render(&mut h, 1); // still draining (64 of 240 frames)
    assert_eq!(probe.draining(), 2);
    let back = h.device_mut().swap_samples(Some(c.clone()));
    assert!(same(&a, &back), "the draining map is handed back");
    assert_eq!(probe.cuts(), 2, "both draining voices were cut");
    assert_eq!((probe.active(), probe.draining()), (0, 0));
    let (l, _) = render(&mut h, 1);
    assert!(l.iter().all(|v| *v == 0.0));
}

#[test]
fn a_stale_draining_map_is_handed_back_and_the_replaced_map_takes_its_place() {
    let a = flat_map(0.5, 20_000);
    let (b, c, d) = (
        ramp_map(500, |_| {}),
        ramp_map(500, |_| {}),
        ramp_map(500, |_| {}),
    );
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 2);
    assert!(h.device_mut().swap_samples(Some(b.clone())).is_none());
    render(&mut h, 6); // the drain is over: `a` is stale in the device
    assert_eq!(probe.draining(), 0);
    // Nobody needs `b`, yet `a` has to come back: exactly one map is returned, `b` is kept.
    let back = h.device_mut().swap_samples(Some(c.clone()));
    assert!(same(&a, &back));
    assert_eq!(probe.cuts(), 0, "no voice was cut");
    let back = h.device_mut().swap_samples(Some(d.clone()));
    assert!(same(&b, &back), "`b` was held as the stale draining map");
    let back = h.device_mut().swap_samples(None);
    assert!(same(&c, &back));
    let back = h.device_mut().swap_samples(None);
    assert!(same(&d, &back));
    assert!(h.device_mut().swap_samples(None).is_none());
}

#[test]
fn removing_the_map_drains_voices_and_silences_new_notes() {
    let a = ramp_map(20_000, |_| {});
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 3);
    assert!(h.device_mut().swap_samples(None).is_none());
    assert_eq!(probe.draining(), 1);
    render(&mut h, 5);
    assert_eq!(probe.active(), 0);
    h.push_note("notes_in", on(0, 60, 1.0));
    assert!(render(&mut h, 2).0.iter().all(|v| *v == 0.0));
}

#[test]
fn reset_silences_voices_but_keeps_the_maps() {
    let a = ramp_map(20_000, |_| {});
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 2);
    h.device_mut().reset();
    assert_eq!(probe.active(), 0);
    assert!(render(&mut h, 1).0.iter().all(|v| *v == 0.0));
    h.push_note("notes_in", on(0, 60, 1.0));
    let (l, _) = render(&mut h, 2);
    assert!(l[100] > 0.0);
}

#[test]
fn a_note_pending_after_a_steal_survives_only_a_matching_swap() {
    let buf = buffer(vec![vec![0.5f32; 20_000]]);
    let a = map(vec![buf.clone()], vec![zone(0, 20_000)]);
    let mut z = zone(0, 20_000);
    z.key_hi = 10;
    let other = map(vec![buf.clone()], vec![z]);
    let (mut h, probe) = with_map(&a);
    h.set_param("voices", 1.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(8, 62, 1.0)); // steals: key 62 is pending on the same voice
    render(&mut h, 1);
    assert_eq!(probe.keys(), vec![62]);
    // The new map shares the buffer but its zone no longer matches key 62: the pending note is
    // dropped, the stolen voice finishes its fade and ends.
    let _ = h.device_mut().swap_samples(Some(other));
    render(&mut h, 2);
    assert_eq!(probe.active(), 0);
}

#[test]
fn swap_samples_never_allocates() {
    let buf = buffer(vec![ramp(8000)]);
    let a = map(vec![buf.clone()], vec![zone(0, 8000), zone(0, 8000)]);
    let b = map(vec![buf.clone()], vec![zone(0, 8000)]);
    let c = ramp_map(8000, |_| {});
    let (mut h, _) = with_map(&a);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 61, 1.0));
    render(&mut h, 2);
    // Migrating, draining, cutting, handing back a stale map and removing, all under the guard.
    for next in [Some(b), Some(c.clone()), Some(a), None, Some(c)] {
        let back = assert_no_alloc(|| h.device_mut().swap_samples(next));
        drop(back); // the caller frees maps off the audio thread
        render(&mut h, 1);
    }
}

// ---- finiteness and the filter ------------------------------------------------------------------

#[test]
fn parameter_sweeps_with_sounding_looped_voices_stay_finite() {
    let m = looped_sine_map(100);
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(0, 64, 0.5));
    h.set_param("filter.enabled", 1.0);
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn the_filter_is_bypassed_by_default_and_darkens_the_sound_when_on() {
    let m = map(vec![buffer(vec![noise(30_000, 5)])], vec![zone(0, 30_000)]);
    let run = |enabled: f32| {
        let (mut h, _) = with_map(&m);
        h.set_param("filter.enabled", enabled);
        h.set_param("filter.cutoff", 500.0);
        h.set_param("filter.resonance", 0.0);
        h.push_note("notes_in", on(0, 60, 1.0));
        render(&mut h, 20).0
    };
    let (dry, wet) = (run(0.0), run(1.0));
    let bright = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
    assert!(bright(&wet[300..]) < 0.3 * bright(&dry[300..]));
    assert!(peak(&dry) > 0.4);
}

#[derive(Debug, Clone)]
enum Ev {
    On(u8, f32),
    Off(u8),
    AllOff,
    Swap(usize),
}

/// A zone made valid the way a builder does: ordered fields inside a 2000-frame buffer.
fn zone_strategy() -> impl Strategy<Value = Zone> {
    (
        (0u8..128, 0u8..128, 1u8..128, 1u8..128, 0u8..128),
        (
            -48i8..=48,
            -100.0f32..=100.0,
            -60.0f32..=12.0,
            -1.0f32..=1.0,
        ),
        (
            0u64..2000,
            1u64..2000,
            0u8..3,
            0u64..2000,
            0u64..2000,
            0u64..3000,
            any::<bool>(),
        ),
    )
        .prop_map(
            |((k0, k1, v0, v1, root), (tr, cents, gain, pan), (s, len, lm, ls, ll, xf, one))| {
                let start = s.min(1998);
                let end = (start + len).min(2000).max(start + 1);
                let loop_start = start + ls % (end - start);
                let loop_end = loop_start + 1 + ll % (end - loop_start);
                let crossfade = xf.min(loop_end - loop_start).min(loop_start - start);
                Zone {
                    buffer: 0,
                    key_lo: k0.min(k1),
                    key_hi: k0.max(k1),
                    vel_lo: v0.min(v1),
                    vel_hi: v0.max(v1),
                    root_key: root,
                    transpose: tr,
                    tune_cents: cents,
                    gain_db: gain,
                    pan,
                    start,
                    end,
                    loop_mode: [LoopMode::Off, LoopMode::Forward, LoopMode::PingPong][lm as usize],
                    loop_start,
                    loop_end,
                    crossfade,
                    trigger: if one {
                        TriggerMode::OneShot
                    } else {
                        TriggerMode::Gate
                    },
                    reverse: false,
                    ext: atomik_device_sdk::ZoneExt::NONE,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn random_zones_notes_params_and_swaps_stay_finite_and_bounded(
        zones_a in prop::collection::vec(zone_strategy(), 0..6),
        zones_b in prop::collection::vec(zone_strategy(), 0..6),
        norms in prop::collection::vec(0.0f32..=1.0, PARAM_COUNT),
        events in prop::collection::vec(
            (0usize..60, 0u16..64, prop_oneof![
                4 => (0u8..128, 0.0f32..=1.0).prop_map(|(k, v)| Ev::On(k, v)),
                2 => (0u8..128).prop_map(Ev::Off),
                1 => Just(Ev::AllOff),
                1 => (0usize..3).prop_map(Ev::Swap),
            ]),
            0..60,
        ),
        cv in -1.5f32..=1.5,
        stereo in any::<bool>(),
    ) {
        let data: Vec<Vec<f32>> = if stereo {
            vec![noise(2000, 1), noise(2000, 2)]
        } else {
            vec![noise(2000, 1)]
        };
        let shared = buffer(data);
        let maps = [
            map(vec![shared.clone()], zones_a),
            map(vec![buffer(vec![noise(2000, 7)])], zones_b.clone()),
            map(vec![shared.clone()], zones_b),
        ];
        let (mut h, probe) = sampler();
        drop(h.device_mut().swap_samples(Some(maps[0].clone())));
        for (p, n) in PARAMS.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        *h.input_mut("pitch_cv_in") = [cv; BATCH];
        *h.input_mut("cutoff_cv_in") = [cv; BATCH];
        let mut events = events;
        events.sort_by_key(|(b, o, _)| (*b, *o));
        let mut next = 0;
        for b in 0..70 {
            while next < events.len() && events[next].0 == b {
                let offset = events[next].1;
                match &events[next].2 {
                    Ev::On(key, velocity) => h.push_note("notes_in", NoteEvent {
                        offset,
                        kind: NoteKind::On { key: *key, velocity: *velocity },
                    }),
                    Ev::Off(key) => h.push_note("notes_in", off(offset, *key)),
                    Ev::AllOff => h.push_note("notes_in", all_off(offset)),
                    Ev::Swap(i) => drop(h.device_mut().swap_samples(Some(maps[*i].clone()))),
                }
                next += 1;
            }
            h.process();
            for key in OUT {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 4.0, "{key}: {s}");
                }
            }
            prop_assert!(probe.active() <= MAX_VOICES);
        }
    }
}

// ---- reverse through the shared playhead ----------------------------------------

#[test]
fn a_reversed_zone_at_the_root_key_plays_the_slice_backwards_bit_exactly() {
    let (l, r) = (noise(3000, 1), noise(3000, 2));
    let mut z = zone(0, 3000);
    z.start = 100;
    z.end = 2900;
    z.reverse = true;
    // Loop fields are ignored (and need not be valid) when reversed.
    z.loop_mode = LoopMode::Forward;
    z.loop_start = 5000;
    let m = map(vec![buffer(vec![l.clone(), r.clone()])], vec![z]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(5, 60, 1.0));
    let (ol, or) = render(&mut h, 4);
    assert_eq!(probe.active(), 1);
    // out[5 + t] = data[end - 1 - t] once the attack is over.
    for n in 5 + SETTLED..200 {
        let t = n - 5;
        assert_eq!(ol[n].to_bits(), l[2899 - t].to_bits(), "left {n}");
        assert_eq!(or[n].to_bits(), r[2899 - t].to_bits(), "right {n}");
    }
}

#[test]
fn a_reversed_zone_fades_out_at_its_start_and_then_goes_idle() {
    let m = ramp_map(1000, |z| {
        z.start = 200;
        z.end = 400;
        z.reverse = true;
    });
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 4);
    assert_eq!(probe.active(), 0, "idle after the 200 frames");
    // The last frames are faded linearly (64 frames at 48 kHz; the fade is at `start`).
    let tail = &ol[190..200];
    assert!(tail.iter().all(|v| v.is_finite()));
    assert!(tail.windows(2).all(|w| w[1].abs() <= w[0].abs() + 1e-9));
    let want = ramp(1000)[200] * FRAC_1_SQRT_2 / 64.0;
    assert!((ol[199] - want).abs() < 1e-6, "{} vs {want}", ol[199]);
    assert!(ol[200..].iter().all(|v| *v == 0.0));
}

#[test]
fn a_forward_zone_is_unchanged_by_the_reverse_field() {
    let a = ramp_map(1000, |_| {});
    let b = ramp_map(1000, |z| z.reverse = false);
    let run = |m: &SampleMapRef| {
        let (mut h, _) = with_map(m);
        h.push_note("notes_in", on(0, 64, 1.0));
        render(&mut h, 4).0
    };
    assert_eq!(bits(&run(&a)), bits(&run(&b)));
}

// ---- groups, velocity crossfade, round-robin, per-zone envelope and filter -----------------------

use atomik_device_sdk::{ZoneEnv, ZoneFilter};

/// The level of the settled output of one short note on `key` (left channel), then the note-off
/// and a short tail so the next note starts from silence.
fn note_level(h: &mut DeviceHarness, key: u8, velocity: f32) -> f32 {
    h.push_note("notes_in", on(0, key, velocity));
    let (l, _) = render(h, 2);
    let level = l[SETTLED + 8];
    h.push_note("notes_in", off(0, key));
    render(h, 3);
    level
}

fn flat_buffers(levels: &[f32], frames: usize) -> Vec<Arc<SampleBuffer>> {
    levels
        .iter()
        .map(|l| buffer(vec![vec![*l; frames]]))
        .collect()
}

#[test]
fn velocity_crossfaded_neighbours_play_at_constant_power() {
    // Two flat zones, 1..=70 and 60..=127, each fading over the 10 shared velocities.
    let mk = |lo: u8, hi: u8, xfade: u8| {
        let mut z = zone(0, 4000);
        (z.vel_lo, z.vel_hi, z.ext.vel_xfade) = (lo, hi, xfade);
        map(flat_buffers(&[0.05], 4000), vec![z])
    };
    let full = {
        let (mut h, _) = with_map(&mk(1, 127, 0));
        note_level(&mut h, 60, 1.0)
    };
    assert!(full > 0.0);
    for vel7 in 60..=70u8 {
        let v = f32::from(vel7) / 127.0;
        let level = |m: &SampleMapRef| {
            let (mut h, _) = with_map(m);
            note_level(&mut h, 60, v)
        };
        let (a, b) = (level(&mk(1, 70, 10)), level(&mk(60, 127, 10)));
        let (fa, fb) = (
            velocity_fade(1, 70, 10, vel7),
            velocity_fade(60, 127, 10, vel7),
        );
        assert!((fa * fa + fb * fb - 1.0).abs() < 1e-5);
        // The level is linear in the gain (the signal is far below the soft limiter).
        assert!((a - full * fa).abs() < 0.01 * full, "vel {vel7}: {a}");
        assert!((b - full * fb).abs() < 0.01 * full, "vel {vel7}: {b}");
        assert!((a * a + b * b - full * full).abs() < 0.02 * full * full);
    }
}

#[test]
fn a_zone_at_the_silent_end_of_its_crossfade_starts_no_voice() {
    let mut z = zone(0, 4000);
    (z.vel_lo, z.vel_hi, z.ext.vel_xfade) = (60, 127, 10);
    let (mut h, probe) = with_map(&map(flat_buffers(&[0.5], 4000), vec![z]));
    h.push_note("notes_in", on(0, 60, 60.0 / 127.0));
    render(&mut h, 1);
    assert_eq!(probe.active(), 0);
    h.push_note("notes_in", on(0, 60, 61.0 / 127.0));
    render(&mut h, 1);
    assert_eq!(probe.active(), 1);
}

/// Three flat zones of different level on the same key, one round-robin slot each.
fn rr_map(group: u8) -> SampleMapRef {
    let zones = (0..3u32)
        .map(|i| {
            let mut z = zone(i, 4000);
            z.ext.group = group;
            z.ext.rr_len = 3;
            z.ext.rr_pos = i as u8;
            z
        })
        .collect();
    map(flat_buffers(&[0.1, 0.2, 0.3], 4000), zones)
}

#[test]
fn round_robin_cycles_through_the_slots_and_restarts_on_reset() {
    let (mut h, _) = with_map(&rr_map(2));
    let first: Vec<f32> = (0..7).map(|_| note_level(&mut h, 60, 1.0)).collect();
    // Three distinct levels, repeating.
    assert!(first[0] < first[1] && first[1] < first[2], "{first:?}");
    for i in 0..4 {
        assert_eq!(first[i].to_bits(), first[i + 3].to_bits(), "{first:?}");
    }
    // A reset (a locate, the start of a bounce) restarts the cycle: the same notes give the
    // same sounds, bit for bit.
    h.device_mut().reset();
    let again: Vec<f32> = (0..7).map(|_| note_level(&mut h, 60, 1.0)).collect();
    assert_eq!(bits(&first), bits(&again));
}

#[test]
fn round_robin_counts_per_group() {
    // Group 1: three levels on key 60. Group 5: one zone on key 70 that takes slot 1 of 2.
    let mut zones: Vec<Zone> = (0..3u32)
        .map(|i| {
            let mut z = zone(i, 4000);
            (z.ext.group, z.ext.rr_len, z.ext.rr_pos) = (1, 3, i as u8);
            (z.key_lo, z.key_hi) = (60, 60);
            z
        })
        .collect();
    let mut other = zone(3, 4000);
    (other.ext.group, other.ext.rr_len, other.ext.rr_pos) = (5, 2, 1);
    (other.key_lo, other.key_hi) = (70, 70);
    zones.push(other);
    let (mut h, _) = with_map(&map(flat_buffers(&[0.1, 0.2, 0.3, 0.4], 4000), zones));
    let a = note_level(&mut h, 60, 1.0); // group 1, slot 0
    assert_eq!(
        note_level(&mut h, 70, 1.0),
        0.0,
        "group 5 starts at slot 0, which is empty"
    );
    assert!(
        note_level(&mut h, 70, 1.0) > 0.0,
        "group 5 is now at slot 1"
    );
    let b = note_level(&mut h, 60, 1.0); // group 1, slot 1: group 5 did not move it
    assert!(a < b);
}

/// A zone over `buffer` with an own low-pass filter.
fn filtered_zone(buffer: u32, frames: u64, cutoff: f32) -> Zone {
    let mut z = zone(buffer, frames);
    z.ext.filter = Some(ZoneFilter {
        mode: 0,
        cutoff,
        resonance: 0.0,
        key_track: 0.0,
        env_amount: 0.0,
    });
    z
}

#[test]
fn a_zone_filter_runs_with_the_device_filter_off_and_leaves_other_zones_alone() {
    let n = noise(20_000, 3);
    let mut a = filtered_zone(0, 20_000, 300.0);
    (a.key_lo, a.key_hi) = (60, 60);
    let mut b = zone(0, 20_000);
    (b.key_lo, b.key_hi) = (62, 62);
    let play = |zones: Vec<Zone>, key: u8| {
        let (mut h, _) = with_map(&map(vec![buffer(vec![n.clone()])], zones));
        h.push_note("notes_in", on(0, key, 1.0));
        render(&mut h, 40).0
    };
    let dark = play(vec![a, b], 60);
    let open = play(vec![zone(0, 20_000)], 60);
    assert!(rms(&dark[200..]) < 0.25 * rms(&open[200..]));
    // The unfiltered zone of the same map is bit-identical to a map without any filter.
    let plain = play(vec![a, b], 62);
    assert_eq!(bits(&plain), bits(&play(vec![zone(0, 20_000)], 62)));
}

#[test]
fn a_voice_that_changes_zones_carries_no_filter_state_over() {
    // Zone 0 has a resonant filter, zone 1 none. A voice that played zone 0 and is then started
    // on zone 1 sounds exactly like a voice that never played zone 0, and the other way round.
    let mut filtered = filtered_zone(0, 6000, 900.0);
    filtered.ext.filter.as_mut().unwrap().resonance = 0.9;
    let m = map(
        vec![buffer(vec![noise(6000, 11)])],
        vec![filtered, zone(0, 6000)],
    );
    let shared = Sampler::new().shared;
    let zeros = [0.0f32; 64];
    let mods = Mods {
        lfo: &zeros,
        pitch: &zeros,
        cutoff: &zeros,
        resonance: &zeros,
        pan: &zeros,
    };
    let play = |v: &mut Voice| {
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        v.render(&m, Some(&m), [&mut l, &mut r], &mods, &shared);
        l
    };
    let mut used = Voice::new();
    used.start(&m, 0, 60, 1.0, &shared);
    for _ in 0..4 {
        play(&mut used);
    }
    for zone_index in [1, 0] {
        used.start(&m, zone_index, 60, 1.0, &shared);
        let mut fresh = Voice::new();
        fresh.start(&m, zone_index, 60, 1.0, &shared);
        for _ in 0..3 {
            assert_eq!(bits(&play(&mut used)), bits(&play(&mut fresh)));
        }
    }
}

#[test]
fn a_filter_dropped_by_a_map_swap_stops_filtering_without_a_click() {
    let buf = buffer(vec![noise(20_000, 5)]);
    let with = map(vec![buf.clone()], vec![filtered_zone(0, 20_000, 200.0)]);
    let without = map(vec![buf], vec![zone(0, 20_000)]);
    let (mut h, _) = with_map(&with);
    h.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h, 20);
    drop(h.device_mut().swap_samples(Some(without.clone())));
    let (after, _) = render(&mut h, 10);
    assert!(after.iter().all(|v| v.is_finite()));
    // The voice kept its place in the sample; with no filter it now reads it straight.
    let (mut h2, _) = with_map(&without);
    h2.push_note("notes_in", on(0, 60, 1.0));
    render(&mut h2, 20);
    let (want, _) = render(&mut h2, 10);
    assert_eq!(bits(&after), bits(&want));
}

#[test]
fn a_zone_envelope_replaces_the_device_envelope_for_its_voices_only() {
    let mut slow = zone(0, 40_000);
    slow.ext.env = Some(ZoneEnv {
        attack_ms: 50.0,
        decay_ms: 200.0,
        sustain: 1.0,
        release_ms: 400.0,
    });
    (slow.key_lo, slow.key_hi) = (60, 60);
    let mut quick = zone(0, 40_000);
    (quick.key_lo, quick.key_hi) = (62, 62);
    let m = map(flat_buffers(&[0.5], 40_000), vec![slow, quick]);
    let fresh = || {
        let (mut h, _) = with_map(&m);
        // A device change after the map is installed does not touch the zone envelope.
        h.set_param("env.amp.attack", 0.5);
        h
    };
    let level_at = |key: u8, at: usize| {
        let mut h = fresh();
        h.push_note("notes_in", on(0, key, 1.0));
        render(&mut h, at.div_ceil(BATCH) + 1).0[at]
    };
    let (s, q) = (level_at(60, 256), level_at(62, 256));
    assert!(s < 0.25 * q, "slow {s} quick {q}");
    // Release: the zone tail (400 ms) still rings 100 ms after the note-off; the device tail
    // (0.5 ms) is long gone.
    let tail = |key: u8| {
        let mut h = fresh();
        h.push_note("notes_in", on(0, key, 1.0));
        render(&mut h, 200);
        h.push_note("notes_in", off(0, key));
        render(&mut h, 80).0.last().copied().unwrap()
    };
    assert!(
        tail(60).abs() > 0.05 && tail(62) == 0.0,
        "{} {}",
        tail(60),
        tail(62)
    );
}

#[test]
fn sixty_four_voices_of_grouped_filtered_zones_never_allocate() {
    // The harness runs every `process` under the allocation guard.
    let zones: Vec<Zone> = (0..4u32)
        .map(|i| {
            let mut z = filtered_zone(0, 8000, 500.0 + 700.0 * i as f32);
            z.ext.group = i as u8;
            z.ext.rr_len = 2;
            z.ext.rr_pos = (i % 2) as u8;
            z.ext.env = Some(ZoneEnv {
                attack_ms: 1.0,
                decay_ms: 50.0,
                sustain: 0.5,
                release_ms: 30.0,
            });
            z.loop_mode = LoopMode::Forward;
            z.loop_start = 100;
            z.loop_end = 7000;
            z
        })
        .collect();
    let (mut h, _) = with_map(&map(vec![buffer(vec![noise(8000, 7)])], zones));
    h.set_param("voices", 64.0);
    for round in 0..3u8 {
        for k in 0..70u8 {
            h.push_note("notes_in", on(u16::from(k.min(60)), 30 + k, 0.8));
        }
        let (l, r) = render(&mut h, 6);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()), "round {round}");
    }
}

#[test]
fn round_robin_cycles_each_key_on_its_own() {
    let (mut h, _) = with_map(&rr_map(1));
    let a0 = note_level(&mut h, 60, 1.0);
    let b0 = note_level(&mut h, 62, 1.0);
    let a1 = note_level(&mut h, 60, 1.0);
    // Key 62 starts its own cycle at slot 0, whatever key 60 has played.
    assert_eq!(a0.to_bits(), b0.to_bits());
    assert!(a1 > a0);
}

#[test]
fn master_tune_is_a_playback_ratio_like_the_tune_params() {
    let want = ramp(4000);
    // An octave of master tune plays exactly like an octave of tune.semi.
    let m = ramp_map(4000, |_| {});
    let (mut h, _) = with_map(&m);
    h.time_mut().tune_cents = 1200.0;
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 3);
    for n in SETTLED..150 {
        let w = want[2 * n] * FRAC_1_SQRT_2;
        assert_eq!(ol[n].to_bits(), w.to_bits(), "@{n}");
    }
    // 50 cents: the ramp advances by 2^(50/1200) a sample.
    let (mut h, _) = with_map(&m);
    h.time_mut().tune_cents = 50.0;
    h.push_note("notes_in", on(0, 60, 1.0));
    let (ol, _) = render(&mut h, 3);
    let ratio = 2f64.powf(50.0 / 1200.0);
    for (n, v) in ol.iter().enumerate().take(150).skip(SETTLED) {
        let w = f64::from(FRAC_1_SQRT_2) * ratio * n as f64 / 4096.0;
        assert!((f64::from(*v) - w).abs() < 1e-5, "@{n}: {v} vs {w}");
    }
}

#[test]
fn a_tune_of_zero_plays_the_same_bits_as_no_tune() {
    let m = ramp_map(4000, |z| z.tune_cents = 13.0);
    let run = |set: bool| {
        let (mut h, _) = with_map(&m);
        if set {
            h.time_mut().tune_cents = 0.0;
        }
        h.push_note("notes_in", on(0, 63, 1.0));
        render(&mut h, 4)
    };
    let (a, b) = (run(false), run(true));
    assert!(
        a.0.iter()
            .zip(&b.0)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}
