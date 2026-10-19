//! (RT path, audio side): the device hands the plugin exactly 64-frame blocks, maps the
//! cabled sockets to the plugin's buses (stereo, mono, sidechain, silent flags), and the
//! output equals the reference DSP run directly.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, PPQ};
use atomik_device_sdk::Channels;
use atomik_plugin_host::PluginEventKind;
use atomik_test_plugin_core as core;
use support::*;

fn ramp(scale: f32) -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 100.0;
    }
    b
}

#[test]
fn the_plugin_is_called_with_exactly_one_block_per_batch_and_stereo_buffers() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.stereo", &fx_layout(), Box::new(probe)));
    h.set_connected("in_l", true);
    h.set_connected("in_r", true);
    for _ in 0..5 {
        h.process();
    }
    let log = shared.log.lock().unwrap();
    assert_eq!(log.calls, 5);
    assert_eq!(log.starts, 1, "start_processing once, on the first batch");
    assert!(log.frames.iter().all(|f| *f == BATCH));
    assert!(
        log.shapes.iter().all(|s| *s == (2, 2, 2)),
        "{:?}",
        log.shapes
    );
}

#[test]
fn stereo_input_reaches_the_plugin_and_its_output_reaches_the_cabled_outputs() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Gain(0.5));
    let (mut h, _r) = harness(rig("io.copy", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = ramp(1.0);
    *h.input_mut("in_r") = ramp(-2.0);
    h.process();
    let l: Vec<f32> = ramp(1.0).iter().map(|s| s * 0.5).collect();
    let r: Vec<f32> = ramp(-2.0).iter().map(|s| s * 0.5).collect();
    assert_eq!(&h.output("out_l")[..], &l[..]);
    assert_eq!(&h.output("out_r")[..], &r[..]);
    let log = shared.log.lock().unwrap();
    assert_eq!(log.last_in[0], ramp(1.0));
    assert_eq!(log.last_in[1], ramp(-2.0));
}

#[test]
fn unconnected_inputs_are_silent_buffers_with_the_silent_flags_set() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.silent", &fx_layout(), Box::new(probe)));
    h.process(); // nothing cabled
    h.input_mut("in_l")[3] = 1.0; // main input cabled
    h.process();
    h.input_mut("sc_in_r")[0] = 1.0; // sidechain too
    h.process();
    let log = shared.log.lock().unwrap();
    assert_eq!(log.silent[0], (true, true));
    assert_eq!(log.silent[1], (false, true));
    assert_eq!(log.silent[2], (false, false));
}

#[test]
fn a_mono_main_input_is_the_mean_of_the_cabled_inputs() {
    let mut layout = fx_layout();
    layout.main_in = Some(Channels::Mono);
    layout.main_out = Some(Channels::Mono);
    layout.sidechain = false;
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Gain(1.0));
    let (mut h, _r) = harness(rig("io.mono", &layout, Box::new(probe)));
    *h.input_mut("in_l") = ramp(1.0);
    *h.input_mut("in_r") = ramp(3.0);
    h.process();
    let mean: Vec<f32> = ramp(1.0)
        .iter()
        .zip(ramp(3.0).iter())
        .map(|(a, b)| (a + b) * 0.5)
        .collect();
    {
        let log = shared.log.lock().unwrap();
        assert_eq!(
            log.shapes[0],
            (1, 0, 1),
            "one input channel, one output channel"
        );
        assert_eq!(&log.last_in[0][..], &mean[..]);
    }
    // The mono output goes to both outputs.
    assert_eq!(&h.output("out_l")[..], &mean[..]);
    assert_eq!(&h.output("out_r")[..], &mean[..]);
}

#[test]
fn with_one_input_cabled_a_mono_plugin_gets_that_signal() {
    let mut layout = fx_layout();
    layout.main_in = Some(Channels::Mono);
    layout.sidechain = false;
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.mono1", &layout, Box::new(probe)));
    *h.input_mut("in_r") = ramp(4.0);
    h.process();
    assert_eq!(shared.log.lock().unwrap().last_in[0], ramp(4.0));
    // Only L cabled: L itself.
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.mono2", &layout, Box::new(probe)));
    *h.input_mut("in_l") = ramp(5.0);
    h.process();
    assert_eq!(shared.log.lock().unwrap().last_in[0], ramp(5.0));
}

#[test]
fn the_effect_output_equals_the_reference_dsp_run_directly() {
    let mut layout = fx_layout();
    layout.sidechain = true;
    let (mut h, _r) = harness(rig("io.ref", &layout, Box::new(CoreEffect::new())));
    let mut reference = core::GainFx::new(SR);
    h.set_param("p1", -4.5);
    h.set_param("p3", 37.0);
    reference.set_param(core::GainFx::GAIN, -4.5);
    reference.set_param(core::GainFx::LATENCY, 37.0);
    for n in 0..30 {
        let l = ramp(1.0 + n as f32 * 0.1);
        let r = ramp(-0.5 * n as f32);
        *h.input_mut("in_l") = l;
        *h.input_mut("in_r") = r;
        h.process();
        let mut want = [[0.0; BATCH]; 2];
        reference.process(&[l, r], None, &mut want);
        assert_eq!(*h.output("out_l"), want[0], "batch {n}");
        assert_eq!(*h.output("out_r"), want[1], "batch {n}");
    }
}

#[test]
fn a_cabled_sidechain_ducks_and_an_unconnected_one_is_silent() {
    let (mut h, _r) = harness(rig("io.sc", &fx_layout(), Box::new(CoreEffect::new())));
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [0.5; BATCH];
    h.process();
    assert_eq!(h.output("out_l")[63], 0.5, "no sidechain: no ducking");
    *h.input_mut("sc_in_l") = [1.0; BATCH];
    h.process();
    assert!(h.output("out_l")[63] < 0.2, "{}", h.output("out_l")[63]);
}

#[test]
fn the_transport_fields_follow_the_batch_time_info() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.transport", &fx_layout(), Box::new(probe)));
    {
        let t = h.time_mut();
        t.tick = 2.0 * PPQ as f64;
        t.loop_ticks = Some((PPQ as f64, 8.0 * PPQ as f64));
        t.time_signature = (7, 8);
        t.playing = true;
    }
    h.process();
    // A locate: the position jumps and the plugin sees the jump.
    {
        let t = h.time_mut();
        t.tick = 0.5 * PPQ as f64;
        t.playing = false;
        t.loop_ticks = None;
    }
    h.process();
    let log = shared.log.lock().unwrap();
    let a = log.transports[0];
    assert!(a.playing);
    assert_eq!(a.tempo_bpm, 120.0);
    assert_eq!(a.song_pos_beats, 2.0);
    assert!(
        (a.song_pos_seconds - 1.0).abs() < 1e-9,
        "{}",
        a.song_pos_seconds
    );
    assert_eq!(a.steady_frames, 0);
    assert_eq!(a.time_signature, (7, 8));
    assert!(a.loop_active);
    assert_eq!((a.loop_start_beats, a.loop_end_beats), (1.0, 8.0));
    assert_eq!(a.sample_rate, SR);
    let b = log.transports[1];
    assert!(!b.playing);
    assert_eq!(b.song_pos_beats, 0.5);
    assert_eq!(b.steady_frames, BATCH as u64);
    assert!(!b.loop_active);
}

#[test]
fn the_meter_values_report_peaks_state_latency_and_dropped() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Gain(0.5));
    let (mut h, r) = harness(rig("io.meters", &fx_layout(), Box::new(probe)));
    assert!(h.device().has_meter_values());
    h.input_mut("in_l")[10] = -0.8;
    h.process();
    let mut out = [0.0f32; 8];
    let n = h.device().meter_values(&mut out);
    assert_eq!(n, 5);
    assert!((out[0] - 0.8).abs() < 1e-6, "input peak {}", out[0]);
    assert!((out[1] - 0.4).abs() < 1e-6, "output peak {}", out[1]);
    assert_eq!(out[2], 0.0, "running");
    assert_eq!(out[3], 0.0, "latency");
    assert_eq!(out[4], 0.0, "dropped");
    r.manager.slot().set_latency(300, SR);
    h.process();
    h.device().meter_values(&mut out);
    assert_eq!(out[3], 300.0);
    // A short buffer gets the first values only.
    let mut two = [0.0f32; 2];
    assert_eq!(h.device().meter_values(&mut two), 2);
}

#[test]
fn param_events_never_carry_the_bypass_param() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("io.bypassparam", &fx_layout(), Box::new(probe)));
    h.set_param("bypass", 1.0);
    h.process();
    let log = shared.log.lock().unwrap();
    assert!(
        log.events
            .iter()
            .all(|e| !matches!(e.kind, PluginEventKind::Param { .. })),
        "bypass is host-owned"
    );
}
