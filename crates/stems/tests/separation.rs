//! The separation with a small network of fixed pseudo-random weights (the file format, the
//! chunking, the cancel, the determinism) and, when `ATOMIK_STEM_MODELS` names a folder with the
//! four published weight files, with the real models.

use atomik_stems::torch::{read_state, write_state};
use atomik_stems::{Models, Net, Stem, StemError, StemSink, separate};
use std::sync::atomic::{AtomicBool, Ordering};

const N_FFT: usize = 64;
const BINS: usize = N_FFT / 2 + 1;
const MAX_BIN: usize = 20;
const HIDDEN: usize = 16;

fn fill(len: usize, seed: u32, scale: f32) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * scale
        })
        .collect()
}

/// A small network file in the published format, with a sample rate of 100 Hz so that a few
/// thousand samples make several chunks.
fn small_net_file(seed: u32) -> Vec<u8> {
    let g = HIDDEN / 2;
    let mut t: Vec<(String, Vec<usize>, Vec<f32>)> = Vec::new();
    macro_rules! add {
        ($($a:expr),*) => { add_to(&mut t, seed, $($a),*) };
    }
    type Rows = Vec<(String, Vec<usize>, Vec<f32>)>;
    fn add_to(t: &mut Rows, seed: u32, name: &str, shape: &[usize], scale: f32, off: f32) {
        let n: usize = shape.iter().product();
        let v: Vec<f32> = fill(n, seed + t.len() as u32, scale)
            .into_iter()
            .map(|x| x + off)
            .collect();
        t.push((name.to_string(), shape.to_vec(), v));
    }
    add!("input_mean", &[MAX_BIN], 0.1, 0.0);
    add!("input_scale", &[MAX_BIN], 0.1, 1.0);
    add!("output_scale", &[BINS], 0.1, 1.0);
    add!("output_mean", &[BINS], 0.1, 0.5);
    t.push(("sample_rate".into(), vec![], vec![100.0]));
    t.push((
        "stft.window".into(),
        vec![N_FFT],
        (0..N_FFT)
            .map(|i| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N_FFT as f64).cos()) as f32
            })
            .collect(),
    ));
    add!("fc1.weight", &[HIDDEN, 2 * MAX_BIN], 0.3, 0.0);
    for bn in ["bn1", "bn2"] {
        add!(&format!("{bn}.weight"), &[HIDDEN], 0.2, 1.0);
        add!(&format!("{bn}.bias"), &[HIDDEN], 0.2, 0.0);
        add!(&format!("{bn}.running_mean"), &[HIDDEN], 0.2, 0.0);
        add!(&format!("{bn}.running_var"), &[HIDDEN], 0.2, 1.0);
    }
    for l in 0..2 {
        for suffix in ["", "_reverse"] {
            add!(
                &format!("lstm.weight_ih_l{l}{suffix}"),
                &[4 * g, HIDDEN],
                0.4,
                0.0
            );
            add!(
                &format!("lstm.weight_hh_l{l}{suffix}"),
                &[4 * g, g],
                0.4,
                0.0
            );
            add!(&format!("lstm.bias_ih_l{l}{suffix}"), &[4 * g], 0.2, 0.0);
            add!(&format!("lstm.bias_hh_l{l}{suffix}"), &[4 * g], 0.2, 0.0);
        }
    }
    add!("fc2.weight", &[HIDDEN, 2 * HIDDEN], 0.3, 0.0);
    add!("fc3.weight", &[2 * BINS, HIDDEN], 0.3, 0.0);
    add!("bn3.weight", &[2 * BINS], 0.2, 1.0);
    add!("bn3.bias", &[2 * BINS], 0.2, 0.3);
    add!("bn3.running_mean", &[2 * BINS], 0.2, 0.0);
    add!("bn3.running_var", &[2 * BINS], 0.2, 1.0);
    let refs: Vec<(&str, &[usize], &[f32])> = t
        .iter()
        .map(|(n, s, v)| (n.as_str(), s.as_slice(), v.as_slice()))
        .collect();
    write_state(&refs)
}

fn small_models() -> Models {
    let nets = (0..4)
        .map(|i| {
            let state = read_state(&small_net_file(100 * (i + 1))).expect("reads");
            Net::from_state(&state).expect("a network")
        })
        .collect();
    Models::from_nets(nets).expect("models")
}

#[derive(Default)]
struct Collect {
    out: Vec<(Stem, Vec<f32>, Vec<f32>)>,
}

impl Collect {
    fn stem(&self, stem: Stem) -> [Vec<f32>; 2] {
        let mut l = Vec::new();
        let mut r = Vec::new();
        for (s, a, b) in &self.out {
            if *s == stem {
                l.extend(a);
                r.extend(b);
            }
        }
        [l, r]
    }
}

impl StemSink for Collect {
    fn write(&mut self, stem: Stem, left: &[f32], right: &[f32]) -> Result<(), StemError> {
        self.out.push((stem, left.to_vec(), right.to_vec()));
        Ok(())
    }
}

fn run(models: &Models, l: &[f32], r: &[f32], wanted: &[Stem]) -> Collect {
    let mut sink = Collect::default();
    let cancel = AtomicBool::new(false);
    separate(models, l, r, wanted, &mut sink, &cancel, &mut |_| {}).expect("separates");
    sink
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn every_stem_covers_the_whole_song_once() {
    let m = small_models();
    for len in [10usize, 700, 2000, 2001, 7000] {
        let (l, r) = (fill(len, 1, 1.0), fill(len, 2, 1.0));
        let sink = run(&m, &l, &r, &Stem::ALL);
        for stem in Stem::ALL {
            let [a, b] = sink.stem(stem);
            assert_eq!((a.len(), b.len()), (len, len), "{stem:?} of {len}");
        }
    }
}

#[test]
fn the_four_stems_add_up_to_the_mix() {
    let m = small_models();
    let len = 7000;
    let (l, r) = (fill(len, 3, 1.0), fill(len, 4, 1.0));
    let sink = run(&m, &l, &r, &Stem::ALL);
    let parts: Vec<[Vec<f32>; 2]> = Stem::ALL.iter().map(|s| sink.stem(*s)).collect();
    for i in 0..len {
        let sum_l: f32 = parts.iter().map(|p| p[0][i]).sum();
        let sum_r: f32 = parts.iter().map(|p| p[1][i]).sum();
        assert!((sum_l - l[i]).abs() < 2e-3, "left {i}: {sum_l} vs {}", l[i]);
        assert!(
            (sum_r - r[i]).abs() < 2e-3,
            "right {i}: {sum_r} vs {}",
            r[i]
        );
    }
}

#[test]
fn only_the_wanted_stems_reach_the_sink() {
    let m = small_models();
    let (l, r) = (fill(3000, 5, 1.0), fill(3000, 6, 1.0));
    let sink = run(&m, &l, &r, &[Stem::Bass, Stem::Vocals]);
    assert!(
        sink.out
            .iter()
            .all(|(s, _, _)| matches!(s, Stem::Bass | Stem::Vocals))
    );
    assert!(sink.stem(Stem::Drums)[0].is_empty());
    assert_eq!(sink.stem(Stem::Bass)[0].len(), 3000);
}

#[test]
fn silence_gives_exact_silence() {
    let m = small_models();
    let z = vec![0f32; 5000];
    let sink = run(&m, &z, &z, &Stem::ALL);
    for stem in Stem::ALL {
        let [a, b] = sink.stem(stem);
        assert!(a.iter().chain(&b).all(|v| *v == 0.0), "{stem:?}");
        assert_eq!(a.len(), 5000);
    }
}

#[test]
fn non_finite_input_gives_finite_output() {
    let m = small_models();
    let mut l = fill(4000, 7, 1.0);
    let mut r = fill(4000, 8, 1.0);
    l[100] = f32::NAN;
    l[2500] = f32::INFINITY;
    r[3000] = f32::NEG_INFINITY;
    let sink = run(&m, &l, &r, &Stem::ALL);
    for stem in Stem::ALL {
        let [a, b] = sink.stem(stem);
        assert!(a.iter().chain(&b).all(|v| v.is_finite()), "{stem:?}");
    }
}

#[test]
fn the_same_input_gives_the_same_bits() {
    let m = small_models();
    let (l, r) = (fill(6000, 9, 1.0), fill(6000, 10, 1.0));
    let a = run(&m, &l, &r, &Stem::ALL);
    let b = run(&m, &l, &r, &Stem::ALL);
    for stem in Stem::ALL {
        let (x, y) = (a.stem(stem), b.stem(stem));
        assert_eq!(bits(&x[0]), bits(&y[0]));
        assert_eq!(bits(&x[1]), bits(&y[1]));
    }
}

#[test]
fn chunking_does_not_leave_a_seam() {
    // The same loop of a signal, one stretch long enough for several chunks: the output right at
    // a chunk boundary stays close to its neighbours (no jump).
    let m = small_models();
    let len = 9000;
    let l: Vec<f32> = (0..len).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let r = l.clone();
    let sink = run(&m, &l, &r, &[Stem::Other]);
    let [a, _] = sink.stem(Stem::Other);
    for boundary in [2000usize, 4000, 6000, 8000] {
        let jump = (a[boundary] - a[boundary - 1]).abs();
        let around = (a[boundary + 20] - a[boundary + 19]).abs() + 0.05;
        assert!(jump < 4.0 * around, "jump {jump} at {boundary}");
    }
}

#[test]
fn a_cancel_stops_the_run_and_reports_it() {
    let m = small_models();
    let (l, r) = (fill(20_000, 11, 1.0), fill(20_000, 12, 1.0));
    let cancel = AtomicBool::new(false);
    let mut sink = Collect::default();
    let mut chunks = 0;
    let result = separate(&m, &l, &r, &Stem::ALL, &mut sink, &cancel, &mut |p| {
        if p > 0.0 {
            chunks += 1;
            cancel.store(true, Ordering::SeqCst);
        }
    });
    assert_eq!(result, Err(StemError::Cancelled));
    assert_eq!(chunks, 1, "it stopped after the chunk that was running");
    // Cancelled before the start: nothing at all is written.
    let mut sink = Collect::default();
    let result = separate(&m, &l, &r, &Stem::ALL, &mut sink, &cancel, &mut |_| {});
    assert_eq!(result, Err(StemError::Cancelled));
    assert!(sink.out.is_empty());
}

#[test]
fn progress_rises_to_one() {
    let m = small_models();
    let (l, r) = (fill(9000, 13, 1.0), fill(9000, 14, 1.0));
    let cancel = AtomicBool::new(false);
    let mut seen: Vec<f32> = Vec::new();
    separate(
        &m,
        &l,
        &r,
        &[Stem::Vocals],
        &mut Collect::default(),
        &cancel,
        &mut |p| seen.push(p),
    )
    .expect("separates");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(*seen.last().expect("progress"), 1.0);
    assert!(seen.len() >= 4);
}

#[test]
fn nothing_wanted_is_an_error() {
    let m = small_models();
    let cancel = AtomicBool::new(false);
    let z = vec![0f32; 100];
    let r = separate(
        &m,
        &z,
        &z,
        &[],
        &mut Collect::default(),
        &cancel,
        &mut |_| {},
    );
    assert_eq!(r, Err(StemError::NothingWanted));
}

#[test]
fn a_network_that_does_not_fit_is_refused() {
    let mut state = read_state(&small_net_file(1)).expect("reads");
    state.get_mut("fc2.weight").expect("fc2").shape = vec![3, 3];
    assert!(matches!(
        Net::from_state(&state),
        Err(StemError::Weights(_))
    ));
    state.remove("fc2.weight");
    assert!(matches!(
        Net::from_state(&state),
        Err(StemError::Weights(_))
    ));
}

// ---- the real models -------------------------------------------------------------------------

fn real_models() -> Option<Models> {
    let dir = std::env::var_os("ATOMIK_STEM_MODELS")?;
    Some(Models::load(std::path::Path::new(&dir)).expect("the real models load"))
}

const RATE: f32 = 44_100.0;

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
    for i in 0..n {
        ab += f64::from(a[i]) * f64::from(b[i]);
        aa += f64::from(a[i]).powi(2);
        bb += f64::from(b[i]).powi(2);
    }
    (ab / (aa.sqrt() * bb.sqrt() + 1e-12)) as f32
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v).powi(2)).sum()
}

/// The three sources: a bass line below 150 Hz, a pattern of noise bursts (drums), a vowel-like
/// tone with formants and vibrato (voice). Returns (bass, drums, voice) as mono signals.
fn sources(seconds: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let n = (seconds * RATE) as usize;
    let mut s = 12345u32;
    let mut noise = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let bass: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE;
            let note = [55.0f32, 55.0, 73.4, 82.4][(t * 2.0) as usize % 4];
            0.5 * (2.0 * std::f32::consts::PI * note * t).sin()
        })
        .collect();
    let drums: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE;
            let beat = (t * 4.0).fract();
            let env = (-beat * 40.0).exp();
            let kick = if ((t * 4.0) as usize).is_multiple_of(2) {
                (2.0 * std::f32::consts::PI * 60.0 * beat * 0.1).sin()
            } else {
                0.0
            };
            0.6 * env * (noise() * 0.8 + kick * 0.2)
        })
        .collect();
    let voice: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE;
            let f0 = 220.0 * (1.0 + 0.01 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
            let mut v = 0.0;
            for h in 1..=20 {
                let f = f0 * h as f32;
                // Two formants near 700 Hz and 1200 Hz.
                let g = (-((f - 700.0) / 250.0).powi(2)).exp()
                    + 0.6 * (-((f - 1200.0) / 300.0).powi(2)).exp();
                v += g / h as f32 * (2.0 * std::f32::consts::PI * f * t).sin();
            }
            0.35 * v
        })
        .collect();
    (bass, drums, voice)
}

#[test]
fn the_real_models_separate_synthetic_sources() {
    let Some(models) = real_models() else {
        eprintln!("skipped: ATOMIK_STEM_MODELS is not set");
        return;
    };
    let (bass, drums, voice) = sources(12.0);
    let n = bass.len();
    let l: Vec<f32> = (0..n)
        .map(|i| bass[i] + 0.9 * drums[i] + 0.6 * voice[i])
        .collect();
    let r: Vec<f32> = (0..n)
        .map(|i| bass[i] + 0.9 * drums[i] + 0.6 * voice[i])
        .collect();
    let sink = run(&models, &l, &r, &Stem::ALL);
    let s = |st| sink.stem(st)[0].clone();
    let (sb, sd, sv, so) = (
        s(Stem::Bass),
        s(Stem::Drums),
        s(Stem::Vocals),
        s(Stem::Other),
    );
    let corr = |a: &[f32], b: &[f32]| correlation(&a[44_100..n - 44_100], &b[44_100..n - 44_100]);
    eprintln!(
        "bass~bass {:.2} drums~drums {:.2} vocals~voice {:.2} | energies b {:.1} d {:.1} v {:.1} o {:.1}",
        corr(&sb, &bass),
        corr(&sd, &drums),
        corr(&sv, &voice),
        energy(&sb),
        energy(&sd),
        energy(&sv),
        energy(&so)
    );
    // A sanity check, not a quality benchmark: each stem hears its source more than the others.
    assert!(corr(&sb, &bass) > 0.5, "bass stem vs the bass");
    assert!(corr(&sb, &bass) > corr(&sb, &drums));
    assert!(corr(&sd, &drums) > corr(&sd, &bass));
    // The four add up to the mix.
    let sum: Vec<f32> = (0..n).map(|i| sb[i] + sd[i] + sv[i] + so[i]).collect();
    assert!(correlation(&sum, &l) > 0.99);
}

#[test]
fn the_real_models_are_deterministic_and_keep_silence() {
    let Some(models) = real_models() else {
        eprintln!("skipped: ATOMIK_STEM_MODELS is not set");
        return;
    };
    let (bass, drums, voice) = sources(3.0);
    let mix: Vec<f32> = (0..bass.len())
        .map(|i| bass[i] + drums[i] + voice[i])
        .collect();
    let a = run(&models, &mix, &mix, &[Stem::Vocals, Stem::Bass]);
    let b = run(&models, &mix, &mix, &[Stem::Vocals, Stem::Bass]);
    assert_eq!(
        bits(&a.stem(Stem::Vocals)[0]),
        bits(&b.stem(Stem::Vocals)[0])
    );
    assert_eq!(bits(&a.stem(Stem::Bass)[1]), bits(&b.stem(Stem::Bass)[1]));
    let z = vec![0f32; 44_100];
    let sink = run(&models, &z, &z, &[Stem::Drums]);
    assert!(sink.stem(Stem::Drums)[0].iter().all(|v| *v == 0.0));
}

/// The speed of a whole song: set `ATOMIK_STEM_SECONDS` (default 20) and run with `--nocapture`
/// in a release build.
#[test]
fn the_real_models_speed() {
    let Some(models) = real_models() else {
        eprintln!("skipped: ATOMIK_STEM_MODELS is not set");
        return;
    };
    let seconds: f32 = std::env::var("ATOMIK_STEM_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let (bass, drums, voice) = sources(seconds);
    let mix: Vec<f32> = (0..bass.len())
        .map(|i| bass[i] + drums[i] + voice[i])
        .collect();
    #[allow(clippy::disallowed_methods)]
    let started = std::time::Instant::now();
    let sink = run(&models, &mix, &mix, &Stem::ALL);
    #[allow(clippy::disallowed_methods)]
    let took = started.elapsed().as_secs_f32();
    eprintln!(
        "separated {seconds} s of stereo in {took:.1} s ({:.2}x real time)",
        seconds / took
    );
    assert_eq!(sink.stem(Stem::Other)[0].len(), mix.len());
}
