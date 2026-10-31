//! Speed of the DSP primitives the devices are built from: cost of a 4,096-sample block through a
//! smoothed biquad, a state-variable filter, a chorus and a compressor (stereo where the
//! primitive is). Run: `cargo bench -p atomik-dsp --bench blocks`. Each case prints criterion's
//! report and a line `median .. p99 ..` over its timed runs; `ATOMIK_BENCH_ONCE` runs each case
//! once (for an instruction counter).
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_dsp::{BiquadShape, Chorus, Compressor, FilterMode, Svf};
use atomik_testkit::bench::{Recorder, once_mode, time_once};
use criterion::Criterion;
use std::time::Duration;

const SR: f32 = 48_000.0;
const BLOCK: usize = 4096;

/// A deterministic noise block in -0.5..0.5.
fn noise(seed: u32) -> Vec<f32> {
    let mut x = seed;
    (0..BLOCK)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        })
        .collect()
}

type Case = (&'static str, Box<dyn FnMut() -> f32>);

fn cases() -> Vec<Case> {
    let (nl, nr) = (noise(1), noise(2));
    let mut biquad = atomik_dsp::SmoothedBiquad::new(BiquadShape::Peaking, 1_000.0, 1.0, 6.0, SR);
    let (l, r) = (nl.clone(), nr.clone());
    let biquad_case: Case = (
        "biquad-stereo",
        Box::new(move || {
            let mut acc = 0.0;
            for (a, b) in l.iter().zip(&r) {
                let (x, y) = biquad.process(*a, *b);
                acc += x + y;
            }
            acc
        }),
    );
    let mut svf = Svf::new();
    svf.set_params(2_000.0, 0.4, SR);
    let l = nl.clone();
    let svf_case: Case = (
        "svf-lowpass",
        Box::new(move || {
            l.iter()
                .map(|x| svf.process_mode(*x, FilterMode::Lowpass))
                .sum()
        }),
    );
    let mut chorus = Chorus::new(SR, 40.0);
    chorus.set_mix(0.5);
    let (l, r) = (nl.clone(), nr.clone());
    let chorus_case: Case = (
        "chorus-stereo",
        Box::new(move || {
            let (mut a, mut b) = (l.clone(), r.clone());
            chorus.process_block(&mut a, &mut b);
            a[BLOCK - 1] + b[BLOCK - 1]
        }),
    );
    let mut comp = Compressor::new(SR);
    comp.set_threshold_db(-24.0);
    let (l, r) = (nl, nr);
    let comp_case: Case = (
        "compressor-stereo",
        Box::new(move || l.iter().zip(&r).map(|(a, b)| comp.process(*a, *b)).sum()),
    );
    vec![biquad_case, svf_case, chorus_case, comp_case]
}

fn main() {
    let mut all = cases();
    if once_mode() {
        for (name, f) in &mut all {
            println!("{name}: once {:.6} s", time_once(f));
        }
        return;
    }
    let mut c = Criterion::default()
        .sample_size(21)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500))
        .configure_from_args();
    for (name, f) in &mut all {
        let id = format!("dsp/{name}/{BLOCK}");
        let rec = Recorder::new();
        c.bench_function(&id, |b| b.iter_custom(rec.runner(&mut *f)));
        rec.report(&id);
    }
    c.final_summary();
}
