//! Plan swaps at batch boundaries; retired plans are dropped off the RT thread.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Graph, RETIRE_QUEUE_CAPACITY, RenderPlan, new_engine};
use atomik_testkit::fakes::{ConstantSource, DropProbe, DropRecord};
use common::*;
use std::sync::mpsc;

fn constant_plan(value: f32) -> RenderPlan {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(value));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.compile().unwrap()
}

fn probe_plan() -> (RenderPlan, DropRecord) {
    let mut g = Graph::new(&cfg(0, 1));
    let (probe, record) = DropProbe::boxed();
    g.add_device(probe);
    (g.compile().unwrap(), record)
}

#[test]
fn render_plan_swap_happens_at_batch_boundary() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    handle.submit_plan(constant_plan(0.25)).unwrap();
    let mut out = render_blocks(&mut engine, &[], 1, 100, 100).remove(0);
    handle.submit_plan(constant_plan(0.75)).unwrap();
    out.extend(render_blocks(&mut engine, &[], 1, 156, 7).remove(0));
    let timeline = &out[64..];
    assert!(timeline[..64].iter().all(|s| (s - 0.25).abs() < 1e-7));
    assert!(timeline[64..].iter().all(|s| (s - 0.75).abs() < 1e-7));
    assert_eq!(handle.collect_garbage(), 1);
    assert_eq!(handle.collect_garbage(), 0);
}

#[test]
fn render_plan_swap_retires_old_plan_off_rt_thread() {
    let (engine, mut handle) = new_engine(cfg(0, 1));
    let (plan, record) = probe_plan();
    handle.submit_plan(plan).unwrap();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let rt = std::thread::spawn(move || {
        let mut engine = engine;
        render_blocks(&mut engine, &[], 1, 128, 32); // installs the probe plan
        done_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        render_blocks(&mut engine, &[], 1, 128, 32); // swaps: the probe plan is retired
        engine
    });
    done_rx.recv().unwrap();
    handle.submit_plan(constant_plan(0.0)).unwrap();
    go_tx.send(()).unwrap();
    let engine = rt.join().expect("render thread");
    assert_eq!(record.thread(), None, "not dropped by the render thread");
    assert_eq!(handle.collect_garbage(), 1);
    assert_eq!(record.thread(), Some(std::thread::current().id()));
    drop(engine);
}

#[test]
fn render_full_retire_queue_parks_plan_and_postpones_swaps() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    let total = RETIRE_QUEUE_CAPACITY + 3;
    let mut records = Vec::new();
    for _ in 0..total {
        let (plan, record) = probe_plan();
        records.push(record);
        handle.submit_plan(plan).unwrap();
        render_blocks(&mut engine, &[], 1, 64, 64);
    }
    // Nothing was collected: the queue holds CAPACITY plans, one is parked, one still waits.
    assert!(records.iter().all(|r| r.thread().is_none()));
    assert_eq!(handle.collect_garbage(), RETIRE_QUEUE_CAPACITY);
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert_eq!(handle.collect_garbage(), 2);
    let main = Some(std::thread::current().id());
    assert!(records[..total - 1].iter().all(|r| r.thread() == main));
    assert_eq!(
        records[total - 1].thread(),
        None,
        "the newest plan is running"
    );
}
