//! The reference voice: transparency, the crossfade, the gain, the solos and the meter.

#![allow(clippy::float_cmp)]

use super::*;
use crate::preview::preview_channel;
use atomik_core::BATCH;

const RATE: f64 = 48_000.0;

fn voice() -> (ReferenceVoice, Arc<ReferenceShared>) {
    let shared = Arc::new(ReferenceShared::default());
    (ReferenceVoice::new(RATE, Arc::clone(&shared)), shared)
}

/// A reference stream of `frames` frames of `left(frame)` and `right(frame)`, fed in full (the
/// ring is large enough for the lengths used here) and finished.
fn stream(
    frames: usize,
    left: impl Fn(usize) -> f32,
    right: impl Fn(usize) -> f32,
) -> PreviewStream {
    let (mut feed, stream) = preview_channel();
    feed.set_source_rate(RATE as u32);
    let l: Vec<f32> = (0..frames).map(&left).collect();
    let r: Vec<f32> = (0..frames).map(&right).collect();
    let planar = vec![l, r];
    assert_eq!(feed.push(&planar, 0), frames);
    feed.finish();
    // The feed goes out of scope here; the stream keeps the data and the flags.
    drop(feed);
    stream
}

fn batches(
    v: &mut ReferenceVoice,
    count: usize,
    mix: impl Fn(usize) -> (f32, f32),
) -> Vec<(f32, f32)> {
    let mut out = vec![[0.0f32; BATCH]; 2];
    let mut all = Vec::new();
    for n in 0..count {
        let (left, right) = out.split_at_mut(1);
        for (i, (l, r)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
            (*l, *r) = mix(n * BATCH + i);
        }
        v.process(&mut out);
        all.extend(out[0].iter().copied().zip(out[1].iter().copied()));
    }
    all
}

fn settings(listening: bool) -> ReferenceSettings {
    ReferenceSettings {
        listening,
        ..ReferenceSettings::default()
    }
}

fn sine(hz: f64, amp: f32) -> impl Fn(usize) -> f32 {
    move |n| (f64::from(amp) * (std::f64::consts::TAU * hz * n as f64 / RATE).sin()) as f32
}

#[test]
fn an_idle_voice_leaves_the_outputs_bit_for_bit() {
    let (mut v, _) = voice();
    let mix = |n: usize| {
        (
            if n.is_multiple_of(3) { -0.0 } else { 0.3 },
            0.1 * (n % 7) as f32,
        )
    };
    let out = batches(&mut v, 4, mix);
    for (n, (l, r)) in out.iter().enumerate() {
        let (el, er) = mix(n);
        assert_eq!(l.to_bits(), el.to_bits());
        assert_eq!(r.to_bits(), er.to_bits());
    }
    // Loaded but not listening: the stream is let go of after the first batch and the mix
    // still passes untouched.
    let (mut v, shared) = voice();
    v.load(stream(4_000, |_| 0.9, |_| 0.9));
    assert!(shared.loaded());
    let out = batches(&mut v, 4, |_| (0.25, 0.25));
    assert!(out.iter().all(|&(l, r)| l == 0.25 && r == 0.25));
    assert!(v.take_dead().is_some());
    assert!(!shared.loaded());
}

#[test]
fn listening_crossfades_to_the_reference_and_back() {
    let (mut v, _) = voice();
    v.load(stream(48_000, |_| 0.5, |_| 0.5));
    v.set(settings(true));
    let out = batches(&mut v, 40, |_| (0.25, 0.25));
    // The first frame is still the mix; the end of the 30 ms crossfade is the reference.
    assert!((out[0].0 - 0.25).abs() < 0.01, "{}", out[0].0);
    assert!((out.last().unwrap().0 - 0.5).abs() < 1e-3);
    // The path between is monotonic: nothing overshoots.
    assert!(out.windows(2).all(|w| w[1].0 >= w[0].0 - 1e-6));
    v.set(settings(false));
    let out = batches(&mut v, 40, |_| (0.25, 0.25));
    assert!((out.last().unwrap().0 - 0.25).abs() < 1e-6);
    assert!(v.take_dead().is_some(), "the stream is let go of");
}

#[test]
fn the_reference_gain_glides_and_applies() {
    let (mut v, _) = voice();
    v.load(stream(60_000, |_| 0.5, |_| 0.5));
    v.set(ReferenceSettings {
        gain: 0.5,
        ..settings(true)
    });
    let out = batches(&mut v, 60, |_| (0.0, 0.0));
    assert!((out.last().unwrap().0 - 0.25).abs() < 1e-3);
    // A new gain moves smoothly: the very next frame is within a hair of the old level.
    v.set(ReferenceSettings {
        gain: 1.0,
        ..settings(true)
    });
    let out = batches(&mut v, 1, |_| (0.0, 0.0));
    assert!((out[0].0 - 0.25).abs() < 1e-3);
    let out = batches(&mut v, 400, |_| (0.0, 0.0));
    assert!((out.last().unwrap().0 - 0.5).abs() < 0.01);
}

#[test]
fn a_stream_that_ends_is_let_go_of_and_marked_done() {
    let (mut v, shared) = voice();
    let s = stream(500, |_| 0.5, |_| 0.5);
    let monitor = s.monitor();
    v.load(s);
    v.set(settings(true));
    batches(&mut v, 20, |_| (0.0, 0.0));
    assert!(!shared.loaded());
    assert!(monitor.is_done());
    assert!(v.take_dead().is_some());
}

#[test]
fn a_band_is_applied_to_the_mix_and_the_reference_alike() {
    let (mut v, _) = voice();
    v.load(stream(60_000, sine(5_000.0, 0.5), sine(5_000.0, 0.5)));
    v.set(ReferenceSettings {
        band: ReferenceBand::Sub,
        ..settings(false)
    });
    // A 5 kHz mix under a sub band is silent once the duck has swapped the filters.
    let out = batches(&mut v, 60, |n| {
        (sine(5_000.0, 0.5)(n), sine(5_000.0, 0.5)(n))
    });
    let tail = &out[out.len() - 480..];
    assert!(tail.iter().all(|&(l, _)| l.abs() < 1e-3), "mix");
    // Listening: the reference is filtered the same way.
    v.set(ReferenceSettings {
        band: ReferenceBand::Sub,
        ..settings(true)
    });
    let out = batches(&mut v, 60, |_| (0.0, 0.0));
    let tail = &out[out.len() - 480..];
    assert!(tail.iter().all(|&(l, _)| l.abs() < 1e-3), "reference");
    // A 50 Hz tone passes the sub band nearly whole.
    let (mut w, _) = voice();
    w.set(ReferenceSettings {
        band: ReferenceBand::Sub,
        ..settings(false)
    });
    let out = batches(&mut w, 400, |n| (sine(50.0, 0.5)(n), sine(50.0, 0.5)(n)));
    let peak = out[out.len() - 2_000..]
        .iter()
        .fold(0.0f32, |m, &(l, _)| m.max(l.abs()));
    assert!(peak > 0.4 && peak < 0.55, "{peak}");
}

#[test]
fn mid_and_side_solo_play_on_both_sides() {
    let run = |solo, l: f32, r: f32| {
        let (mut v, _) = voice();
        v.set(ReferenceSettings {
            solo,
            ..settings(false)
        });
        let out = batches(&mut v, 40, |_| (l, r));
        *out.last().unwrap()
    };
    assert_eq!(run(ReferenceSolo::Mid, 0.5, -0.5), (0.0, 0.0));
    assert_eq!(run(ReferenceSolo::Mid, 0.25, 0.75), (0.5, 0.5));
    assert_eq!(run(ReferenceSolo::Side, 0.5, 0.5), (0.0, 0.0));
    assert_eq!(run(ReferenceSolo::Side, 0.75, 0.25), (0.25, 0.25));
    assert_eq!(run(ReferenceSolo::Off, 0.75, 0.25), (0.75, 0.25));
}

#[test]
fn a_change_of_solo_ducks_instead_of_clicking() {
    let (mut v, _) = voice();
    let steady = batches(&mut v, 4, |_| (0.5, 0.5));
    assert_eq!(steady.last().unwrap().0, 0.5);
    v.set(ReferenceSettings {
        solo: ReferenceSolo::Mid,
        ..settings(false)
    });
    let out = batches(&mut v, 10, |_| (0.5, 0.5));
    // Down to nothing, then back up; no step larger than a few percent per frame.
    let low = out.iter().fold(1.0f32, |m, &(l, _)| m.min(l));
    assert!(low < 0.01, "{low}");
    assert!(out.windows(2).all(|w| (w[1].0 - w[0].0).abs() < 0.05));
    assert!((out.last().unwrap().0 - 0.5).abs() < 1e-6);
}

#[test]
fn the_meter_reads_the_short_term_loudness_of_the_mix() {
    let (mut v, shared) = voice();
    assert_eq!(shared.short_term_lufs(), None);
    v.set(ReferenceSettings {
        measure: true,
        ..settings(false)
    });
    // A stereo 1 kHz sine of amplitude 0.1 reads about -20 LUFS once 3 s have gone by.
    let count = (4.0 * RATE / BATCH as f64) as usize;
    batches(&mut v, count, |n| {
        (sine(1_000.0, 0.1)(n), sine(1_000.0, 0.1)(n))
    });
    let lufs = shared.short_term_lufs().expect("measured");
    assert!((lufs + 20.0).abs() < 0.6, "{lufs}");
    // The meter does not change the audio.
    let out = batches(&mut v, 2, |_| (0.3, 0.2));
    assert!(out.iter().all(|&(l, r)| l == 0.3 && r == 0.2));
    // Silence reads the floor after the window has emptied.
    batches(&mut v, count, |_| (0.0, 0.0));
    assert_eq!(shared.short_term_lufs(), Some(REFERENCE_FLOOR_LUFS));
    // Switching it off forgets the reading.
    v.set(settings(false));
    assert_eq!(shared.short_term_lufs(), None);
}
