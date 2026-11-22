//! The bypass rule of the plugin device: an effect whose bypass has faded to the dry signal is
//! not called (live), its dry signal and delay compensation are unchanged, and when the bypass
//! is switched off the plugin is cleared, run for a pre-roll with its output held dry and then
//! faded in. A bounce (offline) keeps calling the plugin.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, NoteEvent, NoteKind};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{PluginEventKind as K, PluginIo, PluginProcessor, ProcessStatus};
use std::sync::Arc;
use support::*;

// ----- A: the bypass --------------------------------------------------------------------------

/// 5 ms at 48 kHz.
const FADE: usize = 240;
/// The pre-roll at 48 kHz without latency: 20 ms in blocks.
const PREROLL: usize = 15;

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
fn a_bypassed_effect_is_not_called_once_the_fade_is_done_and_its_dry_signal_is_unchanged() {
    for latency in [0u32, 1, 300] {
        let (probe, shared) = Probe::new();
        shared.set_out(ProbeOut::Constant(0.5));
        let (mut h, r) = harness(rig("sl.parked", &fx_layout(), Box::new(probe)));
        assert!(!r.manager.slot().set_latency(latency, SR));
        let input =
            |n: usize| -> Batch { std::array::from_fn(|i| ((n * BATCH + i) as f32) * 1e-3) };
        h.set_param("bypass", 1.0);
        let mut dry = Vec::new();
        for n in 0..400 {
            *h.input_mut("in_l") = input(n);
            *h.input_mut("in_r") = input(n);
            h.process();
            dry.extend_from_slice(h.output("out_l"));
        }
        let called = shared.calls();
        assert_eq!(called, 4, "latency {latency}: the fade only");
        // After the fade the output is the input delayed by the latency, bit for bit.
        let shift = latency as usize;
        for (i, s) in dry.iter().enumerate().skip(FADE + shift) {
            let want = (i - shift) as f32 * 1e-3;
            assert_eq!(*s, want, "latency {latency}, frame {i}");
        }
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
    for _ in 0..40 {
        h.process();
    }
    let parked = shared.calls();
    assert_eq!(parked, 4);
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
    for _ in 0..40 {
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
fn a_param_changed_while_bypassed_is_applied_after_the_bypass_is_switched_off() {
    let (mut h, shared, _r) = constant_effect(1.0);
    h.set_param("bypass", 1.0);
    for _ in 0..10 {
        h.process();
    }
    let parked = shared.calls();
    h.set_param("p1", 6.0);
    for _ in 0..10 {
        h.process();
    }
    assert_eq!(shared.calls(), parked, "not called, the change waits");
    h.set_param("bypass", 0.0);
    h.process();
    assert_eq!(shared.calls(), parked + 1);
    let log = shared.log.lock().unwrap();
    let seen = log.events_of_call(parked);
    assert!(
        seen.iter()
            .any(|e| matches!(e.kind, K::Param { value, .. } if (value - 6.0).abs() < 1e-9)),
        "{seen:?}"
    );
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
    for _ in 0..10 {
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

/// Renders a bypass that is switched on at block 20 and off at block 60 through a [`Lowpass`]
/// effect in `mode`; returns the left output and the plugin's calls... as frames of output.
fn lowpass_with_toggle(mode: RunMode) -> Vec<f32> {
    let (mut h, _r) = harness(rig(
        "sl.lowpass",
        &fx_layout(),
        Box::new(Lowpass { y: [0.0; 2] }),
    ));
    h.device_mut().set_run_mode(mode);
    let mut out = Vec::new();
    for n in 0..140 {
        match n {
            20 => h.set_param("bypass", 1.0),
            60 => h.set_param("bypass", 0.0),
            _ => {}
        }
        let x: Batch = std::array::from_fn(|i| (((n * BATCH + i) % 97) as f32 - 48.0) / 48.0);
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    out
}

/// The same render computed without the device: the filter runs through the whole input and the
/// bypass mix fades towards the delayed dry signal (no latency) 1/240 per frame.
fn lowpass_model() -> Vec<f32> {
    let mut y = 0.0f32;
    let mut m = 0.0f32;
    let step = 1.0 / FADE as f32;
    let mut out = Vec::new();
    for n in 0..140 {
        let target = if (20..60).contains(&n) { 1.0 } else { 0.0 };
        for i in 0..BATCH {
            let x = (((n * BATCH + i) % 97) as f32 - 48.0) / 48.0;
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
fn a_bounce_keeps_calling_a_bypassed_effect_and_renders_what_processing_all_along_renders() {
    // Offline the bypass never parks the plugin, so a bounce does not depend on when the bypass
    // came: the render is the one of a plugin that heard every block.
    assert_eq!(lowpass_with_toggle(RunMode::Offline), lowpass_model());
    // Live the plugin is cleared and pre-rolled when the bypass is switched off: the same input
    // gives the same output until the switch, and differs after it only where the plugin's own
    // state is what differs.
    let live = lowpass_with_toggle(RunMode::Realtime);
    let model = lowpass_model();
    let switch = 60 * BATCH;
    assert_eq!(live[..switch], model[..switch]);
    let dry: Vec<f32> = (switch..switch + PREROLL * BATCH)
        .map(|f| ((f % 97) as f32 - 48.0) / 48.0)
        .collect();
    assert_eq!(
        live[switch..switch + PREROLL * BATCH],
        dry[..],
        "the input itself through the pre-roll"
    );
    let settled = 100 * BATCH;
    for (a, b) in live[settled..].iter().zip(&model[settled..]) {
        assert!((a - b).abs() < 1e-3, "converged: {a} vs {b}");
    }
}

#[test]
fn an_offline_render_with_the_bypass_never_parks_the_plugin() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("sl.offline", &fx_layout(), Box::new(probe)));
    h.device_mut().set_run_mode(RunMode::Offline);
    h.set_param("bypass", 1.0);
    for _ in 0..50 {
        h.process();
    }
    assert_eq!(shared.calls(), 50);
}

#[test]
fn an_effect_parked_by_its_bypass_and_then_powered_off_still_pre_rolls_when_it_comes_back() {
    let (mut h, shared, r) = constant_effect(0.5);
    h.set_param("bypass", 1.0);
    for _ in 0..10 {
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
