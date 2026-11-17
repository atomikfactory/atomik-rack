//! Tests of Plucked Strings: the descriptor, the pitch over the keyboard (read by the tuner's
//! detector and by a spectral fit), decay, release and palm mute, the expression lanes (bends,
//! palm mute, harmonics, dead notes), voices and stealing, the controllers, and the real-time
//! and determinism contracts.

use super::*;
use crate::test_support::{
    SR, assert_bend_range, assert_keys, assert_sustain_pedal, assert_volume_and_expression, on,
    sweep_params_min_max,
};
use atomik_core::ExprKind;
use atomik_dsp::tuner;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{db, max_step, peak, rms};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const OUT: [&str; 2] = ["out_l", "out_r"];

/// Test-only window into the voice allocator, updated at the end of every `process`.
#[derive(Debug, Default)]
pub struct Probe {
    /// Per voice: 0 when free, else 1 + the key it answers to.
    pub keys: [AtomicU32; MAX_VOICES],
}

impl Probe {
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

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn key_hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

/// A fresh device with every param at its default.
fn fresh_at(sr: f64) -> DeviceHarness {
    DeviceHarness::new(Box::new(PluckedStrings::new()), sr)
}

fn fresh() -> DeviceHarness {
    fresh_at(SR)
}

/// A device for the controller contracts: a long, steady ring that releases quickly.
fn steady() -> DeviceHarness {
    let mut h = fresh();
    h.set_param("decay", 20_000.0);
    h.set_param("sustain", 0.0);
    h
}

fn play(key: u8, velocity: f32) -> NoteKind {
    NoteKind::On { key, velocity }
}

fn expr(key: u8, kind: ExprKind, value: f32) -> NoteKind {
    NoteKind::NoteExpr { key, kind, value }
}

/// Renders `frames` frames with the events `(frame, kind)` at their own frames.
fn run(mut h: DeviceHarness, events: &[(usize, NoteKind)], frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for b in 0..frames.div_ceil(BATCH) {
        for (at, kind) in events {
            if at / BATCH == b {
                h.push_note(
                    "notes_in",
                    NoteEvent {
                        offset: (at % BATCH) as u16,
                        kind: *kind,
                    },
                );
            }
        }
        let o = h.render(1, &OUT);
        l.extend_from_slice(&o[0]);
        r.extend_from_slice(&o[1]);
    }
    (l, r)
}

/// One note on `key` at full velocity.
fn note(h: DeviceHarness, key: u8, frames: usize) -> Vec<f32> {
    run(h, &[(0, play(key, 1.0))], frames).0
}

/// The strongest component near `guess`, by golden-section search of the Hann-windowed spectrum.
fn fit_hz(x: &[f32], sr: f64, guess: f64) -> f64 {
    let n = x.len();
    let win: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mag = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, s) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * f * i as f64 / sr;
            let v = f64::from(*s) * win[i];
            re += v * a.cos();
            im += v * a.sin();
        }
        re * re + im * im
    };
    let (mut lo, mut hi) = (guess * 0.985, guess * 1.015);
    let phi = 0.618_033_988_749_895;
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (mag(a), mag(b));
    for _ in 0..40 {
        if fa > fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = mag(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = mag(b);
        }
    }
    0.5 * (lo + hi)
}

/// The amplitude of the component at `hz` over the whole slice.
fn amp_at(x: &[f32], hz: f64, sr: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in x.iter().enumerate() {
        let a = std::f64::consts::TAU * hz * i as f64 / sr;
        re += f64::from(*s) * a.cos();
        im += f64::from(*s) * a.sin();
    }
    2.0 * (re * re + im * im).sqrt() / x.len() as f64
}

fn window(x: &[f32], from_s: f64, len_s: f64) -> &[f32] {
    let a = (from_s * SR) as usize;
    &x[a..a + (len_s * SR) as usize]
}

#[test]
fn the_descriptor_is_pinned() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "body",
            "brightness",
            "pick.position",
            "pick.hardness",
            "decay",
            "sustain",
            "palm_mute",
            "stiffness",
            "course.mode",
            "course.detune",
            "body.mix",
            "out.gain",
            "stereo.width",
            "vel.sens",
            "bend.range",
        ],
        &["notes_in", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.plucked");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    assert_eq!(BODY_NAMES.len(), 11);
}

#[test]
fn every_param_at_its_ends_stays_finite_under_the_allocation_guard() {
    let mut h = fresh();
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 36, 0.4));
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn a_note_sounds_at_a_sensible_level() {
    let x = note(fresh(), 55, seconds(1.0) * BATCH);
    let p = peak(&x);
    assert!((0.2..1.2).contains(&p), "peak {p}");
    assert!(x.iter().all(|s| s.is_finite()));
}

#[test]
fn the_pitch_is_within_a_cent_over_the_keyboard_at_every_sample_rate() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for key in (28..=100).step_by(3) {
            let want = key_hz(f64::from(key));
            let (x, _) = run(fresh_at(sr), &[(0, play(key, 0.9))], (1.3 * sr) as usize);
            let n = 16_384;
            let from = (0.2 * sr) as usize;
            let got = fit_hz(&x[from..from + n], sr, want);
            let off = cents(got, want).abs();
            assert!(off < 1.0, "key {key} at {sr}: {off:.3} cents (fit)");
            if want < 1_500.0 {
                let frames = tuner::frames_wanted(sr as f32);
                let at = (0.12 * sr) as usize;
                let w = &x[at..at + frames];
                let p = tuner::detect(w, sr as f32).unwrap_or_else(|| panic!("key {key} at {sr}"));
                let off = cents(f64::from(p.hz), want).abs();
                // The detector reads a period to a fraction of a sample, so its own resolution is about
                // a cent in the bass and a few cents above 1 kHz; the fit above is the exact check.
                let limit = if want < 600.0 { 1.0 } else { 3.0 };
                assert!(off < limit, "key {key} at {sr}: {off:.3} cents (tuner)");
            }
        }
    }
}

#[test]
fn the_master_tune_and_the_pitch_bend_move_the_pitch() {
    let want = key_hz(57.0);
    let mut h = fresh();
    h.time_mut().tune_cents = 40.0;
    let x = note(h, 57, 48_000);
    let tuned = want * (40.0f64 / 1200.0).exp2();
    let got = fit_hz(&x[8_000..8_000 + 16_384], SR, tuned);
    assert!(
        (cents(got, want) - 40.0).abs() < 1.0,
        "{}",
        cents(got, want)
    );

    let bend = NoteKind::PitchBend { value: 0.5 };
    let (x, _) = run(fresh(), &[(0, bend), (4, play(57, 1.0))], 48_000);
    let got = fit_hz(&x[8_000..8_000 + 16_384], SR, want * 2f64.powf(1.0 / 12.0));
    assert!(
        (cents(got, want) - 100.0).abs() < 1.0,
        "{}",
        cents(got, want)
    );
}

#[test]
fn the_decay_param_sets_the_fall_of_the_fundamental() {
    for decay_ms in [1_000.0f32, 3_000.0] {
        let mut h = fresh();
        h.set_param("decay", decay_ms);
        h.set_param("brightness", 0.9);
        h.set_param("body.mix", 0.0);
        h.set_param("stiffness", 0.0);
        let key = 52u8;
        // The decay shortens for higher keys by (220 Hz / f)^0.35.
        let hz = key_hz(f64::from(key));
        let t60 = f64::from(decay_ms) * 0.001 * (220.0 / hz).powf(0.35);
        let x = note(h, key, (4.5 * SR) as usize);
        let (t0, t1) = (0.3, 0.3 + t60 * 0.5);
        let amp = |t: f64| amp_at(window(&x, t, 0.2), hz, SR);
        let fall = f64::from(db((amp(t1) / amp(t0)) as f32)).abs();
        let want = 60.0 * (t1 - t0) / t60;
        assert!(
            (fall - want).abs() < want * 0.25,
            "decay {decay_ms}: fell {fall:.1} dB, wanted {want:.1}"
        );
    }
}

#[test]
fn a_released_string_is_damped_and_sustain_lets_it_ring() {
    let ring = |sustain: f32| -> f32 {
        let mut h = fresh();
        h.set_param("sustain", sustain);
        let off = NoteKind::Off { key: 52 };
        let (x, _) = run(h, &[(0, play(52, 1.0)), (24_000, off)], 96_000);
        // The level a second after the release, against the level at the release.
        db(rms(window(&x, 1.5, 0.1)) / rms(window(&x, 0.3, 0.1)))
    };
    let (muted, free, half) = (ring(0.0), ring(1.0), ring(0.5));
    assert!(muted < -60.0, "finger mute: {muted} dB");
    assert!(free > -45.0, "let ring: {free} dB");
    assert!(half > muted && half < free, "in between: {half}");
}

#[test]
fn palm_mute_shortens_the_decay_by_the_param_and_by_the_slide_expression() {
    let fall = |h: DeviceHarness, events: &[(usize, NoteKind)]| -> f32 {
        let (x, _) = run(h, events, 3 * 48_000);
        db(rms(window(&x, 1.0, 0.2)) / rms(window(&x, 0.1, 0.2)))
    };
    let open = fall(fresh(), &[(0, play(45, 1.0))]);
    let mut h = fresh();
    h.set_param("palm_mute", 0.8);
    let param = fall(h, &[(0, play(45, 1.0))]);
    let lane = fall(
        fresh(),
        &[(0, play(45, 1.0)), (0, expr(45, ExprKind::Slide, 0.8))],
    );
    assert!(param < open - 15.0, "param: {param} dB against {open}");
    assert!(lane < open - 15.0, "slide: {lane} dB against {open}");
    // The same amount from either source sounds the same.
    assert!((param - lane).abs() < 1.0, "{param} {lane}");
}

#[test]
fn a_pitch_expression_bends_the_note_without_a_click() {
    let key = 55u8;
    let base = key_hz(f64::from(key));
    // A one-semitone bend up in steps every 32 frames over 0.1 s, then held.
    let mut events = vec![(0usize, play(key, 1.0))];
    let start = 12_000usize;
    for i in 0..=150usize {
        let v = (i as f32 / 150.0).min(1.0);
        events.push((start + i * 32, expr(key, ExprKind::Tuning, v)));
    }
    let (x, _) = run(fresh(), &events, 48_000);
    let top = base * 2f64.powf(1.0 / 12.0);
    let after = fit_hz(&x[30_000..30_000 + 16_384], SR, top);
    assert!(
        (cents(after, base) - 100.0).abs() < 1.0,
        "{} cents",
        cents(after, base)
    );
    let (plain, _) = run(fresh(), &[(0, play(key, 1.0))], 48_000);
    let during = max_step(&x[start..start + 4_800]);
    let reference = max_step(&plain[start..start + 4_800]);
    assert!(
        during < reference * 1.5,
        "bend step {during}, plain {reference}"
    );
    // A single step of two semitones is smoothed too.
    let jump = [
        (0usize, play(key, 1.0)),
        (20_000, expr(key, ExprKind::Tuning, 2.0)),
    ];
    let (y, _) = run(fresh(), &jump, 48_000);
    let step = max_step(&y[19_900..21_000]);
    assert!(step < max_step(&plain[19_900..21_000]) * 1.5, "{step}");
    let late = fit_hz(
        &y[30_000..30_000 + 16_384],
        SR,
        base * 2f64.powf(2.0 / 12.0),
    );
    assert!((cents(late, base) - 200.0).abs() < 1.0);
}

#[test]
fn a_pre_bend_in_the_note_expression_starts_on_pitch() {
    let key = 55u8;
    let want = key_hz(f64::from(key) + 2.0);
    let events = [
        (0usize, play(key, 1.0)),
        (0, expr(key, ExprKind::Tuning, 2.0)),
    ];
    let (x, _) = run(fresh(), &events, 24_000);
    // From the first moments the note is two semitones up: no glide from the key.
    let got = fit_hz(&x[2_000..2_000 + 8_192], SR, want);
    assert!(cents(got, want).abs() < 3.0, "{}", cents(got, want));
}

#[test]
fn a_harmonic_rings_pure_at_the_pitch_of_its_tuning() {
    // A natural harmonic at the twelfth fret: the string's key, an octave up.
    let key = 45u8;
    let normal = key + 12;
    let hz = key_hz(f64::from(normal));
    let harmonic = [
        (0usize, play(key, 1.0)),
        (0, expr(key, ExprKind::Tuning, 12.0)),
        (0, expr(key, ExprKind::Pressure, EXPR_HARMONIC)),
    ];
    let (h, _) = run(fresh(), &harmonic, 48_000);
    let (n, _) = run(fresh(), &[(0, play(normal, 1.0))], 48_000);
    let f = fit_hz(&h[8_000..8_000 + 16_384], SR, hz);
    assert!(cents(f, hz).abs() < 1.0, "{}", cents(f, hz));
    // Pure: its third partial is much weaker, against its fundamental, than the plucked note's.
    let ratio = |x: &[f32]| {
        let w = &x[8_000..8_000 + 16_384];
        db((amp_at(w, 3.0 * f, SR) / amp_at(w, f, SR)) as f32)
    };
    assert!(
        ratio(&h) < ratio(&n) - 8.0,
        "{} against {}",
        ratio(&h),
        ratio(&n)
    );
}

#[test]
fn a_dead_note_is_a_short_unpitched_click() {
    let key = 52u8;
    let dead = [
        (0usize, play(key, 1.0)),
        (0, expr(key, ExprKind::Pressure, EXPR_DEAD)),
    ];
    let (x, _) = run(fresh(), &dead, 48_000);
    let early = rms(&x[0..2_400]);
    let late = rms(&x[9_600..12_000]);
    assert!(early > 0.01, "audible: {early}");
    assert!(
        db(late / early) < -40.0,
        "dies within 0.2 s: {} dB",
        db(late / early)
    );
    // The same note played normally rings on.
    let (open, _) = run(fresh(), &[(0, play(key, 1.0))], 48_000);
    assert!(rms(&open[9_600..12_000]) > 10.0 * late);
}

#[test]
fn velocity_sets_the_level_and_the_sensitivity_removes_it() {
    let level = |vel: f32, sens: f32| -> f32 {
        let mut h = fresh();
        h.set_param("vel.sens", sens);
        let (x, _) = run(h, &[(0, play(60, vel))], 24_000);
        rms(&x[1_000..9_000])
    };
    assert!(level(1.0, 0.7) > 1.5 * level(0.3, 0.7));
    let (a, b) = (level(1.0, 0.0), level(0.2, 0.0));
    assert_eq!(a.to_bits(), b.to_bits(), "no sensitivity, no difference");
}

#[test]
fn the_pick_hardness_and_the_brightness_open_the_sound() {
    let high = |param: &str, v: f32| -> f32 {
        let mut h = fresh();
        h.set_param(param, v);
        let x = note(h, 52, 24_000);
        // A crude high-pass, the second difference.
        let d: Vec<f32> = x.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
        rms(&d[2_000..12_000]) / rms(&x[2_000..12_000])
    };
    assert!(high("pick.hardness", 1.0) > 1.5 * high("pick.hardness", 0.0));
    assert!(high("brightness", 1.0) > 1.5 * high("brightness", 0.0));
}

#[test]
fn the_bodies_colour_the_sound_and_none_is_the_bare_string() {
    let render = |body: f32, mix: f32| -> Vec<f32> {
        let mut h = fresh();
        h.set_param("body", body);
        h.set_param("body.mix", mix);
        note(h, 50, 12_000)
    };
    let dry = render(0.0, 1.0);
    for (b, name) in BODY_NAMES.iter().enumerate().skip(1) {
        assert_ne!(render(b as f32, 1.0), dry, "{name}");
        assert_eq!(render(b as f32, 0.0), dry, "{name} at no mix");
    }
}

#[test]
fn a_course_adds_a_second_string() {
    let level_at = |course: f32, hz: f64| -> f64 {
        let mut h = fresh();
        h.set_param("course.mode", course);
        h.set_param("body.mix", 0.0);
        let x = note(h, 48, 36_000);
        amp_at(&x[6_000..6_000 + 16_384], hz, SR)
    };
    let f2 = key_hz(60.0);
    // The octave string brings energy an octave up.
    assert!(level_at(2.0, f2) > 1.15 * level_at(0.0, f2));
    // A unison pair beats: its envelope moves where a single string's is flat.
    let beat = |course: f32| -> f32 {
        let mut h = fresh();
        h.set_param("course.mode", course);
        h.set_param("course.detune", 40.0);
        h.set_param("decay", 20_000.0);
        h.set_param("body.mix", 0.0);
        let x = note(h, 48, 72_000);
        // The level of the fundamental in windows of 80 ms.
        let hz = key_hz(48.0);
        let r: Vec<f64> = (0..16)
            .map(|i| amp_at(&x[4_000 + i * 3_840..4_000 + (i + 1) * 3_840], hz, SR))
            .collect();
        let hi = r.iter().fold(0.0f64, |a, v| a.max(*v));
        let lo = r.iter().fold(f64::MAX, |a, v| a.min(*v));
        (hi / lo) as f32
    };
    assert!(beat(1.0) > 2.0 * beat(0.0), "{} {}", beat(1.0), beat(0.0));
}

#[test]
fn the_stereo_width_spreads_the_keys() {
    let side = |width: f32, key: u8| -> f32 {
        let mut h = fresh();
        h.set_param("stereo.width", width);
        let (l, r) = run(h, &[(0, play(key, 1.0))], 12_000);
        rms(&l[1_000..8_000]) / rms(&r[1_000..8_000])
    };
    let mut h = fresh();
    h.set_param("stereo.width", 0.0);
    let narrow = run(h, &[(0, play(36, 1.0))], 8_000);
    assert_eq!(narrow.0, narrow.1, "no width, one channel");
    assert!(side(1.0, 36) > 1.5, "low keys left: {}", side(1.0, 36));
    assert!(side(1.0, 90) < 0.67, "high keys right: {}", side(1.0, 90));
}

fn probed() -> (DeviceHarness, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let mut d = PluckedStrings::new();
    d.probe = Some(probe.clone());
    (DeviceHarness::new(Box::new(d), SR), probe)
}

#[test]
fn seventeen_notes_steal_the_oldest_and_a_repeated_key_does_not() {
    let (mut h, probe) = probed();
    for (i, k) in (40..56).enumerate() {
        h.push_note("notes_in", on(i as u16, k, 0.8));
    }
    h.render(2, &OUT);
    assert_eq!(probe.keys(), (40..56).collect::<Vec<u8>>());
    // The 17th key takes the oldest voice, which fades out (2 ms) before the new note starts.
    h.push_note("notes_in", on(0, 56, 0.8));
    h.render(1, &OUT);
    assert!(
        probe.keys().contains(&56) && !probe.keys().contains(&40),
        "{:?}",
        probe.keys()
    );
    h.render(4, &OUT);
    assert_eq!(probe.keys(), (41..57).collect::<Vec<u8>>());
    // A struck-again key plucks its own string.
    let (mut h, probe) = probed();
    for i in 0..40 {
        h.push_note("notes_in", on(i as u16, 60, 0.8));
    }
    h.render(2, &OUT);
    assert_eq!(probe.keys(), vec![60]);
}

#[test]
fn a_stolen_voice_fades_without_a_click() {
    // Sixteen long notes, then a seventeenth: the stolen voice's fade must not step.
    let mut events: Vec<(usize, NoteKind)> = (40..56).map(|k| (0usize, play(k, 0.8))).collect();
    events.push((12_000, play(70, 0.8)));
    let mut h = fresh();
    h.set_param("decay", 20_000.0);
    let (x, _) = run(h, &events, 20_000);
    let mut h = fresh();
    h.set_param("decay", 20_000.0);
    let (y, _) = run(h, &events[..16], 20_000);
    let with = max_step(&x[12_000..13_000]);
    let without = max_step(&y[12_000..13_000]);
    // The new note adds its own attack, but no discontinuity as large as the whole chord.
    assert!(with < without * 3.0 + 0.2 * peak(&x), "{with} {without}");
}

#[test]
fn the_sustain_pedal_volume_expression_and_bend_range_work() {
    assert_sustain_pedal(steady, &OUT, 48);
    assert_volume_and_expression(steady, &OUT, 48);
    assert_bend_range(steady, &OUT, 48);
}

#[test]
fn all_off_releases_every_string() {
    let events = [
        (0usize, play(40, 1.0)),
        (0, play(47, 1.0)),
        (12_000, NoteKind::AllOff),
    ];
    let mut h = fresh();
    h.set_param("sustain", 0.0);
    let (x, _) = run(h, &events, 48_000);
    assert!(rms(&x[6_000..9_000]) > 0.01);
    assert!(rms(&x[30_000..]) < 1e-4 * rms(&x[6_000..9_000]));
}

#[test]
fn the_same_script_twice_is_bit_identical_and_the_seed_matters() {
    let events: Vec<(usize, NoteKind)> = (0..12)
        .map(|i| {
            let vel = 0.5 + (i % 5) as f32 * 0.1;
            (i * 3_000, play(40 + (i * 5 % 30) as u8, vel))
        })
        .chain([
            (5_000, expr(45, ExprKind::Tuning, 1.5)),
            (9_000, NoteKind::PitchBend { value: -0.4 }),
        ])
        .collect();
    let a = run(fresh(), &events, 60_000);
    let b = run(fresh(), &events, 60_000);
    assert!(
        a.0.iter()
            .zip(&b.0)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
    assert!(
        a.1.iter()
            .zip(&b.1)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
    let other = DeviceHarness::with_options(Box::new(PluckedStrings::new()), SR, 120.0, 99);
    let c = run(other, &events, 60_000);
    assert_ne!(a.0, c.0, "a different seed plucks differently");
}

#[test]
fn a_released_tail_has_no_denormals_and_ends() {
    let mut h = fresh();
    h.set_param("sustain", 0.0);
    h.set_param("decay", 100.0);
    let off = NoteKind::Off { key: 60 };
    let (l, r) = run(h, &[(0, play(60, 1.0)), (4_800, off)], 10 * 48_000);
    for x in [&l, &r] {
        assert!(x.iter().all(|s| !s.is_subnormal() && s.is_finite()));
        assert!(x[8 * 48_000..].iter().all(|s| s.abs() < 1e-6));
    }
}

#[test]
fn a_storm_of_notes_and_extreme_params_stays_bounded_without_allocating() {
    for sr in [44_100.0, 96_000.0] {
        let mut h = fresh_at(sr);
        for p in DESCRIPTOR.params {
            h.set_param(p.key, p.max);
        }
        let mut rng = atomik_dsp::Rng::new(7);
        let mut all = Vec::new();
        for b in 0..400 {
            let mut batch = Vec::new();
            for _ in 0..6 {
                let key = (rng.next_f32() * 127.0) as u8;
                let offset = (rng.next_f32() * 63.0) as u16;
                let kind = match (rng.next_f32() * 5.0) as u8 {
                    0 => NoteKind::Off { key },
                    1 => expr(key, ExprKind::Tuning, rng.next_bipolar() * 96.0),
                    2 => expr(key, ExprKind::Pressure, rng.next_f32()),
                    3 => NoteKind::PitchBend {
                        value: rng.next_bipolar(),
                    },
                    _ => play(key, rng.next_f32()),
                };
                batch.push(NoteEvent { offset, kind });
            }
            if b % 100 == 0 {
                batch.push(NoteEvent {
                    offset: 0,
                    kind: NoteKind::AllOff,
                });
            }
            batch.sort_by_key(|e| e.offset);
            for e in batch {
                h.push_note("notes_in", e);
            }
            for x in h.render(1, &OUT) {
                all.extend(x);
            }
        }
        assert!(all.iter().all(|s| s.is_finite() && s.abs() < 4.01), "{sr}");
    }
}

#[test]
fn a_note_shifted_by_whole_batches_renders_the_same_samples() {
    // The control step is counted in frames of the device, so the same events a multiple of the
    // step later (four batches is 16 steps) give the same samples, whatever the offsets in the
    // batch.
    let events = [
        (3usize, play(52, 0.9)),
        (200, expr(52, ExprKind::Tuning, 1.0)),
    ];
    let shifted: Vec<(usize, NoteKind)> = events.iter().map(|(a, k)| (a + 4 * BATCH, *k)).collect();
    let a = run(fresh(), &events, 20_000).0;
    let b = run(fresh(), &shifted, 20_000 + 4 * BATCH).0;
    assert!(
        a.iter()
            .zip(&b[4 * BATCH..])
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}
