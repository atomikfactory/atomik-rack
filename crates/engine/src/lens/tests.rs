//! The lens voice: transparency while off, the settings it applies, and no click on a change.

#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::BATCH;
use atomik_dsp::lens::Voicing;

const RATE: f64 = 48_000.0;

fn small() -> LensSettings {
    LensSettings {
        voicing: Voicing::SmallSpeaker,
        mono: false,
    }
}

fn sine(hz: f64, amp: f32) -> impl Fn(usize) -> f32 {
    move |i| amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin() as f32
}

/// `count` batches of `mix(frame)` through the voice; `at` runs before each batch.
fn run(
    v: &mut LensVoice,
    count: usize,
    mix: impl Fn(usize) -> (f32, f32),
    mut at: impl FnMut(&mut LensVoice, usize),
) -> Vec<(f32, f32)> {
    let mut out = vec![[0.0f32; BATCH]; 2];
    let mut all = Vec::new();
    for n in 0..count {
        at(v, n);
        let (left, right) = out.split_at_mut(1);
        for (i, (l, r)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
            (*l, *r) = mix(n * BATCH + i);
        }
        v.process(&mut out);
        all.extend(out[0].iter().copied().zip(out[1].iter().copied()));
    }
    all
}

fn peak(x: &[(f32, f32)]) -> f32 {
    x.iter().fold(0.0f32, |m, (l, _)| m.max(l.abs()))
}

#[test]
fn a_voice_that_was_never_set_leaves_the_mix_bit_for_bit() {
    let mut v = LensVoice::new(RATE);
    let f = sine(61.0, 0.7);
    let out = run(&mut v, 300, |i| (f(i), -f(i) * 0.5), |_, _| {});
    for (i, (l, r)) in out.iter().enumerate() {
        assert_eq!(l.to_bits(), f(i).to_bits());
        assert_eq!(r.to_bits(), (-f(i) * 0.5).to_bits());
    }
}

#[test]
fn headphones_after_a_voicing_is_bit_exact_again() {
    let mut v = LensVoice::new(RATE);
    let f = sine(80.0, 0.5);
    run(
        &mut v,
        300,
        |i| (f(i), f(i)),
        |v, n| {
            if n == 0 {
                v.set(small());
            }
            if n == 100 {
                v.set(LensSettings::OFF);
            }
        },
    );
    // The duck is long over: whatever comes next is untouched.
    let out = run(&mut v, 50, |i| (f(i), f(i)), |_, _| {});
    for (i, (l, _)) in out.iter().enumerate() {
        assert_eq!(l.to_bits(), f(i).to_bits());
    }
}

#[test]
fn the_small_speaker_takes_the_low_end_and_mono_sums_the_sides() {
    let mut v = LensVoice::new(RATE);
    let low = sine(50.0, 0.5);
    let out = run(
        &mut v,
        400,
        |i| (low(i), low(i)),
        |v, n| {
            if n == 0 {
                v.set(small());
            }
        },
    );
    assert!(peak(&out[out.len() - 4800..]) < 0.07, "{}", peak(&out));
    let mut v = LensVoice::new(RATE);
    let out = run(
        &mut v,
        100,
        |_| (1.0, -1.0),
        |v, n| {
            if n == 0 {
                v.set(LensSettings {
                    voicing: Voicing::Headphones,
                    mono: true,
                });
            }
        },
    );
    assert!(
        out[out.len() - 1000..]
            .iter()
            .all(|(l, r)| *l == 0.0 && *r == 0.0)
    );
}

#[test]
fn a_change_ducks_instead_of_clicking() {
    let mut v = LensVoice::new(RATE);
    let f = sine(1_000.0, 0.8);
    // A loud mid tone: the lens passes it, so only the swap could make a step.
    let out = run(
        &mut v,
        400,
        |i| (f(i), f(i)),
        |v, n| {
            if n == 100 {
                v.set(small());
            }
            if n == 250 {
                v.set(LensSettings {
                    voicing: Voicing::ClubSub,
                    mono: true,
                });
            }
        },
    );
    let step = out
        .windows(2)
        .fold(0.0f32, |m, w| m.max((w[1].0 - w[0].0).abs()));
    // The tone alone steps by 2 pi f A / rate = 0.105; the swap must not add to it.
    assert!(step < 0.15, "{step}");
    assert!(peak(&out[out.len() - 2000..]) > 0.5, "the sound is back");
}

#[test]
fn a_single_channel_is_left_alone() {
    let mut v = LensVoice::new(RATE);
    v.set(small());
    let mut out = vec![[0.3f32; BATCH]];
    v.process(&mut out);
    assert!(out[0].iter().all(|s| *s == 0.3));
}

#[test]
fn the_output_stays_finite_while_it_changes() {
    let mut v = LensVoice::new(RATE);
    let out = run(
        &mut v,
        600,
        |i| if i % 2 == 0 { (4.0, -4.0) } else { (-4.0, 4.0) },
        |v, n| {
            let voicings = [Voicing::SmallSpeaker, Voicing::ClubSub, Voicing::Headphones];
            v.set(LensSettings {
                voicing: voicings[(n / 7) % 3],
                mono: (n / 5) % 2 == 0,
            });
        },
    );
    assert!(out.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
}

#[test]
fn the_audio_thread_never_allocates_in_the_lens() {
    let mut v = LensVoice::new(RATE);
    let mut out = vec![[0.25f32; BATCH]; 2];
    atomik_testkit::no_alloc::assert_no_alloc(|| {
        for n in 0..200 {
            if n % 40 == 0 {
                v.set(LensSettings {
                    voicing: Voicing::ALL[(n / 40) % 3],
                    mono: n % 80 == 0,
                });
            }
            v.process(&mut out);
        }
    });
}
