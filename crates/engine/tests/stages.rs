//! Device processing stages. Loops are checked per (device, stage); render order,
//! worker levels and PDC run over (device, stage) nodes.

mod common;

use atomik_core::{BATCH, NoteKind};
use atomik_device_sdk::SocketId;
use atomik_engine::{
    DeviceInstanceId, Endpoint, Graph, GraphError, OfflineJob, ScheduledNote, Topology,
    render_offline, render_offline_scripted,
};
use atomik_testkit::fakes::{
    INSERT_LOOP, ImpulseOnNote, InsertLoop, LatencyPassthrough, PASSTHROUGH, Passthrough,
    SummingBus,
};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::{bits, cfg, engine_with, ep};
use std::sync::atomic::Ordering;

atomik_testkit::install_alloc_guard!();

fn out(k: u16) -> Endpoint {
    ep(Graph::INTERFACE, SocketId(k - 1))
}

fn click(frame: u64, device: DeviceInstanceId) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: ImpulseOnNote::NOTES_IN,
        kind: NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    }
}

fn nonzero(signal: &[f32]) -> Vec<(usize, f32)> {
    signal
        .iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(n, s)| (n, *s))
        .collect()
}

#[test]
fn an_insert_loop_is_accepted_and_a_true_loop_is_rejected() {
    let mut g = Graph::new(&cfg(0, 1));
    let src = g.add_device(ImpulseOnNote::boxed());
    let ins = g.add_device(InsertLoop::boxed());
    let fx = g.add_device(Passthrough::boxed(2.0));
    let p = g.add_device(Passthrough::boxed(1.0));
    g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), ep(ins, InsertLoop::IN))
        .unwrap();
    // Stage 0 -> effect -> stage 1 of the same device: not a loop.
    assert_eq!(
        g.connect(ep(ins, InsertLoop::SEND), ep(fx, Passthrough::AUDIO_IN)),
        Ok(())
    );
    assert_eq!(
        g.connect(ep(fx, Passthrough::AUDIO_OUT), ep(ins, InsertLoop::RET)),
        Ok(())
    );
    // Stage 1 back into stage 0 (through a device, or directly): a loop.
    g.connect(ep(ins, InsertLoop::OUT), ep(p, Passthrough::AUDIO_IN))
        .unwrap();
    g.disconnect(ep(ins, InsertLoop::IN)).unwrap();
    assert_eq!(
        g.connect(ep(p, Passthrough::AUDIO_OUT), ep(ins, InsertLoop::IN)),
        Err(GraphError::WouldCreateCycle)
    );
    let mut g2 = Graph::new(&cfg(0, 1));
    let a = g2.add_device(InsertLoop::boxed());
    assert_eq!(
        g2.connect(ep(a, InsertLoop::OUT), ep(a, InsertLoop::IN)),
        Err(GraphError::WouldCreateCycle)
    );
    assert_eq!(
        g2.connect(ep(a, InsertLoop::SEND), ep(a, InsertLoop::RET)),
        Ok(()),
        "a device's own stage 0 may feed its stage 1"
    );

    // Topology: the same rules.
    let mut t = Topology::new(&cfg(0, 1));
    let (ins, fx, p) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    t.add_node(ins, &INSERT_LOOP).unwrap();
    t.add_node(fx, &PASSTHROUGH).unwrap();
    t.add_node(p, &PASSTHROUGH).unwrap();
    t.connect(ep(ins, InsertLoop::SEND), ep(fx, Passthrough::AUDIO_IN))
        .unwrap();
    t.connect(ep(fx, Passthrough::AUDIO_OUT), ep(ins, InsertLoop::RET))
        .unwrap();
    t.connect(ep(ins, InsertLoop::OUT), ep(p, Passthrough::AUDIO_IN))
        .unwrap();
    assert_eq!(
        t.connect(ep(p, Passthrough::AUDIO_OUT), ep(ins, InsertLoop::IN)),
        Err(GraphError::WouldCreateCycle)
    );
    let routing = t.compile().unwrap();
    // (ins,0) -> fx -> (ins,1) -> p: one node per level; the device appears once per stage.
    let levels: Vec<Vec<_>> = routing.levels().map(<[_]>::to_vec).collect();
    assert_eq!(levels, vec![vec![ins], vec![fx], vec![ins], vec![p]]);
    assert_eq!(routing.order(), &[ins, fx, ins, p]);
}

#[test]
fn an_insert_loop_renders_sample_exact_with_no_added_latency() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let src = g.add_device(ImpulseOnNote::boxed());
    let (ins_dev, calls) = InsertLoop::with_latency(0);
    let ins = g.add_device(ins_dev);
    let fx = g.add_device(Passthrough::boxed(2.0));
    g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), ep(ins, InsertLoop::IN))
        .unwrap();
    g.connect(ep(ins, InsertLoop::SEND), ep(fx, Passthrough::AUDIO_IN))
        .unwrap();
    g.connect(ep(fx, Passthrough::AUDIO_OUT), ep(ins, InsertLoop::RET))
        .unwrap();
    g.connect(ep(ins, InsertLoop::OUT), out(1)).unwrap();
    let plan = g.compile().unwrap();
    assert_eq!(plan.order(), &[src, ins, fx, ins]);
    let got = render_offline(plan, c, 4 * BATCH, 50, &[click(77, src)], &[]);
    // out = 2 * in (through the effect) + 0.5 * in (dry, read again in stage 1), same frame.
    assert_eq!(nonzero(&got[0]), vec![(77, 2.5)]);
    let batches = calls[0].load(Ordering::Relaxed);
    assert_eq!(batches, calls[1].load(Ordering::Relaxed));
    assert!(batches >= 4, "each stage runs once per batch");
}

#[test]
fn pdc_applies_device_latency_to_the_last_stage() {
    // click -> ins(reports 7).in; ins.send -> L10 -> ins.ret; ins.send -> bus;
    // ins.out -> bus. Stage 0 (send) has latency 0, stage 1 (out) max(0, 10) + 7 = 17.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let src = g.add_device(ImpulseOnNote::boxed());
    let (ins_dev, _) = InsertLoop::with_latency(7);
    let ins = g.add_device(ins_dev);
    let l10 = g.add_device(LatencyPassthrough::boxed(10));
    let bus = g.add_device(SummingBus::boxed());
    g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), ep(ins, InsertLoop::IN))
        .unwrap();
    g.connect(
        ep(ins, InsertLoop::SEND),
        ep(l10, LatencyPassthrough::AUDIO_IN),
    )
    .unwrap();
    g.connect(
        ep(l10, LatencyPassthrough::AUDIO_OUT),
        ep(ins, InsertLoop::RET),
    )
    .unwrap();
    g.connect(ep(ins, InsertLoop::SEND), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(ins, InsertLoop::OUT), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out(1)).unwrap();
    let plan = g.compile().unwrap();
    let report = plan.pdc_report();
    assert_eq!(
        report.latencies,
        vec![
            (ep(ins, InsertLoop::SEND), 0),
            (ep(ins, InsertLoop::OUT), 17)
        ]
    );
    assert_eq!(
        report.cable_delays,
        vec![
            (ep(ins, InsertLoop::SEND), ep(bus, SummingBus::SUM_IN), 17),
            (ep(ins, InsertLoop::OUT), ep(bus, SummingBus::SUM_IN), 0),
        ]
    );
    let got = render_offline(plan, c, 4 * BATCH, 64, &[click(5, src)], &[]);
    // The send path is delayed 17; the loop path really takes 10 (the fake only reports 7):
    // dry 0.5 at 5, wet 1.0 at 15, send copy 1.0 at 22.
    assert_eq!(nonzero(&got[0]), vec![(5, 0.5), (15, 1.0), (22, 1.0)]);
}

/// Six click channels, each with an insert loop (a latency effect inside), all summed into a
/// bus with PDC; the loops sit at shared levels so workers run several stage nodes per level.
fn staged_console(c: &atomik_engine::EngineConfig) -> (Graph, Vec<ScheduledNote>) {
    let mut g = Graph::new(c);
    let bus = g.add_device(SummingBus::boxed());
    let mut notes = Vec::new();
    for k in 0..6u32 {
        let src = g.add_device(ImpulseOnNote::boxed());
        let ins = g.add_device(InsertLoop::boxed());
        let fx = g.add_device(LatencyPassthrough::boxed(k * 17 % 41));
        g.set_param(ins, InsertLoop::DRY, 0.125 * k as f32).unwrap();
        g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), ep(ins, InsertLoop::IN))
            .unwrap();
        g.connect(
            ep(ins, InsertLoop::SEND),
            ep(fx, LatencyPassthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(
            ep(fx, LatencyPassthrough::AUDIO_OUT),
            ep(ins, InsertLoop::RET),
        )
        .unwrap();
        g.connect(ep(ins, InsertLoop::OUT), ep(bus, SummingBus::SUM_IN))
            .unwrap();
        for j in 0..10u64 {
            notes.push(click(j * 263 + u64::from(k) * 3, src));
        }
    }
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out(1)).unwrap();
    notes.sort_by_key(|n| n.frame);
    (g, notes)
}

#[test]
fn staged_bounce_is_bit_identical_across_blocks_and_workers_without_allocation() {
    let c = cfg(0, 1);
    let mut results = Vec::new();
    let mut engines = Vec::new();
    for threads in [0u16, 3] {
        for block in [1usize, 64, 333] {
            let (g, notes) = staged_console(&c);
            let plan = g.compile().unwrap();
            assert!(plan.pdc_report().active);
            let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
            handle.submit_plan(plan).unwrap();
            let job = OfflineJob {
                frames: 3000,
                host_block: block,
                notes: &notes,
                params: &[],
                inputs: &[],
            };
            let got = render_offline_scripted(&mut engine, &mut handle, &job, Vec::new(), |f| {
                assert_no_alloc(f);
            });
            assert_eq!(worker_violations(), 0, "threads {threads}, block {block}");
            engines.push((engine, handle));
            results.push(((threads, block), bits(&got)));
        }
    }
    assert_eq!(worker_violations(), 0);
    let first = &results[0].1;
    assert!(first[0].iter().any(|b| *b != 0));
    for (key, r) in &results[1..] {
        assert_eq!(r, first, "{key:?}");
    }
    // With PDC, every channel's clicks (dry and wet) line up at the bus: the max loop latency.
    let max = (0..6u32).map(|k| k * 17 % 41).max().unwrap() as usize;
    let (_, notes) = staged_console(&c);
    let signal: Vec<f32> = first[0].iter().map(|b| f32::from_bits(*b)).collect();
    for n in &notes {
        let at = n.frame as usize + max;
        if at < signal.len() {
            assert_ne!(signal[at], 0.0, "click at {} aligned to {at}", n.frame);
        }
    }
}
