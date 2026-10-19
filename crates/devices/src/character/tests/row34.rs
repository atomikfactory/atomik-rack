//! Per-preset checks of rows 3 and 4 (presets 8 to 15): `lossy`, `swirl`, `torn-cone`,
//! `tin-robot`, `bit-bender`, `spin-cycle`, `ion-sweep`, `ghost-pipes`, and the long latency
//! class of `lossy` with real chains. Each check measures a character-level property that the
//! voicing table promises (a rate, a delay, a frequency, a level law), isolating the stage under
//! test with small table edits where other stages would blur it.

#![allow(clippy::chunks_exact_to_as_chunks)]

use super::*;
use crate::character::chain::long_latency;

/// The shipped tables with some rows replaced: `(row prefix up to and including the param
/// cell, new cells after it)`. A tail that starts with `=` replaces the whole line with the text
/// after it, an empty tail deletes the row. Every edit must match exactly one row.
fn edited(rows: &[(&str, &str)], routes: &[(&str, &str)]) -> Voicing {
    let patch = |text: &str, edits: &[(&str, &str)]| -> String {
        let mut out = String::new();
        let mut hit = vec![0usize; edits.len()];
        for line in text.lines() {
            let mut line = Some(line.to_string());
            for (i, (prefix, tail)) in edits.iter().enumerate() {
                if line.as_deref().is_some_and(|l| l.starts_with(prefix)) {
                    line = if tail.is_empty() {
                        None
                    } else if let Some(full) = tail.strip_prefix('=') {
                        Some(full.to_string())
                    } else {
                        Some(format!("{prefix}{tail}"))
                    };
                    hit[i] += 1;
                }
            }
            if let Some(line) = line {
                out.push_str(&line);
                out.push('\n');
            }
        }
        assert!(
            hit.iter().all(|h| *h == 1),
            "each edit matches once: {hit:?}"
        );
        out
    };
    load_voicing(&patch(VOICING_CSV, rows), &patch(ROUTES_CSV, routes)).expect("edited tables load")
}

fn dev_with(v: Voicing, sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Character::with_voicing(v)), sr, 120.0, 11)
}

/// A device on `preset` with Transform `t` (default tables, 48 kHz).
fn on(preset: usize, t: f32) -> DeviceHarness {
    let mut h = dev();
    set_preset(&mut h, preset);
    h.set_param("transform", t);
    h
}

fn lin(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Zero-mean bipolar white noise at RMS level `amp`.
fn white(n: usize, seed: u64, amp: f32) -> Vec<f32> {
    noise(n, seed, amp * 1.732)
}

/// The mean power spectrum of `x` over `win`-sample Hann frames (hop `win`), linear power.
fn power_spectrum(x: &[f32], win: usize) -> Vec<f64> {
    let mut acc = vec![0.0f64; win / 2];
    let mut frames = 0.0;
    for c in x.chunks_exact(win) {
        for (a, m) in acc
            .iter_mut()
            .zip(atomik_testkit::signal::magnitude_spectrum(c))
        {
            *a += m * m;
        }
        frames += 1.0;
    }
    acc.iter().map(|v| v / frames).collect()
}

/// Short-time RMS in windows of `win` samples.
fn envelope(x: &[f32], win: usize) -> Vec<f64> {
    x.chunks_exact(win).map(rms).collect()
}

/// Normalised autocorrelation of `x` at `lag` samples.
fn autocorr(x: &[f32], lag: usize) -> f64 {
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for n in lag..x.len() {
        num += f64::from(x[n]) * f64::from(x[n - lag]);
    }
    for v in x {
        den += f64::from(*v) * f64::from(*v);
    }
    num / den
}

fn count_sign_changes(x: &[f64]) -> usize {
    x.windows(2)
        .filter(|w| w[0].signum() != w[1].signum())
        .count()
}

// --- Table and class bookkeeping -----------------------------------------------------------------

#[test]
fn all_sixteen_presets_are_voiced_and_use_only_declared_stages() {
    assert_eq!(chain::IMPLEMENTED_PRESETS, 16);
    assert_eq!(voiced_presets(), 16);
    let chains: Vec<Box<dyn PresetChain>> = (0..16).map(build_chain).collect();
    let specs: Vec<ChainSpec> = chains.iter().map(|c| c.spec()).collect();
    let v = Voicing::load(VOICING_CSV, ROUTES_CSV, &specs).unwrap();
    for (i, spec) in specs.iter().enumerate().skip(8) {
        assert!(v.preset(i).has_rows(), "preset {i}");
        assert!(v.preset(i).morph_count() >= 2, "preset {i} morphs");
        assert!(
            v.preset(i).morph_count() <= 8,
            "preset {i} at most 8 morph values"
        );
        assert!(spec.implemented);
    }
    for (i, def) in all_rack_defs() {
        for st in def.stages {
            // Every stage of every rack preset is routed (nothing is declared and unused).
            assert!(
                v.preset(i)
                    .routes()
                    .iter()
                    .any(|r| r.from == st.name || r.to == st.name),
                "preset {i} stage {}",
                st.name
            );
        }
    }
}

#[test]
fn only_lossy_is_in_the_long_latency_class() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for i in 0..16 {
            let mut c = build_chain(i);
            let chains: Vec<Box<dyn PresetChain>> = (0..16).map(build_chain).collect();
            let specs: Vec<ChainSpec> = chains.iter().map(|c| c.spec()).collect();
            let v = Voicing::load(VOICING_CSV, ROUTES_CSV, &specs).unwrap();
            c.prepare(sr, 1, v.preset(i));
            let want = if i == 8 {
                long_latency(sr)
            } else {
                short_latency(sr)
            };
            assert_eq!(c.latency(), want, "preset {i} at {sr}");
        }
    }
}

// --- lossy ---------------------------------------------------------------------------------------

/// The lossy preset with the bells flat and the low-pass edge out of the signal path, so only the
/// spectral stage acts.
fn lossy_alone() -> Voicing {
    edited(
        &[
            ("lossy,bell1,gain_db,", "0,0,0,lin,original,"),
            ("lossy,bell2,gain_db,", "0,0,0,lin,original,"),
        ],
        &[
            (
                "lossy,bell2,1,lp,",
                "=lossy,bell2,1,out,1,1,1,lin,original,",
            ),
            ("lossy,lp,1,out,", ""),
        ],
    )
}

#[test]
fn lossy_wet_impulse_arrives_exactly_at_the_long_class_latency_at_every_rate() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        // With the transparent setting of the table at Transform 0 and the filters neutral, the
        // spectral stage is a near identity: the peak is exactly `short + frame` frames late.
        let mut h = dev_with(lossy_alone(), sr);
        set_preset(&mut h, 8);
        h.set_param("transform", 0.0);
        let mut x = vec![0.0f32; BATCH * 120];
        x[3] = 0.2;
        let (l, _) = run(&mut h, &x, None);
        let lat = latency(&h);
        assert_eq!(lat, long_latency(sr as f32) as usize);
        let at = l
            .iter()
            .enumerate()
            .fold(0, |m, (i, v)| if v.abs() > l[m].abs() { i } else { m });
        assert_eq!(at, lat + 3, "sr {sr}: peak {at}, latency {lat}");
        assert!(peak(&l) > 0.15, "the impulse comes through: {}", peak(&l));
    }
}

#[test]
fn lossy_gates_the_quiet_band_as_transform_rises_and_keeps_the_loud_one() {
    let n = BATCH * 750;
    let loud = sine(n, 1_000.0, f64::from(lin(-12.0)), SR);
    let quiet = sine(n, 5_000.0, f64::from(lin(-60.0)), SR);
    let x: Vec<f32> = loud.iter().zip(&quiet).map(|(a, b)| a + b).collect();
    let measure = |t: f32| {
        let mut h = dev_with(lossy_alone(), SR);
        set_preset(&mut h, 8);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        let w = &l[n / 2..];
        (tone_amp(w, 1_000.0), tone_amp(w, 5_000.0))
    };
    let (loud0, quiet0) = measure(0.0);
    let (loud1, quiet1) = measure(1.0);
    let a_loud = f64::from(lin(-12.0));
    let a_quiet = f64::from(lin(-60.0));
    // Transform 0: both pass (the 1 dB quantiser rounds each by at most half a step).
    assert!(db(loud0 / a_loud).abs() < 1.0, "{}", db(loud0 / a_loud));
    assert!(db(quiet0 / a_quiet).abs() < 1.0, "{}", db(quiet0 / a_quiet));
    // Transform 1: the quiet band is a hole (well below its own level), the loud one stays within
    // half of the coarse quantiser step (9 dB).
    assert!(db(quiet1 / a_quiet) < -20.0, "{}", db(quiet1 / a_quiet));
    assert!(db(loud1 / a_loud).abs() < 5.0, "{}", db(loud1 / a_loud));
}

#[test]
fn lossy_gates_the_quiet_top_of_the_spectrum_first_as_transform_rises() {
    // Pink noise is loud at the bottom and quiet at the top: the bins at the top fall below the
    // gate threshold as it rises, so the energy above 5 kHz against the energy below 1 kHz drops.
    let n = BATCH * 750;
    let x = pink(n, 5, 0.03);
    let render = |t: f32| {
        let mut h = dev_with(lossy_alone(), SR);
        set_preset(&mut h, 8);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        let lat = latency(&h);
        (l, lat)
    };
    let top_over_bottom = |l: &[f32]| {
        let sp = power_spectrum(&l[n / 2..], 4096);
        let band = |lo: f64, hi: f64| {
            sp.iter()
                .enumerate()
                .filter(|(k, _)| (lo..hi).contains(&(*k as f64 * SR / 4096.0)))
                .map(|(_, p)| *p)
                .sum::<f64>()
        };
        10.0 * (band(5_000.0, 20_000.0) / band(50.0, 1_000.0)).log10()
    };
    let (l0, lat) = render(0.0);
    let (l1, _) = render(1.0);
    // Transform 0 is transparent: the output is the delayed input within the 1 dB quantiser.
    let (mut xy, mut xx, mut yy) = (0.0f64, 0.0f64, 0.0f64);
    for i in n / 2..n {
        let (a, b) = (f64::from(x[i - lat]), f64::from(l0[i]));
        xy += a * b;
        xx += a * a;
        yy += b * b;
    }
    assert!(xy / (xx * yy).sqrt() > 0.99, "{}", xy / (xx * yy).sqrt());
    let drop = top_over_bottom(&l1) - top_over_bottom(&l0);
    assert!(
        drop < -10.0,
        "the top fell by only {drop} dB against the bottom"
    );
}

#[test]
fn lossy_dropouts_follow_the_seed_and_leave_holes_that_move() {
    let n = BATCH * 400;
    let x = white(n, 8, 0.1);
    let render = |seed: u64| {
        let mut h = DeviceHarness::with_options(Box::new(Character::new()), SR, 120.0, seed);
        set_preset(&mut h, 8);
        h.set_param("transform", 1.0);
        run(&mut h, &x, None).0
    };
    assert_eq!(render(5), render(5));
    assert_ne!(render(5), render(6), "the dropouts are seeded");
    // A hole moves: the spectral envelope of two consecutive frames differs.
    let y = render(5);
    let frame = |k: usize| {
        atomik_testkit::signal::magnitude_spectrum(&y[8_192 + k * 512..8_192 + k * 512 + 1024])
    };
    let (a, b) = (frame(0), frame(3));
    let diff: f64 = a.iter().zip(&b).map(|(p, q)| (p - q).abs()).sum();
    let tot: f64 = a.iter().sum();
    assert!(diff / tot > 0.3, "{}", diff / tot);
}

#[test]
fn a_real_class_crossing_republishes_the_latency_once_and_a_switch_inside_a_class_never_does() {
    let x = noise(BATCH, 3, 0.4);
    let lat_short = short_latency(48_000.0);
    let lat_long = long_latency(48_000.0);
    let mut h = dev();
    set_preset(&mut h, 3);
    let mut seen = Vec::new();
    let step = |h: &mut DeviceHarness, batches: usize, seen: &mut Vec<u32>| {
        for _ in 0..batches {
            h.input_mut("in_l").copy_from_slice(&x);
            h.input_mut("in_r").copy_from_slice(&x);
            h.process();
            seen.push(h.device().latency_frames());
        }
    };
    step(&mut h, 40, &mut seen);
    // Inside the short class: no change of the reported latency at any batch.
    for p in [5usize, 12, 0, 15, 9] {
        set_preset(&mut h, p);
        step(&mut h, 40, &mut seen);
    }
    assert!(seen.iter().all(|v| *v == lat_short), "{seen:?}");
    // Across the classes: the latency changes exactly once, when the fade ends.
    seen.clear();
    set_preset(&mut h, 8);
    step(&mut h, 40, &mut seen);
    let changes = seen.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(changes, 1, "{seen:?}");
    assert_eq!(*seen.last().unwrap(), lat_long);
    assert_eq!(
        seen[0], lat_short,
        "the old value stands while the fade runs"
    );
    // Inside the long class (a switch away and back during the same fade) still no extra bump,
    // and back across the classes: once more.
    seen.clear();
    set_preset(&mut h, 2);
    step(&mut h, 40, &mut seen);
    let changes = seen.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(changes, 1, "{seen:?}");
    assert_eq!(*seen.last().unwrap(), lat_short);
}

#[test]
fn automation_across_the_classes_publishes_when_the_last_fade_ends() {
    // The preset flips between the classes every 10 ms (faster than a fade): the published
    // latency follows only the preset that finally plays.
    let x = noise(BATCH, 4, 0.3);
    let mut h = dev();
    let mut seen = Vec::new();
    for k in 0..60 {
        set_preset(&mut h, if k % 2 == 0 { 8 } else { 2 });
        for _ in 0..8 {
            h.input_mut("in_l").copy_from_slice(&x);
            h.process();
            seen.push(h.device().latency_frames());
        }
    }
    set_preset(&mut h, 8);
    for _ in 0..120 {
        h.input_mut("in_l").copy_from_slice(&x);
        h.process();
        seen.push(h.device().latency_frames());
    }
    assert_eq!(*seen.last().unwrap(), long_latency(48_000.0));
    assert!(
        seen.iter()
            .all(|v| *v == short_latency(48_000.0) || *v == long_latency(48_000.0))
    );
}

#[test]
fn the_dry_path_is_aligned_for_the_long_class_with_a_real_chain() {
    // Mix 0.5 of a near-transparent lossy (Transform 0, neutral filters): the sum is coherent,
    // so the output is the input delayed by the latency (no comb filtering).
    let n = BATCH * 600;
    // A multitone (steady tones sit on stable quantiser levels).
    let mut x = vec![0.0f32; n];
    for hz in [310.0, 770.0, 1_210.0, 2_050.0, 2_990.0] {
        for (i, v) in sine(n, hz, 0.05, SR).iter().enumerate() {
            x[i] += v;
        }
    }
    let mut h = dev_with(lossy_alone(), SR);
    set_preset(&mut h, 8);
    h.set_param("transform", 0.0);
    h.set_param("mix", 0.5);
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    let (mut e, mut s) = (0.0f64, 0.0f64);
    for i in n / 2..n {
        let d = f64::from(l[i] - x[i - lat]);
        e += d * d;
        s += f64::from(x[i - lat]).powi(2);
    }
    assert!(10.0 * (e / s).log10() < -26.0, "{}", 10.0 * (e / s).log10());
}

#[test]
fn lossy_stays_finite_and_bounded_at_every_rate_for_hostile_input() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for t in [0.0f32, 0.5, 1.0] {
            let mut h = dev_at(sr);
            set_preset(&mut h, 8);
            h.set_param("transform", t);
            h.set_param("gain", 1.0);
            let mut x = noise(BATCH * 200, 3, 1.0);
            x[100] = f32::NAN;
            x[200] = f32::INFINITY;
            x[300] = -1.0e9;
            let (l, r) = run(&mut h, &x, None);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 8.0));
        }
    }
}

// --- swirl ---------------------------------------------------------------------------------------

#[test]
fn swirl_pans_at_the_table_rate_and_the_echo_repeats_at_its_time() {
    // The rate of the auto-pan: the left/right balance of a steady noise crosses zero twice per
    // cycle. At Transform 1 the table rate is 1.2 Hz.
    let secs = 10.0;
    let n = (secs * SR) as usize / BATCH * BATCH;
    let x = white(n, 3, 0.1);
    let (l, r) = run(&mut on(9, 1.0), &x, None);
    let win = 480;
    let bal: Vec<f64> = l
        .chunks_exact(win)
        .zip(r.chunks_exact(win))
        .map(|(a, b)| {
            let (p, q) = (rms(a).powi(2), rms(b).powi(2));
            // Smooth over 50 ms so the noise does not make the sign flicker.
            (p - q) / (p + q + 1e-12)
        })
        .collect();
    let smooth: Vec<f64> = bal
        .windows(10)
        .step_by(5)
        .map(|w| w.iter().sum::<f64>() / 10.0)
        .collect();
    let crossings = count_sign_changes(&smooth[4..]) as f64;
    let expect = 2.0 * 1.2 * (secs - 0.4);
    assert!(
        (crossings - expect).abs() <= 5.0,
        "{crossings} crossings, expected about {expect}"
    );
    // Echo: an impulse at Transform 0 (feedback 0) leaves one repeat 120 ms after the main
    // arrival, at about the table mix of 0.3 of the level.
    let mut x = vec![0.0f32; BATCH * 400];
    x[0] = 0.5;
    let mut h = on(9, 0.0);
    h.set_param("gain", 0.5);
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    let main_at = l
        .iter()
        .take(lat + 200)
        .enumerate()
        .fold(0, |m, (i, v)| if v.abs() > l[m].abs() { i } else { m });
    let want = main_at + (0.120 * SR) as usize;
    let win = want - 60..want + 60; // +-1.25 ms (the echo time is modulated by 0.5 ms)
    let rep = peak(&l[win]);
    let main = peak(&l[..lat + 200]);
    assert!(
        rep > 0.1 * main && rep < 0.6 * main,
        "repeat {rep} against main {main}"
    );
    // And nothing sits between the main arrival and the repeat at a comparable level.
    let between = peak(&l[main_at + 400..want - 120]);
    assert!(between < 0.1 * rep + 0.02 * main, "{between}");
}

#[test]
fn swirl_phaser_notches_move_with_the_lfo() {
    // Spectral centroid of the output of white noise changes over time at Transform 1 (the notch
    // pattern sweeps); at Transform 0 the rate is slow and the notches barely move in 200 ms.
    let n = BATCH * 750;
    let x = white(n, 4, 0.1);
    let spread = |t: f32| {
        let (l, _) = run(&mut on(9, t), &x, None);
        let cents: Vec<f64> = l[4800..]
            .chunks_exact(2048)
            .map(|c| {
                let sp = atomik_testkit::signal::magnitude_spectrum(c);
                let (mut num, mut den) = (0.0, 0.0);
                for (k, m) in sp.iter().enumerate() {
                    num += k as f64 * m * m;
                    den += m * m;
                }
                num / den
            })
            .collect();
        let mean = cents.iter().sum::<f64>() / cents.len() as f64;
        (cents.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / cents.len() as f64).sqrt() / mean
    };
    assert!(
        spread(1.0) > 2.0 * spread(0.0),
        "{} {}",
        spread(1.0),
        spread(0.0)
    );
}

// --- torn-cone -----------------------------------------------------------------------------------

#[test]
fn torn_cone_distorts_hard_and_hardly_follows_the_input_level() {
    let level = |amp_db: f64, t: f32| {
        let x = sine(BATCH * 200, 1_000.0, f64::from(lin(amp_db)), SR);
        let (l, _) = run(&mut on(10, t), &x, None);
        rms(&l[BATCH * 100..])
    };
    // 20 dB more input is much less than 20 dB more output at Transform 1 (a hard clip).
    let (lo, hi) = (level(-30.0, 1.0), level(-10.0, 1.0));
    let gain_db = db(hi / lo);
    assert!(gain_db < 8.0, "{gain_db} dB for +20 dB");
    // Harmonics: both even (the asymmetric clip) and odd are strong at Transform 1.
    let x = sine(BATCH * 400, 500.0, 0.2, SR);
    // Transform 0 (12 dB of drive): only the negative peaks reach the lower limit, so the clip
    // is lopsided and the even harmonic is strong.
    let (l0, _) = run(&mut on(10, 0.0), &x, None);
    let w0 = &l0[BATCH * 200..];
    let (a1, a2) = (tone_amp(w0, 500.0), tone_amp(w0, 1_000.0));
    assert!(db(a2 / a1) > -25.0, "H2 at Transform 0: {}", db(a2 / a1));
    // Transform 1 (42 dB): close to a square wave, so the odd harmonics are strong.
    let (l, _) = run(&mut on(10, 1.0), &x, None);
    let w = &l[BATCH * 200..];
    let h1 = tone_amp(w, 500.0);
    let h3 = tone_amp(w, 1_500.0) / h1;
    assert!(db(h3) > -16.0, "H3 at Transform 1: {}", db(h3));
    // Transform 0 distorts far less than Transform 1 at the same input.
    let thd = |w: &[f32]| {
        let f = tone_amp(w, 500.0);
        let h: f64 = (2..8)
            .map(|k| tone_amp(w, 500.0 * f64::from(k)).powi(2))
            .sum();
        h.sqrt() / f
    };
    assert!(thd(w0) < thd(w), "{} {}", thd(w0), thd(w));
}

#[test]
fn torn_cone_rattle_follows_the_input_envelope_and_dies_with_it() {
    let burst = noise(BATCH * 100, 4, 0.5);
    let silence = vec![0.0f32; BATCH * 200];
    let mut x = burst.clone();
    x.extend_from_slice(&silence);
    let run_with = |rattle: &str| {
        let v = edited(&[("torn-cone,rattle,amount,", rattle)], &[]);
        let mut h = dev_with(v, SR);
        set_preset(&mut h, 10);
        h.set_param("transform", 0.5);
        run(&mut h, &x, None).0
    };
    let with = run_with("0.9,0.9,0.9,lin,original,");
    let without = run_with("0,0,0,lin,original,");
    let lat = latency(&dev());
    let n_b = burst.len() + lat;
    let diff: Vec<f32> = with.iter().zip(&without).map(|(a, b)| a - b).collect();
    // The difference (the rattle) is audible while the burst runs ...
    let during = rms(&diff[1000..n_b - 500]);
    assert!(during > 0.003, "{during}");
    // ... and has died 200 ms after it stops (its envelope follows the input).
    let after = rms(&diff[n_b + 9_600..]);
    assert!(after < 0.01 * during, "{after} against {during}");
}

// --- tin-robot -----------------------------------------------------------------------------------

#[test]
fn tin_robot_comb_rings_at_its_delay_and_the_feedback_sets_its_depth() {
    // Autocorrelation of the left channel at the comb delay is the feedback (the band filters
    // after the comb only smear it by a fraction of a millisecond).
    let n = BATCH * 1500;
    let x = white(n, 6, 0.1);
    for (t, delay_ms, fb) in [(0.0f32, 6.0f64, 0.3f64), (0.5, 3.0, 0.6), (1.0, 1.5, 0.85)] {
        let v = edited(
            &[
                // Neutral band: the comb alone, plus the ring modulator off.
                ("tin-robot,hp,freq_hz,", "10,10,10,lin,original,"),
                ("tin-robot,lp,freq_hz,", "20000,20000,20000,lin,original,"),
                ("tin-robot,f1,gain_db,", "0,0,0,lin,original,"),
                ("tin-robot,f2,gain_db,", "0,0,0,lin,original,"),
                ("tin-robot,ring,amount,", "0,0,0,lin,original,"),
                ("tin-robot,comb,detune,", "0,0,0,lin,original,"),
                ("tin-robot,comb,wander,", "0,0,0,lin,original,"),
            ],
            &[],
        );
        let mut h = dev_with(v, SR);
        set_preset(&mut h, 11);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        let lag = (delay_ms * 0.001 * SR).round() as usize;
        let r = autocorr(&l[BATCH * 200..], lag);
        assert!(
            (r - fb).abs() < 0.07,
            "T {t}: autocorr {r} at {lag}, feedback {fb}"
        );
        // Half the delay shows no such resonance.
        assert!(autocorr(&l[BATCH * 200..], lag / 2).abs() < 0.12);
    }
}

#[test]
fn tin_robot_ring_modulation_makes_sidebands_around_the_carrier() {
    let v = edited(
        &[
            ("tin-robot,comb,feedback,", "0,0,0,lin,original,"),
            ("tin-robot,hp,freq_hz,", "10,10,10,lin,original,"),
            ("tin-robot,lp,freq_hz,", "20000,20000,20000,lin,original,"),
            ("tin-robot,f1,gain_db,", "0,0,0,lin,original,"),
            ("tin-robot,f2,gain_db,", "0,0,0,lin,original,"),
            ("tin-robot,comb,detune,", "0,0,0,lin,original,"),
            ("tin-robot,comb,wander,", "0,0,0,lin,original,"),
        ],
        &[],
    );
    let n = BATCH * 750;
    let x = sine(n, 1_500.0, 0.2, SR);
    let amp = |t: f32, hz: f64| {
        let mut h = dev_with(v.clone(), SR);
        set_preset(&mut h, 11);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        tone_amp(&l[n / 2..], hz)
    };
    // Transform 1: ring amount 0.5: carrier 0.5, sidebands 0.25 each (-6 dB) at 1500 +- 55.
    let c = amp(1.0, 1_500.0);
    let (lo, hi) = (amp(1.0, 1_445.0), amp(1.0, 1_555.0));
    assert!((db(lo / c) + 6.0).abs() < 1.5, "{}", db(lo / c));
    assert!((db(hi / c) + 6.0).abs() < 1.5, "{}", db(hi / c));
    // Transform 0: no ring modulation, no sidebands.
    let c0 = amp(0.0, 1_500.0);
    assert!(
        db(amp(0.0, 1_445.0) / c0) < -35.0,
        "{}",
        db(amp(0.0, 1_445.0) / c0)
    );
}

#[test]
fn tin_robot_channels_are_detuned_so_a_mono_source_has_width() {
    let x = white(BATCH * 600, 7, 0.1);
    let (l, r) = run(&mut on(11, 0.5), &x, None);
    assert_ne!(l, r);
    let (mut d, mut s) = (0.0f64, 0.0f64);
    for i in 4_000..l.len() {
        d += f64::from(l[i] - r[i]).powi(2);
        s += f64::from(l[i]).powi(2);
    }
    assert!(d / s > 0.02, "{}", d / s);
}

// --- bit-bender ----------------------------------------------------------------------------------

/// Bit-bender with fixed bits and hold rate, no random modulation and a DC blocker out of the way.
fn crush_fixed(bits: f32, rate_hz: f32) -> Voicing {
    edited(
        &[
            (
                "bit-bender,crush,bits,",
                &format!("{bits},{bits},{bits},lin,original,"),
            ),
            (
                "bit-bender,crush,rate_hz,",
                &format!("{rate_hz},{rate_hz},{rate_hz},lin,original,"),
            ),
            ("bit-bender,crush,mod_depth,", "0,0,0,lin,original,"),
            ("bit-bender,crush,makeup_db,", "0,0,0,lin,original,"),
            ("bit-bender,dc,freq_hz,", "10,10,10,lin,original,"),
            ("bit-bender,chain,target_gain_db,", "0,0,0,lin,original,"),
        ],
        &[],
    )
}

#[test]
fn bit_bender_snr_falls_six_db_per_bit() {
    let n = BATCH * 750;
    let x = sine(n, 997.0, 0.9, SR);
    let snr = |bits: f32| {
        let mut h = dev_with(crush_fixed(bits, 96_000.0), SR);
        set_preset(&mut h, 12);
        let (l, _) = run(&mut h, &x, None);
        let lat = latency(&h);
        let w = &l[lat + 2_000..n - lat];
        // The best-fit sinusoid at the test frequency (amplitude and phase: the DC blocker
        // leads a little) is the signal; the rest is quantisation noise.
        let (mut ss, mut cc, mut sc, mut sy, mut cy, mut yy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, v) in w.iter().enumerate() {
            let ph = std::f64::consts::TAU * 997.0 * i as f64 / SR;
            let (s, c, y) = (ph.sin(), ph.cos(), f64::from(*v));
            ss += s * s;
            cc += c * c;
            sc += s * c;
            sy += s * y;
            cy += c * y;
            yy += y * y;
        }
        let det = ss * cc - sc * sc;
        let (a, b) = ((sy * cc - cy * sc) / det, (cy * ss - sy * sc) / det);
        let sig = a * sy + b * cy;
        10.0 * (sig / (yy - sig)).log10()
    };
    let (s4, s6, s8) = (snr(4.0), snr(6.0), snr(8.0));
    assert!((s6 - s4 - 12.0).abs() < 4.0, "{s4} {s6}");
    assert!((s8 - s6 - 12.0).abs() < 4.0, "{s6} {s8}");
}

#[test]
fn bit_bender_holds_each_value_for_the_programmed_time_and_leaves_images() {
    let rate = 6_000.0f32;
    let n = BATCH * 750;
    // 997 Hz: not a divisor of the hold rate, so neighbouring held values differ.
    let x = sine(n, 997.0, 0.5, SR);
    let mut h = dev_with(crush_fixed(16.0, rate), SR);
    set_preset(&mut h, 12);
    let (l, _) = run(&mut h, &x, None);
    let w = &l[n / 2..];
    // Plateau lengths: the hold alternates between floor and ceil of fs / rate (= 8 here).
    let mut runs = Vec::new();
    let mut len = 1usize;
    for p in w.windows(2) {
        if (p[1] - p[0]).abs() < 3e-3 {
            len += 1;
        } else {
            runs.push(len);
            len = 1;
        }
    }
    let mean = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
    assert!(
        (mean - SR / f64::from(rate)).abs() < 0.8,
        "mean plateau {mean} samples"
    );
    // Images of the tone at rate +- 997 Hz, strong and not harmonic.
    let a1 = tone_amp(w, 997.0);
    for hz in [6_000.0 - 997.0, 6_000.0 + 997.0] {
        assert!(db(tone_amp(w, hz) / a1) > -20.0, "image at {hz}");
    }
}

#[test]
fn bit_bender_random_modulation_is_independent_per_channel_and_seeded() {
    // Transform 1: depth 1, so bits and hold rate wander per channel.
    let x = sine(BATCH * 400, 440.0, 0.5, SR);
    let (l, r) = run(&mut on(12, 1.0), &x, None);
    assert_ne!(l, r, "independent random per channel");
    let a = run(&mut on(12, 1.0), &x, None).0;
    assert_eq!(a, l, "deterministic");
    let mut h2 = DeviceHarness::with_options(Box::new(Character::new()), SR, 120.0, 99);
    set_preset(&mut h2, 12);
    h2.set_param("transform", 1.0);
    assert_ne!(run(&mut h2, &x, None).0, l, "the draws depend on the seed");
}

#[test]
fn bit_bender_has_no_dc_and_stays_bounded_at_two_bits() {
    let x = sine(BATCH * 750, 220.0, 0.3, SR);
    let (l, r) = run(&mut on(12, 1.0), &x, None);
    for ch in [&l, &r] {
        let mean = ch[BATCH * 400..].iter().map(|v| f64::from(*v)).sum::<f64>()
            / (ch.len() - BATCH * 400) as f64;
        assert!(mean.abs() < 0.02, "dc {mean}");
        assert!(peak(ch) <= 2.0);
    }
}

// --- spin-cycle ----------------------------------------------------------------------------------

/// Spin-cycle with only the tremolo (open slosh filter, no room, echo, hum or water).
fn spin_tremolo_only() -> Voicing {
    edited(
        &[
            ("spin-cycle,slosh,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,hum,level_db,", "-120,-120,-120,lin,original,"),
            ("spin-cycle,water,rate_hz,", "0,0,0,lin,original,"),
            ("spin-cycle,chain,target_gain_db,", "0,0,0,lin,original,"),
        ],
        &[
            ("spin-cycle,slosh,2,room,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,2,echo,", "0,0,0,lin,original,"),
        ],
    )
}

#[test]
fn spin_cycle_tremolo_runs_at_the_table_rate_and_depth() {
    let n = BATCH * 750 * 4;
    let x = sine(n, 440.0, 0.3, SR);
    let mut h = dev_with(spin_tremolo_only(), SR);
    set_preset(&mut h, 13);
    h.set_param("transform", 1.0); // rate 7 Hz, depth 0.8
    let (l, _) = run(&mut h, &x, None);
    let env = envelope(&l[BATCH * 100..], 240); // 5 ms
    let (hi, lo) = (
        env.iter().fold(0.0f64, |m, v| m.max(*v)),
        env.iter().fold(f64::MAX, |m, v| m.min(*v)),
    );
    // Gain swings between (1 - depth) and 1: a ratio of 0.2 (the compensation is common).
    assert!((lo / hi - 0.2).abs() < 0.04, "{}", lo / hi);
    // Cycles per second: count the crossings of the middle of the envelope.
    let mid = 0.5 * (hi + lo);
    let centred: Vec<f64> = env.iter().map(|v| v - mid).collect();
    let secs = env.len() as f64 * 0.005;
    let rate = count_sign_changes(&centred) as f64 / 2.0 / secs;
    assert!((rate - 7.0).abs() < 0.4, "{rate} Hz");
    // The two channels are offset in phase.
    let (l, r) = run(&mut on(13, 1.0), &x[..BATCH * 400], None);
    assert_ne!(l, r);
}

#[test]
fn spin_cycle_echo_wash_repeats_at_its_time_and_is_brought_in_by_transform() {
    let v = edited(
        &[
            ("spin-cycle,trem,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,hum,level_db,", "-120,-120,-120,lin,original,"),
            ("spin-cycle,water,rate_hz,", "0,0,0,lin,original,"),
            ("spin-cycle,echo,mod_depth_ms,", "0,0,0,lin,original,"),
            ("spin-cycle,chain,target_gain_db,", "0,0,0,lin,original,"),
        ],
        &[
            ("spin-cycle,slosh,2,room,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,1,out,", "0,0,0,lin,original,"),
        ],
    );
    let mut x = vec![0.0f32; BATCH * 1200];
    x[0] = 0.3;
    let render = |t: f32| {
        let mut h = dev_with(v.clone(), SR);
        set_preset(&mut h, 13);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        (l, latency(&h))
    };
    let (l, lat) = render(1.0);
    // Echo send gain 0.6 and fully wet: the first repeat 380 ms after the input, then a 0.55
    // repeat of it.
    let first = (lat as f64 + 0.380 * SR) as usize;
    let second = (lat as f64 + 0.760 * SR) as usize;
    let p1 = peak(&l[first - 12..first + 12]);
    let p2 = peak(&l[second - 12..second + 12]);
    // The 3 kHz low-pass of the slosh stage takes the top of the impulse.
    assert!(p1 > 0.02 && p1 < 0.18, "first repeat {p1}");
    assert!(
        peak(&l[lat + 9_600..first - 400]) < 0.05 * p1,
        "nothing before the repeat"
    );
    assert!(
        (p2 / p1 - 0.55).abs() < 0.12,
        "second over first {}",
        p2 / p1
    );
    // Transform 0 closes the send: no wash.
    let (l0, _) = render(0.0);
    assert!(peak(&l0[first - 12..first + 12]) < 1e-4);
}

#[test]
fn spin_cycle_resonant_box_decays_at_its_decay_time() {
    let v = edited(
        &[
            ("spin-cycle,trem,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,hum,level_db,", "-120,-120,-120,lin,original,"),
            ("spin-cycle,water,rate_hz,", "0,0,0,lin,original,"),
            ("spin-cycle,chain,target_gain_db,", "0,0,0,lin,original,"),
        ],
        &[
            ("spin-cycle,slosh,1,out,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,2,echo,", "0,0,0,lin,original,"),
        ],
    );
    let mut x = vec![0.0f32; BATCH * 400];
    x[0] = 0.5;
    let mut h = dev_with(v, SR);
    set_preset(&mut h, 13);
    h.set_param("transform", 0.5);
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    let rt = atomik_dsp::room::rt60_from_impulse(&l[lat..], SR as f32);
    // The table asks for 0.35 s (the small range's decay is within 30 %).
    assert!((rt - 0.35).abs() < 0.35 * 0.35, "rt60 {rt}");
}

#[test]
fn spin_cycle_hum_and_water_bursts_are_presence_gated_and_run_at_their_rates() {
    // Hum at Transform 1 (-30 dB, 55 Hz): the fundamental dominates the spectrum of silence-in
    // programme-out... run with a quiet programme so the gate is open.
    let prog = noise(BATCH * 750, 3, 0.002);
    let v = edited(
        &[
            ("spin-cycle,trem,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,depth,", "0,0,0,lin,original,"),
            ("spin-cycle,water,rate_hz,", "0,0,0,lin,original,"),
        ],
        &[
            ("spin-cycle,slosh,2,room,", "0,0,0,lin,original,"),
            ("spin-cycle,slosh,2,echo,", "0,0,0,lin,original,"),
        ],
    );
    let mut h = dev_with(v, SR);
    set_preset(&mut h, 13);
    h.set_param("transform", 1.0);
    let (l, _) = run(&mut h, &prog, None);
    let w = &l[BATCH * 400..];
    let h1 = tone_amp(w, 55.0);
    let h2 = tone_amp(w, 110.0);
    assert!(h1 > 0.01, "fundamental present: {h1}");
    assert!(
        (db(h2 / h1) - db(0.55)).abs() < 3.0,
        "second harmonic {}",
        db(h2 / h1)
    );
    // No programme: silent (the hum is gated).
    let (z, _) = run(&mut on(13, 1.0), &vec![0.0; BATCH * 100], None);
    assert!(z.iter().all(|v| v.abs() < f32::MIN_POSITIVE));
}

// --- ion-sweep -----------------------------------------------------------------------------------

#[test]
fn ion_sweep_transform_is_the_sweep_position_across_the_table_range() {
    // The spectral centroid of white noise through the preset follows Transform from the
    // bottom of the sweep (400 Hz, a 9 ms comb) to the top (8 kHz, a 0.3 ms comb).
    let x = white(BATCH * 750, 9, 0.1);
    let centroid = |t: f32| {
        let (l, _) = run(&mut on(14, t), &x, None);
        let sp = power_spectrum(&l[4_800..], 4096);
        let (mut num, mut den) = (0.0, 0.0);
        for (k, p) in sp.iter().enumerate() {
            let f = k as f64 * SR / 4096.0;
            num += f * p;
            den += p;
        }
        num / den
    };
    let (c0, c5, c1) = (centroid(0.0), centroid(0.5), centroid(1.0));
    assert!(c0 < 1_200.0, "{c0}");
    assert!(c1 > 4_500.0, "{c1}");
    assert!(c0 < c5 && c5 < c1, "monotone {c0} {c5} {c1}");
    // The table's own sweep range.
    assert!((table_value(14, "flange", "delay_ms", 0.0) - 9.0).abs() < 1e-3);
    assert!((table_value(14, "flange", "delay_ms", 1.0) - 0.3).abs() < 1e-3);
    assert!((table_value(14, "bp", "freq_hz", 0.0) - 400.0).abs() < 1.0);
    assert!((table_value(14, "bp", "freq_hz", 1.0) - 8_000.0).abs() < 1.0);
}

#[test]
fn ion_sweep_comb_delay_shows_in_the_autocorrelation_at_every_position() {
    // The band-pass is opened to its widest so the comb dominates.
    let v = edited(
        &[
            ("ion-sweep,bp,q,", "0.5,0.5,0.5,lin,original,"),
            ("ion-sweep,ring,amount,", "0,0,0,lin,original,"),
            ("ion-sweep,flange,detune,", "0,0,0,lin,original,"),
        ],
        &[],
    );
    let x = white(BATCH * 1000, 10, 0.1);
    for (t, d_ms) in [(0.0f32, 9.0f64), (0.5, 1.64), (1.0, 0.3)] {
        let mut h = dev_with(v.clone(), SR);
        set_preset(&mut h, 14);
        h.set_param("transform", t);
        let (l, _) = run(&mut h, &x, None);
        let lag = (d_ms * 0.001 * SR).round() as usize;
        let y = &l[BATCH * 200..];
        let (at, half, off) = (
            autocorr(y, lag),
            autocorr(y, lag / 2),
            autocorr(y, lag + lag / 3),
        );
        // A band-passed comb autocorrelates at its delay; the band-pass adds its own
        // correlation at short lags, so compare against the neighbouring lags.
        assert!(
            at > half.abs() + 0.02 || at > 0.3,
            "T {t}: at {at} half {half} off {off}"
        );
        assert!(at > off, "T {t}: {at} {off}");
    }
}

#[test]
fn ion_sweep_channels_are_offset_so_a_mono_source_has_width() {
    let x = white(BATCH * 600, 12, 0.1);
    let (l, r) = run(&mut on(14, 0.4), &x, None);
    assert_ne!(l, r);
}

// --- ghost-pipes ---------------------------------------------------------------------------------

/// Ghost-pipes with fixed root, decay, no detune, all twelve modes and a linear level.
fn ghost_fixed(root: f32, decay: f32, gain: f32) -> Voicing {
    edited(
        &[
            (
                "ghost-pipes,modal,root_hz,",
                &format!("{root},{root},{root},lin,original,"),
            ),
            (
                "ghost-pipes,modal,decay_s,",
                &format!("{decay},{decay},{decay},lin,original,"),
            ),
            ("ghost-pipes,modal,detune,", "0,0,0,lin,original,"),
            ("ghost-pipes,modal,active,", "12,12,12,lin,original,"),
            (
                "ghost-pipes,modal,gain,",
                &format!("{gain},{gain},{gain},lin,original,"),
            ),
            ("ghost-pipes,hp,freq_hz,", "20,20,20,lin,original,"),
            ("ghost-pipes,chain,target_gain_db,", "0,0,0,lin,original,"),
        ],
        &[],
    )
}

const RATIOS: [f64; 12] = [
    1.0, 1.59, 2.14, 2.30, 2.65, 2.92, 3.16, 3.50, 3.87, 4.21, 4.60, 5.02,
];

#[test]
fn ghost_pipes_rings_at_the_inharmonic_mode_frequencies_alternating_between_channels() {
    let root = 200.0f64;
    let mut h = dev_with(ghost_fixed(root as f32, 3.0, 1.0), SR);
    set_preset(&mut h, 15);
    let n = 1 << 16;
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    let (l, r) = run(&mut h, &x, None);
    let lat = latency(&h);
    let (wl, wr) = (
        &l[lat + 64..lat + 64 + 32_768],
        &r[lat + 64..lat + 64 + 32_768],
    );
    for (i, ratio) in RATIOS.iter().enumerate() {
        let f = root * ratio;
        let (a_l, a_r) = (tone_amp(wl, f), tone_amp(wr, f));
        // Even modes sound on the left, odd modes on the right.
        let (own, other) = if i % 2 == 0 { (a_l, a_r) } else { (a_r, a_l) };
        assert!(own > 20.0 * other, "mode {i}: {own} against {other}");
        assert!(own > 2e-6, "mode {i} rings: {own}");
    }
    // Every strong spectral peak sits on a mode.
    let sp = atomik_testkit::signal::magnitude_spectrum(
        &wl.iter().zip(wr).map(|(a, b)| a + b).collect::<Vec<_>>(),
    );
    let top = sp.iter().cloned().fold(0.0, f64::max);
    for k in 2..sp.len() - 2 {
        if sp[k] > 0.2 * top && sp[k] > sp[k - 1] && sp[k] >= sp[k + 1] {
            let f = k as f64 * SR / wl.len() as f64;
            let near = RATIOS.iter().any(|r| (f / (root * r) - 1.0).abs() < 0.004);
            assert!(near, "peak at {f} Hz is not a mode");
        }
    }
}

#[test]
fn ghost_pipes_decay_time_follows_the_table_and_modes_stop_at_the_active_count() {
    // Decay: the level of the first mode falls 60 dB per rt60 (the decay law of the bank).
    let root = 200.0f64;
    let decay = 1.0f32;
    let mut h = dev_with(ghost_fixed(root as f32, decay, 1.0), SR);
    set_preset(&mut h, 15);
    let mut x = vec![0.0f32; BATCH * 1500];
    x[0] = 1.0;
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    let at = |t_s: f64| {
        let s = lat + (t_s * SR) as usize;
        tone_amp(&l[s..s + 4800], root)
    };
    let (a1, a2) = (at(0.1), at(0.5));
    // rt60 of the first mode is `decay` (no taper at i = 0): 0.4 s of decay is 24 dB.
    assert!((db(a2 / a1) + 24.0).abs() < 3.0, "{}", db(a2 / a1));
    // Active count: the table has 4 active modes at Transform 0 and 12 at Transform 1.
    let v = edited(
        &[
            ("ghost-pipes,modal,root_hz,", "200,200,200,lin,original,"),
            ("ghost-pipes,modal,decay_s,", "3,3,3,lin,original,"),
            ("ghost-pipes,modal,detune,", "0,0,0,lin,original,"),
            ("ghost-pipes,modal,gain,", "1,1,1,lin,original,"),
            ("ghost-pipes,hp,freq_hz,", "20,20,20,lin,original,"),
        ],
        &[],
    );
    let amp_of_mode = |t: f32, i: usize| {
        let mut h = dev_with(v.clone(), SR);
        set_preset(&mut h, 15);
        h.set_param("transform", t);
        let mut x = vec![0.0f32; BATCH * 600];
        x[0] = 1.0;
        let (l, r) = run(&mut h, &x, None);
        let lat = latency(&h);
        let both: Vec<f32> = l[lat + 64..]
            .iter()
            .zip(&r[lat + 64..])
            .map(|(a, b)| a + b)
            .collect();
        tone_amp(&both[..24_000], 200.0 * RATIOS[i])
    };
    // Mode 7 (index) is silent with 4 active modes and rings with 12.
    let (a0, a8_t0, a8_t1) = (
        amp_of_mode(0.0, 0),
        amp_of_mode(0.0, 8),
        amp_of_mode(1.0, 8),
    );
    assert!(a0 > 1e-7, "{a0}");
    assert!(
        db(a8_t0 / a0) < -35.0,
        "mode 8 at 4 active modes: {}",
        db(a8_t0 / a0)
    );
    assert!(
        db(a8_t1 / a0) > -20.0,
        "mode 8 at 12 active modes: {}",
        db(a8_t1 / a0)
    );
}

#[test]
fn ghost_pipes_seeded_detune_spreads_the_modes_and_depends_on_the_seed() {
    let mut x = vec![0.0f32; BATCH * 300];
    x[0] = 0.01;
    let render = |seed: u64| {
        let mut h = DeviceHarness::with_options(Box::new(Character::new()), SR, 120.0, seed);
        set_preset(&mut h, 15);
        h.set_param("transform", 1.0);
        run(&mut h, &x, None).0
    };
    assert_eq!(render(2), render(2));
    assert_ne!(render(2), render(3));
}

// --- level, switches and robustness for the new rows -------------------------------------------

#[test]
fn rows_three_and_four_are_level_matched_and_bounded_like_the_first_two() {
    let x = pink(48_000 * 11 / BATCH * BATCH, 99, 0.1);
    for preset in 8..16 {
        for (t, tol) in [(0.5f32, 1.5f64), (0.0, 6.0), (1.0, 6.0)] {
            let mut h = dev();
            set_preset(&mut h, preset);
            h.set_param("transform", t);
            let (l, r) = run(&mut h, &x, None);
            let g = db(rms(&l[48_000..]).max(rms(&r[48_000..]))) - db(rms(&x[48_000..]));
            assert!(g.abs() <= tol, "preset {preset} T {t}: {g:.2} dB");
            assert!(peak(&l).max(peak(&r)) <= 8.0);
        }
    }
}

#[test]
fn every_switch_into_and_out_of_the_new_rows_is_click_free_and_leaves_no_tail() {
    // The all-pairs test in the parent module covers every ordered pair; here the tail of the
    // two long-memory presets (the resonator bank and the long echo) is cut after the fade.
    for from in [13usize, 15] {
        let mut h = dev();
        set_preset(&mut h, from);
        h.set_param("transform", 1.0);
        let burst = noise(BATCH * 300, 21, 0.8);
        let _ = run(&mut h, &burst, None);
        set_preset(&mut h, 1);
        let silence = vec![0.0f32; BATCH * 250];
        let (l, r) = run(&mut h, &silence, None);
        let late = (0.130 * SR) as usize;
        assert!(
            db(f64::from(peak(&l[late..]).max(peak(&r[late..])))) < -90.0,
            "preset {from}"
        );
    }
}

#[test]
fn rows_three_and_four_are_allocation_free_while_processing_and_switching() {
    let mut h = dev();
    let x = noise(BATCH, 5, 0.5);
    for preset in 0..16 {
        set_preset(&mut h, preset);
        for _ in 0..4 {
            h.input_mut("in_l").copy_from_slice(&x);
            assert_no_alloc(|| h.process());
        }
        h.set_param("transform", preset as f32 / 15.0);
        for _ in 0..2 {
            assert_no_alloc(|| h.process());
        }
    }
}

#[test]
fn state_round_trip_reproduces_the_render_for_every_preset() {
    // The same params on a fresh device give the same bounce (nothing hides outside the params).
    let x = noise(BATCH * 160, 77, 0.4);
    for preset in 8..16 {
        let setup = |h: &mut DeviceHarness| {
            set_preset(h, preset);
            for (k, v) in [
                ("gain", 0.6),
                ("transform", 0.7),
                ("mix", 0.8),
                ("volume", 0.45),
            ] {
                h.set_param(k, v);
            }
        };
        let mut a = dev();
        setup(&mut a);
        let first = run(&mut a, &x, None).0;
        let mut b = dev();
        setup(&mut b);
        assert_eq!(run(&mut b, &x, None).0, first, "preset {preset}");
        // And a reset device repeats itself.
        a.device_mut().reset();
        assert_eq!(run(&mut a, &x, None).0, first, "reset, preset {preset}");
    }
}

#[test]
fn the_new_rows_render_identically_on_several_threads() {
    let x = noise(BATCH * 160, 41, 0.5);
    for preset in 8..16 {
        let reference = render_preset(&mut dev(), preset, &x);
        for workers in [2usize, 4] {
            let outs: Vec<Vec<f32>> = std::thread::scope(|s| {
                (0..workers)
                    .map(|_| s.spawn(|| render_preset(&mut dev(), preset, &x)))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|j| j.join().unwrap())
                    .collect()
            });
            for o in outs {
                assert_eq!(o, reference, "preset {preset} on {workers} workers");
            }
        }
    }
}
