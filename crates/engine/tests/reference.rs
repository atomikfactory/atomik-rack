//! The reference voice through the public engine API: it replaces the mix after the master chain,
//! never allocates on the render path, hands its streams back, and an engine that was never told
//! about a reference renders exactly what it did before.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{PreviewFeed, ReferenceBand, ReferenceSettings, ReferenceSolo, new_engine};
use common::*;

fn tone(n: usize, amp: f32) -> Vec<Vec<f32>> {
    vec![(0..n).map(|i| (i as f32 * 0.05).sin() * amp).collect()]
}

fn new_stream(frames: usize) -> (PreviewFeed, atomik_engine::PreviewStream) {
    let (mut feed, stream) = atomik_engine::preview_channel();
    feed.set_source_rate(48_000);
    let src = tone(frames, 0.5);
    let mut from = 0;
    while from < frames {
        let n = feed.push(&src, from);
        assert!(n > 0);
        from += n;
    }
    (feed, stream)
}

fn on() -> ReferenceSettings {
    ReferenceSettings {
        listening: true,
        ..ReferenceSettings::default()
    }
}

#[test]
fn the_reference_never_allocates_or_frees_on_the_render_path() {
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let silence: [Vec<f32>; 0] = [];
    let render = |engine: &mut _, n: usize| {
        render_block(engine, &silence, 2, n);
    };
    let shared = handle.reference_shared();
    // Measure, start, solo a band, change gain, switch to another stream, stop, clear.
    handle
        .reference_set(ReferenceSettings {
            measure: true,
            ..ReferenceSettings::default()
        })
        .unwrap();
    render(&mut engine, 4_800);
    let (fa, a) = new_stream(30_000);
    handle
        .reference_start(
            a,
            ReferenceSettings {
                measure: true,
                ..on()
            },
        )
        .unwrap();
    render(&mut engine, 3_000);
    assert!(shared.loaded());
    handle
        .reference_set(ReferenceSettings {
            gain: 0.4,
            band: ReferenceBand::Mid,
            solo: ReferenceSolo::Side,
            measure: true,
            ..on()
        })
        .unwrap();
    render(&mut engine, 3_000);
    let (fb, b) = new_stream(30_000);
    handle.reference_start(b, on()).unwrap();
    render(&mut engine, 600);
    handle.collect_garbage();
    assert!(fa.is_abandoned(), "the cut stream is handed back");
    handle.reference_set(ReferenceSettings::default()).unwrap();
    render(&mut engine, 6_000);
    handle.collect_garbage();
    assert!(
        fb.is_abandoned(),
        "a stream that is not listened to is let go of"
    );
    assert!(!shared.loaded());
    let (_fc, c) = new_stream(1_000);
    handle.reference_start(c, on()).unwrap();
    render(&mut engine, 300);
    handle.reference_clear().unwrap();
    render(&mut engine, 300);
    handle.collect_garbage();
}

#[test]
fn listening_plays_the_reference_and_stopping_returns_to_silence() {
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let silence: [Vec<f32>; 0] = [];
    let (_feed, stream) = new_stream(40_000);
    handle.reference_start(stream, on()).unwrap();
    let out = render_block(&mut engine, &silence, 2, 6_000);
    // The engine has no routing, so the mix is silence and the end of the crossfade is the
    // reference itself, on both sides.
    let tail = &out[0][4_000..];
    let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.45 && peak < 0.55, "{peak}");
    assert_eq!(out[0][4_000..], out[1][4_000..]);
    handle.reference_set(ReferenceSettings::default()).unwrap();
    let out = render_block(&mut engine, &silence, 2, 6_000);
    assert!(out[0][3_000..].iter().all(|&v| v == 0.0));
}

#[test]
fn an_engine_without_a_reference_renders_what_it_always_did() {
    // Two engines, one of them told about a measuring-only reference voice: the audio is the
    // same bit for bit, and the unused one stays silent.
    let silence: [Vec<f32>; 0] = [];
    let (mut a, _ha) = new_engine(cfg(0, 2));
    let (mut b, mut hb) = new_engine(cfg(0, 2));
    hb.reference_set(ReferenceSettings {
        measure: true,
        ..ReferenceSettings::default()
    })
    .unwrap();
    let oa = render_block(&mut a, &silence, 2, 6_000);
    let ob = render_block(&mut b, &silence, 2, 6_000);
    assert_eq!(oa, ob);
    assert!(hb.reference_shared().short_term_lufs().is_some());
}
