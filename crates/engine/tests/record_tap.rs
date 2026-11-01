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

// ---------------------------------------------------------------- gain and clip-safe

use atomik_engine::{TapChannel, TapControls};
use std::sync::Arc;

/// Records `frames` frames of `inputs` through a gained tap, one block size, calling `each`
/// before every block with the index of the first frame of the block, and returns every
/// recorded channel.
fn record_gained(
    inputs: &[Vec<f32>],
    channels: &[TapChannel],
    controls: &Arc<TapControls>,
    frames: usize,
    block: usize,
    mut each: impl FnMut(usize),
) -> Vec<Vec<f32>> {
    let (mut engine, mut handle) = new_engine(cfg(inputs.len() as u16, 2));
    let mut tap = handle
        .record_tap_gained(channels, 4_096, Arc::clone(controls))
        .unwrap();
    let mut runs = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        each(done);
        let ins: Vec<Vec<f32>> = inputs.iter().map(|c| c[done..done + n].to_vec()).collect();
        render_block(&mut engine, &ins, 2, n);
        drain_all(&mut tap, &mut runs);
        done += n;
    }
    assert_eq!(tap.overflow_frames(), 0);
    let mut out = vec![Vec::new(); channels.len()];
    for (_, run) in runs {
        for (o, r) in out.iter_mut().zip(run) {
            o.extend(r);
        }
    }
    out
}

fn ch(host: usize, strip: usize) -> TapChannel {
    TapChannel::host(host, strip)
}

#[test]
fn the_gain_is_applied_to_every_sample_whatever_the_block_size() {
    let inputs = [input(0), input(1)];
    let controls = Arc::new(TapControls::new(2));
    controls.set(0, 2.5, false);
    controls.set(1, 0.25, false);
    let mut first: Option<Vec<Vec<f32>>> = None;
    for block in [1usize, 64, 333, 1_000] {
        let got = record_gained(
            &inputs,
            &[ch(0, 0), ch(1, 1)],
            &controls,
            TOTAL,
            block,
            |_| {},
        );
        let whole = TOTAL / 64 * 64;
        assert_eq!(got[0].len(), whole, "block {block}");
        for (i, v) in got[0].iter().enumerate() {
            assert_eq!(
                v.to_bits(),
                (inputs[0][i] * 2.5).to_bits(),
                "strip 0, frame {i}"
            );
        }
        for (i, v) in got[1].iter().enumerate() {
            assert_eq!(
                v.to_bits(),
                (inputs[1][i] * 0.25).to_bits(),
                "strip 1, frame {i}"
            );
        }
        match &first {
            None => first = Some(got),
            Some(f) => assert_eq!(bits(f), bits(&got), "block {block} equals block 1"),
        }
    }
}

#[test]
fn two_strips_can_record_the_same_host_channel_with_different_gains() {
    let inputs = [vec![0.5f32; 2_048]];
    let controls = Arc::new(TapControls::new(2));
    controls.set(0, 1.0, false);
    controls.set(1, 2.0, false);
    let got = record_gained(&inputs, &[ch(0, 0), ch(0, 1)], &controls, 2_048, 64, |_| {});
    assert!(got[0].iter().all(|s| s.to_bits() == 0.5f32.to_bits()));
    assert!(got[1].iter().all(|s| s.to_bits() == 1.0f32.to_bits()));
}

#[test]
fn a_gain_change_glides_with_no_step() {
    let inputs = [vec![1.0f32; 8_192]];
    let controls = Arc::new(TapControls::new(1));
    let got = record_gained(&inputs, &[ch(0, 0)], &controls, 8_192, 64, |at| {
        if at == 2_048 {
            controls.set(0, 4.0, false);
        }
    });
    let y = &got[0];
    // Before the change (a whole batch boundary) the take is untouched.
    assert!(y[..2_048].iter().all(|s| s.to_bits() == 1.0f32.to_bits()));
    // The change starts at the next batch and ends at the target.
    assert!(
        y[2_048] > 1.0 && y[2_048] < 1.2,
        "starts gently: {}",
        y[2_048]
    );
    assert!(y[4_500..].iter().all(|s| (s - 4.0).abs() < 1.0e-3));
    // Monotone and no step bigger than a fraction of the whole move.
    for w in y[2_047..4_096].windows(2) {
        assert!(w[1] >= w[0], "monotone");
        assert!(w[1] - w[0] < 0.1, "no step: {} -> {}", w[0], w[1]);
    }
}

#[test]
fn clip_safe_never_writes_a_sample_above_full_scale_for_a_hot_input() {
    // A loud sine, 12 dB hot, with spikes and a stereo pair of different levels.
    let hot = |amp: f32, phase: f32| -> Vec<f32> {
        (0..TOTAL)
            .map(|i| {
                let v = amp * (i as f32 * 0.07 + phase).sin();
                if i % 997 == 0 { v * 3.0 } else { v }
            })
            .collect()
    };
    let inputs = [hot(1.0, 0.0), hot(0.3, 1.0)];
    let controls = Arc::new(TapControls::new(1));
    controls.set(0, atomik_core::db_to_gain(12.0), true);
    for block in [64usize, 333] {
        let got = record_gained(
            &inputs,
            &[ch(0, 0), ch(1, 0)],
            &controls,
            TOTAL,
            block,
            |_| {},
        );
        let peak = got.iter().flatten().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 1.0, "block {block}: peak {peak}");
        assert!(
            peak > 0.9,
            "the limiter holds the level near full scale: {peak}"
        );
        // The pair is limited together: the image is kept (both channels share one gain).
        let (l, r) = (&got[0], &got[1]);
        let k = (0..l.len())
            .find(|&i| l[i].abs() > 0.5 && inputs[0][i].abs() > 0.5)
            .unwrap();
        let ratio_in = inputs[1][k] / inputs[0][k];
        assert!(((r[k] / l[k]) - ratio_in).abs() < 1.0e-4);
    }
    // Without clip-safe the same take keeps its overs (float files lose nothing).
    controls.set(0, atomik_core::db_to_gain(12.0), false);
    let got = record_gained(&inputs, &[ch(0, 0), ch(1, 0)], &controls, TOTAL, 64, |_| {});
    assert!(got[0].iter().any(|s| s.abs() > 1.0));
}

#[test]
fn clip_safe_leaves_a_take_below_full_scale_untouched_to_the_bit() {
    let inputs = [(0..TOTAL)
        .map(|i| 0.8 * (i as f32 * 0.05).sin())
        .collect::<Vec<_>>()];
    let controls = Arc::new(TapControls::new(1));
    controls.set(0, 1.0, true);
    let got = record_gained(&inputs, &[ch(0, 0)], &controls, TOTAL, 64, |_| {});
    for (i, v) in got[0].iter().enumerate() {
        assert_eq!(v.to_bits(), inputs[0][i].to_bits(), "frame {i}");
    }
}

#[test]
fn clip_safe_releases_back_to_the_original_level_after_a_hot_passage() {
    // Hot for a while, then quiet: the gain reduction releases back to unity.
    let inputs = [(0..16_384)
        .map(|i| if i < 4_096 { 3.0 } else { 0.5 })
        .collect::<Vec<f32>>()];
    let controls = Arc::new(TapControls::new(1));
    controls.set(0, 1.0, true);
    let got = record_gained(&inputs, &[ch(0, 0)], &controls, 16_384, 64, |_| {});
    assert!(got[0][..4_096].iter().all(|s| s.abs() <= 1.0));
    // About twenty time constants later the take is back at its original level.
    assert!((got[0][16_000] - 0.5).abs() < 1.0e-3, "{}", got[0][16_000]);
}

#[test]
fn clip_safe_writes_silence_for_a_sample_that_is_not_a_number() {
    let mut x = vec![0.25f32; 1_024];
    x[100] = f32::NAN;
    x[200] = f32::INFINITY;
    let controls = Arc::new(TapControls::new(1));
    controls.set(0, 1.0, true);
    let got = record_gained(&[x], &[ch(0, 0)], &controls, 1_024, 64, |_| {});
    assert!(got[0].iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    assert_eq!(got[0][100], 0.0);
    assert_eq!(got[0][200], 0.0);
}

#[test]
fn controls_clamp_and_report_what_was_set() {
    let c = TapControls::new(2);
    assert_eq!((c.len(), c.is_empty()), (2, false));
    assert_eq!(c.get(0), Some((1.0, false)));
    c.set(1, 1_000.0, true);
    assert_eq!(c.get(1), Some((64.0, true)));
    c.set(1, -3.0, false);
    assert_eq!(c.get(1), Some((0.0, false)));
    c.set(1, f32::NAN, false);
    assert_eq!(c.get(1), Some((1.0, false)));
    c.set(9, 2.0, true); // no such strip: ignored
    assert_eq!(c.get(9), None);
}

// ---------------------------------------------------------------- resampling a device output

use atomik_engine::{Graph, RenderPlan};
use atomik_testkit::fakes::Passthrough;

/// `in_1 -> passthrough (gain) -> out_1`; returns the plan and the device.
fn gain_plan(gain: f32) -> (RenderPlan, atomik_engine::DeviceInstanceId) {
    let mut g = Graph::new(&cfg(1, 1));
    let p = g.add_device(Passthrough::boxed(gain));
    g.connect(iface(&g, "in_1"), ep(p, Passthrough::AUDIO_IN))
        .unwrap();
    g.connect(ep(p, Passthrough::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    (g.compile().unwrap(), p)
}

/// Renders `frames` frames through `in_1 -> x0.5 -> out_1` with a source tap on the device
/// output, in host blocks of `block`, on `threads` workers. Returns the tapped signal and the
/// host output.
fn resample(threads: u16, block: usize, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut engine, mut handle) = engine_with(cfg(1, 1), threads, None);
    let (plan, p) = gain_plan(0.5);
    handle.submit_plan(plan).unwrap();
    let controls = Arc::new(TapControls::new(1));
    let mut tap = handle
        .record_tap_gained(
            &[TapChannel::source(p, Passthrough::AUDIO_OUT, 0)],
            8_192,
            Arc::clone(&controls),
        )
        .unwrap();
    let inputs = [input(0)];
    let (mut runs, mut out) = (Vec::new(), Vec::new());
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let ins = vec![inputs[0][done..done + n].to_vec()];
        out.extend(render_block(&mut engine, &ins, 1, n).remove(0));
        drain_all(&mut tap, &mut runs);
        done += n;
    }
    assert_eq!(tap.overflow_frames(), 0);
    let mut taken = Vec::new();
    let mut expect = 0u64;
    for (start, run) in runs {
        assert_eq!(start, expect, "contiguous tags");
        expect += run[0].len() as u64;
        taken.extend_from_slice(&run[0]);
    }
    (taken, out)
}

#[test]
fn a_source_tap_equals_the_rendered_output_sample_for_sample() {
    // The host output lags the render by the engine's one-batch FIFO, so the tap, which reads the
    // device output of the batch the moment it ran, is the output 64 frames earlier.
    for (threads, block) in [(0u16, 64usize), (0, 1), (0, 333), (2, 64), (2, 1_000)] {
        let (taken, out) = resample(threads, block, TOTAL);
        let whole = TOTAL / 64 * 64;
        assert_eq!(taken.len(), whole, "{threads} workers, block {block}");
        let x = input(0);
        for (i, v) in taken.iter().enumerate() {
            assert_eq!(v.to_bits(), (x[i] * 0.5).to_bits(), "frame {i}");
            if i + 64 < out.len() {
                assert_eq!(v.to_bits(), out[i + 64].to_bits(), "null test at frame {i}");
            }
        }
    }
}

#[test]
fn a_source_that_is_not_routed_or_not_an_output_reads_silence() {
    let (mut engine, mut handle) = new_engine(cfg(1, 1));
    let (plan, p) = gain_plan(0.5);
    handle.submit_plan(plan).unwrap();
    let controls = Arc::new(TapControls::new(1));
    let unknown = atomik_engine::DeviceInstanceId(99);
    let mut tap = handle
        .record_tap_gained(
            &[
                TapChannel::source(unknown, Passthrough::AUDIO_OUT, 0),
                // An input socket is not a source.
                TapChannel::source(p, Passthrough::AUDIO_IN, 0),
                TapChannel::source(p, Passthrough::AUDIO_OUT, 0),
            ],
            4_096,
            controls,
        )
        .unwrap();
    let ins = [vec![1.0f32; 640]];
    render_blocks(&mut engine, &ins, 1, 640, 64);
    let mut out = Vec::new();
    tap.drain(&mut out).unwrap();
    assert!(out[0].iter().all(|s| s.to_bits() == 0));
    assert!(out[1].iter().all(|s| s.to_bits() == 0));
    assert!(out[2].iter().all(|s| s.to_bits() == 0.5f32.to_bits()));
}

#[test]
fn a_source_and_a_host_channel_share_one_tap_and_the_strip_gain_applies() {
    let (mut engine, mut handle) = new_engine(cfg(1, 1));
    let (plan, p) = gain_plan(0.5);
    handle.submit_plan(plan).unwrap();
    let controls = Arc::new(TapControls::new(2));
    controls.set(0, 1.0, false);
    controls.set(1, 4.0, false);
    let mut tap = handle
        .record_tap_gained(
            &[
                TapChannel::host(0, 0),
                TapChannel::source(p, Passthrough::AUDIO_OUT, 1),
            ],
            4_096,
            controls,
        )
        .unwrap();
    let ins = [vec![0.25f32; 1_280]];
    render_blocks(&mut engine, &ins, 1, 1_280, 64);
    let mut out = Vec::new();
    tap.drain(&mut out).unwrap();
    assert!(
        out[0].iter().all(|s| s.to_bits() == 0.25f32.to_bits()),
        "the host input, untouched"
    );
    assert!(
        out[1].iter().all(|s| s.to_bits() == 0.5f32.to_bits()),
        "0.25 x 0.5 x 4"
    );
}
