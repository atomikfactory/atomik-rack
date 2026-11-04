//! A device that panics outside `process` (taking a param, answering a meter read) is contained
//! per device, like one that panics in `process`: only that device is silenced and named, the
//! others keep sounding, and the healthy control path allocates nothing.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::BATCH;
use atomik_engine::{DeviceInstanceId, Engine, EngineHandle, Graph};
use atomik_testkit::fakes::Brittle;
use common::*;

/// Two `Brittle` devices, one per output channel, and the engine rendering them with `threads`
/// workers.
fn pair(threads: u16) -> (Engine, EngineHandle, DeviceInstanceId, DeviceInstanceId) {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let a = g.add_device(Brittle::boxed());
    let b = g.add_device(Brittle::boxed());
    g.connect(ep(a, Brittle::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.connect(ep(b, Brittle::AUDIO_OUT), iface(&g, "out_2"))
        .unwrap();
    let (engine, mut handle) = engine_with(c, threads, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle, a, b)
}

/// Renders `blocks` host blocks of one batch without the allocation guard (a panic allocates by
/// design) and returns the last block of both channels.
fn render_unguarded(engine: &mut Engine, blocks: usize) -> [Vec<f32>; 2] {
    let mut left = vec![0.0f32; BATCH];
    let mut right = vec![0.0f32; BATCH];
    for _ in 0..blocks {
        engine.render(&[], &mut [&mut left[..], &mut right[..]]);
    }
    [left, right]
}

fn sounds(channel: &[f32]) -> bool {
    channel.iter().all(|s| (s - Brittle::LEVEL).abs() < 1e-7)
}

#[test]
fn a_param_that_panics_silences_only_its_device_and_names_it() {
    for threads in [0u16, 3] {
        let (mut engine, mut handle, a, b) = pair(threads);
        let [left, right] = render_unguarded(&mut engine, 4);
        assert!(sounds(&left) && sounds(&right), "{threads} workers");
        assert_eq!(handle.fault(), None);

        handle.set_param(a, Brittle::BREAK_PARAM, 1.0).unwrap();
        let [left, right] = render_unguarded(&mut engine, 4);
        assert_eq!(handle.fault(), Some(a), "{threads} workers");
        assert_eq!(engine.fault(), Some(a));
        assert!(
            left.iter().all(|s| *s == 0.0),
            "the broken device is silent"
        );
        assert!(sounds(&right), "the other device keeps sounding");

        // A later fault of the other device does not rename the first one.
        handle.set_param(b, Brittle::BREAK_PARAM, 1.0).unwrap();
        let [left, right] = render_unguarded(&mut engine, 4);
        assert_eq!(handle.fault(), Some(a));
        assert!(left.iter().all(|s| *s == 0.0));
        assert!(right.iter().all(|s| *s == 0.0));
    }
}

#[test]
fn a_meter_read_that_panics_is_contained_per_device() {
    let (mut engine, handle, a, b) = pair(0);
    let mut handle = handle;
    let [_, _] = render_unguarded(&mut engine, 2);
    // Arming is harmless; the next batch's read panics.
    handle.set_param(b, Brittle::BREAK_METER, 1.0).unwrap();
    let [left, right] = render_unguarded(&mut engine, 4);
    assert_eq!(handle.fault(), Some(b));
    assert!(sounds(&left), "the healthy device is untouched");
    assert!(right.iter().all(|s| *s == 0.0));
    let _ = a;
}

#[test]
fn a_faulted_device_ignores_further_params() {
    let (mut engine, mut handle, a, _) = pair(0);
    handle.set_param(a, Brittle::BREAK_PARAM, 1.0).unwrap();
    render_unguarded(&mut engine, 2);
    assert_eq!(handle.fault(), Some(a));
    // Once faulted the device is never called again: more params neither panic nor sound.
    for _ in 0..10 {
        handle.set_param(a, Brittle::BREAK_PARAM, 1.0).unwrap();
        let [left, _] = render_unguarded(&mut engine, 1);
        assert!(left.iter().all(|s| *s == 0.0));
    }
}

#[test]
fn healthy_params_and_meter_reads_allocate_nothing() {
    let (mut engine, mut handle, a, b) = pair(0);
    render_blocks(&mut engine, &[], 2, BATCH * 4, BATCH);
    for round in 0..16 {
        let v = if round % 2 == 0 { 0.2 } else { 0.0 };
        handle.set_param(a, Brittle::BREAK_PARAM, v).unwrap();
        handle.set_param(b, Brittle::BREAK_METER, v).unwrap();
        let out = render_blocks(&mut engine, &[], 2, BATCH, BATCH);
        assert!(sounds(&out[0]) && sounds(&out[1]));
    }
    assert_eq!(handle.fault(), None);
}
