//! Tests of the granular sampler: the grain scheduler (density, size), positions, spray and scan,
//! pitch, direction, windows, the grain cap and stealing, voices and envelopes, the filter, the
//! modulation sources and the CVs, the built-in source, swapping samples, determinism, the
//! panel values and the factory sounds.

use super::*;
use crate::test_support::{
    SR, assert_bend_range, assert_sustain_pedal, assert_volume_and_expression, harness, off, on,
    sweep_params_min_max,
};
use atomik_core::NoteEvent;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::{max_step, peak, rms};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Test-only window into the grain scheduler, written at every grain start (atomic stores only:
/// no allocation).
#[derive(Debug)]
pub struct Probe {
    /// Grains started.
    spawned: AtomicU64,
    /// Grains started over a full pool (a grain was replaced).
    stolen: AtomicU64,
    /// Grains started reversed.
    reversed: AtomicU64,
    /// The lowest, highest and latest start position (a fraction of the window, as f32 bits).
    pos_min: AtomicU32,
    pos_max: AtomicU32,
    pos_last: AtomicU32,
    /// The shortest, longest and latest grain in samples.
    len_min: AtomicU32,
    len_max: AtomicU32,
    len_last: AtomicU32,
    /// The lowest, highest and latest pitch in semitones (f32 bits).
    semis_min: AtomicU32,
    semis_max: AtomicU32,
    semis_last: AtomicU32,
}

impl Probe {
    fn new() -> Self {
        Self {
            spawned: AtomicU64::new(0),
            stolen: AtomicU64::new(0),
            reversed: AtomicU64::new(0),
            pos_min: AtomicU32::new(f32::MAX.to_bits()),
            pos_max: AtomicU32::new(f32::MIN.to_bits()),
            pos_last: AtomicU32::new(0),
            len_min: AtomicU32::new(u32::MAX),
            len_max: AtomicU32::new(0),
            len_last: AtomicU32::new(0),
            semis_min: AtomicU32::new(f32::MAX.to_bits()),
            semis_max: AtomicU32::new(f32::MIN.to_bits()),
            semis_last: AtomicU32::new(0),
        }
    }

    pub(super) fn on_spawn(&self, pos: f32, len: u32, semis: f32, reverse: bool, stole: bool) {
        let lo = |a: &AtomicU32, v: f32| {
            if v < f32::from_bits(a.load(Ordering::Relaxed)) {
                a.store(v.to_bits(), Ordering::Relaxed);
            }
        };
        let hi = |a: &AtomicU32, v: f32| {
            if v > f32::from_bits(a.load(Ordering::Relaxed)) {
                a.store(v.to_bits(), Ordering::Relaxed);
            }
        };
        self.spawned.fetch_add(1, Ordering::Relaxed);
        self.stolen.fetch_add(u64::from(stole), Ordering::Relaxed);
        self.reversed
            .fetch_add(u64::from(reverse), Ordering::Relaxed);
        lo(&self.pos_min, pos);
        hi(&self.pos_max, pos);
        self.pos_last.store(pos.to_bits(), Ordering::Relaxed);
        lo(&self.semis_min, semis);
        hi(&self.semis_max, semis);
        self.semis_last.store(semis.to_bits(), Ordering::Relaxed);
        self.len_min.fetch_min(len, Ordering::Relaxed);
        self.len_max.fetch_max(len, Ordering::Relaxed);
        self.len_last.store(len, Ordering::Relaxed);
    }

    fn spawned(&self) -> u64 {
        self.spawned.load(Ordering::Relaxed)
    }
    fn stolen(&self) -> u64 {
        self.stolen.load(Ordering::Relaxed)
    }
    fn reversed(&self) -> u64 {
        self.reversed.load(Ordering::Relaxed)
    }
    fn pos(&self) -> (f32, f32) {
        (
            f32::from_bits(self.pos_min.load(Ordering::Relaxed)),
            f32::from_bits(self.pos_max.load(Ordering::Relaxed)),
        )
    }
    fn pos_last(&self) -> f32 {
        f32::from_bits(self.pos_last.load(Ordering::Relaxed))
    }
    fn len(&self) -> (u32, u32) {
        (
            self.len_min.load(Ordering::Relaxed),
            self.len_max.load(Ordering::Relaxed),
        )
    }
    fn semis(&self) -> (f32, f32) {
        (
            f32::from_bits(self.semis_min.load(Ordering::Relaxed)),
            f32::from_bits(self.semis_max.load(Ordering::Relaxed)),
        )
    }
    fn semis_last(&self) -> f32 {
        f32::from_bits(self.semis_last.load(Ordering::Relaxed))
    }
}

const OUT: [&str; 2] = ["out_l", "out_r"];
/// The key at which the test samples play at their recorded pitch.
const ROOT: u8 = 60;

fn granular() -> (DeviceHarness, Arc<Probe>) {
    let probe = Arc::new(Probe::new());
    let mut g = Granular::new();
    g.probe = Some(probe.clone());
    (harness(Box::new(g)), probe)
}

fn with_map(m: &SampleMapRef) -> (DeviceHarness, Arc<Probe>) {
    let (mut h, probe) = granular();
    assert!(h.device_mut().swap_samples(Some(m.clone())).is_none());
    (h, probe)
}

fn buffer(channels: Vec<Vec<f32>>) -> Arc<SampleBuffer> {
    Arc::new(SampleBuffer::new(channels.into()).unwrap())
}

/// A gate zone over the whole of buffer `buffer` (`frames` long) on every key, root 60.
fn zone(buffer: u32, frames: u64) -> Zone {
    Zone {
        buffer,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: ROOT,
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

fn mono_map(x: Vec<f32>, edit: impl Fn(&mut Zone)) -> SampleMapRef {
    let mut z = zone(0, x.len() as u64);
    edit(&mut z);
    map(vec![buffer(vec![x])], vec![z])
}

fn sine(n: usize, hz: f64, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / SR).sin() as f32)
        .collect()
}

fn same(a: &SampleMapRef, b: &Option<SampleMapRef>) -> bool {
    b.as_ref().is_some_and(|b| Arc::ptr_eq(a, b))
}

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn push(h: &mut DeviceHarness, offset: u16, kind: NoteKind) {
    h.push_note("notes_in", NoteEvent { offset, kind });
}

/// Renders `batches` batches and returns the two outputs.
fn render(h: &mut DeviceHarness, batches: usize) -> (Vec<f32>, Vec<f32>) {
    let mut o = h.render(batches, &OUT);
    let r = o.pop().unwrap();
    (o.pop().unwrap(), r)
}

fn meter(h: &DeviceHarness) -> [f32; METER_COUNT] {
    let mut v = [f32::NAN; METER_COUNT];
    assert_eq!(h.device().meter_values(&mut v), METER_COUNT);
    v
}

/// A neutral patch: one 500 ms Tukey grain a second at 10 % of the sample, no spray, scan,
/// timing, spread, reverse, pitch scatter, wheel, velocity, filter or modulation; key tracking
/// on; a flat 0.5 ms attack; master level 0 dB.
fn plain(h: &mut DeviceHarness) {
    for (k, v) in [
        ("master.level", 0.0),
        ("grain.density", 1.0),
        ("grain.size", 500.0),
        ("grain.window", 1.0),
        ("grain.timing", 0.0),
        ("grain.reverse", 0.0),
        ("grain.spread", 0.0),
        ("grain.limit", 64.0),
        ("pos.position", 0.1),
        ("pos.spray", 0.0),
        ("pos.scan", 0.0),
        ("pitch.transpose", 0.0),
        ("pitch.fine", 0.0),
        ("pitch.random", 0.0),
        ("pitch.key_track", 1.0),
        ("env.amp.attack", 0.5),
        ("env.amp.decay", 100.0),
        ("env.amp.sustain", 1.0),
        ("env.amp.release", 100.0),
        ("velocity.amp", 0.0),
        ("filter.enabled", 0.0),
        ("wheel.to_spray", 0.0),
        ("wheel.to_position", 0.0),
    ] {
        h.set_param(k, v);
    }
}

/// The frequency of `y` from the interpolated rising zero crossings.
fn zero_crossing_hz(y: &[f32]) -> f64 {
    let (mut first, mut last, mut count) = (None, 0.0, 0usize);
    for (i, w) in y.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = i as f64 + f64::from(-w[0]) / f64::from(w[1] - w[0]);
            first.get_or_insert(t);
            last = t;
            count += 1;
        }
    }
    let first = first.expect("no zero crossing");
    (count - 1) as f64 * SR / (last - first)
}

/// The pitch a sine sample of `hz` is heard at when the patch is `setup` and `key` is played:
/// the flat middle of one 500 ms Tukey grain (125 to 375 ms).
fn heard_hz(hz: f64, setup: impl Fn(&mut DeviceHarness), key: u8) -> f64 {
    let m = mono_map(sine(96_000, hz, 0.8), |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    setup(&mut h);
    push(&mut h, 0, NoteKind::On { key, velocity: 1.0 });
    let (l, _) = render(&mut h, seconds(0.5));
    zero_crossing_hz(&l[6_000..18_000])
}

// -------------------------------------------------------------------------------- descriptor

#[test]
fn descriptor_validates_and_lists_the_documented_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(DESCRIPTOR.type_id, "atomik.granular");
    assert_eq!(DESCRIPTOR.name, "Grain Sampler");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    assert_eq!(PARAMS.len(), 51);
    for (i, p) in DESCRIPTOR.params.iter().enumerate() {
        assert_eq!(p.id.0 as usize, i, "{}", p.key);
        assert!(
            (p.min..=p.max).contains(&p.default),
            "{} default in range",
            p.key
        );
    }
    let keys: Vec<&str> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(
        keys,
        [
            "notes_in",
            "pitch_cv_in",
            "position_cv_in",
            "density_cv_in",
            "cutoff_cv_in",
            "level_cv_in",
            "out_l",
            "out_r"
        ]
    );
    assert_eq!(
        DESCRIPTOR.param_by_key("sound").unwrap().unit,
        Unit::Choice(&sounds::SOUND_NAMES)
    );
    assert_eq!(DESCRIPTOR.param_by_key("voices").unwrap().default, 8.0);
    assert_eq!(DESCRIPTOR.param_by_key("grain.limit").unwrap().max, 64.0);
    assert_eq!(DESCRIPTOR.param_by_key("pos.scan").unwrap().default, 0.0);
}

#[test]
fn the_factory_registers_a_device_with_the_same_descriptor() {
    let f = factory();
    assert_eq!(f.descriptor.type_id, DESCRIPTOR.type_id);
    assert_eq!((f.create)().descriptor().type_id, DESCRIPTOR.type_id);
}

// ------------------------------------------------------------------------ the built-in source

#[test]
fn without_a_map_the_built_in_source_sounds_in_stereo() {
    let (mut h, _) = granular();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(1.0));
    assert!(
        rms(&l[4_800..]) > 0.02 && rms(&r[4_800..]) > 0.02,
        "audible"
    );
    assert!(peak(&l) <= 4.0 && peak(&r) <= 4.0);
    // The spread pans the grains: the channels differ.
    let diff: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum::<f32>() / l.len() as f32;
    assert!(diff > 0.005, "{diff}");
    assert_eq!(meter(&h)[0].to_bits(), 0.0f32.to_bits(), "source: built-in");
}

#[test]
fn the_built_in_source_follows_the_key_like_a_sample_rooted_at_middle_c() {
    let (mut h, p) = granular();
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 72,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    assert!((p.semis_last() - 12.0).abs() < 1e-5);
    let (mut h, p) = granular();
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 48,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    assert!((p.semis_last() + 12.0).abs() < 1e-5);
}

#[test]
fn a_map_without_a_matching_zone_is_silent() {
    let mut z = zone(0, 1000);
    z.key_lo = 0;
    z.key_hi = 50;
    let m = map(vec![buffer(vec![vec![0.5; 1000]])], vec![z]);
    let (mut h, p) = with_map(&m);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 70,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(0.5));
    assert_eq!(peak(&l).max(peak(&r)).to_bits(), 0.0f32.to_bits());
    assert_eq!(p.spawned(), 0);
    // A note in the zone plays.
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 40,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.5));
    assert!(peak(&l) > 0.1);
}

#[test]
fn the_first_matching_zone_plays() {
    let a = buffer(vec![vec![0.25; 4_000]]);
    let b = buffer(vec![vec![0.75; 4_000]]);
    let mut z0 = zone(0, 4_000);
    z0.key_hi = 59;
    let z1 = zone(1, 4_000);
    let z2 = zone(0, 4_000); // also matches, later in the list
    let m = map(vec![a, b], vec![z0, z1, z2]);
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.window", 1.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.5));
    // The 0.75 buffer through the centre pan (0.707) at the flat part of the window.
    assert!(
        (l[12_000] - 0.75 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
        "{}",
        l[12_000]
    );
    assert!((meter(&h)[0] - 2.0).abs() < 1e-6, "zone 1 is source 2");
}

// ---------------------------------------------------------------------------------- pitch

#[test]
fn a_sine_sample_is_heard_at_the_pitch_the_key_asks_for() {
    for key in [36u8, 48, 55, 60, 64, 67, 72, 79, 84] {
        let want = 480.0 * ((f64::from(key) - 60.0) / 12.0).exp2();
        let got = heard_hz(480.0, |_| {}, key);
        assert!(
            (got / want - 1.0).abs() < 5e-4,
            "key {key}: {got} Hz, want {want}"
        );
    }
}

#[test]
fn key_tracking_zero_plays_every_key_at_the_recorded_pitch() {
    for key in [36u8, 60, 84] {
        let got = heard_hz(480.0, |h| h.set_param("pitch.key_track", 0.0), key);
        assert!((got / 480.0 - 1.0).abs() < 5e-4, "key {key}: {got}");
    }
    // Half tracking: half the distance.
    let got = heard_hz(480.0, |h| h.set_param("pitch.key_track", 0.5), 72);
    assert!((got / (480.0 * 0.5f64.exp2()) - 1.0).abs() < 5e-4, "{got}");
}

#[test]
fn transpose_fine_tune_and_the_zones_tuning_add_up() {
    let cents = |c: f64| (c / 1200.0).exp2();
    let got = heard_hz(480.0, |h| h.set_param("pitch.transpose", 7.0), 60);
    assert!((got / (480.0 * cents(700.0)) - 1.0).abs() < 5e-4, "{got}");
    let got = heard_hz(480.0, |h| h.set_param("pitch.fine", 50.0), 60);
    assert!((got / (480.0 * cents(50.0)) - 1.0).abs() < 5e-4, "{got}");
    let got = heard_hz(
        480.0,
        |h| {
            h.set_param("pitch.transpose", -5.0);
            h.set_param("pitch.fine", -30.0);
        },
        62,
    );
    assert!(
        (got / (480.0 * cents(200.0 - 500.0 - 30.0)) - 1.0).abs() < 5e-4,
        "{got}"
    );
    // The zone's own transpose, tune and root key.
    let m = mono_map(sine(96_000, 480.0, 0.8), |z| {
        z.transpose = 3;
        z.tune_cents = 25.0;
        z.root_key = 57;
    });
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.5));
    let got = zero_crossing_hz(&l[6_000..18_000]);
    assert!(
        (got / (480.0 * cents(300.0 + 300.0 + 25.0)) - 1.0).abs() < 5e-4,
        "{got}"
    );
}

#[test]
fn pitch_bend_moves_the_grains_by_two_semitones_at_most() {
    let setup = |bend: f32| {
        move |h: &mut DeviceHarness| {
            push(h, 0, NoteKind::PitchBend { value: bend });
        }
    };
    let up = heard_hz(480.0, setup(1.0), 60);
    assert!(
        (up / (480.0 * (2.0f64 / 12.0).exp2()) - 1.0).abs() < 5e-4,
        "{up}"
    );
    let down = heard_hz(480.0, setup(-0.5), 60);
    assert!(
        (down / (480.0 * (-1.0f64 / 12.0).exp2()) - 1.0).abs() < 5e-4,
        "{down}"
    );
}

#[test]
fn the_pitch_cv_moves_the_pitch_by_its_amount() {
    let got = heard_hz(
        480.0,
        |h| {
            *h.input_mut("pitch_cv_in") = [0.5; BATCH];
        },
        60,
    );
    // 0.5 of the default 12 semitones.
    assert!((got / (480.0 * 0.5f64.exp2()) - 1.0).abs() < 5e-4, "{got}");
}

#[test]
fn the_pitch_scatter_spreads_the_grains_within_its_range_on_both_sides() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pitch.random", 100.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(5.0));
    let (lo, hi) = p.semis();
    assert!(lo >= -1.0 - 1e-5 && hi <= 1.0 + 1e-5, "{lo}..{hi}");
    assert!(lo < -0.9 && hi > 0.9, "uses the whole range: {lo}..{hi}");
}

#[test]
fn an_extreme_transposition_is_limited_to_three_octaves() {
    let m = mono_map(sine(96_000, 300.0, 0.8), |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("pitch.transpose", 24.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 127,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(0.5));
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 4.0));
    assert!(peak(&l) > 0.05);
    let hz = zero_crossing_hz(&l[6_000..18_000]);
    assert!((hz / (300.0 * 8.0) - 1.0).abs() < 5e-3, "{hz}");
}

// ------------------------------------------------------------------------------- aliasing

/// RMS of the middle of the first grain `semis` semitones up (the transpose up to its 24, the
/// key the rest) for a sine sample of `hz`.
fn grain_rms(hz: f64, semis: f32) -> f32 {
    let m = mono_map(sine(96_000, hz, 0.8), |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    let transpose = semis.min(24.0);
    h.set_param("pitch.transpose", transpose);
    h.set_param("pos.position", 0.05);
    let key = 60 + (semis - transpose) as u8;
    push(&mut h, 0, NoteKind::On { key, velocity: 1.0 });
    let (l, _) = render(&mut h, seconds(0.5));
    rms(&l[8_000..16_000])
}

#[test]
fn a_high_transposition_does_not_alias() {
    // 9 kHz two octaves up is 36 kHz: it would fold to 12 kHz at full level.
    let level = grain_rms(9_000.0, 0.0);
    let up = grain_rms(9_000.0, 24.0);
    assert!(level > 0.3, "premise: {level}");
    assert!(up < 0.2 * level, "aliased: {up} against {level}");
    // Three octaves.
    let up = grain_rms(4_800.0, 36.0);
    assert!(up < 0.25 * grain_rms(4_800.0, 0.0), "{up}");
}

#[test]
fn content_that_survives_a_transposition_keeps_its_level() {
    let level = grain_rms(1_000.0, 0.0);
    for t in [12.0, 24.0] {
        let db = 20.0 * (grain_rms(1_000.0, t) / level).log10();
        assert!(db > -1.5 && db < 0.5, "+{t} st: {db} dB");
    }
}

// ------------------------------------------------------------------------------ the scheduler

fn spawned_in(setup: impl Fn(&mut DeviceHarness), secs: f64) -> u64 {
    let (mut h, p) = granular();
    plain(&mut h);
    setup(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(secs));
    p.spawned()
}

#[test]
fn density_is_the_grains_a_second() {
    for density in [1.0f32, 5.0, 24.0, 90.0, 200.0] {
        let n = spawned_in(
            |h| {
                h.set_param("grain.density", density);
                h.set_param("grain.size", 20.0);
            },
            10.0,
        );
        let want = f64::from(density) * 10.0;
        assert!(
            (n as f64 - want).abs() <= 2.0,
            "{density}/s: {n} grains in 10 s, want {want}"
        );
    }
}

#[test]
fn the_timing_randomises_the_intervals_but_not_the_rate() {
    let n = spawned_in(
        |h| {
            h.set_param("grain.density", 50.0);
            h.set_param("grain.size", 20.0);
            h.set_param("grain.timing", 1.0);
        },
        20.0,
    );
    assert!((n as f64 - 1_000.0).abs() < 60.0, "{n}");
}

#[test]
fn the_density_cv_and_the_lfo_scale_the_density_in_octaves() {
    let n = spawned_in(
        |h| {
            h.set_param("grain.density", 20.0);
            h.set_param("grain.size", 20.0);
            *h.input_mut("density_cv_in") = [1.0; BATCH];
        },
        5.0,
    );
    // Full CV is two octaves up by default: four times the grains.
    assert!((n as f64 - 400.0).abs() < 6.0, "{n}");
    let n = spawned_in(
        |h| {
            h.set_param("grain.density", 20.0);
            h.set_param("grain.size", 20.0);
            h.set_param("cv.density_amount", 1.0);
            *h.input_mut("density_cv_in") = [-1.0; BATCH];
        },
        5.0,
    );
    assert!((n as f64 - 50.0).abs() < 4.0, "{n}");
}

#[test]
fn the_mod_envelope_speeds_the_density_up_and_it_settles() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 20.0);
    h.set_param("grain.size", 20.0);
    h.set_param("env.mod.attack", 0.5);
    h.set_param("env.mod.decay", 500.0);
    h.set_param("env.mod.sustain", 0.0);
    h.set_param("env.mod.to_density", 2.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.25));
    let early = p.spawned();
    render(&mut h, seconds(3.0));
    let before = p.spawned();
    render(&mut h, seconds(1.0));
    let late = p.spawned() - before;
    // 20 a second would give 5 in 250 ms; the envelope starts at four times that and settles.
    assert!(early >= 7, "{early} grains in 250 ms");
    assert!(
        (late as f64 - 20.0).abs() < 4.0,
        "settles to 20 a second: {late}"
    );
}

#[test]
fn the_size_sets_the_grain_length_and_the_lfo_scales_it_in_octaves() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 10.0);
    h.set_param("grain.size", 100.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(2.0));
    assert_eq!(p.len(), (4_800, 4_800), "100 ms is 4800 samples at 48 kHz");

    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 10.0);
    h.set_param("grain.size", 100.0);
    h.set_param("lfo.rate", 2.0);
    h.set_param("lfo.to_size", 1.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(4.0));
    let (lo, hi) = p.len();
    assert!(
        (2_400..3_100).contains(&lo),
        "half the size at the bottom: {lo}"
    );
    assert!(hi <= 9_600 && hi > 7_500, "double at the top: {hi}");
}

#[test]
fn the_size_is_limited_between_1_ms_and_2_seconds() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.size", 5.0);
    h.set_param("env.mod.to_size", -2.0);
    h.set_param("env.mod.sustain", 1.0);
    h.set_param("lfo.to_size", -2.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(1.0));
    assert!(p.len().0 >= 48, "1 ms at the least: {:?}", p.len());
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("env.mod.to_size", 2.0);
    h.set_param("env.mod.sustain", 1.0);
    h.set_param("grain.size", 500.0);
    h.set_param("lfo.to_size", 2.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(3.0));
    assert!(p.len().1 <= 96_000, "2 s at the most: {:?}", p.len());
}

// --------------------------------------------------------------- position, spray and scan

#[test]
fn a_frozen_unsprayed_cloud_starts_every_grain_at_the_position() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 50.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.37);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(2.0));
    assert_eq!(p.pos(), (0.37, 0.37));
}

#[test]
fn the_spray_is_a_band_centred_on_the_position() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.5);
    h.set_param("pos.spray", 0.2);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(10.0));
    let (lo, hi) = p.pos();
    assert!(
        lo >= 0.4 - 1e-6 && hi <= 0.6 + 1e-6,
        "inside the band: {lo}..{hi}"
    );
    assert!(lo < 0.41 && hi > 0.59, "and fills it: {lo}..{hi}");
}

#[test]
fn positions_stay_inside_the_sample_whatever_the_spray_and_position() {
    for (pos, spray) in [(0.0, 0.5), (1.0, 0.5), (0.02, 1.0), (0.98, 1.0), (0.5, 1.0)] {
        let (mut h, p) = granular();
        plain(&mut h);
        h.set_param("grain.density", 100.0);
        h.set_param("grain.size", 20.0);
        h.set_param("pos.position", pos);
        h.set_param("pos.spray", spray);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        render(&mut h, seconds(3.0));
        let (lo, hi) = p.pos();
        assert!(
            lo >= 0.0 && hi <= 1.0,
            "position {pos} spray {spray}: {lo}..{hi}"
        );
    }
}

#[test]
fn the_position_at_the_ends_reflects_the_spray_instead_of_piling_up() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.0);
    h.set_param("pos.spray", 0.3);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(5.0));
    let (lo, hi) = p.pos();
    assert!(lo >= 0.0 && hi <= 0.15 + 1e-6 && hi > 0.14, "{lo}..{hi}");
}

#[test]
fn scan_moves_the_position_through_the_sample_at_that_speed() {
    // A 2 s sample: a scan of 1 is half the window a second.
    for (scan, want) in [(1.0f32, 0.5), (0.5, 0.25), (-1.0, -0.5), (2.0, 1.0)] {
        let m = mono_map(sine(96_000, 200.0, 0.5), |_| {});
        let (mut h, p) = with_map(&m);
        plain(&mut h);
        h.set_param("grain.density", 50.0);
        h.set_param("grain.size", 20.0);
        h.set_param("pos.position", 0.5);
        h.set_param("pos.scan", scan);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        render(&mut h, seconds(1.0));
        let moved = (f64::from(p.pos_last()) - 0.5).rem_euclid(1.0);
        let moved = if want < 0.0 { moved - 1.0 } else { moved };
        assert!(
            (moved - want)
                .rem_euclid(1.0)
                .min((want - moved).rem_euclid(1.0))
                < 0.03,
            "scan {scan}: moved {moved}, want {want}"
        );
    }
}

#[test]
fn a_scan_of_zero_freezes_the_position_and_it_wraps_round_the_sample() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 50.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.25);
    render(&mut h, 1);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(3.0));
    assert_eq!(p.pos(), (0.25, 0.25), "frozen");
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 50.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.scan", 4.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(10.0));
    let (lo, hi) = p.pos();
    assert!(
        lo >= 0.0 && hi <= 1.0 && lo < 0.05 && hi > 0.95,
        "wraps: {lo}..{hi}"
    );
}

#[test]
fn the_scan_restarts_at_each_note() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 50.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.2);
    h.set_param("pos.scan", 1.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(1.0));
    push(&mut h, 0, NoteKind::Off { key: 60 });
    render(&mut h, seconds(0.5));
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, 1);
    assert!((p.pos_last() - 0.2).abs() < 0.01, "{}", p.pos_last());
}

#[test]
fn the_mod_envelope_the_lfo_the_wheel_and_the_cv_move_the_position() {
    // Mod envelope: up by 0.5 at the peak, back to the position as it decays.
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.2);
    h.set_param("env.mod.attack", 0.5);
    h.set_param("env.mod.decay", 400.0);
    h.set_param("env.mod.sustain", 0.0);
    h.set_param("env.mod.to_position", 0.5);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.1));
    assert!(p.pos().1 > 0.55, "up: {:?}", p.pos());
    render(&mut h, seconds(3.0));
    assert!(
        (p.pos_last() - 0.2).abs() < 0.01,
        "settled: {}",
        p.pos_last()
    );
    // LFO: a sine through 0.1 either side.
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.5);
    h.set_param("lfo.rate", 2.0);
    h.set_param("lfo.to_position", 0.1);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(3.0));
    let (lo, hi) = p.pos();
    assert!(
        lo > 0.39 && lo < 0.41 && hi < 0.61 && hi > 0.59,
        "{lo}..{hi}"
    );
    // Wheel to position (CC 1 at full) and CV.
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.2);
    h.set_param("wheel.to_position", 0.5);
    push(
        &mut h,
        0,
        NoteKind::Cc {
            controller: 1,
            value: 1.0,
        },
    );
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    assert!((p.pos_last() - 0.7).abs() < 1e-5, "{}", p.pos_last());
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.size", 20.0);
    h.set_param("pos.position", 0.2);
    *h.input_mut("position_cv_in") = [1.0; BATCH];
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    // Full CV adds 0.5 by default.
    assert!((p.pos_last() - 0.7).abs() < 1e-5, "{}", p.pos_last());
}

#[test]
fn the_mod_wheel_opens_the_spray_by_default() {
    let run = |wheel: f32| {
        let (mut h, p) = granular();
        plain(&mut h);
        h.set_param("grain.density", 100.0);
        h.set_param("grain.size", 20.0);
        h.set_param("pos.position", 0.5);
        h.set_param("wheel.to_spray", 0.5);
        push(
            &mut h,
            0,
            NoteKind::Cc {
                controller: 1,
                value: wheel,
            },
        );
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        render(&mut h, seconds(5.0));
        p.pos()
    };
    assert_eq!(run(0.0), (0.5, 0.5));
    let (lo, hi) = run(1.0);
    // A spray of 0.5: a band of 0.5 centred on the position.
    assert!(
        lo >= 0.25 - 1e-6 && hi <= 0.75 + 1e-6 && lo < 0.27 && hi > 0.73,
        "{lo}..{hi}"
    );
}

#[test]
fn the_default_wheel_amounts_open_the_spray_and_leave_the_position() {
    assert!(DESCRIPTOR.param_by_key("wheel.to_spray").unwrap().default > 0.0);
    assert!(
        DESCRIPTOR
            .param_by_key("wheel.to_position")
            .unwrap()
            .default
            .abs()
            < f32::EPSILON
    );
}

// ------------------------------------------------------------------------------- direction

#[test]
fn the_reverse_amount_is_the_share_of_grains_that_run_backwards() {
    let share = |amount: f32| {
        let (mut h, p) = granular();
        plain(&mut h);
        h.set_param("grain.density", 100.0);
        h.set_param("grain.size", 20.0);
        h.set_param("grain.reverse", amount);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        render(&mut h, seconds(10.0));
        p.reversed() as f64 / p.spawned() as f64
    };
    assert!(share(0.0).abs() < f64::EPSILON);
    assert!((share(1.0) - 1.0).abs() < f64::EPSILON);
    let half = share(0.5);
    assert!((half - 0.5).abs() < 0.06, "{half}");
    let tenth = share(0.1);
    assert!((tenth - 0.1).abs() < 0.04, "{tenth}");
}

#[test]
fn a_reversed_grain_plays_the_sample_backwards() {
    let ramp: Vec<f32> = (0..96_000).map(|i| i as f32 / 96_000.0).collect();
    let slope = |reverse: f32| {
        let m = mono_map(ramp.clone(), |_| {});
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("pos.position", 0.5);
        h.set_param("grain.reverse", reverse);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let (l, _) = render(&mut h, seconds(0.5));
        (l[18_000] - l[8_000]) / 10_000.0
    };
    // Forwards the ramp rises by 10000/96000 over the stretch, backwards it falls.
    assert!(slope(0.0) > 0.0);
    assert!(slope(1.0) < 0.0);
    let rise = 10_000.0 / 96_000.0 * std::f32::consts::FRAC_1_SQRT_2 / 10_000.0;
    assert!((slope(0.0) / rise - 1.0).abs() < 0.01);
    assert!((slope(1.0) / rise + 1.0).abs() < 0.01);
}

// --------------------------------------------------------------------------------- windows

/// The output of one grain over a constant source (so it is the window, scaled by the
/// centre pan): 500 ms, then 500 ms of silence.
fn window_shape(window: f32) -> Vec<f32> {
    let m = mono_map(vec![1.0; 96_000], |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.window", window);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.5)).0
}

#[test]
fn the_three_windows_have_their_shapes() {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let hann = window_shape(0.0);
    let (at, top) = hann
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
    assert!(
        (11_000..13_000).contains(&at),
        "Hann peaks in the middle: {at}"
    );
    assert!((top - k).abs() < 0.01, "{top}");
    assert!(hann[200] < 0.01 && hann[23_800] < 0.01);

    let tukey = window_shape(1.0);
    for i in [7_000, 12_000, 17_000] {
        assert!(
            (tukey[i] - k).abs() < 0.01,
            "Tukey is flat at {i}: {}",
            tukey[i]
        );
    }
    assert!(tukey[1_000] < 0.5 * k, "and tapers in");

    let perc = window_shape(2.0);
    let (at, top) = perc
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
    assert!(at < 1_500, "percussive peaks at once: {at}");
    assert!((top - k).abs() < 0.02, "{top}");
    assert!(perc[12_000] < 0.2 * k, "and falls away: {}", perc[12_000]);
    let first: f32 = perc[..12_000].iter().map(|v| v * v).sum();
    let second: f32 = perc[12_000..24_000].iter().map(|v| v * v).sum();
    assert!(first > 5.0 * second, "front-loaded");
}

#[test]
fn hann_grains_half_a_length_apart_sum_to_a_steady_level() {
    let m = mono_map(vec![1.0; 96_000], |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.window", 0.0);
    h.set_param("grain.density", 8.0);
    h.set_param("grain.size", 250.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(2.0));
    // Overlap 2: each grain is scaled by 1/sqrt(2), the two sum to 1, the pan gives 0.707.
    let want = std::f32::consts::FRAC_1_SQRT_2 * std::f32::consts::FRAC_1_SQRT_2;
    for (i, v) in l[24_000..].iter().enumerate() {
        assert!((v - want).abs() < 0.01, "sample {}: {v}", i + 24_000);
    }
}

#[test]
fn the_level_stays_in_the_same_range_as_density_and_size_change() {
    let level = |density: f32, size: f32| {
        let (mut h, _) = granular();
        plain(&mut h);
        h.set_param("grain.window", 0.0);
        h.set_param("grain.density", density);
        h.set_param("grain.size", size);
        h.set_param("grain.timing", 0.5);
        h.set_param("pos.spray", 0.5);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let (l, _) = render(&mut h, seconds(3.0));
        rms(&l[48_000..])
    };
    let base = level(24.0, 90.0);
    for (d, s) in [
        (8.0, 90.0),
        (60.0, 90.0),
        (24.0, 30.0),
        (24.0, 300.0),
        (100.0, 400.0),
    ] {
        let r = level(d, s) / base;
        assert!(r > 0.3 && r < 3.0, "density {d}, size {s}: {r}");
    }
}

// ----------------------------------------------------------------------- the grain cap

fn grains_of(h: &DeviceHarness) -> usize {
    meter(h)[3] as usize
}

#[test]
fn the_grain_limit_caps_the_sounding_grains_and_steals_deterministically() {
    for limit in [1.0f32, 4.0, 16.0, 64.0] {
        let (mut h, p) = granular();
        plain(&mut h);
        h.set_param("grain.density", 200.0);
        h.set_param("grain.size", 500.0);
        h.set_param("grain.limit", limit);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let mut most = 0;
        for _ in 0..seconds(2.0) {
            h.process();
            most = most.max(grains_of(&h));
        }
        assert_eq!(most, limit as usize, "limit {limit}");
        assert!(p.stolen() > 0, "limit {limit}: the full pool steals");
    }
    // Two runs steal the same grains: the output is the same.
    let run = || {
        let (mut h, _) = granular();
        plain(&mut h);
        h.set_param("grain.density", 150.0);
        h.set_param("grain.size", 400.0);
        h.set_param("grain.limit", 6.0);
        h.set_param("pos.spray", 0.6);
        h.set_param("grain.reverse", 0.4);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        h.render(seconds(1.0), &OUT)
    };
    assert_eq!(run(), run());
}

#[test]
fn a_pool_below_its_limit_never_steals() {
    let (mut h, p) = granular();
    plain(&mut h);
    h.set_param("grain.density", 20.0);
    h.set_param("grain.size", 100.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(3.0));
    assert_eq!(p.stolen(), 0);
}

#[test]
fn stealing_a_grain_makes_no_click() {
    let (mut h, _) = granular();
    plain(&mut h);
    h.set_param("grain.window", 0.0);
    h.set_param("grain.density", 200.0);
    h.set_param("grain.size", 300.0);
    h.set_param("grain.limit", 8.0);
    h.set_param("pos.spray", 0.4);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(2.0));
    // Hann grains at 200 a second: sounds like a smooth bed, with no step much above the
    // sample-to-sample movement of the source itself.
    assert!(max_step(&l[4_800..]) < 0.25, "{}", max_step(&l[4_800..]));
}

// ----------------------------------------------------------------------- voices and envelope

fn voices_of(h: &DeviceHarness) -> usize {
    meter(h)[4] as usize
}

#[test]
fn nine_notes_on_eight_voices_steal_the_oldest_with_a_short_fade() {
    let (mut h, _) = granular();
    plain(&mut h);
    h.set_param("grain.density", 40.0);
    h.set_param("grain.size", 60.0);
    for (i, key) in [48u8, 50, 52, 53, 55, 57, 59, 60, 62].iter().enumerate() {
        push(
            &mut h,
            (i * 4) as u16,
            NoteKind::On {
                key: *key,
                velocity: 0.8,
            },
        );
    }
    let (l, _) = render(&mut h, seconds(0.5));
    assert_eq!(voices_of(&h), 8);
    assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 4.0));
}

#[test]
fn the_voices_param_limits_polyphony_and_lowering_it_fades_the_extra_voices() {
    let (mut h, _) = granular();
    plain(&mut h);
    h.set_param("voices", 3.0);
    for (i, key) in [48u8, 52, 55, 59, 62].iter().enumerate() {
        push(
            &mut h,
            i as u16,
            NoteKind::On {
                key: *key,
                velocity: 0.8,
            },
        );
    }
    render(&mut h, seconds(0.3));
    assert_eq!(voices_of(&h), 3);
    h.set_param("voices", 1.0);
    render(&mut h, seconds(0.2));
    assert_eq!(voices_of(&h), 1);
}

#[test]
fn a_note_on_is_sample_accurate() {
    let m = mono_map(vec![0.5; 96_000], |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.window", 2.0);
    push(
        &mut h,
        40,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, 1);
    assert!(l[..40].iter().all(|v| v.abs() < f32::EPSILON));
    assert!(l[40..].iter().any(|v| v.abs() > 0.0));
}

#[test]
fn the_amp_envelope_attacks_sustains_and_releases() {
    let m = mono_map(vec![0.5; 96_000], |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.window", 1.0);
    h.set_param("grain.density", 100.0);
    h.set_param("grain.size", 400.0);
    h.set_param("env.amp.attack", 300.0);
    h.set_param("env.amp.release", 300.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(1.0));
    assert!(peak(&l[..480]) < 0.2 * peak(&l[24_000..]), "attack");
    push(&mut h, 0, NoteKind::Off { key: 60 });
    let (l, _) = render(&mut h, seconds(0.3));
    assert!(
        peak(&l[..240]) > 4.0 * peak(&l[l.len() - 480..]).max(1e-6),
        "release falls"
    );
    render(&mut h, seconds(0.5));
    assert_eq!(voices_of(&h), 0, "the voice ends after its release");
    assert_eq!(grains_of(&h), 0);
}

/// The built-in source with a quick release.
fn performance_synth() -> DeviceHarness {
    let (mut h, _) = granular();
    h.set_param("env.amp.release", 5.0);
    h
}

#[test]
fn sustain_pedal_holds_the_release_until_pedal_up() {
    assert_sustain_pedal(performance_synth, &OUT, ROOT);
}

#[test]
fn volume_and_expression_scale_the_output() {
    assert_volume_and_expression(performance_synth, &OUT, ROOT);
}

#[test]
fn bend_range_param_and_rpn_zero() {
    assert_bend_range(performance_synth, &OUT, ROOT);
}

#[test]
fn pressure_reaches_the_modulation_source() {
    let mut s = Granular::new();
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
fn note_off_releases_only_its_own_key_and_all_off_releases_every_voice() {
    let (mut h, _) = granular();
    plain(&mut h);
    h.set_param("env.amp.release", 20.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 48,
            velocity: 1.0,
        },
    );
    push(
        &mut h,
        1,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    push(&mut h, 0, NoteKind::Off { key: 48 });
    render(&mut h, seconds(0.3));
    assert_eq!(voices_of(&h), 1);
    push(&mut h, 0, NoteKind::AllOff);
    render(&mut h, seconds(0.3));
    assert_eq!(voices_of(&h), 0);
}

#[test]
fn velocity_scales_the_level_by_the_amount() {
    let level = |amount: f32, velocity: f32| {
        let m = mono_map(vec![0.5; 96_000], |_| {});
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("velocity.amp", amount);
        push(&mut h, 0, NoteKind::On { key: 60, velocity });
        let (l, _) = render(&mut h, seconds(0.5));
        peak(&l)
    };
    let full = level(1.0, 1.0);
    assert!((level(1.0, 0.5) / full - 0.5).abs() < 0.01);
    assert!((level(0.5, 0.5) / full - 0.75).abs() < 0.01);
    assert!((level(0.0, 0.2) / full - 1.0).abs() < 0.01);
}

#[test]
fn the_master_level_and_the_level_cv_scale_the_output() {
    let level = |setup: &dyn Fn(&mut DeviceHarness)| {
        let m = mono_map(vec![0.5; 96_000], |_| {});
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        setup(&mut h);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let (l, _) = render(&mut h, seconds(0.5));
        peak(&l)
    };
    let base = level(&|_| {});
    let down = level(&|h| h.set_param("master.level", -6.0));
    assert!((20.0 * (down / base).log10() + 6.0).abs() < 0.1);
    let cv = level(&|h| *h.input_mut("level_cv_in") = [-0.5; BATCH]);
    // Half of 24 dB down.
    assert!((20.0 * (cv / base).log10() + 12.0).abs() < 0.2);
}

// ------------------------------------------------------------------------------ the filter

#[test]
fn the_filter_shapes_the_grains_and_is_off_by_default() {
    assert!(DESCRIPTOR.param_by_key("filter.enabled").unwrap().default < 0.5);
    let level = |hz: f64, mode: f32, cutoff: f32| {
        let m = mono_map(sine(96_000, hz, 0.8), |_| {});
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("pitch.key_track", 0.0);
        h.set_param("filter.enabled", 1.0);
        h.set_param("filter.mode", mode);
        h.set_param("filter.cutoff", cutoff);
        h.set_param("filter.resonance", 0.0);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let (l, _) = render(&mut h, seconds(0.5));
        rms(&l[8_000..16_000])
    };
    let open = level(4_000.0, 0.0, 20_000.0);
    assert!(level(4_000.0, 0.0, 400.0) < 0.05 * open, "low-pass");
    assert!(
        level(200.0, 2.0, 4_000.0) < 0.05 * level(200.0, 2.0, 20.0).max(open),
        "high-pass"
    );
    assert!(
        level(1_000.0, 1.0, 1_000.0) > 0.3 * open,
        "band-pass passes its centre"
    );
}

#[test]
fn the_filter_follows_the_key_the_mod_envelope_and_the_cv() {
    let level = |setup: &dyn Fn(&mut DeviceHarness), key: u8| {
        let m = mono_map(sine(96_000, 480.0, 0.8), |_| {});
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("pitch.key_track", 0.0);
        h.set_param("filter.enabled", 1.0);
        h.set_param("filter.cutoff", 300.0);
        h.set_param("filter.resonance", 0.0);
        setup(&mut h);
        push(&mut h, 0, NoteKind::On { key, velocity: 1.0 });
        let (l, _) = render(&mut h, seconds(0.5));
        rms(&l[8_000..16_000])
    };
    let shut = level(&|_| {}, 60);
    // One octave of key tracking per octave: a high key opens a low-pass on a 480 Hz tone.
    assert!(level(&|h| h.set_param("filter.key_track", 1.0), 84) > 2.5 * shut);
    assert!(
        level(
            &|h| {
                h.set_param("filter.env_amount", 1.0);
                h.set_param("env.mod.sustain", 1.0);
            },
            60
        ) > 2.5 * shut,
        "mod envelope"
    );
    assert!(
        level(&|h| *h.input_mut("cutoff_cv_in") = [1.0; BATCH], 60) > 2.5 * shut,
        "cv"
    );
}

// --------------------------------------------------------------------------- stereo and zones

#[test]
fn a_stereo_sample_keeps_its_channels_apart_and_a_mono_one_is_panned_by_the_law() {
    let m = map(
        vec![buffer(vec![sine(96_000, 480.0, 0.8), vec![0.0; 96_000]])],
        vec![zone(0, 96_000)],
    );
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(0.5));
    assert!(rms(&l[8_000..16_000]) > 0.3);
    assert!(
        peak(&r).abs() < f32::EPSILON,
        "the right channel stays silent"
    );
    // A mono sample at the centre is 3 dB down on each side; panned hard left it is on the left
    // only.
    let m = mono_map(vec![0.5; 96_000], |z| z.pan = -1.0);
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(0.5));
    assert!((l[12_000] - 0.5).abs() < 0.01 && r[12_000].abs() < 1e-4);
}

#[test]
fn the_spread_pans_each_grain_at_random_and_zero_keeps_the_centre() {
    let m = mono_map(vec![0.5; 96_000], |_| {});
    let run = |spread: f32| {
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("grain.window", 1.0);
        h.set_param("grain.density", 40.0);
        h.set_param("grain.size", 30.0);
        h.set_param("grain.spread", spread);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        render(&mut h, seconds(1.0))
    };
    let (l, r) = run(0.0);
    assert!(
        l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-6),
        "centred"
    );
    let (l, r) = run(1.0);
    let d: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum::<f32>() / l.len() as f32;
    assert!(d > 0.01, "spread: {d}");
}

#[test]
fn the_zone_window_defines_the_source_and_the_zone_gain_applies() {
    // The first half is silent, the second half is 0.5.
    let mut x = vec![0.0f32; 48_000];
    x.extend(vec![0.5f32; 48_000]);
    let level = |start: u64, end: u64, gain: f32| {
        let m = mono_map(x.clone(), |z| {
            z.start = start;
            z.end = end;
            z.gain_db = gain;
        });
        let (mut h, _) = with_map(&m);
        plain(&mut h);
        h.set_param("pos.position", 0.5);
        push(
            &mut h,
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        let (l, _) = render(&mut h, seconds(0.5));
        peak(&l)
    };
    assert!(level(0, 48_000, 0.0) < 1e-6, "the silent half");
    let on = level(48_000, 96_000, 0.0);
    assert!(
        (on - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
        "{on}"
    );
    let quiet = level(48_000, 96_000, -6.0);
    assert!((20.0 * (quiet / on).log10() + 6.0).abs() < 0.1);
    // The position is a fraction of the window, not of the buffer: 0 % of the second half.
    let m = mono_map(x.clone(), |z| {
        z.start = 48_000;
        z.end = 96_000;
    });
    let (mut h, p) = with_map(&m);
    plain(&mut h);
    h.set_param("pos.position", 0.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.5));
    assert!(peak(&l) > 0.3 && p.pos() == (0.0, 0.0));
}

// ------------------------------------------------------------------------ swapping samples

#[test]
fn without_voices_the_replaced_map_comes_back_at_once() {
    let (mut h, _) = granular();
    let a = mono_map(vec![0.5; 4_000], |_| {});
    let b = mono_map(vec![0.25; 4_000], |_| {});
    assert!(h.device_mut().swap_samples(Some(a.clone())).is_none());
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(same(&a, &back), "nobody needs `a`: it is handed back");
    let back = h.device_mut().swap_samples(None);
    assert!(same(&b, &back));
    assert!(h.device_mut().swap_samples(None).is_none());
}

#[test]
fn removing_the_map_returns_to_the_built_in_source() {
    let a = mono_map(vec![0.5; 4_000], |z| z.key_hi = 10);
    let (mut h, p) = with_map(&a);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.3));
    assert!(peak(&l) < 1e-6, "no zone for the key: silent");
    assert!(h.device_mut().swap_samples(None).is_some());
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, _) = render(&mut h, seconds(0.5));
    assert!(peak(&l) > 0.05 && p.spawned() > 0);
}

#[test]
fn voices_move_to_a_map_that_shares_their_buffer() {
    let buf = buffer(vec![vec![0.5f32; 96_000]]);
    let a = map(vec![buf.clone()], vec![zone(0, 96_000)]);
    let b = map(vec![buf.clone()], vec![zone(0, 96_000), zone(0, 96_000)]);
    let (mut h, _) = with_map(&a);
    plain(&mut h);
    h.set_param("grain.size", 500.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (before, _) = render(&mut h, seconds(0.2));
    let back = h.device_mut().swap_samples(Some(b.clone()));
    assert!(same(&a, &back), "the voice moved to `b`: `a` is not needed");
    let (after, _) = render(&mut h, seconds(0.2));
    assert!(peak(&before) > 0.2 && peak(&after) > 0.2);
    assert!(max_step(&after) < 0.05, "no jump: {}", max_step(&after));
    assert_eq!(voices_of(&h), 1);
}

#[test]
fn a_voice_whose_zone_vanished_fades_out_in_5_ms_and_the_map_is_handed_back_later() {
    let a = mono_map(vec![0.5; 96_000], |_| {});
    let b = mono_map(vec![0.25; 96_000], |_| {});
    let c = mono_map(vec![0.125; 96_000], |_| {});
    let (mut h, _) = with_map(&a);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    assert!(
        h.device_mut().swap_samples(Some(b.clone())).is_none(),
        "`a` is draining: it stays in the device"
    );
    let (fade, _) = render(&mut h, 4);
    // 5 ms is 240 samples: falling from the old sample's level to nothing.
    assert!(
        fade[0] > 0.2 && fade[239] < 0.05 * fade[0],
        "{} {}",
        fade[0],
        fade[239]
    );
    assert!(fade[250..].iter().all(|v| v.abs() < 1e-6), "and gone");
    assert_eq!(voices_of(&h), 0);
    // The stale map comes back at the next swap, and exactly one map at a time.
    let back = h.device_mut().swap_samples(Some(c.clone()));
    assert!(same(&a, &back), "the draining map is handed back");
    let back = h.device_mut().swap_samples(None);
    assert!(same(&b, &back));
    let back = h.device_mut().swap_samples(None);
    assert!(same(&c, &back));
}

#[test]
fn a_second_swap_cuts_a_voice_still_draining() {
    let a = mono_map(vec![0.5; 96_000], |_| {});
    let b = mono_map(vec![0.25; 96_000], |_| {});
    let c = mono_map(vec![0.125; 96_000], |_| {});
    let (mut h, _) = with_map(&a);
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    assert!(h.device_mut().swap_samples(Some(b)).is_none());
    // Not a sample later: the voice of `a` is still draining.
    let back = h.device_mut().swap_samples(Some(c));
    assert!(same(&a, &back), "the cut frees `a`");
    let (l, _) = render(&mut h, 1);
    assert!(l.iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn a_voice_on_the_built_in_source_plays_on_through_a_swap() {
    let (mut h, _) = granular();
    plain(&mut h);
    h.set_param("grain.size", 100.0);
    h.set_param("grain.density", 30.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.5));
    let a = mono_map(vec![0.5; 96_000], |_| {});
    assert!(h.device_mut().swap_samples(Some(a.clone())).is_none());
    let (l, _) = render(&mut h, seconds(0.3));
    assert!(rms(&l) > 0.02, "still sounding");
    assert_eq!(voices_of(&h), 1);
    assert!(h.device_mut().swap_samples(None).is_some());
}

#[test]
fn a_swap_while_notes_play_allocates_nothing() {
    let a = mono_map(sine(96_000, 300.0, 0.5), |_| {});
    let b = mono_map(sine(96_000, 500.0, 0.5), |_| {});
    let (mut h, _) = with_map(&a);
    h.set_param("grain.density", 80.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    push(
        &mut h,
        1,
        NoteKind::On {
            key: 67,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.2));
    let mut held = Vec::new();
    for m in [&b, &a, &b] {
        let back = assert_no_alloc(|| h.device_mut().swap_samples(Some(m.clone())));
        held.push(back);
        render(&mut h, 20);
    }
    let back = assert_no_alloc(|| h.device_mut().swap_samples(None));
    held.push(back);
    render(&mut h, 20);
    drop(held);
}

// ---------------------------------------------------------------------------- panel values

#[test]
fn an_idle_device_publishes_no_position_and_no_grains() {
    let (mut h, _) = granular();
    h.process();
    let v = meter(&h);
    assert!(v[0].abs() < f32::EPSILON, "built-in source");
    assert!((v[1] - NONE).abs() < f32::EPSILON && (v[2] - NONE).abs() < f32::EPSILON);
    assert!(v[3].abs() < f32::EPSILON && v[4].abs() < f32::EPSILON);
    assert!(v[5..].iter().all(|x| (x - NONE).abs() < f32::EPSILON));
    let a = mono_map(vec![0.5; 4_000], |_| {});
    assert!(h.device_mut().swap_samples(Some(a)).is_none());
    h.process();
    assert!(
        (meter(&h)[0] - 1.0).abs() < f32::EPSILON,
        "zone 0: source 1"
    );
}

#[test]
fn the_device_publishes_the_newest_voices_position_spray_and_the_live_grains() {
    let m = mono_map(sine(96_000, 200.0, 0.5), |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.density", 40.0);
    h.set_param("grain.size", 200.0);
    h.set_param("pos.position", 0.3);
    h.set_param("pos.spray", 0.1);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(1.0));
    let v = meter(&h);
    assert!((v[1] - 0.3).abs() < 1e-5, "position {}", v[1]);
    assert!((v[2] - 0.1).abs() < 1e-5, "spray {}", v[2]);
    assert!(v[3] >= 6.0 && v[4].to_bits() == 1.0f32.to_bits());
    let live: Vec<(f32, f32)> = (0..TRAIL_GRAINS)
        .map(|i| (v[5 + 2 * i], v[6 + 2 * i]))
        .filter(|(p, _)| *p >= 0.0)
        .collect();
    assert!(live.len() >= 6, "{live:?}");
    for (pos, life) in live {
        assert!((0.0..=1.0).contains(&life), "life {life}");
        // The grains started inside the spray band, then moved at the pitch ratio of 1: the
        // window is two seconds, a grain is 0.2 s.
        assert!(
            (0.25 - 1e-5..=0.35 + 0.1 + 1e-5).contains(&pos),
            "grain at {pos}"
        );
    }
}

#[test]
fn a_scanning_cloud_publishes_a_moving_position() {
    let m = mono_map(sine(96_000, 200.0, 0.5), |_| {});
    let (mut h, _) = with_map(&m);
    plain(&mut h);
    h.set_param("grain.density", 40.0);
    h.set_param("grain.size", 100.0);
    h.set_param("pos.position", 0.0);
    h.set_param("pos.scan", 1.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    render(&mut h, seconds(0.5));
    let a = meter(&h)[1];
    render(&mut h, seconds(0.5));
    let b = meter(&h)[1];
    // A scan of 1 is half the two-second window a second: a quarter in half a second.
    assert!((f64::from(b - a) - 0.25).abs() < 0.02, "{a} -> {b}");
}

#[test]
fn the_published_values_are_bounded_and_finite_for_any_params() {
    let (mut h, _) = granular();
    for p in DESCRIPTOR.params {
        h.set_param(p.key, p.max);
    }
    h.set_param("voices", 16.0);
    for k in 0..16u8 {
        push(
            &mut h,
            u16::from(k),
            NoteKind::On {
                key: 30 + 5 * k,
                velocity: 1.0,
            },
        );
    }
    for _ in 0..seconds(1.0) {
        h.process();
        let v = meter(&h);
        assert!(v.iter().all(|x| x.is_finite()));
        assert!(v[1] <= 1.0 && v[2] <= 1.0);
        for i in 0..TRAIL_GRAINS {
            let (p, l) = (v[5 + 2 * i], v[6 + 2 * i]);
            assert!(((0.0..=1.0).contains(&p) && (0.0..=1.0).contains(&l)) || (p < 0.0 && l < 0.0));
        }
    }
}

// ---------------------------------------------------------------------------- determinism

fn busy(seed: u64, with_map_installed: bool) -> Vec<Vec<f32>> {
    let mut h = DeviceHarness::with_options(Box::new(Granular::new()), SR, 120.0, seed);
    if with_map_installed {
        let m = mono_map(sine(96_000, 333.0, 0.6), |_| {});
        assert!(h.device_mut().swap_samples(Some(m)).is_none());
    }
    h.set_param("grain.density", 60.0);
    h.set_param("grain.size", 80.0);
    h.set_param("grain.timing", 0.7);
    h.set_param("grain.reverse", 0.3);
    h.set_param("grain.spread", 0.8);
    h.set_param("pos.spray", 0.3);
    h.set_param("pos.scan", 0.5);
    h.set_param("pitch.random", 200.0);
    h.set_param("lfo.wave", 4.0);
    h.set_param("lfo.rate", 7.0);
    h.set_param("lfo.to_position", 0.2);
    h.set_param("filter.enabled", 1.0);
    for (i, k) in [40u8, 47, 52, 59].iter().enumerate() {
        h.push_note("notes_in", on(i as u16 * 5, *k, 0.7 + 0.05 * i as f32));
    }
    let a = h.render(seconds(0.3), &OUT);
    h.push_note("notes_in", off(10, 47));
    h.push_note("notes_in", on(20, 76, 0.9));
    let b = h.render(seconds(0.3), &OUT);
    a.into_iter().chain(b).collect()
}

#[test]
fn two_renders_are_identical_with_and_without_a_sample() {
    assert_eq!(busy(77, true), busy(77, true));
    assert_eq!(busy(77, false), busy(77, false));
    assert_ne!(busy(77, false), busy(78, false), "the seed matters");
}

#[test]
fn reset_replays_the_same_audio() {
    let mut h = DeviceHarness::with_options(Box::new(Granular::new()), SR, 120.0, 5);
    h.set_param("grain.timing", 0.8);
    h.set_param("pos.spray", 0.4);
    h.set_param("grain.reverse", 0.5);
    let play = |h: &mut DeviceHarness| {
        h.push_note("notes_in", on(0, 55, 0.8));
        h.push_note("notes_in", on(3, 62, 0.8));
        h.render(seconds(0.4), &OUT)
    };
    let first = play(&mut h);
    h.device_mut().reset();
    assert_eq!(play(&mut h), first);
}

#[test]
fn a_notes_grains_do_not_depend_on_what_sounded_before() {
    let play = |h: &mut DeviceHarness| {
        h.push_note("notes_in", on(0, 60, 1.0));
        h.render(seconds(0.3), &OUT)
    };
    let mut a = DeviceHarness::with_options(Box::new(Granular::new()), SR, 120.0, 9);
    a.set_param("pos.spray", 0.5);
    let fresh = play(&mut a);
    // The same note after another one that has ended: its own number, its own random stream.
    let mut b = DeviceHarness::with_options(Box::new(Granular::new()), SR, 120.0, 9);
    b.set_param("pos.spray", 0.5);
    b.device_mut().reset();
    assert_eq!(play(&mut b), fresh);
}

// ----------------------------------------------------------------------------- realtime

#[test]
fn set_param_reset_and_every_batch_allocate_nothing() {
    let (mut h, _) = granular();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    push(
        &mut h,
        5,
        NoteKind::On {
            key: 64,
            velocity: 0.7,
        },
    );
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            assert_no_alloc(|| h.set_param(p.key, v));
            h.render(3, &OUT);
        }
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.render(2, &OUT);
    // Meter values too.
    let mut buf = [0.0f32; 32];
    assert_no_alloc(|| h.device().meter_values(&mut buf));
}

#[test]
fn every_param_at_min_max_and_default_stays_finite_while_notes_play() {
    let (mut h, _) = granular();
    for (i, k) in [36u8, 60, 84, 108].iter().enumerate() {
        push(
            &mut h,
            i as u16,
            NoteKind::On {
                key: *k,
                velocity: 1.0,
            },
        );
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn output_is_bounded_with_everything_open() {
    let (mut h, _) = granular();
    for p in DESCRIPTOR.params {
        if !matches!(p.key, "sound") {
            h.set_param(p.key, p.max);
        }
    }
    h.set_param("master.level", 6.0);
    h.set_param("voices", 16.0);
    for k in 0..16u8 {
        push(
            &mut h,
            u16::from(k),
            NoteKind::On {
                key: 24 + 6 * k,
                velocity: 1.0,
            },
        );
    }
    *h.input_mut("pitch_cv_in") = [1.0; BATCH];
    *h.input_mut("position_cv_in") = [1.0; BATCH];
    *h.input_mut("density_cv_in") = [1.0; BATCH];
    *h.input_mut("cutoff_cv_in") = [1.0; BATCH];
    *h.input_mut("level_cv_in") = [1.0; BATCH];
    let (l, r) = render(&mut h, seconds(2.0));
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 4.0));
}

#[test]
fn bad_cv_values_are_ignored() {
    let (mut h, _) = granular();
    for socket in [
        "pitch_cv_in",
        "position_cv_in",
        "density_cv_in",
        "cutoff_cv_in",
        "level_cv_in",
    ] {
        *h.input_mut(socket) = [f32::NAN; BATCH];
    }
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (l, r) = render(&mut h, seconds(0.5));
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    assert!(peak(&l) > 0.01);
}

// -------------------------------------------------------------------------------- sounds

#[test]
fn every_sound_is_complete_and_in_range() {
    assert_eq!(sounds::SOUNDS.len(), sounds::SOUND_NAMES.len());
    for (i, s) in sounds::SOUNDS.iter().enumerate() {
        assert_eq!(s.name, sounds::SOUND_NAMES[i]);
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in s.changes {
            let p = DESCRIPTOR
                .param_by_key(key)
                .unwrap_or_else(|| panic!("{}: {key} is not a param", s.name));
            assert_ne!(*key, "sound", "{}", s.name);
            assert!(seen.insert(*key), "{}: {key} twice", s.name);
            assert!(
                (p.min..=p.max).contains(value),
                "{}: {key} = {value} outside {}..{}",
                s.name,
                p.min,
                p.max
            );
            if p.discrete {
                assert!(
                    (value.round() - *value).abs() < f32::EPSILON,
                    "{}: {key} is discrete",
                    s.name
                );
            }
        }
        let all = sounds::values(i);
        assert_eq!(all.len(), PARAM_COUNT - 1, "{}", s.name);
        assert!(all.iter().all(|(k, _)| *k != "sound"));
    }
    assert_eq!(sounds::values(99).len(), PARAM_COUNT - 1);
}

#[test]
fn the_init_sound_is_the_default_patch() {
    for (key, value) in sounds::values(0) {
        let p = DESCRIPTOR.param_by_key(key).unwrap();
        assert!((value - p.default).abs() < f32::EPSILON, "{key}");
    }
    assert_eq!(sounds::SOUND_NAMES[0], "Init");
}

fn sound_notes(index: usize, key: u8, velocity: f32, secs: f64) -> Vec<f32> {
    let (mut h, _) = granular();
    for (k, v) in sounds::values(index) {
        h.set_param(k, v);
    }
    push(&mut h, 0, NoteKind::On { key, velocity });
    render(&mut h, seconds(secs)).0
}

#[test]
fn every_sound_plays_the_built_in_source_on_every_register_and_stays_bounded() {
    for (i, s) in sounds::SOUNDS.iter().enumerate() {
        for key in [28u8, 48, 60, 72, 96] {
            let o = sound_notes(i, key, 0.8, 2.0);
            assert!(
                o.iter().all(|v| v.is_finite() && v.abs() <= 2.0),
                "{} key {key}",
                s.name
            );
            assert!(peak(&o) > 0.01, "{} key {key}: audible", s.name);
        }
    }
}

#[test]
fn the_sounds_differ_from_each_other() {
    let rendered: Vec<Vec<f32>> = (0..sounds::SOUNDS.len())
        .map(|i| sound_notes(i, 60, 0.8, 1.5))
        .collect();
    for a in 0..rendered.len() {
        for b in a + 1..rendered.len() {
            let d = rms_diff(&rendered[a][24_000..], &rendered[b][24_000..]);
            assert!(
                d > 0.005,
                "{} and {} sound alike ({d})",
                sounds::SOUND_NAMES[a],
                sounds::SOUND_NAMES[b]
            );
        }
    }
}

fn rms_diff(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    (a[..n]
        .iter()
        .zip(&b[..n])
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        / n as f32)
        .sqrt()
}

#[test]
fn the_sounds_do_what_their_names_say() {
    // The pad swells in slowly and is still sounding long after the note is released.
    let pad = sound_notes(1, 60, 0.9, 3.0);
    assert!(peak(&pad[..2_400]) < 0.15 * peak(&pad[96_000..]), "swell");
    let (mut h, _) = granular();
    for (k, v) in sounds::values(1) {
        h.set_param(k, v);
    }
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 0.9,
        },
    );
    render(&mut h, seconds(2.0));
    push(&mut h, 0, NoteKind::Off { key: 60 });
    let (tail, _) = render(&mut h, seconds(1.0));
    assert!(peak(&tail[24_000..]) > 0.005, "a long release");
    // The shimmer is an octave up.
    let (mut h, p) = granular();
    for (k, v) in sounds::values(2) {
        h.set_param(k, v);
    }
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 0.9,
        },
    );
    render(&mut h, seconds(1.0));
    let (lo, hi) = p.semis();
    assert!(lo > 11.0 && hi < 13.0, "{lo}..{hi}");
    // The stutter repeats: its grains are rigid and start at two positions.
    let (mut h, p) = granular();
    for (k, v) in sounds::values(3) {
        h.set_param(k, v);
    }
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 0.9,
        },
    );
    render(&mut h, seconds(2.0));
    let (lo, hi) = p.pos();
    assert!(
        hi - lo > 0.3,
        "a square LFO jumps between two slices: {lo}..{hi}"
    );
    // The texture scans and reverses.
    let (mut h, p) = granular();
    for (k, v) in sounds::values(4) {
        h.set_param(k, v);
    }
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 0.9,
        },
    );
    render(&mut h, seconds(2.0));
    let share = p.reversed() as f64 / p.spawned() as f64;
    assert!((share - 0.5).abs() < 0.15, "{share}");
    let (lo, hi) = p.pos();
    assert!(hi - lo > 0.3, "{lo}..{hi}");
}

#[test]
fn the_built_in_source_is_shared_between_devices_of_one_sample_rate() {
    let a = builtin_buffer(48_000.0);
    let b = builtin_buffer(48_000.0);
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(a.frames, 144_000);
    assert_eq!(builtin_buffer(44_100.0).frames, 132_300);
}

#[test]
fn master_tune_shifts_the_grains_by_the_cents_ratio() {
    for cents in [50.0f32, -100.0] {
        let got = heard_hz(480.0, |h| h.time_mut().tune_cents = cents, 60);
        let want = 480.0 * 2f64.powf(f64::from(cents) / 1200.0);
        assert!((got / want - 1.0).abs() < 5e-4, "{cents} cents: {got}");
    }
}
