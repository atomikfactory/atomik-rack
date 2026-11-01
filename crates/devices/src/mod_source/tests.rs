//! Mod Source tests.

use super::*;
use crate::test_support::{SR, assert_keys, harness, off, on, sweep_params_min_max};
use atomik_core::{LoopWrap, Tempo};
use atomik_dsp::sin_cycles;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const OUTS: [&str; 3] = ["lfo1_cv_out", "lfo2_cv_out", "env_cv_out"];

fn h() -> DeviceHarness {
    harness(Box::new(ModSource::new()))
}

fn h_seed(seed: u64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(ModSource::new()), SR, 120.0, seed)
}

fn lfo1(h: &mut DeviceHarness, batches: usize) -> Vec<f32> {
    h.render(batches, &["lfo1_cv_out"]).remove(0)
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Free-running LFO 1 at 40 Hz and 48 kHz: one cycle is exactly 1200 samples.
fn quarter(h: &mut DeviceHarness) {
    h.set_param("lfo1_rate", 40.0);
}

#[test]
fn descriptor_validates() {
    let mut params: Vec<String> = Vec::new();
    for n in ["lfo1", "lfo2"] {
        for k in [
            "shape",
            "morph",
            "rate",
            "sync",
            "div",
            "phase",
            "amount",
            "polarity",
            "key_sync",
            "rate_cv_trim",
        ] {
            params.push(format!("{n}_{k}"));
        }
    }
    for k in ["attack", "decay", "sustain", "release", "amount", "mode"] {
        params.push(format!("env_{k}"));
    }
    for k in [
        "lfo2_to_lfo1_rate",
        "lfo2_to_lfo1_level",
        "lag",
        "key_follow",
    ] {
        params.push(k.to_string());
    }
    let params: Vec<&str> = params.iter().map(String::as_str).collect();
    assert_keys(
        &DESCRIPTOR,
        &params,
        &[
            "notes_in",
            "gate_cv_in",
            "lfo1_rate_cv_in",
            "lfo1_amount_cv_in",
            "lfo2_rate_cv_in",
            "lfo2_amount_cv_in",
            "lfo1_cv_out",
            "lfo2_cv_out",
            "env_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.mod-source");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    for s in DESCRIPTOR.sockets {
        assert!(s.kind == SocketKind::Cv || s.key == "notes_in", "{}", s.key);
    }
    let rate = DESCRIPTOR.param_by_key("lfo1_rate").unwrap();
    assert_eq!((rate.min, rate.max), (0.01, 1_000.0));
    assert_eq!(DIV_LABELS.len(), 30);
    let div = DESCRIPTOR.param_by_key("lfo1_div").unwrap();
    assert_eq!(DIV_LABELS[div.default as usize], "1/4");
}

#[test]
fn set_param_every_param_min_max_does_not_panic() {
    let mut hh = h();
    sweep_params_min_max(&mut hh, &DESCRIPTOR);
}

#[test]
fn reset_restores_the_initial_state() {
    let mut a = h();
    let first = a.render(20, &OUTS);
    a.push_note("notes_in", on(3, 60, 1.0));
    a.render(10, &OUTS);
    assert_no_alloc(|| a.device_mut().reset());
    a.time_mut().frame = 0;
    a.time_mut().tick = 0.0;
    let again = a.render(20, &OUTS);
    assert_eq!(first, again);
}

#[test]
fn process_is_allocation_free() {
    let mut hh = h();
    hh.input_mut("gate_cv_in").fill(1.0);
    hh.input_mut("lfo1_rate_cv_in").fill(0.3);
    hh.input_mut("lfo2_amount_cv_in").fill(0.8);
    hh.set_param("lfo1_shape", 6.0);
    hh.set_param("lfo2_shape", 5.0);
    hh.set_param("lfo1_key_sync", 1.0);
    hh.set_param("lfo1_rate_cv_trim", 0.5);
    for i in 0..40 {
        if i % 7 == 0 {
            hh.push_note("notes_in", on(5, 60, 0.8));
        }
        hh.process();
    }
    let mut out = [0.0f32; METER_COUNT];
    assert_no_alloc(|| hh.device().meter_values(&mut out));
}

#[test]
fn default_lfo_is_a_one_hertz_sine() {
    let mut hh = h();
    let x = lfo1(&mut hh, 750); // exactly one second
    assert!(x[0].abs() < 1e-6);
    assert!(close(x[12_000], 1.0, 1e-4), "{}", x[12_000]);
    assert!(close(x[24_000], 0.0, 1e-4));
    assert!(close(x[36_000], -1.0, 1e-4));
    // LFO 2 defaults to a 0.5 Hz triangle.
    let mut hh = h();
    let y = hh.render(750, &["lfo2_cv_out"]).remove(0);
    assert!(close(y[24_000], 1.0, 1e-4), "{}", y[24_000]);
}

#[test]
fn every_shape_matches_the_dsp_function_at_known_phases() {
    let shapes = [
        ModShape::Sine,
        ModShape::Triangle,
        ModShape::SawUp,
        ModShape::SawDown,
        ModShape::Pulse,
    ];
    for (i, shape) in shapes.into_iter().enumerate() {
        for morph in [-1.0f32, -0.3, 0.0, 0.6, 1.0] {
            let mut hh = h();
            quarter(&mut hh);
            hh.set_param("lfo1_shape", i as f32);
            hh.set_param("lfo1_morph", morph);
            let x = lfo1(&mut hh, 19); // 1216 samples
            for n in [0usize, 150, 300, 450, 600, 899, 1000, 1199] {
                let want = mod_shape(shape, n as f64 / 1200.0, morph, [0.0; 3]);
                assert!(
                    close(x[n], want, 2e-4),
                    "{shape:?} morph {morph} n {n}: {} vs {want}",
                    x[n]
                );
            }
        }
    }
}

#[test]
fn shapes_hit_their_textbook_values() {
    let at = |shape: f32, n: usize| {
        let mut hh = h();
        quarter(&mut hh);
        hh.set_param("lfo1_shape", shape);
        lfo1(&mut hh, 19)[n]
    };
    assert!(close(at(0.0, 300), 1.0, 1e-4));
    assert!(close(at(0.0, 900), -1.0, 1e-4));
    assert!(close(at(1.0, 300), 1.0, 1e-4));
    assert!(close(at(1.0, 150), 0.5, 1e-4));
    assert!(close(at(2.0, 0), -1.0, 1e-4));
    assert!(close(at(2.0, 600), 0.0, 1e-4));
    assert!(close(at(3.0, 0), 1.0, 1e-4));
    assert!(close(at(3.0, 600), 0.0, 1e-4));
    assert!(close(at(4.0, 100), 1.0, 1e-6));
    assert!(close(at(4.0, 700), -1.0, 1e-6));
}

#[test]
fn morph_sets_the_pulse_width_and_skews_the_sine() {
    let duty = |morph: f32| {
        let mut hh = h();
        quarter(&mut hh);
        hh.set_param("lfo1_shape", 4.0);
        hh.set_param("lfo1_morph", morph);
        let x = lfo1(&mut hh, 19);
        x[..1200].iter().filter(|v| **v > 0.0).count() as f32 / 1200.0
    };
    assert!(close(duty(0.0), 0.5, 0.002));
    assert!(close(duty(1.0), 0.95, 0.002));
    assert!(close(duty(-1.0), 0.05, 0.002));
    // Sine skew: the peak moves from phase 0.25 to s / 2 = 0.49.
    let peak = |morph: f32| {
        let mut hh = h();
        quarter(&mut hh);
        hh.set_param("lfo1_morph", morph);
        let x = lfo1(&mut hh, 19);
        x[..1200]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0 as f32
            / 1200.0
    };
    assert!(close(peak(0.0), 0.25, 0.002));
    assert!(close(peak(1.0), 0.49, 0.003));
    assert!(close(peak(-1.0), 0.01, 0.003));
}

#[test]
fn sample_and_hold_steps_once_per_cycle_and_glides_with_morph() {
    let mut hh = h();
    quarter(&mut hh);
    hh.set_param("lfo1_shape", 5.0);
    let x = lfo1(&mut hh, 75); // 4800 samples, 4 cycles
    let levels: Vec<f32> = (0..4).map(|c| x[c * 1200 + 600]).collect();
    for c in 0..4 {
        assert!(
            x[c * 1200 + 5..(c + 1) * 1200 - 5]
                .iter()
                .all(|v| v.to_bits() == levels[c].to_bits()),
            "cycle {c} must hold one value"
        );
    }
    assert!(
        levels.windows(2).all(|w| w[0].to_bits() != w[1].to_bits()),
        "{levels:?}"
    );
    // Glide 50 %: the first half cycle slides from the previous value.
    let mut hh = h();
    quarter(&mut hh);
    hh.set_param("lfo1_shape", 5.0);
    hh.set_param("lfo1_morph", 0.5);
    let g = lfo1(&mut hh, 75);
    assert!(g[1200].to_bits() != g[1500].to_bits() && close(g[1200], levels[0], 0.01));
    assert_eq!(g[1200 + 1199].to_bits(), levels[1].to_bits());
    assert_eq!(g[1200 + 1199].to_bits(), g[1200 + 1190].to_bits());
}

#[test]
fn smooth_random_is_continuous_and_reaches_the_cycle_values() {
    let mut hh = h();
    quarter(&mut hh);
    hh.set_param("lfo1_shape", 6.0);
    let x = lfo1(&mut hh, 75);
    let max_step = x
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.01, "{max_step}");
    assert!(x.iter().any(|v| v.abs() > 0.1));
}

#[test]
fn phase_offset_shifts_the_cycle() {
    let mut hh = h();
    quarter(&mut hh);
    hh.set_param("lfo1_phase", 0.25);
    let x = lfo1(&mut hh, 6);
    assert!(close(x[0], 1.0, 1e-4));
    assert!(close(x[300], 0.0, 1e-4));
}

#[test]
fn polarity_and_amount_scale_the_output() {
    let mut hh = h();
    quarter(&mut hh);
    hh.set_param("lfo1_polarity", 1.0);
    let x = lfo1(&mut hh, 19);
    assert!(x[..1200].iter().all(|v| (0.0..=1.0).contains(v)));
    assert!(close(x[0], 0.5, 1e-4) && close(x[300], 1.0, 1e-4) && close(x[900], 0.0, 1e-4));
    hh.set_param("lfo1_amount", 0.5);
    let mut hh2 = h();
    quarter(&mut hh2);
    hh2.set_param("lfo1_amount", 0.5);
    let y = lfo1(&mut hh2, 19);
    assert!(close(y[300], 0.5, 1e-4) && close(y[900], -0.5, 1e-4));
    hh2.set_param("lfo1_amount", 0.0);
    assert!(lfo1(&mut hh2, 3).iter().all(|v| v.abs() < 1e-9));
}

#[test]
fn amount_cv_multiplies_and_is_clamped() {
    let render = |level: Option<f32>| {
        let mut hh = h();
        quarter(&mut hh);
        if let Some(l) = level {
            hh.input_mut("lfo1_amount_cv_in").fill(l);
        }
        lfo1(&mut hh, 19)[300]
    };
    assert!(close(render(None), 1.0, 1e-4), "unconnected means x1");
    assert!(close(render(Some(0.5)), 0.5, 1e-4));
    assert!(close(render(Some(3.0)), 1.0, 1e-4));
    assert!(close(render(Some(-1.0)), 0.0, 1e-9));
    assert!(
        close(render(Some(0.0)), 0.0, 1e-9),
        "connected 0 means silence"
    );
}

#[test]
fn rate_cv_bends_the_rate_in_octaves() {
    // Sample index of the first upward zero crossing after the start.
    let period = |cv: f32, trim: f32| {
        let mut hh = h();
        hh.set_param("lfo1_rate", 10.0);
        hh.set_param("lfo1_rate_cv_trim", trim);
        hh.input_mut("lfo1_rate_cv_in").fill(cv);
        let x = lfo1(&mut hh, 400);
        let mut crossings = Vec::new();
        for n in 1..x.len() {
            if x[n - 1] < 0.0 && x[n] >= 0.0 {
                crossings.push(n);
            }
        }
        (crossings[1] - crossings[0]) as f32
    };
    let base = period(0.0, 1.0);
    assert!(close(base, 4800.0, 2.0), "{base}");
    assert!(close(period(1.0, 0.25), 2400.0, 2.0), "+1 octave");
    assert!(close(period(-1.0, 0.25), 9600.0, 2.0), "-1 octave");
    assert!(
        close(period(0.5, 0.5), 2400.0, 2.0),
        "trim 0.5 x cv 0.5 x 4 octaves"
    );
    assert!(close(period(5.0, 0.25), 2400.0, 2.0), "cv clamps to 1");
    assert!(close(period(1.0, 0.0), 4800.0, 2.0), "trim 0 is off");
    assert!(
        close(period(1.0, -0.25), 9600.0, 2.0),
        "a negative trim inverts"
    );
    assert!(
        close(period(1.0, 1.0), 300.0, 1.0),
        "full CV and trim: 4 octaves"
    );
    // The default trim is +1: a connected CV acts at once.
    let mut hh = h();
    hh.set_param("lfo1_rate", 10.0);
    hh.input_mut("lfo1_rate_cv_in").fill(0.25);
    let x = lfo1(&mut hh, 400);
    let up: Vec<usize> = (1..x.len())
        .filter(|n| x[n - 1] < 0.0 && x[*n] >= 0.0)
        .collect();
    assert!(close((up[1] - up[0]) as f32, 2400.0, 2.0), "default trim");
    // A non-finite CV reads as 0.
    let mut hh = h();
    hh.set_param("lfo1_rate", 10.0);
    hh.input_mut("lfo1_rate_cv_in").fill(f32::NAN);
    let y = lfo1(&mut hh, 400);
    let up: Vec<usize> = (1..y.len())
        .filter(|n| y[n - 1] < 0.0 && y[*n] >= 0.0)
        .collect();
    assert!(close((up[1] - up[0]) as f32, 4800.0, 2.0), "NaN CV");
}

#[test]
fn unconnected_rate_cv_is_bit_identical_whatever_the_trim() {
    let mut a = h();
    a.set_param("lfo1_shape", 6.0);
    a.set_param("lfo1_rate_cv_trim", -0.7);
    let mut b = h();
    b.set_param("lfo1_shape", 6.0);
    assert_eq!(a.render(40, &OUTS), b.render(40, &OUTS));
}

/// The song tick of frame `n` of a harness started at `tick0` (120 BPM, 48 kHz).
fn tick_at(tick0: f64, n: usize) -> f64 {
    tick0 + n as f64 * (120.0 * PPQ as f64 / (60.0 * SR))
}

#[test]
fn sync_phase_is_locked_to_the_song_tick() {
    // 1/4 note: one cycle is one quarter = PPQ ticks.
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.time_mut().tick = 123_456.0;
    hh.time_mut().frame = 99_999;
    let x = lfo1(&mut hh, 6);
    for n in [0usize, 1, 63, 64, 200, 383] {
        let want = sin_cycles(tick_at(123_456.0, n) / PPQ as f64) as f32;
        assert!(close(x[n], want, 1e-5), "n {n}: {} vs {want}", x[n]);
    }
}

#[test]
fn sync_follows_a_locate() {
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.render(5, &OUTS);
    // Locate to 2.25 quarters (phase 0.25): the very next sample is the peak.
    hh.time_mut().tick = 2.25 * PPQ as f64;
    let x = lfo1(&mut hh, 1);
    assert!(close(x[0], 1.0, 1e-5), "{}", x[0]);
    // Also with the rate CV integrating: it re-aligns at the locate.
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.set_param("lfo1_rate_cv_trim", 1.0);
    hh.input_mut("lfo1_rate_cv_in").fill(0.0);
    hh.render(5, &OUTS);
    hh.time_mut().tick = 2.25 * PPQ as f64;
    let x = lfo1(&mut hh, 1);
    assert!(close(x[0], 1.0, 1e-5), "{}", x[0]);
}

#[test]
fn sync_follows_the_loop_wrap_inside_a_batch() {
    for cv in [false, true] {
        let mut hh = h();
        hh.set_param("lfo1_sync", 1.0);
        if cv {
            hh.set_param("lfo1_rate_cv_trim", 1.0);
            hh.input_mut("lfo1_rate_cv_in").fill(0.0);
        }
        hh.render(3, &OUTS);
        hh.time_mut().loop_wrap = Some(LoopWrap {
            offset: 20,
            to_tick: 0.25 * PPQ as f64,
        });
        let x = lfo1(&mut hh, 1);
        // From frame 20 the tick is the loop start plus the frames since the wrap.
        let tpf = hh.time_mut().ticks_per_frame;
        for n in [20usize, 21, 40, 63] {
            let t = 0.25 * PPQ as f64 + (n - 20) as f64 * tpf;
            let want = sin_cycles(t / PPQ as f64) as f32;
            assert!(close(x[n], want, 1e-5), "cv {cv} n {n}: {} vs {want}", x[n]);
        }
    }
}

#[test]
fn sync_division_table_covers_straight_triplet_and_dotted() {
    assert!(close_f(div_beats(13, (4, 4)), 1.0));
    assert!(close_f(div_beats(12, (4, 4)), 2.0 / 3.0));
    assert!(close_f(div_beats(14, (4, 4)), 1.5));
    assert!(close_f(div_beats(1, (4, 4)), 0.0625));
    assert!(close_f(div_beats(19, (4, 4)), 4.0));
    assert!(close_f(div_beats(19, (3, 4)), 3.0));
    assert!(close_f(div_beats(19, (6, 8)), 3.0));
    assert!(close_f(div_beats(28, (4, 4)), 32.0));
    assert!(close_f(div_beats(29, (4, 4)), 48.0));
    assert!((0..30).all(|i| div_beats(i, (4, 4)) > 0.0));
    // Through the device: a 1 bar division at 120 BPM lasts 2 s.
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.set_param("lfo1_div", 19.0);
    let x = lfo1(&mut hh, 750);
    assert!(close(x[24_000], 1.0, 1e-3), "{}", x[24_000]);
}

fn close_f(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

#[test]
fn sync_free_runs_at_the_synced_rate_when_stopped() {
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.set_param("lfo1_div", 16.0); // 1/2 note: 2 beats = 1 s at 120 BPM
    hh.time_mut().playing = false;
    hh.time_mut().tick = 7.0 * PPQ as f64; // the stopped playhead is not used
    let x = lfo1(&mut hh, 750);
    assert!(x[0].abs() < 1e-6);
    assert!(close(x[12_000], 1.0, 1e-4), "{}", x[12_000]);
    assert!(close(x[24_000], 0.0, 1e-4));
    // Continues from the last phase when stopped mid-cycle (no restart).
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.set_param("lfo1_div", 16.0);
    hh.time_mut().playing = false;
    let a = lfo1(&mut hh, 94); // 6016 samples
    let b = lfo1(&mut hh, 1);
    assert!(close(b[0], sin_cycles(6016.0 / 48_000.0) as f32, 1e-4));
    assert!(a.len() == 6016);
}

#[test]
fn sync_follows_a_tempo_change() {
    let mut hh = h();
    hh.set_param("lfo1_sync", 1.0);
    hh.render(3, &OUTS);
    let t = hh.time_mut();
    t.tempo = Tempo::new(60.0).unwrap();
    t.ticks_per_frame = 60.0 * PPQ as f64 / (60.0 * SR);
    let tick = t.tick;
    let x = lfo1(&mut hh, 1);
    let want = sin_cycles(tick / PPQ as f64) as f32;
    assert!(close(x[0], want, 1e-5));
}

#[test]
fn key_sync_restarts_the_phase_at_the_note_offset() {
    for locked in [false, true] {
        let mut hh = h();
        hh.set_param("lfo1_shape", 2.0); // saw up: -1 at phase 0
        hh.set_param("lfo1_rate", 5.0);
        hh.set_param("lfo1_key_sync", 1.0);
        if locked {
            hh.set_param("lfo1_sync", 1.0);
        }
        hh.render(30, &OUTS);
        hh.push_note("notes_in", on(17, 60, 1.0));
        let x = lfo1(&mut hh, 1);
        assert!(x[16] > -0.9, "locked {locked}: before the note {}", x[16]);
        assert!(close(x[17], -1.0, 1e-5), "locked {locked}: {}", x[17]);
        assert!(x[18] > x[17] && x[63] > x[18], "rising after the restart");
    }
}

#[test]
fn key_sync_off_ignores_notes() {
    let mut a = h();
    a.set_param("lfo1_shape", 2.0);
    a.set_param("lfo1_rate", 5.0);
    let mut b = h();
    b.set_param("lfo1_shape", 2.0);
    b.set_param("lfo1_rate", 5.0);
    a.render(3, &OUTS);
    b.render(3, &OUTS);
    a.push_note("notes_in", on(17, 60, 1.0));
    assert_eq!(a.render(2, &OUTS)[0], b.render(2, &OUTS)[0]);
}

#[test]
fn key_sync_gives_new_random_values_per_note() {
    let mut hh = h();
    hh.set_param("lfo1_shape", 5.0);
    hh.set_param("lfo1_rate", 1.0);
    hh.set_param("lfo1_key_sync", 1.0);
    let mut seen = Vec::new();
    for _ in 0..4 {
        hh.push_note("notes_in", on(0, 60, 1.0));
        hh.process();
        seen.push(hh.output("lfo1_cv_out")[10]);
    }
    assert!(
        seen.windows(2).all(|w| w[0].to_bits() != w[1].to_bits()),
        "{seen:?}"
    );
}

#[test]
fn random_shapes_are_deterministic_per_seed() {
    let run = |seed: u64| {
        let mut hh = h_seed(seed);
        quarter(&mut hh);
        hh.set_param("lfo1_shape", 5.0);
        hh.set_param("lfo2_shape", 6.0);
        hh.set_param("lfo2_rate", 20.0);
        hh.render(60, &OUTS)
    };
    let bits =
        |v: &Vec<Vec<f32>>| -> Vec<u32> { v.iter().flatten().map(|f| f.to_bits()).collect() };
    assert_eq!(bits(&run(7)), bits(&run(7)));
    assert_ne!(bits(&run(7)), bits(&run(8)));
}

#[test]
fn random_values_do_not_depend_on_how_the_run_is_cut() {
    // A locate back to a cycle gives the same S&H value as the first time.
    let mut hh = h_seed(3);
    hh.set_param("lfo1_shape", 5.0);
    hh.set_param("lfo1_sync", 1.0);
    hh.time_mut().tick = 5.5 * PPQ as f64;
    let a = lfo1(&mut hh, 1)[0];
    hh.render(200, &OUTS);
    hh.time_mut().tick = 5.5 * PPQ as f64;
    let b = lfo1(&mut hh, 1)[0];
    assert_eq!(a.to_bits(), b.to_bits());
}

fn env(hh: &mut DeviceHarness, batches: usize) -> Vec<f32> {
    hh.render(batches, &["env_cv_out"]).remove(0)
}

fn fast_env(hh: &mut DeviceHarness) {
    hh.set_param("env_attack", 10.0); // 480 samples
    hh.set_param("env_decay", 20.0);
    hh.set_param("env_sustain", 0.5);
    hh.set_param("env_release", 30.0);
}

#[test]
fn envelope_runs_its_stages() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.push_note("notes_in", on(5, 60, 1.0));
    let a = env(&mut hh, 1);
    assert!(a[..5].iter().all(|v| *v == 0.0), "silent before the note");
    assert!(a[5] > 0.0 || a[6] > 0.0);
    let rest = env(&mut hh, 14); // up to sample 960 + 64
    // Peak 1.0 near sample 5 + 480.
    let all: Vec<f32> = a.iter().chain(&rest).copied().collect();
    let peak = all
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .unwrap();
    assert!(close(*peak.1, 1.0, 1e-6));
    assert!((peak.0 as i32 - 485).abs() <= 2, "{}", peak.0);
    // Sustain 0.5.
    let s = env(&mut hh, 40);
    assert!(close(*s.last().unwrap(), 0.5, 1e-3));
    // Release after the note off at offset 9 of the next batch.
    hh.push_note("notes_in", off(9, 60));
    let r = env(&mut hh, 1);
    assert!(close(r[8], 0.5, 1e-3) && r[10] < r[8]);
    let tail = env(&mut hh, 40);
    assert!(tail.iter().all(|v| (0.0..=0.5).contains(v)));
    assert_eq!(*tail.last().unwrap(), 0.0);
}

#[test]
fn envelope_starts_at_the_note_sample_offset() {
    for offset in [0u16, 1, 31, 63] {
        let mut hh = h();
        hh.set_param("env_attack", 1.0);
        hh.push_note("notes_in", on(offset, 64, 1.0));
        let x = env(&mut hh, 1);
        let first = x.iter().position(|v| *v > 0.0).unwrap();
        assert_eq!(first, usize::from(offset), "offset {offset}");
    }
}

#[test]
fn envelope_amount_scales_the_output() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.set_param("env_amount", 0.4);
    hh.push_note("notes_in", on(0, 60, 1.0));
    let x = env(&mut hh, 20);
    let peak = x.iter().copied().fold(0.0, f32::max);
    assert!(close(peak, 0.4, 1e-6), "{peak}");
}

#[test]
fn retrigger_mode_restarts_the_attack_from_the_current_level() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.push_note("notes_in", on(0, 60, 1.0));
    env(&mut hh, 40); // settle at sustain 0.5
    hh.push_note("notes_in", on(0, 64, 1.0));
    let x = env(&mut hh, 12);
    assert!(x[0] > 0.5, "continues upward, no dip: {}", x[0]);
    assert!(
        x.iter().copied().fold(0.0, f32::max) > 0.99,
        "attacks again"
    );
}

#[test]
fn legato_mode_ignores_overlapping_notes() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.set_param("env_mode", 1.0);
    hh.push_note("notes_in", on(0, 60, 1.0));
    env(&mut hh, 40);
    hh.push_note("notes_in", on(0, 64, 1.0));
    let x = env(&mut hh, 12);
    assert!(x.iter().all(|v| close(*v, 0.5, 1e-3)), "no new attack");
    // Releasing one of two held notes keeps the gate; releasing the last releases.
    hh.push_note("notes_in", off(0, 60));
    let y = env(&mut hh, 4);
    assert!(y.iter().all(|v| close(*v, 0.5, 1e-3)));
    hh.push_note("notes_in", off(0, 64));
    let z = env(&mut hh, 40);
    assert_eq!(*z.last().unwrap(), 0.0);
    // After a full release a new note attacks again.
    hh.push_note("notes_in", on(0, 60, 1.0));
    let w = env(&mut hh, 12);
    assert!(w.iter().copied().fold(0.0, f32::max) > 0.99);
}

#[test]
fn all_off_releases_every_note() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.push_note("notes_in", on(0, 60, 1.0));
    hh.push_note("notes_in", on(0, 64, 1.0));
    env(&mut hh, 40);
    hh.push_note(
        "notes_in",
        atomik_core::NoteEvent {
            offset: 0,
            kind: NoteKind::AllOff,
        },
    );
    let x = env(&mut hh, 40);
    assert_eq!(*x.last().unwrap(), 0.0);
}

#[test]
fn gate_cv_triggers_above_one_half_at_the_sample() {
    let mut hh = h();
    fast_env(&mut hh);
    {
        let g = hh.input_mut("gate_cv_in");
        g.fill(0.0);
        g[20..].fill(1.0);
    }
    let x = env(&mut hh, 1);
    assert!(x[..20].iter().all(|v| *v == 0.0));
    assert!(x[21] > 0.0);
    hh.input_mut("gate_cv_in").fill(1.0);
    env(&mut hh, 40);
    let s = env(&mut hh, 1);
    assert!(close(s[63], 0.5, 1e-3), "sustains while the gate is high");
    hh.input_mut("gate_cv_in").fill(0.0);
    let r = env(&mut hh, 40);
    assert_eq!(*r.last().unwrap(), 0.0);
    // The threshold is exclusive.
    hh.input_mut("gate_cv_in").fill(0.5);
    assert!(env(&mut hh, 2).iter().all(|v| *v == 0.0));
    hh.input_mut("gate_cv_in").fill(0.51);
    assert!(env(&mut hh, 2).iter().any(|v| *v > 0.0));
}

#[test]
fn gate_cv_and_notes_share_one_gate() {
    let mut hh = h();
    fast_env(&mut hh);
    hh.set_param("env_mode", 1.0);
    hh.input_mut("gate_cv_in").fill(1.0);
    env(&mut hh, 40);
    // A held note on top of the high gate does not retrigger in legato mode, and the release
    // waits for the gate CV as well.
    hh.push_note("notes_in", on(0, 60, 1.0));
    hh.push_note("notes_in", off(10, 60));
    let x = env(&mut hh, 3);
    assert!(x.iter().all(|v| close(*v, 0.5, 1e-3)));
    hh.input_mut("gate_cv_in").fill(0.0);
    assert_eq!(*env(&mut hh, 40).last().unwrap(), 0.0);
}

#[test]
fn outputs_are_zero_when_idle_and_env_starts_silent() {
    let mut hh = h();
    hh.set_param("lfo1_amount", 0.0);
    hh.set_param("lfo2_amount", 0.0);
    let o = hh.render(5, &OUTS);
    assert!(o.iter().flatten().all(|v| v.abs() < 1e-9));
}

#[test]
fn meter_values_have_the_documented_layout() {
    let mut hh = h();
    fast_env(&mut hh);
    quarter(&mut hh);
    hh.push_note("notes_in", on(0, 60, 1.0));
    assert!(hh.device().has_meter_values());
    let mut m = [0.0f32; METER_COUNT];
    assert_eq!(hh.device().meter_values(&mut m), METER_COUNT);
    assert_eq!(hh.device().meter_values(&mut m[..10]), 10);
    let o = hh.render(5, &OUTS);
    assert_eq!(hh.device().meter_values(&mut m), 32);
    let last = 319;
    assert!(close(m[0], o[0][last], 1e-9));
    assert!(close(m[1], o[1][last], 1e-9));
    assert!(close(m[2], o[2][last], 1e-9));
    assert!(
        close(m[3], (320.0 / 1200.0) - (1.0 / 1200.0), 1e-4),
        "{}",
        m[3]
    );
    assert!((m[4] - 0.0).abs() <= 1.0);
    assert!(close(m[5], 1.0, 0.0), "attack stage");
    // 320 frames at 120 frames per push = 2 pushes.
    assert!(close(m[6], 2.0, 0.0), "{}", m[6]);
    assert!(close(m[31], 2.5, 1e-6));
    // The newest history point is the output at the last push (frame 239).
    assert!(close(m[7 + SCOPE_POINTS - 1], o[0][239], 1e-9));
    assert!(close(m[23 + SCOPE_POINTS - 1], o[2][239], 1e-9));
    assert!(close(m[7 + SCOPE_POINTS - 2], o[0][119], 1e-9));
    // The counter wraps at 4096.
    hh.render(4_000, &OUTS);
    hh.device().meter_values(&mut m);
    assert!(m[6] >= 0.0 && m[6] < 4096.0);
    assert_no_alloc(|| hh.device().meter_values(&mut m));
}

#[test]
fn history_covers_the_time_between_two_sixty_hertz_polls() {
    let ms = (SCOPE_POINTS as f64) * 1000.0 / SCOPE_RATE_HZ;
    assert!(ms > 16.7, "{ms}");
}

/// FNV-1a over the bit patterns.
fn fnv(values: &[Vec<f32>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in values.iter().flatten() {
        for b in v.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

fn determinism_run(sr: f64) -> u64 {
    let mut hh = DeviceHarness::with_options(Box::new(ModSource::new()), sr, 133.0, 42);
    hh.set_param("lfo1_shape", 6.0);
    hh.set_param("lfo1_rate", 7.3);
    hh.set_param("lfo1_rate_cv_trim", 0.375);
    hh.set_param("lfo1_key_sync", 1.0);
    hh.set_param("lfo2_shape", 0.0);
    hh.set_param("lfo2_sync", 1.0);
    hh.set_param("lfo2_div", 14.0);
    hh.set_param("lfo2_morph", 0.37);
    hh.set_param("env_attack", 33.0);
    hh.set_param("env_decay", 120.0);
    hh.set_param("env_release", 400.0);
    for (i, v) in hh.input_mut("lfo1_rate_cv_in").iter_mut().enumerate() {
        *v = sin_cycles(i as f64 * 0.0173) as f32 * 0.5;
    }
    let mut all = Vec::new();
    for b in 0..300 {
        if b % 50 == 0 {
            hh.push_note("notes_in", on(11, 60, 1.0));
        }
        if b % 50 == 20 {
            hh.push_note("notes_in", off(40, 60));
        }
        all.extend(hh.render(1, &OUTS));
    }
    fnv(&all)
}

#[test]
fn output_is_identical_on_every_platform() {
    // Pinned: the stream uses no libm call, so every platform must produce these bits.
    assert_eq!(
        determinism_run(48_000.0),
        833_311_107_582_873_098,
        "pinned stream changed"
    );
}

#[test]
fn rerunning_gives_the_same_bits() {
    assert_eq!(determinism_run(44_100.0), determinism_run(44_100.0));
    assert_ne!(determinism_run(44_100.0), determinism_run(48_000.0));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn outputs_stay_in_range_for_any_params_and_cv(
        values in proptest::collection::vec(0.0f32..=1.0, 30),
        extreme in proptest::collection::vec(proptest::bool::ANY, 30),
        cv in -4.0f32..4.0,
        gate in -1.0f32..2.0,
        sync_playing in proptest::bool::ANY,
        seed in 0u64..1000,
    ) {
        let mut hh = h_seed(seed);
        for (i, p) in DESCRIPTOR.params.iter().enumerate() {
            let t = if extreme[i] { values[i].round() } else { values[i] };
            hh.set_param(p.key, p.min + (p.max - p.min) * t);
        }
        hh.time_mut().playing = sync_playing;
        for k in ["lfo1_rate_cv_in", "lfo1_amount_cv_in", "lfo2_rate_cv_in", "lfo2_amount_cv_in"] {
            hh.input_mut(k).fill(cv);
        }
        hh.input_mut("gate_cv_in").fill(gate);
        for b in 0..12 {
            if b % 3 == 0 {
                hh.push_note("notes_in", on((b * 5) as u16, 60 + b as u8, 0.7));
            }
            hh.process();
            for (key, lo) in [("lfo1_cv_out", -1.0), ("lfo2_cv_out", -1.0), ("env_cv_out", 0.0)] {
                for v in hh.output(key) {
                    prop_assert!(v.is_finite() && (lo..=1.0).contains(v), "{key} {v}");
                }
            }
        }
    }

    #[test]
    fn unipolar_lfo_output_is_never_negative(
        shape in 0u8..7, morph in -1.0f32..1.0, rate in 0.01f32..40.0, seed in 0u64..100,
    ) {
        let mut hh = h_seed(seed);
        hh.set_param("lfo1_polarity", 1.0);
        hh.set_param("lfo1_shape", f32::from(shape));
        hh.set_param("lfo1_morph", morph);
        hh.set_param("lfo1_rate", rate);
        for v in lfo1(&mut hh, 30) {
            prop_assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }
}

// ---- LFO cross-modulation, lag, key follow and the 1 kHz rate ------------------------------

/// LFO 2 as a +1 pulse for its first second (0.5 Hz, 50 % width), LFO 1 a 1 Hz sine.
fn pulse_over_sine() -> DeviceHarness {
    let mut hh = h();
    hh.set_param("lfo2_shape", 4.0);
    hh
}

#[test]
fn the_free_rate_reaches_one_kilohertz() {
    let mut hh = h();
    hh.set_param("lfo1_rate", 1_000.0);
    let x = lfo1(&mut hh, 75); // 4800 samples: 100 cycles of 48 samples
    assert!(close(x[12], 1.0, 1e-3), "{}", x[12]);
    assert!(close(x[36], -1.0, 1e-3), "{}", x[36]);
    let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!((99..=100).contains(&crossings), "{crossings}");
    // Beyond the range clamps.
    hh.set_param("lfo1_rate", 5_000.0);
    assert!(lfo1(&mut hh, 4).iter().all(|s| s.abs() <= 1.0));
}

#[test]
fn lfo_two_bends_the_rate_of_lfo_one_in_octaves() {
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_rate", 1.0); // +4 octaves while LFO 2 is +1: 16 Hz
    let x = lfo1(&mut hh, 30);
    assert!(
        close(x[750], 1.0, 2e-2),
        "a 16 Hz quarter cycle: {}",
        x[750]
    );
    // Half the amount is two octaves.
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_rate", 0.5);
    let x = lfo1(&mut hh, 60);
    assert!(
        close(x[3_000], 1.0, 2e-2),
        "a 4 Hz quarter cycle: {}",
        x[3_000]
    );
    // A negative amount slows it down.
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_rate", -0.25); // one octave down
    let x = lfo1(&mut hh, 400);
    assert!(
        close(x[24_000], 1.0, 2e-2),
        "a 0.5 Hz quarter cycle: {}",
        x[24_000]
    );
}

#[test]
fn a_silent_source_leaves_lfo_one_untouched_bit_for_bit() {
    let mut plain = h();
    let want = lfo1(&mut plain, 100);
    let mut hh = h();
    hh.set_param("lfo2_amount", 0.0);
    hh.set_param("lfo2_to_lfo1_rate", 1.0);
    assert_eq!(
        lfo1(&mut hh, 100)
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
    );
    // And the defaults of the new params change nothing either.
    let mut hh = pulse_over_sine();
    let with = lfo1(&mut hh, 100);
    assert_eq!(
        with.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
    );
}

#[test]
fn lfo_two_dips_the_level_of_lfo_one() {
    // LFO 2 sits at +1: a positive amount keeps full level, a negative one silences LFO 1.
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_level", 1.0);
    let kept = lfo1(&mut hh, 200);
    assert!(close(kept[12_000], 1.0, 1e-3), "{}", kept[12_000]);
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_level", -1.0);
    let dipped = lfo1(&mut hh, 200);
    assert!(dipped.iter().all(|s| s.abs() < 1e-6));
    let mut hh = pulse_over_sine();
    hh.set_param("lfo2_to_lfo1_level", -0.5);
    let half = lfo1(&mut hh, 200);
    assert!(close(half[12_000], 0.5, 1e-3), "{}", half[12_000]);
}

#[test]
fn cross_modulation_stays_in_range_and_deterministic() {
    let run = || {
        let mut hh = h_seed(9);
        hh.set_param("lfo1_shape", 5.0);
        hh.set_param("lfo2_shape", 6.0);
        hh.set_param("lfo2_to_lfo1_rate", 0.8);
        hh.set_param("lfo2_to_lfo1_level", 0.7);
        hh.set_param("lfo1_sync", 1.0);
        hh.render(200, &OUTS)
    };
    let a = run();
    assert_eq!(a, run());
    assert!(a.iter().flatten().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
fn lag_smooths_the_outputs_with_a_one_pole_of_the_set_time() {
    let mut hh = h();
    hh.set_param("env_attack", 1.0);
    hh.set_param("env_sustain", 1.0);
    hh.set_param("lag", 100.0);
    hh.input_mut("gate_cv_in").fill(1.0);
    let env = hh.render(80, &["env_cv_out"]).remove(0);
    // One time constant after the start the output has covered 1 - 1/e of the way.
    assert!(
        close(env[4_800], 1.0 - (-1.0f32).exp(), 0.03),
        "{}",
        env[4_800]
    );
    assert!(
        env.windows(2).all(|w| w[1] >= w[0] - 1e-6),
        "monotonic rise"
    );
    // With the lag off the same envelope is at full level within a few milliseconds.
    let mut hh = h();
    hh.set_param("env_attack", 1.0);
    hh.set_param("env_sustain", 1.0);
    hh.input_mut("gate_cv_in").fill(1.0);
    let env = hh.render(80, &["env_cv_out"]).remove(0);
    assert!(env[4_800] > 0.99);
}

#[test]
fn lag_never_leaves_the_range_and_resets_cleanly() {
    let mut hh = h();
    hh.set_param("lag", 2_000.0);
    hh.set_param("lfo1_shape", 4.0);
    hh.set_param("lfo1_rate", 40.0);
    let x = lfo1(&mut hh, 200);
    assert!(x.iter().all(|s| (-1.0..=1.0).contains(s)));
    let mut again = h();
    again.set_param("lag", 2_000.0);
    again.set_param("lfo1_shape", 4.0);
    again.set_param("lfo1_rate", 40.0);
    assert_eq!(x, lfo1(&mut again, 200));
    assert_no_alloc(|| {
        for _ in 0..20 {
            hh.process();
        }
    });
}

#[test]
fn key_follow_scales_the_free_rate_with_the_latest_key() {
    let mut hh = h();
    hh.set_param("key_follow", 1.0);
    hh.push_note("notes_in", on(0, 72, 1.0)); // an octave up: 2 Hz
    let x = lfo1(&mut hh, 150);
    assert!(
        close(x[6_000], 1.0, 2e-3),
        "a 2 Hz quarter cycle: {}",
        x[6_000]
    );
    // Half follow is half an octave per octave.
    let mut hh = h();
    hh.set_param("key_follow", 0.5);
    hh.push_note("notes_in", on(0, 72, 1.0));
    let x = lfo1(&mut hh, 200);
    let want = (48_000.0 / 2.0f32.sqrt() / 4.0).round() as usize;
    assert!(close(x[want], 1.0, 5e-3), "{} at {want}", x[want]);
    // Below the reference key it slows down.
    let mut hh = h();
    hh.set_param("key_follow", 1.0);
    hh.push_note("notes_in", on(0, 48, 1.0)); // an octave down: 0.5 Hz
    let x = lfo1(&mut hh, 800);
    assert!(close(x[24_000], 1.0, 5e-3), "{}", x[24_000]);
    // With no follow a note changes nothing.
    let mut hh = h();
    hh.push_note("notes_in", on(0, 72, 1.0));
    let x = lfo1(&mut hh, 190);
    assert!(close(x[12_000], 1.0, 1e-4));
}

#[test]
fn key_follow_leaves_a_tempo_locked_lfo_alone() {
    let run = |follow: f32| {
        let mut hh = h();
        hh.set_param("lfo1_sync", 1.0);
        hh.set_param("key_follow", follow);
        hh.time_mut().playing = true;
        hh.push_note("notes_in", on(0, 84, 1.0));
        hh.render(100, &["lfo1_cv_out"])
    };
    assert_eq!(run(0.0), run(1.0));
}
