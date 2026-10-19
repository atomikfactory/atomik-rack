//! Param messages apply at batch boundaries (D4).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::ParamId;
use atomik_engine::{DeviceInstanceId, Engine, EngineHandle, Graph, new_engine};
use atomik_testkit::fakes::ConstantSource;
use common::*;

fn constant_engine() -> (Engine, EngineHandle, DeviceInstanceId) {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(0.0));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let (engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle, k)
}

#[test]
fn render_param_change_applies_next_batch() {
    let (mut engine, mut handle, k) = constant_engine();
    // 100 host frames: batch 0 (timeline 0..64) has been processed.
    let mut out = render_blocks(&mut engine, &[], 1, 100, 100);
    handle.set_param(k, ConstantSource::VALUE, 0.5).unwrap();
    out[0].extend(render_blocks(&mut engine, &[], 1, 156, 13).remove(0));
    // Timeline frame n is host frame n + 64.
    let timeline = &out[0][64..];
    assert!(
        timeline[..64].iter().all(|s| s.abs() < 1e-7),
        "batch 0 unchanged"
    );
    assert!(
        timeline[64..].iter().all(|s| (s - 0.5).abs() < 1e-7),
        "batch 1 onwards"
    );
}

#[test]
fn render_param_is_clamped_by_descriptor() {
    let (mut engine, mut handle, k) = constant_engine();
    handle.set_param(k, ConstantSource::VALUE, 1e6).unwrap();
    let out = render_blocks(&mut engine, &[], 1, 128, 128);
    assert!(out[0][64..].iter().all(|s| (s - 10.0).abs() < 1e-6));
}

#[test]
fn render_param_for_unknown_target_is_ignored() {
    let (mut engine, mut handle, k) = constant_engine();
    handle
        .set_param(DeviceInstanceId(77), ParamId(0), 1.0)
        .unwrap();
    handle.set_param(k, ParamId(5), 1.0).unwrap();
    handle.set_param(Graph::INTERFACE, ParamId(0), 1.0).unwrap();
    let out = render_blocks(&mut engine, &[], 1, 128, 128);
    assert!(out[0].iter().all(|s| s.to_bits() == 0));
}
