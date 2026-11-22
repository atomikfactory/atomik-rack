//! The bypass rule of the plugin device: an effect is called as ever while the bypass is on for less
//! than the park time, so toggling is seamless; after that it is parked (not called), its dry
//! signal and delay compensation unchanged, and when the bypass is switched off the plugin is
//! cleared, run for a pre-roll with its output held dry and then faded in. The rule counts blocks,
//! so a bounce and live playback agree.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, NoteEvent, NoteKind};
use atomik_plugin_host::{PluginEventKind as K, PluginIo, PluginProcessor, ProcessStatus};
use std::sync::Arc;
use support::*;

// ----- A: the bypass --------------------------------------------------------------------------

/// 5 ms at 48 kHz.
const FADE: usize = 240;
/// The pre-roll at 48 kHz without latency: 20 ms in blocks.
const PREROLL: usize = 15;
/// The park time at 48 kHz: 10 s in blocks.
const PARK: usize = 7_500;
/// Calls of an effect bypassed from the first block until it is parked: the fade (4 blocks) and
/// the blocks of the park time before the last.
const CALLED: usize = 4 + PARK - 1;

fn constant_effect(
    gain: f32,
) -> (
    atomik_testkit::harness::DeviceHarness,
    Arc<ProbeShared>,
    Rest,
) {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Gain(gain));
    let (mut h, r) = harness(rig("sl.bypass", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.25; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    (h, shared, r)
}

#[test]
fn a_bypass_longer_than_the_park_time_parks_the_effect_and_its_dry_signal_is_unchanged() {
    for latency in [0u32, 1, 300] {
        let (probe, shared) = Probe::new();
        shared.set_out(ProbeOut::Constant(0.5));
        let (mut h, r) = harness(rig("sl.parked", &fx_layout(), Box::new(probe)));
        assert!(!r.manager.slot().set_latency(latency, SR));
        let input =
            |n: usize| -> Batch { std::array::from_fn(|i| ((n * BATCH + i) as f32) * 1e-3) };
        h.set_param("bypass", 1.0);
        let mut dry = Vec::new();
        for n in 0..PARK + 100 {
            *h.input_mut("in_l") = input(n);
            *h.input_mut("in_r") = input(n);
            h.process();
            dry.extend_from_slice(h.output("out_l"));
        }
        let called = shared.calls();
        assert_eq!(called, CALLED, "latency {latency}: called until parked");
        // After the fade the output is the input delayed by the latency, bit for bit.
        let shift = latency as usize;
        for (i, s) in dry.iter().enumerate().skip(FADE + shift) {
            let want = (i - shift) as f32 * 1e-3;
            assert_eq!(*s, want, "latency {latency}, frame {i}");
        }
        let dry_all = dry
            .iter()
            .enumerate()
            .skip(FADE + shift)
            .all(|(i, s)| *s == (i - shift) as f32 * 1e-3);
        assert!(dry_all);
        assert_eq!(h.device().latency_frames(), latency);
        let mut m = [0.0f32; 4];
        h.device().meter_values(&mut m);
        assert_eq!(m[2], 2.0, "STOPPED");
        assert_eq!(m[3], latency as f32, "the reported latency is unchanged");
    }
}

#[test]
fn switching_the_bypass_off_gives_a_pre_roll_then_the_fade_without_a_click() {
    // The effect halves its input: the dry signal is 0.25, the wet one 0.125.
    let (mut h, shared, _r) = constant_effect(0.5);
    h.set_param("bypass", 1.0);
    for _ in 0..PARK + 100 {
        h.process();
    }
    let parked = shared.calls();
    assert_eq!(parked, CALLED);
    h.set_param("bypass", 0.0);
    let mut out = Vec::new();
    for _ in 0..PREROLL + FADE / BATCH + 4 {
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    let hold = PREROLL * BATCH;
    assert!(
        out[..hold].iter().all(|s| *s == 0.25),
        "the output stays dry during the pre-roll"
    );
    // The reference: the linear fade from dry to wet the device promises.
    for (i, s) in out[hold..hold + FADE].iter().enumerate() {
        let m = 1.0 - (i + 1) as f32 / FADE as f32;
        let want = 0.125 * (1.0 - m) + 0.25 * m;
        assert!((s - want).abs() < 1e-6, "fade frame {i}: {s} vs {want}");
    }
    assert!(out[hold + FADE..].iter().all(|s| *s == 0.125));
    let worst = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst <= 0.125 / FADE as f32 + 1e-6, "no step: {worst}");
    assert_eq!(
        shared.calls(),
        parked + PREROLL + FADE / BATCH + 4,
        "called in every block from the switch"
    );
    assert_eq!(
        shared.log.lock().unwrap().resets,
        1,
        "cleared before the pre-roll"
    );
}

#[test]
fn a_latent_effect_runs_a_pre_roll_of_at_least_its_latency() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    let (mut h, r) = harness(rig("sl.preroll-latency", &fx_layout(), Box::new(probe)));
    assert!(!r.manager.slot().set_latency(1_000, SR));
    *h.input_mut("in_l") = [0.25; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    h.set_param("bypass", 1.0);
    for _ in 0..PARK + 100 {
        h.process();
    }
    h.set_param("bypass", 0.0);
    let mut out = Vec::new();
    for _ in 0..40 {
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    let first_wet = out.iter().position(|s| *s != 0.25).unwrap();
    assert!(first_wet >= 1_000, "{first_wet}");
    assert!(first_wet < 1_000 + 2 * BATCH, "{first_wet}");
}

#[test]
fn a_param_set_while_parked_reaches_the_plugin_in_one_silent_call_and_the_plugin_stays_parked() {
    let (mut h, shared, _r) = constant_effect(1.0);
    h.set_param("bypass", 1.0);
    for _ in 0..PARK + 10 {
        h.process();
    }
    let parked = shared.calls();
    assert_eq!(parked, CALLED);
    h.set_param("p1", 6.0);
    h.process();
    assert_eq!(shared.calls(), parked + 1, "one call carries it");
    {
        let log = shared.log.lock().unwrap();
        let seen = log.events_of_call(parked);
        assert!(
            seen.iter()
                .any(|e| matches!(e.kind, K::Param { value, .. } if (value - 6.0).abs() < 1e-9)),
            "{seen:?}"
        );
        assert!(log.last_in[0].iter().all(|s| *s == 0.0), "it hears nothing");
        assert_eq!(log.resets, 0);
    }
    for _ in 0..10 {
        h.process();
    }
    assert_eq!(shared.calls(), parked + 1, "and is parked again");
    // Automation does not call it every block: at most once per 50 ms (38 blocks).
    let before = shared.calls();
    for n in 0..380 {
        h.set_param("p1", (n % 7) as f32);
        h.process();
    }
    let calls = shared.calls() - before;
    assert!((9..=11).contains(&calls), "{calls}");
    // Switching the bypass off: the value is not sent again.
    h.set_param("bypass", 0.0);
    h.process();
    assert_eq!(shared.log.lock().unwrap().resets, 1);
}

#[test]
fn the_end_of_a_note_still_reaches_a_bypassed_effect_and_a_new_note_is_held_back() {
    let mut layout = fx_layout();
    layout.notes_in = true;
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("sl.notes", &layout, Box::new(probe)));
    let on = |key| NoteEvent {
        offset: 0,
        kind: NoteKind::On { key, velocity: 1.0 },
    };
    let off = |key| NoteEvent {
        offset: 2,
        kind: NoteKind::Off { key },
    };
    h.push_note("notes_in", on(60));
    h.process();
    h.set_param("bypass", 1.0);
    for _ in 0..PARK + 10 {
        h.process();
    }
    let parked = shared.calls();
    h.push_note("notes_in", on(64));
    h.process();
    assert_eq!(shared.calls(), parked, "a new note is held back");
    h.push_note("notes_in", off(60));
    h.process();
    assert_eq!(shared.calls(), parked + 1, "the end of a note needs a call");
    let log = shared.log.lock().unwrap();
    let seen = log.events_of_call(parked);
    assert!(
        matches!(seen, [e] if matches!(e.kind, K::NoteOff { key: 60, .. })),
        "{seen:?}"
    );
}

/// A one-pole low-pass: a processor whose output depends on what it heard before.
struct Lowpass {
    y: [f32; 2],
}

impl PluginProcessor for Lowpass {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {
        self.y = [0.0; 2];
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        for (ch, out) in io.main_out.iter_mut().enumerate() {
            for (o, x) in out.iter_mut().zip(&io.main_in[ch]) {
                self.y[ch] += 0.1 * (x - self.y[ch]);
                *o = self.y[ch];
            }
        }
        ProcessStatus::Continue
    }
}

/// The input of the low-pass tests, frame `f`.
fn lp_input(f: usize) -> f32 {
    ((f % 97) as f32 - 48.0) / 48.0
}

/// Renders `blocks` blocks through a [`Lowpass`] effect, the bypass set to `v` at each `(block,
/// v)` of `switches`; returns the left output.
fn lowpass_with(blocks: usize, switches: &[(usize, f32)]) -> Vec<f32> {
    let (mut h, _r) = harness(rig(
        "sl.lowpass",
        &fx_layout(),
        Box::new(Lowpass { y: [0.0; 2] }),
    ));
    let mut out = Vec::new();
    for n in 0..blocks {
        for (at, v) in switches {
            if *at == n {
                h.set_param("bypass", *v);
            }
        }
        let x: Batch = std::array::from_fn(|i| lp_input(n * BATCH + i));
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    out
}

/// The same render computed without the device, for a plugin that is called in every block: the
/// filter runs through the whole input and the bypass mix fades towards the dry signal 1/240 per
/// frame while the bypass is on.
fn lowpass_model(blocks: usize, on: impl Fn(usize) -> bool) -> Vec<f32> {
    let mut y = 0.0f32;
    let mut m = 0.0f32;
    let step = 1.0 / FADE as f32;
    let mut out = Vec::new();
    for n in 0..blocks {
        let target = if on(n) { 1.0 } else { 0.0 };
        for i in 0..BATCH {
            let x = lp_input(n * BATCH + i);
            y += 0.1 * (x - y);
            if m < target {
                m = (m + step).min(target);
            } else if m > target {
                m = (m - step).max(target);
            }
            out.push(if m <= 0.0 {
                y
            } else if m >= 1.0 {
                x
            } else {
                y * (1.0 - m) + x * m
            });
        }
    }
    out
}

#[test]
fn a_bypass_that_is_toggled_inside_the_park_time_is_bit_for_bit_what_always_calling_gives() {
    // On, off, on, off, a few seconds apart: the plugin is never cleared, its state is warm.
    let switches = [
        (20, 1.0),
        (60, 0.0),
        (1_000, 1.0),
        (4_000, 0.0),
        (4_100, 1.0),
        (4_300, 0.0),
    ];
    let blocks = 5_000;
    let on = |n: usize| {
        (20..60).contains(&n) || (1_000..4_000).contains(&n) || (4_100..4_300).contains(&n)
    };
    assert_eq!(lowpass_with(blocks, &switches), lowpass_model(blocks, on));
}

#[test]
fn a_bypass_longer_than_the_park_time_returns_cleared_pre_rolled_and_the_same_in_every_run() {
    let (on_at, off_at, blocks) = (20, 20 + PARK + 200, 20 + PARK + 600);
    let live = lowpass_with(blocks, &[(on_at, 1.0), (off_at, 0.0)]);
    // Until the switch: what a plugin called in every block renders (dry after the fade).
    let all_along = lowpass_model(blocks, |n| (on_at..off_at).contains(&n));
    let switch = off_at * BATCH;
    assert_eq!(live[..switch], all_along[..switch]);
    // After it: the filter starts from a cleared state at the first block of the switch, the
    // output stays dry through the pre-roll, then fades to the wet signal.
    let mut y = 0.0f32;
    let mut m = 1.0f32;
    let step = 1.0 / FADE as f32;
    for (k, got) in live[switch..].iter().enumerate() {
        let x = lp_input(switch + k);
        y += 0.1 * (x - y);
        if k >= PREROLL * BATCH {
            m = (m - step).max(0.0);
        }
        let want = if m <= 0.0 {
            y
        } else if m >= 1.0 {
            x
        } else {
            y * (1.0 - m) + x * m
        };
        assert_eq!(*got, want, "frame {k} after the switch");
    }
    // The run is a pure function of the blocks: the same again gives the same bits.
    assert_eq!(live, lowpass_with(blocks, &[(on_at, 1.0), (off_at, 0.0)]));
}

#[test]
fn an_effect_bypassed_from_the_first_block_to_the_last_renders_its_dry_signal() {
    let blocks = PARK + 500;
    let out = lowpass_with(blocks, &[(0, 1.0)]);
    for (f, s) in out.iter().enumerate().skip(FADE) {
        assert_eq!(*s, lp_input(f), "frame {f}");
    }
}

#[test]
fn a_toggle_inside_the_park_time_never_clears_the_plugin() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("sl.toggle", &fx_layout(), Box::new(probe)));
    for n in 0..PARK {
        match n {
            100 | 2_000 => h.set_param("bypass", 1.0),
            1_500 | 6_000 => h.set_param("bypass", 0.0),
            _ => {}
        }
        h.process();
    }
    assert_eq!(shared.calls(), PARK, "called in every block");
    assert_eq!(shared.log.lock().unwrap().resets, 0, "never cleared");
}

#[test]
fn an_effect_parked_by_its_bypass_and_then_powered_off_still_pre_rolls_when_it_comes_back() {
    let (mut h, shared, r) = constant_effect(0.5);
    h.set_param("bypass", 1.0);
    for _ in 0..PARK + 10 {
        h.process();
    }
    r.manager.slot().set_powered(false);
    for _ in 0..10 {
        h.process();
    }
    h.set_param("bypass", 0.0);
    r.manager.slot().set_powered(true);
    let mut out = Vec::new();
    for _ in 0..PREROLL {
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    assert!(out.iter().all(|s| *s == 0.25), "dry through the pre-roll");
    assert_eq!(shared.log.lock().unwrap().resets, 1);
}
