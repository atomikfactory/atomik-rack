//! The sleep rule on the real plugin formats, CLAP and VST3 alike: an effect that reports a tail
//! of two seconds is called through it and then rests; one that reports none but rings for a
//! second is not cut (its own output keeps it called) and rests after the ring; one that reports
//! a tail without end rests on the silence of its output; and what a plugin that rests would
//! have made is exactly what it makes (the output equals the DSP run in every block), also
//! across a wake by input. The CLAP test effects answer "tail" while they ring and "continue
//! if not quiet" after, so the statuses are honoured too.
// Justification: the output is compared bit for bit with the reference DSP.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_device_sdk::RunMode;
use atomik_test_plugin_core as core;
use support::*;

/// Blocks in one second at 48 kHz.
const SECOND: usize = SR as usize / BATCH;

/// The input of block `n`: bursts of three blocks at block 0 and at block `again`.
fn input(n: usize, again: usize) -> f32 {
    if n < 3 || (again..again + 3).contains(&n) {
        0.5
    } else {
        0.0
    }
}

/// What the DSP makes of the same input run in every block.
fn expected(ring: u32, delay: u32, blocks: usize, again: usize) -> Vec<f32> {
    let mut fx = core::RingFx::delayed(ring, delay);
    let mut out = Vec::new();
    for n in 0..blocks {
        let x = [[input(n, again); BATCH]; 2];
        let mut y = [[0.0; BATCH]; 2];
        fx.process(&x, &mut y);
        out.extend_from_slice(&y[0]);
    }
    out
}

struct Run {
    out: Vec<f32>,
    /// Calls of the plugin after block `n`, for the blocks asked for.
    calls_at: Vec<u64>,
    /// The state meter (0 running, 1 sleeping) after block `n`.
    state_at: Vec<f32>,
}

fn run(format: Format, variant: Variant, blocks: usize, again: usize, ask: &[usize]) -> Run {
    let _serial = serial();
    let k = kit_variants(format);
    let lib = TestLib::open_path(&k.path);
    let mut rig = rig_for(&*k.backend, &k.variant(variant), RunMode::Realtime);
    let mut h = rig.harness();
    let mut out = Vec::new();
    let (mut calls_at, mut state_at) = (Vec::new(), Vec::new());
    for n in 0..blocks {
        *h.input_mut("in_l") = [input(n, again); BATCH];
        *h.input_mut("in_r") = [input(n, again); BATCH];
        h.process();
        out.extend_from_slice(h.output("out_l"));
        if ask.contains(&n) {
            calls_at.push(lib.probe(&*rig.instance, probe::PROCESS_CALLS));
            let mut m = [0.0f32; 5];
            h.device().meter_values(&mut m);
            state_at.push(m[2]);
        }
    }
    Run {
        out,
        calls_at,
        state_at,
    }
}

#[test]
fn an_effect_with_a_two_second_tail_is_called_through_it_then_rests_and_wakes_on_input() {
    for format in FORMATS {
        let again = 2 * SECOND * 2 + 100;
        let blocks = again + 3 + 10;
        // Three blocks of signal, 1500 blocks of ring, then the idle second.
        let asleep_from = 3 + 1_500 - 1 + SECOND;
        let r = run(
            format,
            Variant::Tail2s,
            blocks,
            again,
            &[asleep_from - 1, asleep_from + 200, blocks - 1],
        );
        assert_eq!(
            r.out,
            expected(2 * SR as u32, 0, blocks, again),
            "{format:?}: what rests is what the plugin would have made"
        );
        assert_eq!(
            r.calls_at[0], asleep_from as u64,
            "{format:?}: called through its tail"
        );
        assert_eq!(
            r.calls_at[1], r.calls_at[0],
            "{format:?}: no call while it rests"
        );
        assert_eq!(r.state_at[1], 1.0, "{format:?}: SLEEPING");
        assert!(
            r.calls_at[2] > r.calls_at[1],
            "{format:?}: woken by its input"
        );
    }
}

#[test]
fn an_effect_that_reports_no_tail_but_rings_for_a_second_is_not_cut() {
    for format in FORMATS {
        let (blocks, again) = (3_000, 2_500);
        let ringing = 3 + SECOND - 1;
        let asleep_from = ringing + SECOND;
        let r = run(
            format,
            Variant::Underreported,
            blocks,
            again,
            &[asleep_from - 1, again - 1],
        );
        assert_eq!(
            r.out,
            expected(SR as u32, 0, blocks, again),
            "{format:?}: the whole ring is heard"
        );
        assert_eq!(r.calls_at[0], asleep_from as u64, "{format:?}");
        assert_eq!(
            r.calls_at[1], r.calls_at[0],
            "{format:?}: it rests after the ring"
        );
    }
}

#[test]
fn an_effect_with_an_infinite_tail_is_never_skipped_in_vst3_and_rests_by_its_own_word_in_clap() {
    // Silent for two seconds after its input, then it rings for one: nothing may be lost.
    for format in FORMATS {
        let (blocks, again) = (4_600, 4_500);
        let r = run(
            format,
            Variant::Endless,
            blocks,
            again,
            &[3_500, again - 1, blocks - 1],
        );
        assert_eq!(
            r.out,
            expected(SR as u32, 2 * SR as u32, blocks, again),
            "{format:?}: the late ring is heard"
        );
        match format {
            Format::Vst3 => {
                assert_eq!(r.calls_at[0], 3_501, "{format:?}: called in every block");
                assert_eq!(r.calls_at[1], again as u64, "{format:?}");
                assert_eq!(r.state_at[0], 0.0, "{format:?}: never SLEEPING");
            }
            Format::Clap => {
                // Tail while it has something left to sound (the ring ends in block 2251),
                // then continue if not quiet: it rests after the idle second.
                assert_eq!(r.calls_at[0], 3_002, "{format:?}");
                assert_eq!(r.calls_at[1], 3_002, "{format:?}");
                assert_eq!(r.state_at[0], 1.0, "{format:?}: SLEEPING");
            }
        }
    }
}
