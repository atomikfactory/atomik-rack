//! Drum machine tests: (pad playback) (choke and retrigger) (outputs) and
//! the device part of (allocation freedom, determinism, stealing).

#![allow(clippy::float_cmp)]
use super::*;
use crate::test_support::{SR, assert_keys, harness, off, on, sweep_params_min_max};
use atomik_core::{NoteEvent, NoteKind, db_to_gain};
use atomik_device_sdk::{LoopMode, SampleBuffer, TriggerMode};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::dominant_frequency;
use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Test-only window into the voice allocator (atomic stores only: no allocation).
#[derive(Debug)]
pub struct Probe {
    pub active: AtomicUsize,
    pub draining: AtomicUsize,
    pub cuts: AtomicU64,
    /// Per voice: 0 when idle, else 1 + the key it is playing (or will play after a steal).
    pub keys: [AtomicU32; VOICES],
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

/// `i / 4096`: exactly representable, so comparisons can be bit-exact.
fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 / 4096.0).collect()
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

/// A zone over the whole of `buffer` (`frames` long) on the pad key `key`, root = key.
fn zone(buffer: u32, frames: u64, key: u8) -> Zone {
    Zone {
        buffer,
        key_lo: key,
        key_hi: key,
        vel_lo: 1,
        vel_hi: 127,
        root_key: key,
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
    }
}

fn map(buffers: Vec<Arc<SampleBuffer>>, zones: Vec<Zone>) -> SampleMapRef {
    Arc::new(SampleMap::new(buffers, zones).unwrap())
}

fn same(a: &SampleMapRef, b: &Option<SampleMapRef>) -> bool {
    b.as_ref().is_some_and(|b| Arc::ptr_eq(a, b))
}

/// Pad 1..=16 to its key.
fn key_of(pad: usize) -> u8 {
    35 + pad as u8
}

/// A fresh drum machine: master 0 dB, every pad in gate mode (the amp holds at 1 while the key
/// is down), no velocity sensitivity.
fn drum() -> (DeviceHarness, Arc<Probe>) {
    let probe = Arc::new(Probe::new());
    let mut d = DrumMachine::new();
    d.probe = Some(probe.clone());
    let mut h = harness(Box::new(d));
    h.set_param("master.level", 0.0);
    for p in 1..=PAD_COUNT {
        h.set_param(&format!("pad{p}.mode"), 0.0);
        h.set_param(&format!("pad{p}.velocity"), 0.0);
    }
    (h, probe)
}

fn with_map(m: &SampleMapRef) -> (DeviceHarness, Arc<Probe>) {
    let (mut h, p) = drum();
    assert!(h.device_mut().swap_samples(Some(m.clone())).is_none());
    (h, p)
}

fn render(h: &mut DeviceHarness, batches: usize) -> (Vec<f32>, Vec<f32>) {
    let mut o = h.render(batches, &OUT);
    let r = o.pop().unwrap();
    (o.pop().unwrap(), r)
}

fn hit(h: &mut DeviceHarness, offset: u16, pad: usize, velocity: f32) {
    h.push_note("notes_in", on(offset, key_of(pad), velocity));
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

fn pad_out_keys(pad: usize) -> [String; 2] {
    [format!("pad{pad}_out_l"), format!("pad{pad}_out_r")]
}

fn connect_pad(h: &mut DeviceHarness, pad: usize) {
    for k in pad_out_keys(pad) {
        h.set_connected(&k, true);
    }
}

/// One stereo constant-level buffer on pads `pads`, each pad its own zone over buffer 0.
fn flat_kit(pads: &[usize], level: f32, frames: usize) -> SampleMapRef {
    let zones = pads
        .iter()
        .map(|p| zone(0, frames as u64, key_of(*p)))
        .collect();
    map(vec![buffer(vec![vec![level; frames]; 2])], zones)
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
    let mut params = vec!["master.level".to_string()];
    let mut sockets: Vec<String> = ["notes_in", "out_l", "out_r"].map(String::from).to_vec();
    for n in 1..=16 {
        for k in [
            "level", "pan", "tune", "fine", "decay", "mode", "choke", "velocity",
        ] {
            params.push(format!("pad{n}.{k}"));
        }
        sockets.push(format!("pad{n}_out_l"));
        sockets.push(format!("pad{n}_out_r"));
    }
    let p: Vec<&str> = params.iter().map(String::as_str).collect();
    let s: Vec<&str> = sockets.iter().map(String::as_str).collect();
    assert_keys(&DESCRIPTOR, &p, &s);
    assert_eq!(DESCRIPTOR.type_id, "atomik.drum-machine");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    assert_eq!(DESCRIPTOR.params.len(), 129);
    assert_eq!(DESCRIPTOR.sockets.len(), 35);
    let d = |k: &str| {
        let d = DESCRIPTOR.param_by_key(k).unwrap();
        (d.min, d.max, d.default)
    };
    assert_eq!(d("master.level"), (-60.0, 6.0, -6.0));
    for n in [1, 9, 16] {
        assert_eq!(d(&format!("pad{n}.level")), (-60.0, 6.0, 0.0));
        assert_eq!(d(&format!("pad{n}.pan")), (-1.0, 1.0, 0.0));
        assert_eq!(d(&format!("pad{n}.tune")), (-24.0, 24.0, 0.0));
        assert_eq!(d(&format!("pad{n}.fine")), (-100.0, 100.0, 0.0));
        assert_eq!(d(&format!("pad{n}.decay")), (5.0, 10_000.0, 500.0));
        assert_eq!(d(&format!("pad{n}.mode")), (0.0, 1.0, 1.0));
        assert_eq!(d(&format!("pad{n}.choke")), (0.0, 4.0, 0.0));
        assert_eq!(d(&format!("pad{n}.velocity")), (0.0, 1.0, 0.7));
    }
    for (i, sd) in DESCRIPTOR.sockets.iter().enumerate().skip(1) {
        assert_eq!(sd.kind, SocketKind::Audio, "{i}");
        assert_eq!(sd.direction, Direction::Output);
    }
    assert_eq!(factory().descriptor.type_id, "atomik.drum-machine");
    assert_eq!(pad_key(0), 36);
    assert_eq!(pad_key(15), 51);
}

// ---- pad playback ----------------------------------------------------------------

#[test]
fn every_pad_reproduces_its_zone_bit_exactly_after_the_attack() {
    let bufs: Vec<_> = (0..PAD_COUNT)
        .map(|p| {
            buffer(vec![
                noise(3000, 2 * p as u32),
                noise(3000, 2 * p as u32 + 1),
            ])
        })
        .collect();
    let zones = (1..=PAD_COUNT)
        .map(|p| {
            let mut z = zone(p as u32 - 1, 3000, key_of(p));
            z.start = 100;
            z
        })
        .collect();
    let m = map(bufs.clone(), zones);
    for p in 1..=PAD_COUNT {
        let (mut h, probe) = with_map(&m);
        hit(&mut h, 5, p, 1.0);
        let (ol, or) = render(&mut h, 4);
        assert!(ol[..5].iter().chain(&or[..5]).all(|v| *v == 0.0));
        assert_eq!(probe.keys(), vec![key_of(p)]);
        let (l, r) = (&bufs[p - 1].data[0], &bufs[p - 1].data[1]);
        let k = 5 + SETTLED;
        assert_eq!(bits(&ol[k..200]), bits(&l[100 + SETTLED..295]), "pad {p}");
        assert_eq!(bits(&or[k..200]), bits(&r[100 + SETTLED..295]), "pad {p}");
    }
}

#[test]
fn only_pad_zones_sound_and_other_keys_are_ignored() {
    let b = buffer(vec![vec![0.5; 2000]]);
    let mut range = zone(0, 2000, 37);
    range.key_hi = 38; // not exactly one key: not a pad zone
    let m = map(
        vec![b],
        vec![
            zone(0, 2000, 60),
            range,
            zone(0, 2000, 35),
            zone(0, 2000, 52),
        ],
    );
    let (mut h, probe) = with_map(&m);
    for key in [60, 37, 38, 35, 52, 36] {
        h.push_note("notes_in", on(0, key, 1.0));
    }
    let (ol, or) = render(&mut h, 4);
    assert!(ol.iter().chain(&or).all(|v| *v == 0.0));
    assert_eq!(probe.active(), 0);
}

#[test]
fn without_a_map_or_a_matching_velocity_a_hit_does_nothing() {
    let (mut h, probe) = drum();
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 2);
    assert!(ol.iter().all(|v| *v == 0.0));
    let mut z = zone(0, 1000, 36);
    z.vel_lo = 100;
    let m = map(vec![buffer(vec![vec![0.5; 1000]])], vec![z]);
    assert!(h.device_mut().swap_samples(Some(m)).is_none());
    hit(&mut h, 0, 1, 0.2);
    render(&mut h, 2);
    assert_eq!(probe.active(), 0);
    hit(&mut h, 0, 1, 0.9);
    render(&mut h, 1);
    assert_eq!(probe.active(), 1);
}

#[test]
fn velocity_layers_pick_the_zone_of_the_velocity() {
    let mut low = zone(0, 4000, 36);
    low.vel_hi = 63;
    let mut high = zone(1, 4000, 36);
    high.vel_lo = 64;
    let m = map(
        vec![
            buffer(vec![vec![0.25; 4000]; 2]),
            buffer(vec![vec![0.5; 4000]; 2]),
        ],
        vec![low, high],
    );
    for (vel, want) in [(0.3, 0.25f32), (0.9, 0.5), (0.5, 0.5), (0.49, 0.25)] {
        let (mut h, probe) = with_map(&m);
        hit(&mut h, 0, 1, vel);
        let (ol, _) = render(&mut h, 3);
        assert_eq!(ol[SETTLED].to_bits(), want.to_bits(), "velocity {vel}");
        assert_eq!(probe.active(), 1);
    }
}

#[test]
fn overlapping_velocity_ranges_layer_and_extra_zones_past_eight_are_ignored() {
    let zones: Vec<Zone> = (0..10).map(|_| zone(0, 4000, 36)).collect();
    let m = map(vec![buffer(vec![vec![0.0625; 4000]; 2])], zones);
    let (mut h, probe) = with_map(&m);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 3);
    assert_eq!(probe.active(), MAX_ZONES_PER_PAD);
    assert_eq!(ol[SETTLED], 0.0625 * MAX_ZONES_PER_PAD as f32);
}

#[test]
fn tune_of_twelve_semitones_reads_every_second_frame() {
    let m = map(vec![buffer(vec![ramp(4000)])], vec![zone(0, 4000, 36)]);
    let want = ramp(4000);
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.tune", 12.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, or) = render(&mut h, 4);
    for n in SETTLED..200 {
        assert_eq!(
            ol[n].to_bits(),
            (want[2 * n] * FRAC_1_SQRT_2).to_bits(),
            "{n}"
        );
        assert_eq!(ol[n].to_bits(), or[n].to_bits());
    }
    // Zone transpose, root offset and fine cents add to the same exponent.
    let mut z = zone(0, 4000, 36);
    z.root_key = 24; // 12 semitones up from the root
    let m = map(vec![buffer(vec![ramp(4000)])], vec![z]);
    let (mut h, _) = with_map(&m);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 4);
    for n in SETTLED..200 {
        assert_eq!(ol[n].to_bits(), (want[2 * n] * FRAC_1_SQRT_2).to_bits());
    }
    let m = map(vec![buffer(vec![ramp(4000)])], vec![zone(0, 4000, 36)]);
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.tune", 13.0);
    h.set_param("pad1.fine", -100.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 4);
    for n in SETTLED..200 {
        assert_eq!(ol[n].to_bits(), (want[2 * n] * FRAC_1_SQRT_2).to_bits());
    }
}

#[test]
fn tune_sets_the_equal_tempered_pitch() {
    let data: Vec<f32> = (0..8000)
        .map(|i| 0.5 * (std::f64::consts::TAU * i as f64 / 40.0).sin() as f32)
        .collect();
    let m = map(vec![buffer(vec![data])], vec![zone(0, 8000, 36)]);
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.tune", 7.0);
    h.set_param("pad1.fine", 50.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 60);
    let want = SR / 40.0 * 2f64.powf(7.5 / 12.0);
    let f = dominant_frequency(&ol[100..3500], SR);
    assert!((f - want).abs() < 0.01 * want, "{f} Hz, wanted {want}");
}

#[test]
fn level_and_velocity_scale_the_gain() {
    let m = flat_kit(&[1], 0.5, 6000);
    // Level in dB.
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.level", -6.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 3);
    let want = 0.5 * db_to_gain(-6.0);
    assert!((ol[SETTLED] - want).abs() < 1e-6, "{}", ol[SETTLED]);
    // -60 dB is silence.
    h.device_mut().reset();
    h.set_param("pad1.level", -60.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 3);
    assert!(ol.iter().all(|v| *v == 0.0));
    // Velocity amount: level x (1 - amount x (1 - v)).
    for (amount, vel, gain) in [
        (0.7, 0.5, 0.65),
        (1.0, 0.5, 0.5),
        (0.0, 0.2, 1.0),
        (1.0, 1.0, 1.0),
    ] {
        let (mut h, _) = with_map(&m);
        h.set_param("pad1.velocity", amount);
        hit(&mut h, 0, 1, vel);
        let (ol, _) = render(&mut h, 3);
        let want = 0.5 * gain;
        assert!(
            (ol[SETTLED] - want).abs() < 1e-6,
            "{amount}/{vel}: {}",
            ol[SETTLED]
        );
    }
}

#[test]
fn pan_is_constant_power_for_mono_and_balance_for_stereo() {
    let mono = map(vec![buffer(vec![vec![0.5; 6000]])], vec![zone(0, 6000, 36)]);
    for (pan, l, r) in [
        (0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2),
        (-1.0, 1.0, 0.0),
        (1.0, 0.0, 1.0),
    ] {
        let (mut h, _) = with_map(&mono);
        h.set_param("pad1.pan", pan);
        hit(&mut h, 0, 1, 1.0);
        let (ol, or) = render(&mut h, 3);
        assert!((ol[SETTLED] - 0.5 * l).abs() < 1e-6, "mono {pan}");
        assert!((or[SETTLED] - 0.5 * r).abs() < 1e-6, "mono {pan}");
    }
    for pan in [-0.5f32, 0.3, 1.0] {
        let (mut h, _) = with_map(&mono);
        h.set_param("pad1.pan", pan);
        hit(&mut h, 0, 1, 1.0);
        let (ol, or) = render(&mut h, 3);
        // Constant power: the squared gains sum to the centre value's.
        let p = (ol[SETTLED] * ol[SETTLED] + or[SETTLED] * or[SETTLED]) / 0.25;
        assert!((p - 1.0).abs() < 1e-5, "{pan}: {p}");
    }
    let stereo = flat_kit(&[1], 0.5, 6000);
    let (mut h, _) = with_map(&stereo);
    h.set_param("pad1.pan", 1.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, or) = render(&mut h, 3);
    assert_eq!(ol[SETTLED], 0.0);
    assert_eq!(or[SETTLED], 0.5);
}

#[test]
fn attack_is_a_half_millisecond_ramp() {
    let m = flat_kit(&[1], 1.0, 6000);
    let (mut h, _) = with_map(&m);
    hit(&mut h, 3, 1, 1.0);
    let (ol, _) = render(&mut h, 2);
    for k in 0..24 {
        let want = (k + 1) as f32 / 24.0;
        assert!((ol[3 + k] - want).abs() < 1e-6, "{k}: {}", ol[3 + k]);
    }
    assert_eq!(ol[3 + 24], 1.0);
}

#[test]
fn one_shot_decays_exponentially_to_minus_60_db_and_ignores_note_off() {
    let m = map(
        vec![buffer(vec![vec![1.0; 20_000]])],
        vec![zone(0, 20_000, 36)],
    );
    let (mut h, probe) = with_map(&m);
    h.set_param("pad1.mode", 1.0);
    h.set_param("pad1.decay", 100.0);
    hit(&mut h, 0, 1, 1.0);
    let (mut ol, _) = render(&mut h, 1);
    h.push_note("notes_in", off(0, 36));
    let (rest, _) = render(&mut h, 120);
    ol.extend(rest);
    let n = 4800; // 100 ms
    for (i, frac) in [(n / 2, 0.5f32), (n, 1.0)] {
        let want = FRAC_1_SQRT_2 * 10f32.powf(-3.0 * frac);
        assert!(
            (ol[i] - want).abs() < 0.03 * want,
            "{i}: {} vs {want}",
            ol[i]
        );
    }
    assert!(ol[n / 4] > ol[n / 2] && ol[n / 2] > ol[n]);
    assert_eq!(probe.active(), 0, "idle after -80 dB");
}

#[test]
fn gate_mode_holds_while_the_key_is_down_then_decays() {
    let m = map(
        vec![buffer(vec![vec![1.0; 20_000]])],
        vec![zone(0, 20_000, 36)],
    );
    let (mut h, probe) = with_map(&m);
    h.set_param("pad1.mode", 0.0);
    h.set_param("pad1.decay", 100.0);
    hit(&mut h, 0, 1, 1.0);
    let (held, _) = render(&mut h, 30);
    assert!(held[SETTLED..].iter().all(|v| *v == FRAC_1_SQRT_2));
    h.push_note("notes_in", off(0, 36));
    let (rel, _) = render(&mut h, 120);
    assert_eq!(rel[0], FRAC_1_SQRT_2, "the decay starts at the note-off");
    let want = FRAC_1_SQRT_2 * 10f32.powf(-3.0 * 0.5);
    assert!((rel[2400] - want).abs() < 0.03 * want);
    assert_eq!(probe.active(), 0);
}

#[test]
fn gate_mode_loops_while_held_and_one_shot_loops_until_the_decay_ends() {
    let mut z = zone(0, 1000, 36);
    z.loop_mode = LoopMode::Forward;
    z.loop_start = 100;
    z.loop_end = 900;
    let m = map(vec![buffer(vec![vec![0.5; 1000]])], vec![z]);
    let (mut h, probe) = with_map(&m);
    h.set_param("pad1.decay", 50.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 200); // 12 800 samples, far past the 1000 frames
    assert_eq!(ol[12_000], 0.5 * FRAC_1_SQRT_2);
    h.push_note("notes_in", off(0, 36));
    render(&mut h, 200); // 500 ms decay, then idle
    assert_eq!(probe.active(), 0);
    h.set_param("pad1.mode", 1.0);
    h.set_param("pad1.decay", 50.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 60);
    assert!(ol[100] > 0.0);
    assert_eq!(probe.active(), 0);
}

#[test]
fn the_end_of_the_sample_fades_to_silence_and_frees_the_voice() {
    let m = flat_kit(&[1], 1.0, 200);
    let (mut h, probe) = with_map(&m);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 4);
    // 64 frames before the end the gain falls linearly (end fade of the sampler).
    assert_eq!(ol[135], 1.0);
    assert!((ol[199] - 1.0 / 64.0).abs() < 1e-6, "{}", ol[199]);
    assert!(ol[200..].iter().all(|v| *v == 0.0));
    assert_eq!(probe.active(), 0);
}

#[test]
fn all_off_fades_every_voice_within_two_milliseconds() {
    let m = flat_kit(&[1, 2], 1.0, 20_000);
    let (mut h, probe) = with_map(&m);
    hit(&mut h, 0, 1, 1.0);
    hit(&mut h, 0, 2, 1.0);
    render(&mut h, 4);
    h.push_note("notes_in", all_off(0));
    let (ol, _) = render(&mut h, 4);
    assert_eq!(ol[0], 2.0);
    assert!((ol[FADE / 2] - 2.0 * 0.5).abs() < 0.05);
    assert!(ol[FADE..].iter().all(|v| *v == 0.0));
    assert_eq!(probe.active(), 0);
}

// ---- choke and retrigger ---------------------------------------------------------

/// Four connected pads on a long flat kit: pads 1 and 2 in choke group 1, pad 3 in group 2,
/// pad 4 and 5 in group 0.
fn choke_rig() -> (DeviceHarness, Arc<Probe>) {
    let m = flat_kit(&[1, 2, 3, 4, 5], 1.0, 40_000);
    let (mut h, probe) = with_map(&m);
    for (p, g) in [(1, 1.0), (2, 1.0), (3, 2.0)] {
        h.set_param(&format!("pad{p}.choke"), g);
    }
    for p in 1..=5 {
        connect_pad(&mut h, p);
    }
    (h, probe)
}

fn pad_l(h: &mut DeviceHarness, batches: usize, pads: &[usize]) -> Vec<Vec<f32>> {
    let keys: Vec<String> = pads.iter().map(|p| format!("pad{p}_out_l")).collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    h.render(batches, &refs)
}

#[test]
fn a_hit_chokes_its_group_within_two_milliseconds_and_leaves_other_groups_alone() {
    let (mut h, probe) = choke_rig();
    for p in [1, 3, 4] {
        hit(&mut h, 0, p, 1.0);
    }
    pad_l(&mut h, 10, &[1]);
    assert_eq!(probe.active(), 3);
    hit(&mut h, 0, 2, 1.0);
    let o = pad_l(&mut h, 4, &[1, 2, 3, 4]);
    for (n, v) in o[0].iter().enumerate().take(FADE) {
        let want = (FADE - n) as f32 / FADE as f32;
        assert!((v - want).abs() < 1e-5, "choke fade {n}: {v} vs {want}");
    }
    assert!(
        o[0][FADE..].iter().all(|v| *v == 0.0),
        "pad 1 silent at 2 ms"
    );
    // Pad 2 attacks to full level.
    assert_eq!(o[1][SETTLED], 1.0);
    // Pad 3 (group 2) and pad 4 (group 0) are untouched.
    assert!(o[2].iter().all(|v| *v == 1.0));
    assert!(o[3].iter().all(|v| *v == 1.0));
    assert_eq!(probe.keys(), vec![key_of(2), key_of(3), key_of(4)]);
}

#[test]
fn group_zero_neither_chokes_nor_is_choked() {
    let (mut h, probe) = choke_rig();
    hit(&mut h, 0, 4, 1.0);
    hit(&mut h, 0, 5, 1.0);
    hit(&mut h, 0, 3, 1.0);
    pad_l(&mut h, 8, &[1]);
    hit(&mut h, 0, 5, 1.0); // pad 5 (group 0) retriggers itself only
    let o = pad_l(&mut h, 4, &[3, 4]);
    assert!(o[0].iter().all(|v| *v == 1.0));
    assert!(o[1].iter().all(|v| *v == 1.0));
    assert_eq!(probe.active(), 3, "pad 5's old voice has faded out");
}

#[test]
fn a_retrigger_fades_the_previous_hit_of_the_same_pad() {
    let (mut h, probe) = choke_rig();
    hit(&mut h, 0, 4, 1.0);
    pad_l(&mut h, 10, &[4]);
    hit(&mut h, 0, 4, 1.0);
    let o = pad_l(&mut h, 4, &[4]);
    for (n, v) in o[0].iter().enumerate().take(FADE) {
        let old = (FADE - n) as f32 / FADE as f32;
        let new = ((n + 1) as f32 / 24.0).min(1.0);
        assert!((v - (old + new)).abs() < 1e-5, "retrigger {n}: {v}");
    }
    assert!(o[0][FADE..].iter().all(|v| *v == 1.0));
    assert_eq!(probe.active(), 1);
}

#[test]
fn choke_cancels_a_hit_that_is_still_waiting_for_a_stolen_voice() {
    // Fill all 32 voices with 4 pads x 8 layers, then hit pad 1 (group 1) and pad 2 (group 1).
    let zones: Vec<Zone> = (1..=4)
        .flat_map(|p| (0..8).map(move |_| zone(0, 40_000, key_of(p))))
        .collect();
    let m = map(vec![buffer(vec![vec![0.0625; 40_000]; 2])], zones);
    let (mut h, probe) = with_map(&m);
    for p in 1..=4 {
        hit(&mut h, 0, p, 1.0);
    }
    render(&mut h, 4);
    assert_eq!(probe.active(), 32);
    h.set_param("pad1.choke", 1.0);
    h.set_param("pad2.choke", 1.0);
    hit(&mut h, 0, 1, 1.0); // steals 8 voices (pending hits of pad 1)
    hit(&mut h, 10, 2, 1.0); // chokes group 1: pad 1's pending hits are cancelled
    render(&mut h, 8);
    assert!(
        probe.keys().iter().all(|k| *k != key_of(1)),
        "pad 1 must not start after the choke: {:?}",
        probe.keys()
    );
}

// ---- outputs ---------------------------------------------------------------------

fn ramp_kit(pads: &[usize]) -> SampleMapRef {
    let zones = pads
        .iter()
        .enumerate()
        .map(|(i, p)| zone(i as u32, 4000, key_of(*p)))
        .collect();
    let bufs = pads
        .iter()
        .enumerate()
        .map(|(i, _)| {
            buffer(vec![
                ramp(4000)
                    .iter()
                    .map(|v| v * (i + 1) as f32 * 0.25)
                    .collect::<Vec<f32>>();
                2
            ])
        })
        .collect();
    map(bufs, zones)
}

#[test]
fn without_a_cable_every_pad_is_in_the_main_mix_and_individual_outputs_are_silent() {
    let m = ramp_kit(&[1, 2]);
    let (mut h, _) = with_map(&m);
    hit(&mut h, 0, 1, 1.0);
    hit(&mut h, 0, 2, 1.0);
    let o = h.render(
        3,
        &[
            "out_l",
            "pad1_out_l",
            "pad1_out_r",
            "pad2_out_l",
            "pad16_out_r",
        ],
    );
    assert!(o[0][SETTLED] > 0.0);
    for v in &o[1..] {
        assert!(v.iter().all(|x| *x == 0.0));
    }
}

#[test]
fn a_connected_pad_leaves_the_main_mix_and_the_sum_is_sample_exact() {
    let m = ramp_kit(&[1, 2]);
    let (mut reference, _) = with_map(&m);
    hit(&mut reference, 0, 1, 1.0);
    hit(&mut reference, 0, 2, 1.0);
    let (rl, rr) = render(&mut reference, 8);
    for (connect, side) in [(2usize, "l"), (2, "r"), (1, "l"), (1, "r")] {
        let (mut h, _) = with_map(&m);
        h.set_connected(&format!("pad{connect}_out_{side}"), true);
        hit(&mut h, 0, 1, 1.0);
        hit(&mut h, 0, 2, 1.0);
        let keys = [
            "out_l".to_string(),
            "out_r".to_string(),
            format!("pad{connect}_out_l"),
            format!("pad{connect}_out_r"),
        ];
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let o = h.render(8, &refs);
        for n in 0..8 * BATCH {
            // Either side connected routes both channels of the pad.
            assert_eq!(
                (o[0][n] + o[2][n]).to_bits(),
                rl[n].to_bits(),
                "{connect}{side} L {n}"
            );
            assert_eq!(
                (o[1][n] + o[3][n]).to_bits(),
                rr[n].to_bits(),
                "{connect}{side} R {n}"
            );
        }
        assert!(o[2].iter().any(|v| *v != 0.0));
    }
}

#[test]
fn exactly_the_connected_pads_leave_the_main_mix() {
    let m = ramp_kit(&[1, 2, 3, 4]);
    let play = |connected: &[usize]| {
        let (mut h, _) = with_map(&m);
        for p in connected {
            connect_pad(&mut h, *p);
        }
        for p in 1..=4 {
            hit(&mut h, 0, p, 1.0);
        }
        let keys: Vec<String> = std::iter::once("out_l".to_string())
            .chain((1..=4).map(|p| format!("pad{p}_out_l")))
            .collect();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        h.render(8, &refs)
    };
    let reference = play(&[]);
    let routed = play(&[2, 4]);
    let single = |o: &Vec<Vec<f32>>, p: usize| o[p].clone();
    for n in SETTLED..8 * BATCH {
        // Ramp values are multiples of 2^-14, so every partial sum is exact.
        let main_ref = reference[0][n];
        let sum = routed[0][n] + single(&routed, 2)[n] + single(&routed, 4)[n];
        assert_eq!(sum.to_bits(), main_ref.to_bits(), "{n}");
        assert!(single(&routed, 1)[n] == 0.0 && single(&routed, 3)[n] == 0.0);
    }
    assert!(routed[2].iter().any(|v| *v != 0.0));
}

#[test]
fn individual_outputs_are_not_scaled_or_limited_by_the_main_stage() {
    let zones: Vec<Zone> = (0..4).map(|_| zone(0, 6000, 36)).collect();
    let m = map(vec![buffer(vec![vec![1.0; 6000]; 2])], zones);
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.level", 6.0);
    h.set_param("master.level", -60.0);
    connect_pad(&mut h, 1);
    hit(&mut h, 0, 1, 1.0);
    let o = h.render(3, &["out_l", "pad1_out_l", "pad1_out_r"]);
    let want = 4.0 * db_to_gain(6.0);
    assert!((o[1][SETTLED] - want).abs() < 1e-4, "{}", o[1][SETTLED]);
    assert_eq!(o[1][SETTLED], o[2][SETTLED]);
    assert!(o[0].iter().all(|v| *v == 0.0));
    // The same hit in the main mix is scaled by master.level and soft limited below 4.
    let (mut h, _) = with_map(&m);
    h.set_param("pad1.level", 6.0);
    h.set_param("master.level", 6.0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 40);
    assert!(ol[SETTLED] > 2.0 && ol[SETTLED] <= 4.0, "{}", ol[SETTLED]);
}

// ---- stealing, determinism --------------------------------------------------------------

/// Four pads with eight layers each: one hit of every pad takes all 32 voices.
fn thirty_two_voice_map() -> SampleMapRef {
    let bufs: Vec<_> = (0..4)
        .map(|b| buffer(vec![noise(30_000, b), noise(30_000, 9 + b)]))
        .collect();
    let zones = (1..=4)
        .flat_map(|p| (0..8).map(move |_| zone(p as u32 - 1, 30_000, key_of(p))))
        .collect();
    map(bufs, zones)
}

fn busy_run() -> (Vec<u32>, Vec<u32>, Vec<u8>) {
    let m = thirty_two_voice_map();
    let (mut h, probe) = with_map(&m);
    for p in 1..=4 {
        h.set_param(&format!("pad{p}.mode"), 1.0);
        h.set_param(&format!("pad{p}.decay"), 400.0);
        h.set_param(&format!("pad{p}.velocity"), 0.5);
    }
    h.set_param("pad2.choke", 1.0);
    h.set_param("pad3.choke", 1.0);
    connect_pad(&mut h, 4);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for b in 0..120 {
        if b % 3 == 0 {
            for p in 1..=4 {
                hit(&mut h, (b % 7) as u16 * 5, p, 0.3 + 0.1 * (b % 6) as f32);
            }
        }
        let (bl, br) = render(&mut h, 1);
        l.extend(bits(&bl));
        r.extend(bits(&br));
        assert!(probe.active() <= VOICES);
    }
    (l, r, probe.keys())
}

#[test]
fn stealing_with_all_voices_busy_is_deterministic_and_bounded() {
    let a = busy_run();
    let b = busy_run();
    assert_eq!(a, b);
    assert!(a.0.iter().any(|v| f32::from_bits(*v) != 0.0));
    assert!(
        a.0.iter()
            .chain(&a.1)
            .all(|v| f32::from_bits(*v).is_finite())
    );
}

#[test]
fn the_oldest_voice_is_stolen_with_a_two_millisecond_fade() {
    // Pads 2..=5 with eight layers each take all 32 voices; pad 2 is the oldest.
    let mut zones: Vec<Zone> = (2..=5)
        .flat_map(|p| (0..8).map(move |_| zone(0, 40_000, key_of(p))))
        .collect();
    zones.push(zone(1, 40_000, key_of(6)));
    let m = map(
        vec![
            buffer(vec![vec![0.0625; 40_000]; 2]),
            buffer(vec![vec![0.0625; 40_000]; 2]),
        ],
        zones,
    );
    let (mut h, probe) = with_map(&m);
    for p in 2..=5 {
        hit(&mut h, 0, p, 1.0);
        render(&mut h, 1);
    }
    render(&mut h, 4);
    assert_eq!(probe.active(), 32);
    hit(&mut h, 0, 6, 1.0);
    let (ol, _) = render(&mut h, 4);
    for (n, v) in ol.iter().enumerate().take(FADE) {
        let want = 31.0 * 0.0625 + 0.0625 * (FADE - n) as f32 / FADE as f32;
        assert!((v - want).abs() < 1e-5, "steal fade {n}: {v} vs {want}");
    }
    assert!((ol[FADE + 40] - 2.0).abs() < 1e-6, "pad 6 took the voice");
    let keys = probe.keys();
    assert_eq!(keys.iter().filter(|k| **k == key_of(2)).count(), 7);
    assert_eq!(keys.iter().filter(|k| **k == key_of(6)).count(), 1);
    assert_eq!(keys.len(), 32);
}

// ---- swaps (as the sampler) -------------------------------------------------------------

fn kit_with_buffer(b: &Arc<SampleBuffer>, key: u8, gain_db: f32) -> SampleMapRef {
    let mut z = zone(0, b.frames, key);
    z.gain_db = gain_db;
    map(vec![b.clone()], vec![z])
}

#[test]
fn a_swap_that_keeps_the_buffer_migrates_the_voice_without_a_cut() {
    let b = buffer(vec![ramp(6000); 2]);
    let m1 = kit_with_buffer(&b, 36, 0.0);
    let m2 = kit_with_buffer(&b, 36, -6.0);
    let (mut h, probe) = with_map(&m1);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 2);
    let back = h.device_mut().swap_samples(Some(m2.clone()));
    assert!(same(&m1, &back), "nothing drains, the old map comes back");
    assert_eq!(probe.draining(), 0);
    let (nl, _) = render(&mut h, 2);
    let k = 128;
    assert_eq!(ol[127].to_bits(), ramp(6000)[127].to_bits());
    let want = ramp(6000)[k] * db_to_gain(-6.0);
    assert!((nl[0] - want).abs() < 1e-6, "{} vs {want}", nl[0]);
}

#[test]
fn a_zone_index_that_changes_pad_keeps_playing_the_same_buffer() {
    let b = buffer(vec![ramp(6000); 2]);
    let other = buffer(vec![vec![0.5; 6000]; 2]);
    let m1 = kit_with_buffer(&b, 36, 0.0);
    // Zone 0 of the new map is on pad 2 but shares the buffer (root left at pad 1's key).
    let mut z = zone(0, 6000, 37);
    z.root_key = 36;
    let m2 = map(vec![b.clone(), other], vec![z]);
    let (mut h, probe) = with_map(&m1);
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 2);
    h.device_mut().swap_samples(Some(m2));
    assert_eq!(probe.draining(), 0);
    let (nl, nr) = render(&mut h, 2);
    for n in 0..BATCH {
        assert_eq!(nl[n].to_bits(), ramp(6000)[128 + n].to_bits(), "{n}");
        assert_eq!(nl[n].to_bits(), nr[n].to_bits());
    }
    // A hit on pad 1 in the new map has no zone (the pad moved): silent.
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 1);
    assert_eq!(probe.active(), 1);
}

#[test]
fn a_swap_to_other_buffers_drains_for_five_milliseconds() {
    let m1 = flat_kit(&[1], 0.5, 20_000);
    let m2 = flat_kit(&[1], 0.25, 20_000);
    let (mut h, probe) = with_map(&m1);
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 2);
    assert!(h.device_mut().swap_samples(Some(m2.clone())).is_none());
    assert_eq!(probe.draining(), 1);
    let (ol, _) = render(&mut h, 6);
    assert_eq!(ol[0], 0.5 * 240.0 / 240.0);
    assert!((ol[120] - 0.25).abs() < 0.01);
    assert!(ol[240..].iter().all(|v| *v == 0.0));
    assert_eq!(probe.active(), 0);
    // The stale draining map is handed back by the next swap.
    let back = h.device_mut().swap_samples(None);
    assert!(same(&m1, &back));
}

#[test]
fn a_second_swap_cuts_voices_that_still_drain() {
    let m1 = flat_kit(&[1], 0.5, 20_000);
    let m2 = flat_kit(&[1], 0.25, 20_000);
    let m3 = flat_kit(&[1], 0.125, 20_000);
    let (mut h, probe) = with_map(&m1);
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 2);
    assert!(h.device_mut().swap_samples(Some(m2.clone())).is_none());
    render(&mut h, 1);
    let back = h.device_mut().swap_samples(Some(m3));
    assert!(same(&m1, &back));
    assert_eq!(probe.cuts(), 1);
    assert_eq!(probe.draining(), 0);
}

#[test]
fn swapping_in_a_map_without_the_pad_silences_after_the_drain() {
    let m1 = flat_kit(&[1], 0.5, 20_000);
    let (mut h, probe) = with_map(&m1);
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 2);
    h.device_mut().swap_samples(None);
    render(&mut h, 6);
    assert_eq!(probe.active(), 0);
    hit(&mut h, 0, 1, 1.0);
    let (ol, _) = render(&mut h, 2);
    assert!(ol.iter().all(|v| *v == 0.0));
}

#[test]
fn a_hit_waiting_for_a_steal_survives_a_swap_only_if_its_zone_still_matches() {
    let m = thirty_two_voice_map();
    let (mut h, probe) = with_map(&m);
    for p in 1..=4 {
        hit(&mut h, 0, p, 1.0);
    }
    render(&mut h, 2);
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 1);
    // Same map contents again: pending hits keep their zones.
    h.device_mut().swap_samples(Some(thirty_two_voice_map()));
    render(&mut h, 6);
    assert!(probe.active() <= VOICES);
    // A map with no pad zones removes them.
    let empty = map(vec![buffer(vec![vec![0.1; 100]])], vec![zone(0, 100, 60)]);
    h.device_mut().swap_samples(Some(empty));
    render(&mut h, 10);
    assert_eq!(probe.active(), 0);
}

// ---- (device): allocation freedom -------------------------------------------------

#[test]
fn process_swap_and_params_are_allocation_free_with_32_voices_and_outputs_connected() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = DeviceHarness::with_options(Box::new(DrumMachine::new()), sr, 133.0, 9);
        let first = thirty_two_voice_map();
        assert!(assert_no_alloc(|| h.device_mut().swap_samples(Some(first))).is_none());
        for p in 1..=PAD_COUNT {
            connect_pad(&mut h, p);
        }
        for p in 1..=4 {
            h.set_param(&format!("pad{p}.mode"), (p % 2) as f32);
            h.set_param(&format!("pad{p}.choke"), (p % 3) as f32);
        }
        for round in 0..20u16 {
            for p in 1..=4 {
                hit(&mut h, round % 60, p, 0.5 + 0.02 * f32::from(round));
            }
            h.process();
        }
        let next = thirty_two_voice_map();
        let back = assert_no_alloc(|| h.device_mut().swap_samples(Some(next)));
        drop(back);
        for _ in 0..10 {
            h.process();
        }
        assert_no_alloc(|| h.device_mut().reset());
        h.process();
    }
}

#[test]
fn param_sweep_stays_finite_with_a_map_and_held_notes() {
    let m = thirty_two_voice_map();
    let (mut h, _) = with_map(&m);
    for p in 1..=4 {
        hit(&mut h, 0, p, 1.0);
    }
    let d = h.device().descriptor();
    sweep_params_min_max(&mut h, d);
}

// ---- proptest ---------------------------------------------------------------------------

fn arb_zone() -> impl Strategy<Value = Zone> {
    (
        (0u32..3, 30u8..56, 1u8..128, 1u8..128, 20u8..100),
        (-48i8..48, -100.0f32..100.0, -60.0f32..12.0, -1.0f32..1.0),
        (0u64..500, 1000u64..=2000, 0u8..3, 0u64..100, 0u64..300),
    )
        .prop_map(
            |((buf, key, v0, v1, root), (tr, cents, gain, pan), (start, end, lm, ls, le))| {
                let loop_start = start + ls;
                Zone {
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
                    end,
                    loop_mode: match lm {
                        0 => LoopMode::Off,
                        1 => LoopMode::Forward,
                        _ => LoopMode::PingPong,
                    },
                    loop_start,
                    loop_end: loop_start + 1 + le,
                    crossfade: 0,
                    trigger: TriggerMode::Gate,
                    reverse: false,
                }
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
    fn random_maps_notes_and_params_stay_finite_and_in_bounds(
        zones in prop::collection::vec(arb_zone(), 0..20),
        zones2 in prop::collection::vec(arb_zone(), 0..20),
        events in prop::collection::vec((0usize..30, 0u16..64, 30u8..56, 0.0f32..1.2, 0u8..4), 0..60),
        params in prop::collection::vec((0usize..129, 0.0f32..1.0), 0..40),
        mask in any::<u16>(),
    ) {
        let (mut h, probe) = drum();
        let _ = h.device_mut().swap_samples(Some(random_map(zones)));
        for p in 1..=PAD_COUNT {
            if (mask >> (p - 1)) & 1 == 1 {
                connect_pad(&mut h, p);
            }
        }
        for (i, f) in &params {
            let d = &PARAMS[*i];
            h.set_param(d.key, d.min + (d.max - d.min) * f);
        }
        let outs: Vec<&str> = DESCRIPTOR.sockets.iter().skip(1).map(|s| s.key).collect();
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
            for k in &outs {
                let o = h.output(k);
                prop_assert!(o.iter().all(|v| v.is_finite() && v.abs() <= 1000.0), "{k}");
            }
            for k in ["out_l", "out_r"] {
                prop_assert!(h.output(k).iter().all(|v| v.abs() <= 4.0));
            }
            prop_assert!(probe.active() <= VOICES);
        }
    }
}

// ---- reverse through the shared playhead ----------------------------------------

#[test]
fn a_reversed_pad_zone_plays_backwards_bit_exactly_after_the_attack() {
    let (l, r) = (noise(3000, 1), noise(3000, 2));
    let mut z = zone(0, 3000, key_of(1));
    z.start = 100;
    z.end = 2900;
    z.reverse = true;
    let m = map(vec![buffer(vec![l.clone(), r.clone()])], vec![z]);
    let (mut h, probe) = with_map(&m);
    hit(&mut h, 5, 1, 1.0);
    let (ol, or) = render(&mut h, 4);
    assert_eq!(probe.active(), 1);
    for n in 5 + SETTLED..200 {
        let t = n - 5;
        assert_eq!(ol[n].to_bits(), l[2899 - t].to_bits(), "left {n}");
        assert_eq!(or[n].to_bits(), r[2899 - t].to_bits(), "right {n}");
    }
}

#[test]
fn a_reversed_pad_zone_ends_with_a_fade_at_its_start() {
    let mut z = zone(0, 1000, key_of(2));
    z.start = 200;
    z.end = 400;
    z.reverse = true;
    let m = map(vec![buffer(vec![vec![0.5; 1000]; 2])], vec![z]);
    let (mut h, probe) = with_map(&m);
    hit(&mut h, 0, 2, 1.0);
    let (ol, _) = render(&mut h, 4);
    assert_eq!(probe.active(), 0);
    assert!((ol[199] - 0.5 / 64.0).abs() < 1e-6, "{}", ol[199]);
    assert!(ol[100..120].iter().all(|v| (*v - 0.5).abs() < 1e-6));
}
