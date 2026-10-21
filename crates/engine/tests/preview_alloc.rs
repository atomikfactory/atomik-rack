//! HI-1 for the preview voice: no allocation and no free on the render path across start,
//! switch (both slots busy), underrun, finish, stop and retirement. `render_block` runs every
//! host block under the counting allocator; streams are created, fed and dropped on this
//! (handle) side only.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{PreviewFeed, new_engine};
use common::*;

fn tone(n: usize) -> Vec<Vec<f32>> {
    vec![(0..n).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()]
}

fn new_stream(frames: usize, finish: bool) -> (PreviewFeed, atomik_engine::PreviewStream) {
    new_stream_at(frames, finish, 48_000)
}

fn new_stream_at(
    frames: usize,
    finish: bool,
    rate: u32,
) -> (PreviewFeed, atomik_engine::PreviewStream) {
    let (mut feed, stream) = atomik_engine::preview_channel();
    feed.set_source_rate(rate);
    let src = tone(frames);
    let mut from = 0;
    while from < frames {
        let n = feed.push(&src, from);
        assert!(n > 0);
        from += n;
    }
    if finish {
        feed.finish();
    }
    (feed, stream)
}

#[test]
fn preview_voice_never_allocates_or_frees_on_the_render_path() {
    let (mut engine, mut handle) = new_engine(cfg(0, 2));
    let silence: [Vec<f32>; 0] = [];
    let render = |engine: &mut _, n: usize| {
        render_block(engine, &silence, 2, n);
    };

    // A pending start (no data yet, rate unknown), then a drop of its feed.
    let (pf, p) = atomik_engine::preview_channel();
    handle.preview_start(p, 1.0).unwrap();
    render(&mut engine, 300);
    assert!(handle.preview_active());
    drop(pf);
    render(&mut engine, 300);
    handle.collect_garbage();
    assert!(!handle.preview_active());
    // A pending start that receives its data later.
    let (mut lf, l) = atomik_engine::preview_channel();
    handle.preview_start(l, 1.0).unwrap();
    render(&mut engine, 300);
    lf.set_source_rate(22_050);
    lf.push(&tone(2_000), 0);
    lf.finish();
    render(&mut engine, 6_000);
    assert!(lf.is_done());
    handle.collect_garbage();

    // Start (resampled), play.
    let (fa, a) = new_stream_at(30_000, false, 44_100);
    handle.preview_start(a, 0.8).unwrap();
    render(&mut engine, 700);

    // Switch while one slot is busy, then again while both are: the oldest is cut and retired.
    let (fb, b) = new_stream(30_000, false);
    handle.preview_start(b, 1.0).unwrap();
    render(&mut engine, 100);
    let (fc, c) = new_stream_at(30_000, false, 22_050);
    handle.preview_start(c, 1.0).unwrap();
    render(&mut engine, 100);
    handle.collect_garbage();
    assert!(fa.is_abandoned());
    // Several starts inside one batch boundary.
    let (_fd, d) = new_stream(1_000, false);
    let (_fe, e) = new_stream(1_000, false);
    handle.preview_start(d, 1.0).unwrap();
    handle.preview_start(e, 1.0).unwrap();
    render(&mut engine, 700);
    handle.collect_garbage();
    assert!(fb.is_abandoned() && fc.is_abandoned());

    // Underrun (the stream above holds 1 000 frames and is not finished), then resume and finish.
    render(&mut engine, 2_000);
    let (mut fu, u) = new_stream(500, false);
    handle.preview_start(u, 1.0).unwrap();
    render(&mut engine, 2_000);
    assert!(fu.underruns() > 0);
    let more = tone(500);
    fu.push(&more, 0);
    fu.finish();
    render(&mut engine, 2_000);
    assert!(fu.is_done());
    handle.collect_garbage();
    assert!(fu.is_abandoned());

    // Finish of a short stream next to a new start, then a stop with a fade-out in flight.
    let (ff, f) = new_stream(300, true);
    handle.preview_start(f, 1.0).unwrap();
    render(&mut engine, 64);
    let (fg, g) = new_stream(30_000, false);
    handle.preview_start(g, 1.0).unwrap();
    render(&mut engine, 1_000);
    handle.preview_stop().unwrap();
    render(&mut engine, 100);
    handle.preview_stop().unwrap();
    render(&mut engine, 1_000);
    handle.collect_garbage();
    assert!(ff.is_abandoned());
    assert!(fg.is_abandoned());
    assert!(!handle.preview_active());

    // A full cycle with the retire queue never drained until the end (parking path).
    for _ in 0..40 {
        let (f, s) = new_stream(200, true);
        // The feed is gone: the engine holds the last reference, so a drop on the audio thread
        // would free the ring and must trip the guard.
        drop(f);
        handle.preview_start(s, 1.0).unwrap();
        render(&mut engine, 300);
    }
    handle.collect_garbage();
    render(&mut engine, 300);
    handle.collect_garbage();
    assert!(!handle.preview_active());
}
