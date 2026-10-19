//! Record tap (engine side): the audio thread copies host input
//! batches into a preallocated ring tagged with their engine frames; the non-RT side drains
//! them bit-exactly. Overflow is counted, never blocking; start and stop land on batch
//! boundaries through the control queue.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{RecordTap, new_engine};
use common::*;

const TOTAL: usize = 20_000;

fn input(channel: usize) -> Vec<f32> {
    (0..TOTAL)
        .map(|i| ((i * 7 + channel * 13) % 4_001) as f32 * 1.0e-3 - 2.0)
        .collect()
}

/// Drains and returns `(start, planar frames)` runs.
fn drain_all(tap: &mut RecordTap, runs: &mut Vec<(u64, Vec<Vec<f32>>)>) {
    loop {
        let mut out = Vec::new();
        match tap.drain(&mut out) {
            Some((start, frames)) => {
                assert!(out.iter().all(|c| c.len() == frames));
                runs.push((start, out));
            }
            None => break,
        }
    }
}

#[test]
fn the_tap_returns_the_input_bit_exactly_with_correct_frame_tags() {
    let inputs = [input(0), input(1)];
    for block in [1usize, 64, 333] {
        let (mut engine, mut handle) = new_engine(cfg(2, 2));
        // Tap host channel 1 then 0 (order is kept).
        let mut tap = handle.record_tap(&[1, 0], 4_096).unwrap();
        assert_eq!(tap.channels(), 2);
        let mut runs = Vec::new();
        let mut done = 0;
        while done < TOTAL {
            let n = block.min(TOTAL - done);
            let ins: Vec<Vec<f32>> = inputs.iter().map(|c| c[done..done + n].to_vec()).collect();
            render_block(&mut engine, &ins, 2, n);
            drain_all(&mut tap, &mut runs);
            done += n;
        }
        assert_eq!(tap.overflow_frames(), 0, "block {block}");
        // Whole batches only: the trailing partial batch has not been processed yet.
        let whole = TOTAL / 64 * 64;
        let (mut got0, mut got1) = (Vec::new(), Vec::new());
        let mut expect_start = 0u64;
        for (start, run) in &runs {
            assert_eq!(*start, expect_start, "contiguous tags, block {block}");
            expect_start += run[0].len() as u64;
            got0.extend_from_slice(&run[0]);
            got1.extend_from_slice(&run[1]);
        }
        assert_eq!(got0.len(), whole);
        let bits = |v: &[f32]| v.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&got0), bits(&inputs[1][..whole]), "block {block}");
        assert_eq!(bits(&got1), bits(&inputs[0][..whole]), "block {block}");
    }
}

#[test]
fn start_and_stop_land_on_batch_boundaries() {
    let inputs = [input(0), input(1)];
    let (mut engine, mut handle) = new_engine(cfg(2, 2));
    render_blocks(&mut engine, &inputs, 2, 1_000, 100); // 15 whole batches processed
    let mut tap = handle.record_tap(&[0], 8_192).unwrap();
    let mut runs = Vec::new();
    // The tap starts at the next boundary, a multiple of 64 at or after the frames rendered.
    let rest: Vec<Vec<f32>> = inputs.iter().map(|c| c[1_000..6_000].to_vec()).collect();
    render_blocks(&mut engine, &rest, 2, 5_000, 333);
    drain_all(&mut tap, &mut runs);
    let (start, first) = &runs[0];
    assert_eq!(start % 64, 0);
    assert!(
        *start >= 960,
        "not before the batches already processed: {start}"
    );
    let s = *start as usize;
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    // Frame `s` of the stream (inputs were continuous across the calls).
    assert_eq!(bits(&first[0]), bits(&inputs[0][s..s + first[0].len()]));
    // Stop: the engine stops copying at the next boundary and the rest stays drainable.
    handle.stop_record_tap().unwrap();
    let more: Vec<Vec<f32>> = inputs.iter().map(|c| c[6_000..8_000].to_vec()).collect();
    render_blocks(&mut engine, &more, 2, 2_000, 64);
    let mut after = Vec::new();
    drain_all(&mut tap, &mut after);
    let frames: usize = after.iter().map(|(_, r)| r[0].len()).sum();
    assert!(
        frames <= 64,
        "at most the batch in flight when stopping: {frames}"
    );
    handle.collect_garbage();
}

#[test]
fn overflow_is_counted_and_the_audio_thread_never_blocks() {
    let inputs = [input(0), input(1)];
    let (mut engine, mut handle) = new_engine(cfg(2, 2));
    let mut tap = handle.record_tap(&[0, 1], 256).unwrap(); // room for 4 batches
    // 20 000 frames with nobody draining: the ring fills, the rest is counted.
    render_blocks(&mut engine, &inputs, 2, TOTAL, 64);
    let batches = (TOTAL / 64) as u64;
    assert_eq!(tap.overflow_frames(), (batches - 4) * 64);
    let mut runs = Vec::new();
    drain_all(&mut tap, &mut runs);
    let kept: usize = runs.iter().map(|(_, r)| r[0].len()).sum();
    assert_eq!(kept, 4 * 64);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    for (start, run) in &runs {
        let s = *start as usize;
        assert_eq!(bits(&run[0]), bits(&inputs[0][s..s + run[0].len()]));
        assert_eq!(bits(&run[1]), bits(&inputs[1][s..s + run[1].len()]));
    }
    // After draining there is room again.
    let more: Vec<Vec<f32>> = inputs.iter().map(|c| c[..640].to_vec()).collect();
    render_blocks(&mut engine, &more, 2, 640, 64);
    let mut again = Vec::new();
    drain_all(&mut tap, &mut again);
    assert!(!again.is_empty());
}

#[test]
fn a_new_tap_replaces_the_old_one() {
    let inputs = [input(0), input(1)];
    let (mut engine, mut handle) = new_engine(cfg(2, 2));
    let mut first = handle.record_tap(&[0], 4_096).unwrap();
    render_blocks(&mut engine, &inputs, 2, 640, 64);
    let mut second = handle.record_tap(&[1], 4_096).unwrap();
    let rest: Vec<Vec<f32>> = inputs.iter().map(|c| c[640..1_280].to_vec()).collect();
    render_blocks(&mut engine, &rest, 2, 640, 64);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    drain_all(&mut first, &mut a);
    drain_all(&mut second, &mut b);
    let frames = |r: &[(u64, Vec<Vec<f32>>)]| r.iter().map(|(_, c)| c[0].len()).sum::<usize>();
    assert_eq!(frames(&a), 640, "the old tap stopped when it was replaced");
    assert_eq!(frames(&b), 640);
    assert_eq!(b[0].0, 640);
    handle.collect_garbage();
}
