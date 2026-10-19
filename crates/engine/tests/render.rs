//! Engine render behaviour (05 "Render" tests).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Graph, new_engine};
use common::*;

#[test]
fn render_without_plan_outputs_silence() {
    let (mut engine, _handle) = new_engine(cfg(2, 2));
    let input = vec![vec![0.5; 300]; 2];
    let out = render_blocks(&mut engine, &input, 2, 300, 100);
    for ch in &out {
        assert!(ch.iter().all(|s| s.to_bits() == 0), "silence");
    }
    // Zero-length renders are accepted.
    let out = render_block(&mut engine, &[vec![], vec![]], 2, 0);
    assert!(out.iter().all(Vec::is_empty));
    assert_eq!(engine.latency_frames(), 64);
    let _ = Graph::INTERFACE;
}

fn ramp(n: usize, scale: f32) -> Vec<f32> {
    (0..n).map(|i| (i as f32 + 1.0) * scale).collect()
}

#[test]
fn render_interface_passthrough_delays_by_one_batch_for_any_block() {
    for block in [1, 7, 63, 64, 65, 100, 1000] {
        let c = cfg(2, 2);
        let mut g = Graph::new(&c);
        g.connect(iface(&g, "in_1"), iface(&g, "out_2")).unwrap();
        g.connect(iface(&g, "in_2"), iface(&g, "out_1")).unwrap();
        let (mut engine, mut handle) = new_engine(c);
        handle.submit_plan(g.compile().unwrap()).unwrap();
        let input = vec![ramp(1000, 1e-3), ramp(1000, -1e-3)];
        let out = render_blocks(&mut engine, &input, 2, 1000, block);
        let latency = engine.latency_frames() as usize;
        assert!(
            out[0][..latency].iter().all(|s| s.to_bits() == 0),
            "block {block}"
        );
        assert_close(&out[1][latency..], &input[0][..1000 - latency], 0.0);
        assert_close(&out[0][latency..], &input[1][..1000 - latency], 0.0);
    }
}

#[test]
fn render_time_info_frame_is_batch_start() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let (log, counts) = atomik_testkit::fakes::CallLog::boxed();
    g.add_device(log);
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    render_blocks(&mut engine, &[], 1, 200, 30);
    assert_eq!(counts.process(), 3, "only complete batches are processed");
    assert_eq!(counts.last_frame(), 128);
}

#[test]
fn render_missing_and_extra_host_channels_are_silent() {
    let c = cfg(1, 1);
    let mut g = Graph::new(&c);
    g.connect(iface(&g, "in_1"), iface(&g, "out_1")).unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    // Host passes no input channels and three output channels.
    let out = render_blocks(&mut engine, &[], 3, 256, 256);
    for ch in &out {
        assert!(ch.iter().all(|s| s.to_bits() == 0));
    }
}

#[test]
fn render_fan_out_delivers_identical_signals() {
    use atomik_testkit::fakes::{Passthrough, RecordingSink};
    let c = cfg(1, 2);
    let mut g = Graph::new(&c);
    let (s1, r1) = RecordingSink::boxed(512);
    let (s2, r2) = RecordingSink::boxed(512);
    let a = g.add_device(Passthrough::boxed(0.5));
    let s1 = g.add_device(s1);
    let s2 = g.add_device(s2);
    // in_1 fans out to the passthrough and both host outputs; the passthrough fans out to
    // two recording sinks.
    g.connect(iface(&g, "in_1"), ep(a, Passthrough::AUDIO_IN))
        .unwrap();
    g.connect(iface(&g, "in_1"), iface(&g, "out_1")).unwrap();
    g.connect(iface(&g, "in_1"), iface(&g, "out_2")).unwrap();
    g.connect(
        ep(a, Passthrough::AUDIO_OUT),
        ep(s1, RecordingSink::AUDIO_IN),
    )
    .unwrap();
    g.connect(
        ep(a, Passthrough::AUDIO_OUT),
        ep(s2, RecordingSink::AUDIO_IN),
    )
    .unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let input = vec![
        (0..512)
            .map(|i| ((i * 37) % 101) as f32 / 101.0 - 0.5)
            .collect::<Vec<f32>>(),
    ];
    let out = render_blocks(&mut engine, &input, 2, 512, 100);
    assert_close(&out[0], &out[1], 0.0);
    let (a1, a2) = (r1.samples(), r2.samples());
    assert_eq!(a1.len(), 512, "8 complete batches reached the sinks");
    assert_close(&a1, &a2, 0.0);
    let halved: Vec<f32> = input[0].iter().map(|x| x * 0.5).collect();
    assert_close(&a1, &halved, 1e-7);
}

#[test]
fn render_unconnected_input_reads_zeros_and_outputs_are_zeroed() {
    use atomik_testkit::fakes::{ConstantSource, Passthrough};
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(0.75));
    let p = g.add_device(Passthrough::boxed(2.0));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.connect(ep(p, Passthrough::AUDIO_OUT), iface(&g, "out_2"))
        .unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let out = render_blocks(&mut engine, &[], 2, 256, 256);
    assert!(out[0][64..].iter().all(|s| (s - 0.75).abs() < 1e-7));
    assert!(out[1].iter().all(|s| s.to_bits() == 0));
}
