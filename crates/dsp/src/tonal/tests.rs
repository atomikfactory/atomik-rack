use super::*;
use crate::test_util::{noise, sine};

const SR: f32 = 48_000.0;

/// A signal whose band levels follow `level_db(centre)`: one sine at every band centre, each on
/// an exact bin of the analysis window.
fn comb(level_db: impl Fn(f32) -> f32, n: usize) -> Vec<f32> {
    let centres = band_centres();
    let mut out = vec![0.0f32; n];
    for f in &centres {
        let bin = (f64::from(*f) / f64::from(SR) * FFT_SIZE as f64)
            .round()
            .max(1.0);
        let hz = bin * f64::from(SR) / FFT_SIZE as f64;
        let amp = 10f32.powf(level_db(*f) / 20.0) * 0.1;
        let s = sine(hz, f64::from(SR), n, amp);
        for (o, v) in out.iter_mut().zip(s) {
            *o += v;
        }
    }
    out
}

/// How far a measured band may be from the formula: the lowest bands are narrower than the main
/// lobe of the window, so a tone in them reads a little low.
fn edge(band: usize) -> f32 {
    if band < 3 { 2.0 } else { 1.0 }
}

fn levels(x: &[f32]) -> [f32; BAND_COUNT] {
    band_levels_of(x, x, SR)
}

#[test]
fn the_regions_split_the_bands_at_the_documented_crossovers() {
    let counts: Vec<usize> = (0..REGIONS).map(|r| region_bands(r).len()).collect();
    assert_eq!(counts, [9, 8, 7, 7]);
    let centres = band_centres();
    assert!(centres[region_bands(0).end - 1] < 150.0);
    assert!(centres[region_bands(1).start] >= 150.0);
    assert!(centres[region_bands(1).end - 1] < 1000.0);
    assert!(centres[region_bands(2).start] >= 1000.0);
    assert!(centres[region_bands(2).end - 1] < 5000.0);
    assert!(centres[region_bands(3).start] >= 5000.0);
    assert_eq!(region_bands(3).end, BAND_COUNT);
}

#[test]
fn a_built_in_target_has_its_formula_and_a_normalised_total() {
    let centres = band_centres();
    let k = k_gains();
    for i in 0..BUILTIN_NAMES.len() {
        let t = builtin(i);
        let total: f64 = CORE_BANDS
            .map(|b| 10f64.powf(f64::from(t.bands[b]) / 10.0) * k[b])
            .sum();
        assert!((10.0 * total.log10()).abs() < 0.01, "{i}: {total}");
        // The shape is the formula up to one common offset.
        let raw = |b: usize| shape_db(&SHAPES[i], centres[b]);
        let offset = t.bands[0] - raw(0);
        for b in 0..BAND_COUNT {
            assert!((t.bands[b] - raw(b) - offset).abs() < 0.01, "{i} band {b}");
        }
    }
    // Balanced falls 4.5 dB an octave through the mids.
    let s = &SHAPES[0];
    let d = shape_db(s, 2000.0) - shape_db(s, 4000.0);
    assert!((d - 4.5).abs() < 0.2, "{d}");
}

#[test]
fn the_built_in_targets_differ_the_way_their_names_say() {
    let [bal, bright, warm, bass] = [0, 1, 2, 3].map(|i| builtin(i).regions());
    assert!(bright[3] > bal[3] + 1.0, "bright has more top");
    assert!(warm[3] < bal[3] - 1.0, "warm has less top");
    assert!(bass[0] > bal[0] + 1.0, "bass-heavy has more bass");
    for t in [builtin(0), builtin(3)] {
        assert!(t.tol.iter().all(|v| *v >= BUILTIN_TOL_DB));
        assert!(t.region_tol.iter().all(|v| *v == BUILTIN_TOL_DB));
    }
}

#[test]
fn a_signal_shaped_like_the_target_reads_centred_in_every_region() {
    let target = builtin(0);
    let x = comb(|f| shape_db(&SHAPES[0], f), FFT_SIZE);
    let r = compare(&levels(&x), &target).unwrap();
    for d in r.deviation {
        assert!(d.abs() < 1.0, "{:?}", r.deviation);
    }
    for i in 0..REGIONS {
        assert!(r.in_range(&target, i));
    }
}

#[test]
fn a_change_of_level_alone_does_not_move_the_regions() {
    let target = builtin(0);
    let x = comb(|f| shape_db(&SHAPES[0], f), FFT_SIZE);
    let quiet: Vec<f32> = x.iter().map(|v| v * 0.1).collect();
    let a = compare(&levels(&x), &target).unwrap();
    let b = compare(&levels(&quiet), &target).unwrap();
    for i in 0..REGIONS {
        assert!((a.deviation[i] - b.deviation[i]).abs() < 0.01);
    }
}

#[test]
fn a_boosted_low_end_reads_low_too_loud_and_leaves_the_core_regions_alone() {
    let target = builtin(0);
    let x = comb(
        |f| shape_db(&SHAPES[0], f) + if f < 150.0 { 9.0 } else { 0.0 },
        FFT_SIZE,
    );
    let r = compare(&levels(&x), &target).unwrap();
    assert!(r.deviation[0] > 3.0, "{:?}", r.deviation);
    assert!(!r.in_range(&target, 0));
    // The core regions anchor the level, so they stay where they were and the others do not move.
    for i in 1..REGIONS {
        assert!(r.deviation[i].abs() < 1.0, "{:?}", r.deviation);
    }
    // A boosted top reads HIGH too loud in the same way.
    let y = comb(
        |f| shape_db(&SHAPES[0], f) + if f >= 5000.0 { 8.0 } else { 0.0 },
        FFT_SIZE,
    );
    let r = compare(&levels(&y), &target).unwrap();
    assert!(
        r.deviation[3] > 3.0 && r.deviation[0].abs() < 1.0,
        "{:?}",
        r.deviation
    );
}

#[test]
fn pink_noise_reads_thin_in_the_bass_and_bright_against_balanced() {
    // Equal energy per octave: flat 1/3-octave levels.
    let x = comb(|_| 0.0, FFT_SIZE);
    let r = compare(&levels(&x), &builtin(0)).unwrap();
    assert!(r.deviation[0] < -3.0, "{:?}", r.deviation);
    assert!(r.deviation[3] > 3.0, "{:?}", r.deviation);
    // And against a flat target it is centred.
    let flat = Target {
        bands: normalise(&[0.0; BAND_COUNT]).unwrap(),
        tol: [3.0; BAND_COUNT],
        region_tol: [3.0; REGIONS],
    };
    let r = compare(&levels(&x), &flat).unwrap();
    for d in r.deviation {
        assert!(d.abs() < 1.0, "{:?}", r.deviation);
    }
}

#[test]
fn silence_reads_nothing() {
    assert!(normalise(&[FLOOR_DB; BAND_COUNT]).is_none());
    assert!(compare(&[FLOOR_DB; BAND_COUNT], &builtin(0)).is_none());
    let mut l = Learner::new();
    assert!(!l.push(&[FLOOR_DB; BAND_COUNT]));
    assert!(l.build().is_none());
}

#[test]
fn a_learner_returns_the_shape_it_saw_with_a_range_from_the_spread() {
    let shape = |f: f32| shape_db(&SHAPES[2], f);
    let x = comb(shape, FFT_SIZE);
    let mut l = Learner::new();
    for _ in 0..4 {
        assert!(l.push(&levels(&x)));
    }
    let t = l.build().unwrap();
    let want = builtin(2);
    for b in 0..BAND_COUNT {
        assert!((t.bands[b] - want.bands[b]).abs() < edge(b), "band {b}");
    }
    // No spread: the narrowest range.
    assert!(t.tol.iter().all(|v| (*v - MIN_TOL_DB).abs() < 1e-4));
    // A wide spread widens it, up to the limit.
    let mut l = Learner::new();
    for k in 0..6 {
        let boost = if k % 2 == 0 { 12.0 } else { -12.0 };
        let y = comb(|f| shape(f) + if f < 150.0 { boost } else { 0.0 }, FFT_SIZE);
        l.push(&levels(&y));
    }
    let wide = l.build().unwrap();
    assert!(
        wide.region_tol[0] > t.region_tol[0] + 2.0,
        "{:?}",
        wide.region_tol
    );
    assert!(wide.region_tol.iter().all(|v| *v <= MAX_TOL_DB));
    // One observation: the default.
    let mut one = Learner::new();
    one.push(&levels(&x));
    assert!(
        one.build()
            .unwrap()
            .region_tol
            .iter()
            .all(|v| *v == LONE_TOL_DB)
    );
}

#[test]
fn learning_from_a_file_of_known_spectrum_yields_that_shape() {
    // Eight seconds of a comb of sines shaped like the Bright target.
    let one = comb(|f| shape_db(&SHAPES[1], f), FFT_SIZE);
    let mut x = Vec::new();
    while x.len() < (8.0 * SR) as usize {
        x.extend_from_slice(&one);
    }
    let mut l = Learner::new();
    let mut last = 0.0;
    let n = learn_from_channels(&mut l, &x, &x, SR, |p| {
        last = p;
        true
    });
    assert!(n >= 2, "{n}");
    assert!(last > 0.9);
    let t = l.build().unwrap();
    let want = builtin(1);
    for b in 0..BAND_COUNT {
        assert!(
            (t.bands[b] - want.bands[b]).abs() < edge(b),
            "band {b}: {} {}",
            t.bands[b],
            want.bands[b]
        );
    }
}

#[test]
fn learning_stops_when_asked_and_skips_silence() {
    let silent = vec![0.0f32; (4.0 * SR) as usize];
    let mut l = Learner::new();
    assert_eq!(
        learn_from_channels(&mut l, &silent, &silent, SR, |_| true),
        0
    );
    let n = noise(5, (9.0 * SR) as usize, 0.3);
    let mut calls = 0;
    let taken = learn_from_channels(&mut l, &n, &n, SR, |_| {
        calls += 1;
        false
    });
    assert_eq!(calls, 1);
    assert_eq!(taken, 1);
    // Too short for one window.
    assert_eq!(
        learn_from_channels(&mut Learner::new(), &n[..1000], &n[..1000], SR, |_| true),
        0
    );
}

#[test]
fn learners_merge_like_one_long_observation_list() {
    let a = comb(|f| shape_db(&SHAPES[0], f), FFT_SIZE);
    let b = comb(|f| shape_db(&SHAPES[3], f), FFT_SIZE);
    let (mut l1, mut l2, mut both) = (Learner::new(), Learner::new(), Learner::new());
    l1.push(&levels(&a));
    l2.push(&levels(&b));
    both.push(&levels(&a));
    both.push(&levels(&b));
    l1.merge(&l2);
    assert_eq!(l1.len(), 2);
    assert_eq!(l1.build(), both.build());
}

#[test]
fn a_sanitised_target_is_finite_and_bounded() {
    let t = Target {
        bands: [f32::NAN; BAND_COUNT],
        tol: [f32::INFINITY; BAND_COUNT],
        region_tol: [-5.0; REGIONS],
    }
    .sanitised();
    assert!(t.bands.iter().all(|v| v.is_finite()));
    assert!(t.tol.iter().all(|v| (1.0..=12.0).contains(v)));
    assert!(t.region_tol.iter().all(|v| (1.0..=12.0).contains(v)));
}
