//! Loop slicer tests: (slice playback, reverse, envelope, retrigger, stealing, allocation
//! freedom, determinism) and a property test over random slice maps, notes and params.

#![allow(clippy::float_cmp, clippy::needless_range_loop)]
use super::*;
use crate::test_support::{SR, assert_keys, harness, off, on, sweep_params_min_max};
use atomik_core::{NoteEvent, NoteKind};
use atomik_device_sdk::{LoopMode, SampleBuffer, TriggerMode};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Test-only window into the voice allocator (atomic stores only: no allocation).
#[derive(Debug)]
pub struct Probe {
    pub active: AtomicUsize,
    pub draining: AtomicUsize,
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
    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
    fn draining(&self) -> usize {
        self.draining.load(Ordering::Relaxed)
    }
    fn cuts(&self) -> u64 {
        self.cuts.load(Ordering::Relaxed)
    }
    /// Sorted keys of the active voices.
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
}

const OUT: [&str; 2] = ["out_l", "out_r"];
/// Samples after a hit when the 0.5 ms attack (24 samples at 48 kHz) is certainly over.
const SETTLED: usize = 40;
/// The 2 ms fade in samples at 48 kHz.
const FADE: usize = 96;
const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn buffer(channels: Vec<Vec<f32>>) -> Arc<SampleBuffer> {
    Arc::new(SampleBuffer::new(channels.into()).unwrap())
}

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.9
        })
        .collect()
}

/// The slice zone on `key` over `start..end` of buffer 0, root = key.
fn slice(key: u8, start: u64, end: u64) -> Zone {
    Zone {
        buffer: 0,
        key_lo: key,
        key_hi: key,
        vel_lo: 1,
        vel_hi: 127,
        root_key: key,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start,
        end,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::Gate,
        reverse: false,
    }
}

fn map(buffers: Vec<Arc<SampleBuffer>>, zones: Vec<Zone>) -> SampleMapRef {
    Arc::new(SampleMap::new(buffers, zones).unwrap())
}

fn same(a: &SampleMapRef, b: &Option<SampleMapRef>) -> bool {
    b.as_ref().is_some_and(|b| Arc::ptr_eq(a, b))
}

/// A stereo noise loop of 8000 frames cut into four slices on keys 24..=27 (2000 frames each).
fn loop_data() -> (Vec<f32>, Vec<f32>) {
    (noise(8000, 1), noise(8000, 2))
}

fn four_slices(edit: impl Fn(&mut [Zone])) -> SampleMapRef {
    let (l, r) = loop_data();
    let mut zones: Vec<Zone> = (0..4)
        .map(|i| slice(24 + i as u8, 2000 * i, 2000 * (i + 1)))
        .collect();
    edit(&mut zones);
    map(vec![buffer(vec![l, r])], zones)
}

/// A stereo constant-level loop of `frames` with slices `keys`, each over the whole loop.
fn flat_slices(level: f32, frames: usize, keys: &[u8]) -> SampleMapRef {
    let zones = keys.iter().map(|k| slice(*k, 0, frames as u64)).collect();
    map(vec![buffer(vec![vec![level; frames]; 2])], zones)
}

/// A fresh slicer: master 0 dB, gate mode (holds at 1 while the key is down), no velocity
/// sensitivity, 0.5 ms attack.
fn slicer() -> (DeviceHarness, Arc<Probe>) {
    let probe = Arc::new(Probe::new());
    let mut d = LoopSlicer::new();
    d.probe = Some(probe.clone());
    let mut h = harness(Box::new(d));
    h.set_param("master.level", 0.0);
    h.set_param("velocity.amp", 0.0);
    h.set_param("env.mode", 0.0);
    (h, probe)
}

fn with_map(m: &SampleMapRef) -> (DeviceHarness, Arc<Probe>) {
    let (mut h, p) = slicer();
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

fn all_off(offset: u16) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::AllOff,
    }
}

// ---- descriptor ------------------------------------------------------------------------

#[test]
fn descriptor_validates_with_the_contract_order() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "master.level",
            "voices",
            "tune.semi",
            "env.attack",
            "env.decay",
            "env.mode",
            "velocity.amp",
            "loop.beats",
            "cv.pitch_amount",
            "filter.cutoff",
            "cv.cutoff_amount",
            "cv.level_amount",
        ],
        &[
            "notes_in",
            "pitch_cv_in",
            "cutoff_cv_in",
            "level_cv_in",
            "out_l",
            "out_r",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.loop-slicer");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    let p = |k: &str| *DESCRIPTOR.params.iter().find(|p| p.key == k).unwrap();
    assert_eq!(
        (p("voices").min, p("voices").max, p("voices").default),
        (1.0, 16.0, 16.0)
    );
    assert_eq!(
        (
            p("env.decay").min,
            p("env.decay").max,
            p("env.decay").default
        ),
        (5.0, 10_000.0, 2_000.0)
    );
    assert_eq!(p("env.mode").default, 1.0, "one-shot by default");
    assert_eq!(
        (
            p("loop.beats").min,
            p("loop.beats").max,
            p("loop.beats").default
        ),
        (1.0, 64.0, 4.0)
    );
    assert!(p("loop.beats").discrete);
    assert_eq!(p("master.level").default, -6.0);
    assert_eq!(p("env.attack").default, 0.5);
    assert_eq!(p("velocity.amp").default, 0.5);
    assert_eq!(p("cv.pitch_amount").default, 12.0);
}

// ---- slices ---------------------------------------------------------------------

// ---- CV inputs: level and cutoff -----------------------------------------------------------

/// Renders a held hit on key 24 of a one-slice map with an optional CV on `socket`.
fn cv_run(
    m: &SampleMapRef,
    setup: impl Fn(&mut DeviceHarness),
    socket: &str,
    cv: Option<f32>,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut h, _) = with_map(m);
    setup(&mut h);
    if let Some(cv) = cv {
        *h.input_mut(socket) = [cv; BATCH];
    }
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, batches)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn level_cv_scales_gain_by_amount_in_db() {
    let m = flat_slices(0.1, 4000, &[24]);
    let at = |cv: Option<f32>, amount: f32| {
        let (l, r) = cv_run(
            &m,
            |h| h.set_param("cv.level_amount", amount),
            "level_cv_in",
            cv,
            3,
        );
        assert_eq!(l[150].to_bits(), r[150].to_bits());
        l[150]
    };
    let base = at(None, 12.0);
    assert!((base - 0.1).abs() < 1e-6);
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
        at(Some(9.0), 12.0).to_bits(),
        at(Some(1.0), 12.0).to_bits(),
        "clamped to +1"
    );
    assert!((at(Some(1.0), -12.0) / base - 10f32.powf(-12.0 / 20.0)).abs() < 1e-4);
}

fn noise_slice() -> SampleMapRef {
    map(
        vec![buffer(vec![noise(40_000, 5), noise(40_000, 6)])],
        vec![slice(24, 0, 40_000)],
    )
}

#[test]
fn cutoff_cv_moves_the_filter_cutoff_by_amount_octaves() {
    let m = noise_slice();
    let level = |cv: Option<f32>, amount: f32| {
        let (l, _) = cv_run(
            &m,
            |h| {
                h.set_param("filter.cutoff", 1000.0);
                h.set_param("cv.cutoff_amount", amount);
            },
            "cutoff_cv_in",
            cv,
            500,
        );
        rms(&l[6000..30_000])
    };
    let (down, mid, up) = (
        level(Some(-1.0), 2.0),
        level(Some(0.0), 2.0),
        level(Some(1.0), 2.0),
    );
    assert!(down < mid * 0.7 && mid < up * 0.7, "{down} {mid} {up}");
    // Amount 0 leaves the filter at the param: the same bits as no cable at all.
    let (a, _) = cv_run(
        &m,
        |h| {
            h.set_param("filter.cutoff", 1000.0);
            h.set_param("cv.cutoff_amount", 0.0);
        },
        "cutoff_cv_in",
        Some(1.0),
        50,
    );
    let (b, _) = cv_run(&m, |h| h.set_param("filter.cutoff", 1000.0), "x", None, 50);
    assert_eq!(bits(&a), bits(&b));
    // Beyond +-1 the CV is clamped; a negative amount mirrors the sign of the CV.
    assert_eq!(
        level(Some(6.0), 2.0).to_bits(),
        level(Some(1.0), 2.0).to_bits()
    );
    assert_eq!(
        level(Some(1.0), -2.0).to_bits(),
        level(Some(-1.0), 2.0).to_bits()
    );
}

#[test]
fn new_inputs_unconnected_leave_output_bit_identical() {
    let m = four_slices(|_| {});
    let run = |amounts: Option<[f32; 2]>| {
        let (mut h, _) = with_map(&m);
        if let Some(a) = amounts {
            h.set_param("cv.cutoff_amount", a[0]);
            h.set_param("cv.level_amount", a[1]);
        }
        h.push_note("notes_in", on(0, 24, 1.0));
        h.push_note("notes_in", on(30, 26, 0.7));
        let (l, r) = render(&mut h, 20);
        (bits(&l), bits(&r))
    };
    assert_eq!(run(None), run(Some([-3.0, 20.0])));
    // `filter.cutoff` at its maximum is the open, bypassed filter.
    let run_open = || {
        let (mut h, _) = with_map(&m);
        h.set_param("filter.cutoff", 20_000.0);
        h.push_note("notes_in", on(0, 24, 1.0));
        h.push_note("notes_in", on(30, 26, 0.7));
        let (l, r) = render(&mut h, 20);
        (bits(&l), bits(&r))
    };
    assert_eq!(run(None), run_open());
}

#[test]
fn filter_cutoff_param_alone_darkens_the_sound() {
    let m = noise_slice();
    let level = |hz: f32| {
        let (l, _) = cv_run(&m, |h| h.set_param("filter.cutoff", hz), "x", None, 500);
        rms(&l[6000..30_000])
    };
    assert!(level(300.0) < level(20_000.0) * 0.5);
}

#[test]
fn new_cv_inputs_are_allocation_free() {
    let m = noise_slice();
    let (mut h, _) = with_map(&m);
    for s in ["cutoff_cv_in", "level_cv_in", "pitch_cv_in"] {
        *h.input_mut(s) = [0.4; BATCH];
    }
    h.push_note("notes_in", on(0, 24, 1.0));
    for i in 0..30 {
        h.set_param("filter.cutoff", 500.0 + 100.0 * i as f32);
        h.process();
    }
}

#[test]
fn every_slice_key_plays_its_slice_bit_exactly_after_the_attack() {
    let (l, r) = loop_data();
    let m = four_slices(|_| {});
    for i in 0..4usize {
        let (mut h, probe) = with_map(&m);
        h.push_note("notes_in", on(5, 24 + i as u8, 1.0));
        let (ol, or) = render(&mut h, 4);
        assert!(ol[..5].iter().chain(&or[..5]).all(|v| *v == 0.0));
        assert_eq!(probe.active(), 1);
        let k = 5 + SETTLED;
        let from = 2000 * i;
        // out[5 + t] = data[start + t] once the attack ramp is over.
        assert_eq!(
            bits(&ol[k..200]),
            bits(&l[from + SETTLED..from + 195]),
            "slice {i}"
        );
        assert_eq!(
            bits(&or[k..200]),
            bits(&r[from + SETTLED..from + 195]),
            "slice {i}"
        );
    }
}

#[test]
fn the_slice_ends_with_the_end_fade_and_the_voice_goes_idle() {
    let m = flat_slices(0.5, 4000, &[24]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 80);
    assert_eq!(probe.active(), 0, "idle after the slice ended");
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 80);
    // 64 frames fade at 48 kHz: the last sample is 1/64 of the level.
    assert!((ol[3999] - 0.5 / 64.0).abs() < 1e-6, "{}", ol[3999]);
    assert!((ol[3935] - 0.5).abs() < 1e-6, "{}", ol[3935]);
    assert!(ol[4000..].iter().all(|v| *v == 0.0));
}

#[test]
fn a_reversed_slice_plays_backwards_bit_exactly_after_the_attack() {
    let (l, r) = loop_data();
    let m = four_slices(|z| {
        z[1].reverse = true;
        // Loop fields are ignored when reversed.
        z[1].loop_mode = LoopMode::PingPong;
        z[1].loop_start = 9000;
    });
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(3, 25, 1.0));
    let (ol, or) = render(&mut h, 4);
    assert_eq!(probe.active(), 1);
    for n in 3 + SETTLED..200 {
        let t = n - 3;
        assert_eq!(ol[n].to_bits(), l[3999 - t].to_bits(), "left {n}");
        assert_eq!(or[n].to_bits(), r[3999 - t].to_bits(), "right {n}");
    }
    // The other slices are untouched by it.
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 26, 1.0));
    let (ol, _) = render(&mut h, 4);
    assert_eq!(bits(&ol[SETTLED..100]), bits(&l[4000 + SETTLED..4100]));
}

#[test]
fn a_reversed_slice_fades_at_its_start_and_ends_there() {
    let m = flat_slices(0.5, 4000, &[24]);
    let z = Zone {
        reverse: true,
        start: 1000,
        end: 1500,
        ..slice(24, 0, 0)
    };
    let m = map(m.buffers().to_vec(), vec![z]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 8);
    assert_eq!(probe.active(), 0);
    assert!((ol[499] - 0.5 / 64.0).abs() < 1e-6, "{}", ol[499]);
    assert!((ol[100] - 0.5).abs() < 1e-6);
    assert!(ol[500..].iter().all(|v| *v == 0.0));
}

#[test]
fn only_slice_keys_with_a_zone_sound_and_other_zones_and_keys_are_ignored() {
    let (l, r) = loop_data();
    // Zones on key 23 (below the slices), 88 (above), a range 24..=25 and a slice on 30.
    let mut below = slice(23, 0, 2000);
    below.key_lo = 23;
    let mut above = slice(88, 0, 2000);
    above.key_lo = 88;
    let mut range = slice(24, 0, 2000);
    range.key_hi = 25;
    let m = map(
        vec![buffer(vec![l.clone(), r])],
        vec![below, above, range, slice(30, 2000, 4000)],
    );
    let (mut h, probe) = with_map(&m);
    for key in [23u8, 24, 25, 87, 88, 60, 0, 127] {
        h.push_note("notes_in", on(0, key, 1.0));
    }
    render(&mut h, 2);
    assert_eq!(probe.active(), 0, "no slice zone on any of those keys");
    h.push_note("notes_in", on(0, 30, 1.0));
    let (ol, _) = render(&mut h, 4);
    assert_eq!(probe.active(), 1);
    assert_eq!(bits(&ol[SETTLED..150]), bits(&l[2000 + SETTLED..2150]));
}

#[test]
fn slice_keys_span_24_to_87_and_the_first_zone_of_a_key_wins() {
    let frames = 100;
    let zones: Vec<Zone> = (24..=87u8).map(|k| slice(k, 0, frames)).collect();
    let m = map(vec![buffer(vec![vec![0.01; frames as usize]; 2])], zones);
    let (mut h, probe) = with_map(&m);
    for k in 24..=87u8 {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    render(&mut h, 1);
    assert!(probe.active() <= MAX_VOICES);
    let (l, _) = loop_data();
    // The first of two zones on key 24 is the slice.
    let m = map(
        vec![buffer(vec![l.clone(), l.clone()])],
        vec![slice(24, 0, 1000), slice(24, 3000, 4000)],
    );
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 2);
    assert_eq!(bits(&ol[SETTLED..90]), bits(&l[SETTLED..90]));
}

#[test]
fn no_map_and_no_matching_velocity_are_silent() {
    let (mut h, probe) = slicer();
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 2);
    assert!(ol.iter().all(|v| *v == 0.0) && probe.active() == 0);
    let mut z = slice(24, 0, 1000);
    z.vel_lo = 100;
    let m = map(vec![buffer(vec![vec![0.5; 1000]; 2])], vec![z]);
    assert!(h.device_mut().swap_samples(Some(m)).is_none());
    h.push_note("notes_in", on(0, 24, 0.2));
    render(&mut h, 2);
    assert_eq!(probe.active(), 0);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 1);
    assert_eq!(probe.active(), 1);
}

#[test]
fn gain_pan_velocity_and_pitch_shape_each_slice() {
    let (l, r) = loop_data();
    // Slice 25: +6 dB, hard right (stereo balance: left side silent). Slice 26: one octave up.
    let m = four_slices(|z| {
        z[1].gain_db = -6.0;
        z[1].pan = 1.0;
        z[2].transpose = 12;
    });
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 25, 1.0));
    let (ol, or) = render(&mut h, 3);
    let g = atomik_core::db_to_gain(-6.0);
    assert!(
        ol[SETTLED..120].iter().all(|v| *v == 0.0),
        "balance: left is gone"
    );
    for n in SETTLED..120 {
        assert_eq!(or[n].to_bits(), (r[2000 + n] * g).to_bits(), "{n}");
    }
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 26, 1.0));
    let (ol, _) = render(&mut h, 3);
    for n in SETTLED..100 {
        assert_eq!(ol[n].to_bits(), l[4000 + 2 * n].to_bits(), "octave up {n}");
    }
    // tune.semi shifts every slice; velocity.amp scales by (1 - amp (1 - v)).
    let (mut h, _) = with_map(&m);
    h.set_param("tune.semi", 12.0);
    h.set_param("velocity.amp", 1.0);
    h.push_note("notes_in", on(0, 24, 0.5));
    let (ol, _) = render(&mut h, 3);
    for n in SETTLED..100 {
        assert_eq!(ol[n].to_bits(), (l[2 * n] * 0.5).to_bits(), "{n}");
    }
}

#[test]
fn pitch_cv_shifts_by_cv_pitch_amount() {
    let m = four_slices(|_| {});
    let (l, _) = loop_data();
    let (mut h, _) = with_map(&m);
    h.set_param("cv.pitch_amount", 12.0);
    h.set_connected("pitch_cv_in", true);
    *h.input_mut("pitch_cv_in") = [1.0; BATCH];
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 3);
    for n in SETTLED..100 {
        assert_eq!(ol[n].to_bits(), l[2 * n].to_bits(), "{n}");
    }
}

// ---- envelope ---------------------------------------------------------------------

#[test]
fn the_attack_is_a_linear_ramp_of_env_attack() {
    let m = flat_slices(0.5, 20_000, &[24]);
    for (ms, n) in [(0.5f32, 24usize), (2.0, 96), (50.0, 2400)] {
        let (mut h, _) = with_map(&m);
        h.set_param("env.attack", ms);
        h.push_note("notes_in", on(0, 24, 1.0));
        let (ol, _) = render(&mut h, n / BATCH + 3);
        for i in 0..n {
            let want = 0.5 * (i + 1) as f32 / n as f32;
            assert!(
                (ol[i] - want).abs() < 1e-5,
                "{ms} ms sample {i}: {} vs {want}",
                ol[i]
            );
        }
        assert!((ol[n + 5] - 0.5).abs() < 1e-5);
    }
}

#[test]
fn one_shot_decays_exponentially_to_minus_60_db_and_ignores_note_off() {
    let m = flat_slices(0.5, 40_000, &[24]);
    let (mut h, probe) = with_map(&m);
    h.set_param("env.mode", 1.0);
    h.set_param("env.decay", 100.0); // 4800 samples to -60 dB
    h.push_note("notes_in", on(0, 24, 1.0));
    h.push_note("notes_in", off(10, 24));
    let (ol, _) = render(&mut h, 160); // 10240 samples
    // The decay starts right after the 24-sample attack: t samples later the level is
    // 0.5 * 10^(-3 t / 4800).
    for t in [100usize, 1000, 2400, 4000] {
        let want = 0.5 * 10f32.powf(-3.0 * t as f32 / 4800.0);
        let got = ol[24 + t];
        assert!((got / want - 1.0).abs() < 0.01, "t {t}: {got} vs {want}");
    }
    let at_60 = ol[24 + 4800];
    assert!((at_60 / 0.5 - 0.001).abs() < 2e-5, "-60 dB: {at_60}");
    // Below -80 dB (decay 1e-4 of full scale, 6400 samples) the voice goes idle.
    assert_eq!(probe.active(), 0);
    assert!(ol[24 + 6500..].iter().all(|v| *v == 0.0));
}

#[test]
fn gate_mode_holds_while_the_key_is_down_then_decays() {
    let m = flat_slices(0.5, 40_000, &[24]);
    let (mut h, probe) = with_map(&m);
    h.set_param("env.mode", 0.0);
    h.set_param("env.decay", 100.0);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (held, _) = render(&mut h, 40); // 5120 samples
    assert!(
        held[24..].iter().all(|v| (*v - 0.5).abs() < 1e-6),
        "holds at full level"
    );
    assert_eq!(probe.active(), 1);
    h.push_note("notes_in", off(0, 24));
    let (ol, _) = render(&mut h, 60);
    for t in [0usize, 480, 2400] {
        let want = 0.5 * 10f32.powf(-3.0 * (t + 1) as f32 / 4800.0);
        assert!(
            (ol[t] / want - 1.0).abs() < 0.01,
            "t {t}: {} vs {want}",
            ol[t]
        );
    }
    render(&mut h, 100);
    assert_eq!(probe.active(), 0);
}

#[test]
fn env_mode_is_read_at_the_hit() {
    let m = flat_slices(0.5, 40_000, &[24, 25]);
    let (mut h, _) = with_map(&m);
    h.set_param("env.mode", 1.0);
    h.set_param("env.decay", 100.0);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 2);
    h.set_param("env.mode", 0.0); // later hits hold; the earlier voice keeps decaying
    h.push_note("notes_in", on(0, 25, 1.0));
    let (ol, _) = render(&mut h, 120);
    // After the one-shot has decayed away only the held voice at 0.5 remains.
    assert!((ol[ol.len() - 1] - 0.5).abs() < 1e-5);
}

#[test]
fn all_notes_off_fades_every_voice_in_two_ms() {
    let m = flat_slices(0.25, 40_000, &[24, 25]);
    let (mut h, probe) = with_map(&m);
    for k in [24u8, 25] {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    render(&mut h, 4);
    h.push_note("notes_in", all_off(0));
    let (ol, _) = render(&mut h, 3);
    assert_eq!(probe.active(), 0);
    assert!((ol[0] - 0.5).abs() < 1e-5);
    assert!((ol[FADE / 2] - 0.25).abs() < 0.01);
    assert!(ol[FADE..].iter().all(|v| *v == 0.0));
}

// ---- retrigger and stealing -------------------------------------------------------

#[test]
fn a_retrigger_fades_the_previous_hit_of_the_slice_in_two_ms() {
    let m = flat_slices(0.5, 40_000, &[24, 25]);
    let (mut h, probe) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 4);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, _) = render(&mut h, 5);
    // Old voice: 0.5 * (96 - n) / 96; new voice: attack ramp 0.5 * (n + 1) / 24.
    for n in 0..FADE {
        let old = 0.5 * (FADE - n) as f32 / FADE as f32;
        let new = 0.5 * ((n + 1) as f32 / 24.0).min(1.0);
        assert!(
            (ol[n] - (old + new)).abs() < 1e-5,
            "n {n}: {} vs {}",
            ol[n],
            old + new
        );
    }
    assert!(
        (ol[FADE + 10] - 0.5).abs() < 1e-5,
        "only the new hit remains"
    );
    assert_eq!(probe.active(), 1);
    // A different slice overlaps instead.
    h.push_note("notes_in", on(0, 25, 1.0));
    let (ol, _) = render(&mut h, 5);
    assert_eq!(probe.active(), 2);
    assert!((ol[200] - 1.0).abs() < 1e-5);
}

#[test]
fn the_oldest_voice_is_stolen_with_a_two_ms_fade() {
    let m = flat_slices(0.5, 40_000, &[24, 25, 26]);
    let (mut h, probe) = with_map(&m);
    h.set_param("voices", 2.0);
    for (i, key) in [24u8, 25, 26].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 1.0));
    }
    let (ol, _) = render(&mut h, 8);
    assert_eq!(probe.active(), 2);
    assert_eq!(probe.keys(), vec![25, 26], "key 24 was the oldest");
    assert!(ol.iter().all(|v| v.is_finite() && *v <= 1.0 + 1e-5));
    assert!((ol[300] - 1.0).abs() < 1e-5);
    // Steal fade is a ramp, not a cut.
    let step = ol[100..]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < 0.03, "steal click {step}");
}

#[test]
fn sixteen_voices_then_the_seventeenth_hit_steals_the_oldest() {
    let keys: Vec<u8> = (24..41).collect(); // 17 slices
    let m = flat_slices(0.05, 40_000, &keys);
    let (mut h, probe) = with_map(&m);
    for (i, k) in keys.iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *k, 1.0));
    }
    render(&mut h, 4);
    assert_eq!(probe.active(), 16);
    let mut want: Vec<u8> = keys[1..].to_vec();
    want.sort_unstable();
    assert_eq!(probe.keys(), want);
}

#[test]
fn note_off_of_a_pending_hit_cancels_it_in_gate_mode() {
    let m = flat_slices(0.5, 40_000, &[24, 25]);
    let (mut h, probe) = with_map(&m);
    h.set_param("voices", 1.0);
    h.push_note("notes_in", on(0, 24, 1.0));
    h.push_note("notes_in", on(10, 25, 1.0));
    h.push_note("notes_in", off(20, 25));
    render(&mut h, 4);
    assert_eq!(
        probe.active(),
        0,
        "the stolen voice ended; the pending hit was cancelled"
    );
}

// ---- swaps --------------------------------------------------------------------------------

#[test]
fn swapping_a_map_with_the_same_buffer_keeps_voices_playing() {
    let (l, r) = loop_data();
    let buf = buffer(vec![l, r]);
    let a = map(vec![buf.clone()], vec![slice(24, 0, 4000)]);
    let mut z = slice(24, 0, 4000);
    z.gain_db = -6.0;
    let b = map(vec![buf], vec![z]);
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 2);
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(
        same(&a, &back),
        "nothing drains: the old map is handed back"
    );
    assert_eq!((probe.active(), probe.draining()), (1, 0));
    render(&mut h, 1);
}

#[test]
fn swapping_in_new_buffers_drains_old_voices_and_a_second_swap_cuts_them() {
    let a = flat_slices(0.5, 40_000, &[24]);
    let b = flat_slices(0.25, 40_000, &[24]);
    let c = flat_slices(0.125, 40_000, &[24]);
    let (mut h, probe) = with_map(&a);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 2);
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(back.is_none(), "the old map is kept as the draining map");
    assert_eq!((probe.active(), probe.draining()), (1, 1));
    // A second swap while it still drains cuts the voice and hands the draining map back.
    let back = h.device_mut().swap_samples(Some(c.clone()));
    assert!(same(&a, &back));
    assert_eq!((probe.cuts(), probe.active()), (1, 0));
    // Without a still-draining voice the replaced map is handed back at once.
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 2);
    let back = h.device_mut().swap_samples(None);
    assert!(
        same(&b, &back),
        "the map kept from the first swap is handed back"
    );
    render(&mut h, 8);
    assert_eq!(probe.active(), 0, "the 5 ms drain fade has ended");
}

#[test]
fn a_pending_hit_survives_a_swap_only_with_a_matching_slice_zone() {
    let m = flat_slices(0.5, 40_000, &[24, 25]);
    let (mut h, probe) = with_map(&m);
    h.set_param("voices", 1.0);
    h.push_note("notes_in", on(0, 24, 1.0));
    h.push_note("notes_in", on(10, 25, 1.0));
    render(&mut h, 1);
    assert_eq!(probe.keys(), vec![25]);
    // Same index, other key: the pending hit goes.
    let other = flat_slices(0.5, 40_000, &[24, 30]);
    let _ = h.device_mut().swap_samples(Some(other));
    render(&mut h, 8);
    assert_eq!(probe.active(), 0);
}

// ---- determinism and allocation freedom ---------------------------------------------------

fn busy_run(sr: f64) -> (Vec<u32>, Vec<u32>) {
    let m = four_slices(|z| {
        z[0].reverse = true;
        z[1].pan = -0.4;
        z[2].transpose = -5;
        z[3].tune_cents = 33.0;
    });
    let mut h = DeviceHarness::with_options(Box::new(LoopSlicer::new()), sr, 120.0, 4);
    h.device_mut().swap_samples(Some(m));
    h.set_param("env.mode", 0.0);
    let mut l = Vec::new();
    let mut r = Vec::new();
    for b in 0..60u16 {
        if b % 3 == 0 {
            h.push_note(
                "notes_in",
                on(b % 60, 24 + (b % 4) as u8, 0.4 + 0.01 * f32::from(b)),
            );
        }
        if b % 5 == 4 {
            h.push_note("notes_in", off(63, 24 + (b % 4) as u8));
        }
        h.process();
        l.extend(h.output("out_l").iter().map(|v| v.to_bits()));
        r.extend(h.output("out_r").iter().map(|v| v.to_bits()));
    }
    (l, r)
}

#[test]
fn identical_runs_are_bit_identical() {
    assert_eq!(busy_run(48_000.0), busy_run(48_000.0));
    assert_ne!(busy_run(48_000.0).0, busy_run(44_100.0).0);
}

fn sixteen_voice_map() -> SampleMapRef {
    let (l, r) = loop_data();
    let zones: Vec<Zone> = (0..16u64)
        .map(|i| Zone {
            reverse: i % 3 == 0,
            pan: -1.0 + 0.13 * i as f32,
            ..slice(24 + i as u8, 400 * i, 400 * (i + 1) + 1500)
        })
        .collect();
    map(vec![buffer(vec![l, r])], zones)
}

#[test]
fn process_swap_and_params_are_allocation_free_with_16_voices() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = DeviceHarness::with_options(Box::new(LoopSlicer::new()), sr, 133.0, 9);
        let first = sixteen_voice_map();
        assert!(assert_no_alloc(|| h.device_mut().swap_samples(Some(first))).is_none());
        h.set_connected("pitch_cv_in", true);
        *h.input_mut("pitch_cv_in") = [0.3; BATCH];
        for round in 0..24u16 {
            for i in 0..16u8 {
                h.push_note(
                    "notes_in",
                    on(u16::from(i) * 3, 24 + i, 0.4 + 0.02 * f32::from(round)),
                );
            }
            h.push_note("notes_in", on(48, 24, 1.0)); // retrigger
            h.push_note("notes_in", off(50, 25));
            if round == 5 {
                h.set_param("env.mode", 0.0);
                h.set_param("env.decay", 30.0);
                h.set_param("voices", 8.0);
            }
            if round == 10 {
                h.set_param("voices", 16.0);
                h.set_param("env.attack", 5.0);
            }
            h.process();
        }
        let next = sixteen_voice_map();
        let back = assert_no_alloc(|| h.device_mut().swap_samples(Some(next)));
        drop(back);
        for _ in 0..10 {
            h.process();
        }
        let none = assert_no_alloc(|| h.device_mut().swap_samples(None));
        drop(none);
        h.process();
        assert_no_alloc(|| h.device_mut().reset());
        h.process();
    }
}

#[test]
fn param_sweep_stays_finite_with_a_map_and_held_notes() {
    let m = sixteen_voice_map();
    let (mut h, _) = with_map(&m);
    for k in 24..32u8 {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    let d = h.device().descriptor();
    sweep_params_min_max(&mut h, d);
}

#[test]
fn the_output_is_limited_below_four() {
    let keys: Vec<u8> = (24..40).collect();
    let m = flat_slices(1.0, 20_000, &keys);
    let (mut h, _) = with_map(&m);
    h.set_param("master.level", 6.0);
    for k in &keys {
        h.push_note("notes_in", on(0, *k, 1.0));
    }
    let (ol, _) = render(&mut h, 20);
    let pk = ol.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!(pk > 2.0 && pk <= 4.0, "peak {pk}");
}

#[test]
fn master_level_is_smoothed() {
    let m = flat_slices(0.25, 40_000, &[24]);
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    render(&mut h, 4);
    h.set_param("master.level", -20.0);
    let (ol, _) = render(&mut h, 4);
    assert!(ol[0] > 0.2, "no jump at the change: {}", ol[0]);
    assert!(ol.windows(2).all(|w| (w[1] - w[0]).abs() < 0.01));
    assert!((ol[ol.len() - 1] - 0.025).abs() < 5e-3);
}

// ---- proptest ----------------------------------------------------------------------------

fn arb_zone() -> impl Strategy<Value = Zone> {
    (
        (0u32..3, 20u8..92, 1u8..128, 1u8..128, 10u8..110),
        (-48i8..48, -100.0f32..100.0, -60.0f32..12.0, -1.0f32..1.0),
        (0u64..1500, 1u64..=500, any::<bool>(), 0u8..3),
    )
        .prop_map(
            |((buf, key, v0, v1, root), (tr, cents, gain, pan), (start, len, reverse, lm))| Zone {
                buffer: buf,
                key_lo: key,
                key_hi: key,
                vel_lo: v0.min(v1),
                vel_hi: v0.max(v1),
                root_key: root,
                transpose: tr,
                tune_cents: cents,
                gain_db: gain,
                pan,
                start,
                end: start + len,
                loop_mode: match lm {
                    0 => LoopMode::Off,
                    1 => LoopMode::Forward,
                    _ => LoopMode::PingPong,
                },
                loop_start: start,
                loop_end: start + len,
                crossfade: 0,
                trigger: TriggerMode::Gate,
                reverse,
            },
        )
}

fn random_map(zones: Vec<Zone>) -> SampleMapRef {
    let bufs = vec![
        buffer(vec![noise(2000, 1), noise(2000, 2)]),
        buffer(vec![noise(2000, 3)]),
        buffer(vec![vec![1.0; 2000]; 2]),
    ];
    map(bufs, zones)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn random_slice_maps_notes_and_params_stay_finite_and_in_bounds(
        zones in prop::collection::vec(arb_zone(), 0..24),
        zones2 in prop::collection::vec(arb_zone(), 0..24),
        events in prop::collection::vec((0usize..30, 0u16..64, 20u8..92, 0.0f32..1.2, 0u8..4), 0..80),
        params in prop::collection::vec((0usize..9, 0.0f32..1.0), 0..30),
        cv in -3.0f32..3.0,
    ) {
        let (mut h, probe) = slicer();
        let _ = h.device_mut().swap_samples(Some(random_map(zones)));
        for (i, f) in &params {
            let d = &PARAMS[*i];
            h.set_param(d.key, d.min + (d.max - d.min) * f);
        }
        h.set_connected("pitch_cv_in", true);
        *h.input_mut("pitch_cv_in") = [cv; BATCH];
        let mut events = events;
        events.sort_by_key(|e| (e.0, e.1));
        for b in 0..30usize {
            for (eb, off_, key, vel, kind) in &events {
                if *eb == b {
                    let ev = match kind {
                        0 | 1 => on(*off_, *key, *vel),
                        2 => off(*off_, *key),
                        _ => all_off(*off_),
                    };
                    h.push_note("notes_in", ev);
                }
            }
            if b == 15 {
                let _ = h.device_mut().swap_samples(Some(random_map(zones2.clone())));
            }
            h.process();
            for k in OUT {
                let o = h.output(k);
                prop_assert!(o.iter().all(|v| v.is_finite() && v.abs() <= 4.0), "{k}");
            }
            prop_assert!(probe.active() <= MAX_VOICES);
        }
    }
}

#[test]
fn pan_is_constant_power_for_mono_loops() {
    let m = map(
        vec![buffer(vec![vec![0.5; 4000]])],
        vec![slice(24, 0, 4000)],
    );
    let (mut h, _) = with_map(&m);
    h.push_note("notes_in", on(0, 24, 1.0));
    let (ol, or) = render(&mut h, 3);
    let want = 0.5 * FRAC_1_SQRT_2;
    assert!((ol[100] - want).abs() < 1e-6 && (or[100] - want).abs() < 1e-6);
}

#[test]
fn sample_rate_scales_the_attack_and_fade_lengths() {
    let m = flat_slices(0.5, 40_000, &[24]);
    let mut h = DeviceHarness::with_options(Box::new(LoopSlicer::new()), 96_000.0, 120.0, 1);
    h.device_mut().swap_samples(Some(m));
    h.set_param("env.mode", 0.0);
    h.set_param("velocity.amp", 0.0);
    h.set_param("master.level", 0.0);
    h.push_note("notes_in", on(0, 24, 1.0));
    let o = h.render(1, &OUT);
    // 0.5 ms at 96 kHz is 48 samples.
    assert!((o[0][47] - 0.5).abs() < 1e-5 && o[0][23] < 0.3);
    let _ = SR;
}
