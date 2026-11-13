//! The signal-processing separation on synthetic mixtures: a sine bass below the crossover,
//! noise-burst drums, a centred formant tone and a hard-panned pad. Each stem must follow its
//! source, the four must add up to the mix, and the run must be deterministic, cancellable and
//! safe on silence, mono and non-finite input.

use atomik_stems::{CHUNK_SECONDS, RATE, Stem, StemError, StemSink, separate, separate_in_chunks};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

const SECONDS: f64 = 8.0;

/// The least correlation a stem must have with its own source. A fast split with bleed is not a
/// learned model, but a stem that does not follow its own source at all (under 0.8) would be
/// useless to mix with. The synthetic mixtures measure 0.96 and up; the margin is for other
/// platforms' float rounding and later tuning.
const MIN_CORRELATION: f64 = 0.8;

fn len() -> usize {
    (SECONDS * f64::from(RATE)) as usize
}

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        })
        .collect()
}

/// The sources of the mixture, each as `[left, right]`.
struct Sources {
    bass: [Vec<f32>; 2],
    drums: [Vec<f32>; 2],
    vocals: [Vec<f32>; 2],
    other: [Vec<f32>; 2],
}

fn sources() -> Sources {
    let n = len();
    let t = |i: usize| i as f64 / f64::from(RATE);
    let bass: Vec<f32> = (0..n)
        .map(|i| {
            (0.35 * (2.0 * PI * 55.0 * t(i)).sin() + 0.2 * (2.0 * PI * 110.0 * t(i)).sin()) as f32
        })
        .collect();
    // A noise burst of 25 ms every 250 ms, decaying fast, a little left of centre.
    let hits = noise(n, 7);
    let burst: Vec<f32> = (0..n)
        .map(|i| {
            let since = t(i) % 0.25;
            if since < 0.025 {
                hits[i] * 0.6 * (-since * 120.0).exp() as f32
            } else {
                0.0
            }
        })
        .collect();
    let drums_r: Vec<f32> = burst.iter().map(|v| v * 0.8).collect();
    // A voice: harmonics of 200 Hz shaped by two formants.
    let vocals: Vec<f32> = (0..n)
        .map(|i| {
            let mut v = 0.0;
            for h in 1..=24 {
                let f = 200.0 * f64::from(h);
                let gain = (-(((f - 700.0) / 250.0).powi(2))).exp()
                    + 0.6 * (-(((f - 1_200.0) / 300.0).powi(2))).exp();
                v += gain * (2.0 * PI * f * t(i)).sin();
            }
            (0.12 * v) as f32
        })
        .collect();
    // A pad on the left only, between the voice's harmonics.
    let pad: Vec<f32> = (0..n)
        .map(|i| {
            (0.15 * (2.0 * PI * 500.0 * t(i)).sin() + 0.15 * (2.0 * PI * 900.0 * t(i)).sin()) as f32
        })
        .collect();
    Sources {
        bass: [bass.clone(), bass],
        drums: [burst, drums_r],
        vocals: [vocals.clone(), vocals],
        other: [pad, vec![0.0; n]],
    }
}

fn mix(s: &Sources) -> [Vec<f32>; 2] {
    let sum = |c: usize| -> Vec<f32> {
        (0..len())
            .map(|i| s.bass[c][i] + s.drums[c][i] + s.vocals[c][i] + s.other[c][i])
            .collect()
    };
    [sum(0), sum(1)]
}

/// Collects the stems whole.
#[derive(Default)]
struct Collect {
    parts: [[Vec<f32>; 2]; 4],
    pieces: usize,
}

impl StemSink for Collect {
    fn write(&mut self, stem: Stem, left: &[f32], right: &[f32]) -> Result<(), StemError> {
        let p = &mut self.parts[stem.index()];
        p[0].extend_from_slice(left);
        p[1].extend_from_slice(right);
        self.pieces += 1;
        Ok(())
    }
}

fn run(l: &[f32], r: &[f32], chunk: Option<f64>) -> Collect {
    let mut out = Collect::default();
    let cancel = AtomicBool::new(false);
    let mut progress = |_: f32| {};
    match chunk {
        Some(c) => separate_in_chunks(l, r, &Stem::ALL, c, &mut out, &cancel, &mut progress),
        None => separate(l, r, &Stem::ALL, &mut out, &cancel, &mut progress),
    }
    .expect("the separation runs");
    out
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len()) as f64;
    let (ma, mb) = (
        a.iter().map(|v| f64::from(*v)).sum::<f64>() / n,
        b.iter().map(|v| f64::from(*v)).sum::<f64>() / n,
    );
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x) - ma, f64::from(*y) - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa.sqrt() * bb.sqrt()).max(1e-30)
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v).powi(2)).sum()
}

#[test]
fn each_stem_follows_its_source() {
    let s = sources();
    let m = mix(&s);
    let out = run(&m[0], &m[1], None);
    let want = [
        (Stem::Vocals, &s.vocals[0]),
        (Stem::Drums, &s.drums[0]),
        (Stem::Bass, &s.bass[0]),
        (Stem::Other, &s.other[0]),
    ];
    // Each stem must also follow its own source clearly more than any other source.
    for (stem, source) in want {
        let got = &out.parts[stem.index()][0];
        let own = correlation(got, source);
        eprintln!("{} follows its source at {own:.3}", stem.label());
        assert!(own > MIN_CORRELATION, "{}: {own}", stem.label());
        for (other, other_source) in want {
            if other != stem {
                let cross = correlation(got, other_source).abs();
                assert!(
                    own > cross + 0.25,
                    "{} vs {}: {own} against {cross}",
                    stem.label(),
                    other.label()
                );
            }
        }
    }
}

#[test]
fn the_four_stems_sum_back_to_the_mix() {
    let s = sources();
    let m = mix(&s);
    for chunk in [None, Some(3.0)] {
        let out = run(&m[0], &m[1], chunk);
        for (c, mixed) in m.iter().enumerate() {
            for (i, want) in mixed.iter().enumerate() {
                let sum: f32 = (0..4).map(|k| out.parts[k][c][i]).sum();
                assert!(
                    (sum - want).abs() < 2e-4,
                    "chunk {chunk:?} channel {c} sample {i}: {sum} vs {want}"
                );
            }
        }
    }
}

#[test]
fn chunks_cover_the_song_once_in_order() {
    let s = sources();
    let m = mix(&s);
    let out = run(&m[0], &m[1], Some(3.0));
    for k in 0..4 {
        assert_eq!(out.parts[k][0].len(), len());
        assert_eq!(out.parts[k][1].len(), len());
    }
    assert!(out.pieces > 4, "it came in several pieces");
    assert!(
        CHUNK_SECONDS.to_bits() == 20.0f64.to_bits(),
        "a chunk is twenty seconds"
    );
}

#[test]
fn silence_gives_silence() {
    let z = vec![0.0f32; 20_000];
    let out = run(&z, &z, None);
    for k in 0..4 {
        assert_eq!(out.parts[k][0].len(), 20_000);
        assert!(
            out.parts[k][0]
                .iter()
                .chain(&out.parts[k][1])
                .all(|v| *v == 0.0)
        );
    }
}

#[test]
fn non_finite_input_is_silence_and_gives_no_nan() {
    let s = sources();
    let mut m = mix(&s);
    for i in (1000..4000).step_by(7) {
        m[0][i] = f32::NAN;
        m[1][i + 1] = f32::INFINITY;
        m[1][i + 2] = f32::NEG_INFINITY;
    }
    let out = run(&m[0], &m[1], Some(3.0));
    for k in 0..4 {
        assert!(
            out.parts[k][0]
                .iter()
                .chain(&out.parts[k][1])
                .all(|v| v.is_finite())
        );
    }
    let all_bad = vec![f32::NAN; 5000];
    let out = run(&all_bad, &all_bad, None);
    assert!(out.parts[0][0].iter().all(|v| *v == 0.0));
}

#[test]
fn the_same_input_gives_the_same_bits() {
    let s = sources();
    let m = mix(&s);
    let a = run(&m[0], &m[1], Some(3.0));
    let b = run(&m[0], &m[1], Some(3.0));
    let bits = |x: &[f32]| x.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    for k in 0..4 {
        for c in 0..2 {
            assert_eq!(
                bits(&a.parts[k][c]),
                bits(&b.parts[k][c]),
                "stem {k} channel {c}"
            );
        }
    }
}

#[test]
fn a_cancel_before_and_during_the_run_stops_it() {
    let s = sources();
    let m = mix(&s);
    let mut out = Collect::default();
    let cancel = AtomicBool::new(true);
    let r = separate(&m[0], &m[1], &Stem::ALL, &mut out, &cancel, &mut |_| {});
    assert_eq!(r, Err(StemError::Cancelled));
    assert_eq!(out.pieces, 0);
    // During: cancel from the progress callback once a third is done.
    let mut out = Collect::default();
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let r = separate_in_chunks(&m[0], &m[1], &Stem::ALL, 2.0, &mut out, &cancel, &mut |p| {
        seen.push(p);
        if p >= 0.3 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(r, Err(StemError::Cancelled));
    assert!(out.parts[0][0].len() < len(), "only a prefix was given");
    assert!(
        seen.windows(2).all(|w| w[0] <= w[1]),
        "progress only goes up: {seen:?}"
    );
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)));
}

#[test]
fn nothing_wanted_is_an_error() {
    let z = vec![0.0f32; 100];
    let mut out = Collect::default();
    let r = separate(&z, &z, &[], &mut out, &AtomicBool::new(false), &mut |_| {});
    assert_eq!(r, Err(StemError::NothingWanted));
}

#[test]
fn only_the_wanted_stems_reach_the_sink() {
    let s = sources();
    let m = mix(&s);
    let n = 3 * RATE as usize;
    let mut out = Collect::default();
    separate(
        &m[0][..n],
        &m[1][..n],
        &[Stem::Bass, Stem::Other],
        &mut out,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(out.parts[Stem::Bass.index()][0].len(), n);
    assert_eq!(out.parts[Stem::Other.index()][0].len(), n);
    assert!(out.parts[Stem::Vocals.index()][0].is_empty());
    assert!(out.parts[Stem::Drums.index()][0].is_empty());
}

#[test]
fn a_mono_input_works_and_gives_the_vocals_a_lower_weight() {
    let s = sources();
    let mono: Vec<f32> = (0..len())
        .map(|i| s.bass[0][i] + s.drums[0][i] + s.vocals[0][i])
        .collect();
    let out = run(&mono, &mono, None);
    for k in 0..4 {
        assert_eq!(
            out.parts[k][0], out.parts[k][1],
            "a mono input gives equal channels"
        );
    }
    for (i, want) in mono.iter().enumerate() {
        let sum: f32 = (0..4).map(|k| out.parts[k][0][i]).sum();
        assert!((sum - want).abs() < 2e-4);
    }
    let bass = correlation(&out.parts[Stem::Bass.index()][0], &s.bass[0]);
    assert!(bass > MIN_CORRELATION, "bass {bass}");
    let drums = correlation(&out.parts[Stem::Drums.index()][0], &s.drums[0]);
    assert!(drums > MIN_CORRELATION, "drums {drums}");
    // The voice is only the band, at a lower weight: what it gets is clearly less than the
    // voice's own energy, yet it is the voice and not the noise.
    let vocals = &out.parts[Stem::Vocals.index()][0];
    assert!(
        energy(vocals) < 0.6 * energy(&s.vocals[0]),
        "a lower weight"
    );
    assert!(correlation(vocals, &s.vocals[0]) > MIN_CORRELATION);
}

#[test]
fn a_hard_panned_sound_is_not_a_vocal() {
    let s = sources();
    let m = mix(&s);
    let out = run(&m[0], &m[1], None);
    let vocals = &out.parts[Stem::Vocals.index()][0];
    let pad_in_vocals = correlation(vocals, &s.other[0]).abs();
    assert!(pad_in_vocals < 0.3, "{pad_in_vocals}");
}

#[test]
fn different_lengths_use_the_shorter_channel_and_tiny_inputs_work() {
    let a = noise(30_000, 1);
    let b = noise(20_000, 2);
    let out = run(&a, &b, None);
    assert_eq!(out.parts[0][0].len(), 20_000);
    for n in [0usize, 1, 5, 700] {
        let x = noise(n, 3);
        let out = run(&x, &x, None);
        assert_eq!(out.parts[3][0].len(), n);
    }
}

/// Measures the speed on three minutes of the mixture, tiled: run it with
/// `cargo test --release -p atomik-stems --test separation -- --ignored --nocapture`.
#[test]
#[ignore = "a timing, for release builds"]
fn three_minutes_of_stereo_take_well_under_thirty_seconds() {
    let s = sources();
    let m = mix(&s);
    let tiles = (180.0 / SECONDS) as usize;
    let l: Vec<f32> = m[0].iter().copied().cycle().take(len() * tiles).collect();
    let r: Vec<f32> = m[1].iter().copied().cycle().take(len() * tiles).collect();
    #[allow(clippy::disallowed_methods)]
    let start = std::time::Instant::now();
    let out = run(&l, &r, None);
    let took = start.elapsed().as_secs_f64();
    eprintln!(
        "{:.0} s of stereo separated in {took:.2} s",
        l.len() as f64 / 44_100.0
    );
    assert_eq!(out.parts[0][0].len(), l.len());
    assert!(took < 30.0, "{took} s");
}
