//! The translation lens through the public engine API. The engine has no routing here, so the mix is
//! silence and the sound is a reference stream, which plays before the lens: the lens colours what
//! the user hears. The lens never allocates on the render path, and an engine that was never told
//! about it (every offline render, bounce and export) renders what it always did, bit for bit.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{LensSettings, LensVoicing, PreviewFeed, ReferenceSettings, new_engine};
use common::*;

fn stream(hz: f32, frames: usize) -> (PreviewFeed, atomik_engine::PreviewStream) {
    let (mut feed, stream) = atomik_engine::preview_channel();
    feed.set_source_rate(48_000);
    let src: Vec<Vec<f32>> = vec![
        (0..frames)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect(),
    ];
    let mut from = 0;
    while from < frames {
        let n = feed.push(&src, from);
        assert!(n > 0);
        from += n;
    }
    (feed, stream)
}

fn listening() -> ReferenceSettings {
    ReferenceSettings {
        listening: true,
        ..ReferenceSettings::default()
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn small() -> LensSettings {
    LensSettings {
        voicing: LensVoicing::SmallSpeaker,
        mono: false,
    }
}

/// The peak of the last 4000 frames of a `hz` tone heard through `lens`.
fn heard(hz: f32, lens: LensSettings) -> f32 {
    let silence: [Vec<f32>; 0] = [];
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let (_feed, s) = stream(hz, 60_000);
    handle.reference_start(s, listening()).unwrap();
    handle.lens_set(lens).unwrap();
    let out = render_block(&mut engine, &silence, 2, 12_000);
    peak(&out[0][8_000..])
}

#[test]
fn the_small_speaker_loses_a_sub_tone_and_keeps_a_mid_tone() {
    let open = heard(50.0, LensSettings::OFF);
    assert!(open > 0.45, "{open}");
    let low = heard(50.0, small());
    assert!(low < 0.08, "{low}");
    let mid = heard(1_000.0, small());
    assert!(mid > 0.45, "{mid}");
    let club = heard(
        50.0,
        LensSettings {
            voicing: LensVoicing::ClubSub,
            mono: false,
        },
    );
    assert!(club > 0.4, "{club}");
}

#[test]
fn mono_plays_the_sum_on_both_sides() {
    let silence: [Vec<f32>; 0] = [];
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let (_feed, s) = stream(300.0, 60_000);
    handle.reference_start(s, listening()).unwrap();
    handle
        .lens_set(LensSettings {
            voicing: LensVoicing::Headphones,
            mono: true,
        })
        .unwrap();
    let out = render_block(&mut engine, &silence, 2, 12_000);
    assert_eq!(out[0][8_000..], out[1][8_000..]);
    assert!(peak(&out[0][8_000..]) > 0.45);
}

#[test]
fn the_lens_never_allocates_or_frees_on_the_render_path() {
    let silence: [Vec<f32>; 0] = [];
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let (_feed, s) = stream(80.0, 30_000);
    handle.reference_start(s, listening()).unwrap();
    let voicings = [
        LensVoicing::SmallSpeaker,
        LensVoicing::ClubSub,
        LensVoicing::Headphones,
    ];
    for n in 0..9 {
        handle
            .lens_set(LensSettings {
                voicing: voicings[n % 3],
                mono: n % 2 == 0,
            })
            .unwrap();
        render_block(&mut engine, &silence, 2, 3_000);
    }
}

#[test]
fn an_engine_that_never_heard_of_the_lens_renders_what_it_always_did() {
    let silence: [Vec<f32>; 0] = [];
    let (mut a, mut ha) = new_engine(cfg(0, 2));
    let (mut b, mut hb) = new_engine(cfg(0, 2));
    let (_fa, sa) = stream(70.0, 40_000);
    let (_fb, sb) = stream(70.0, 40_000);
    ha.reference_start(sa, listening()).unwrap();
    hb.reference_start(sb, listening()).unwrap();
    // The second one is told "off": the same as never being told.
    hb.lens_set(LensSettings::OFF).unwrap();
    let out_a = render_block(&mut a, &silence, 2, 20_000);
    let out_b = render_block(&mut b, &silence, 2, 20_000);
    for (x, y) in out_a.iter().flatten().zip(out_b.iter().flatten()) {
        assert_eq!(x.to_bits(), y.to_bits());
    }
}
