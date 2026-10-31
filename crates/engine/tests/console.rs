//! Engine features: summing inputs, delay compensation, console bounce determinism, no allocation and meters.

mod common;

use atomik_core::{BATCH, NoteKind, SampleRate};
use atomik_device_sdk::SocketId;
use atomik_engine::{
    Cable, DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineHandle, Graph, GraphError,
    HotEdit, MAX_COMPENSATION_SECONDS, METER_CAPACITY, OfflineJob, PdcReport, RenderPlan,
    ScheduledNote, TimedEdit, Topology, new_engine, render_offline, render_offline_scripted,
};
use atomik_testkit::fakes::{
    ConstantSource, ImpulseOnNote, LatencyPassthrough, NoteLog, NoteThru, Passthrough, SummingBus,
};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::{bits, cfg, engine_with, ep};
use proptest::prelude::*;

atomik_testkit::install_alloc_guard!();

fn out(k: u16) -> Endpoint {
    // cfg(0, n): the interface has no inputs, so out_k is socket k - 1.
    ep(Graph::INTERFACE, SocketId(k - 1))
}

fn on(frame: u64, device: DeviceInstanceId, socket: SocketId, key: u8) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket,
        kind: NoteKind::On { key, velocity: 1.0 },
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

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

#[test]
fn summing_input_accepts_many_cables_and_a_plain_input_only_one() {
    let mut g = Graph::new(&cfg(0, 2));
    let a = g.add_device(ConstantSource::boxed(1.0));
    let b = g.add_device(ConstantSource::boxed(2.0));
    let bus = g.add_device(SummingBus::boxed());
    let p = g.add_device(Passthrough::boxed(1.0));
    let sum = ep(bus, SummingBus::SUM_IN);
    assert_eq!(g.connect(ep(b, ConstantSource::AUDIO_OUT), sum), Ok(()));
    assert_eq!(g.connect(ep(a, ConstantSource::AUDIO_OUT), sum), Ok(()));
    assert_eq!(
        g.connect(ep(a, ConstantSource::AUDIO_OUT), sum),
        Err(GraphError::InputAlreadyConnected),
        "the same cable twice"
    );
    // Plain input: one cable.
    let plain = ep(bus, SummingBus::AUDIO_IN);
    assert_eq!(g.connect(ep(a, ConstantSource::AUDIO_OUT), plain), Ok(()));
    assert_eq!(
        g.connect(ep(b, ConstantSource::AUDIO_OUT), plain),
        Err(GraphError::InputAlreadyConnected)
    );
    // Kind and direction rules still apply.
    assert_eq!(
        g.connect(ep(a, ConstantSource::CV_OUT), sum),
        Err(GraphError::KindMismatch)
    );
    assert_eq!(
        g.connect(sum, ep(p, Passthrough::AUDIO_IN)),
        Err(GraphError::DirectionMismatch)
    );
    // The cycle check covers summing edges.
    g.connect(ep(bus, SummingBus::AUDIO_OUT), ep(p, Passthrough::AUDIO_IN))
        .unwrap();
    assert_eq!(
        g.connect(ep(p, Passthrough::AUDIO_OUT), sum),
        Err(GraphError::WouldCreateCycle)
    );
    // Cables are listed by input endpoint, then by source endpoint.
    let into_sum: Vec<Cable> = g.cables().filter(|c| c.to == sum).copied().collect();
    assert_eq!(
        into_sum.iter().map(|c| c.from.device).collect::<Vec<_>>(),
        vec![a, b]
    );
    // disconnect_cable removes one cable; disconnect removes every cable into the input.
    assert_eq!(
        g.disconnect_cable(ep(b, ConstantSource::AUDIO_OUT), sum),
        Ok(Cable {
            from: ep(b, ConstantSource::AUDIO_OUT),
            to: sum
        })
    );
    assert_eq!(
        g.disconnect_cable(ep(b, ConstantSource::AUDIO_OUT), sum),
        Err(GraphError::NotConnected)
    );
    g.connect(ep(b, ConstantSource::AUDIO_OUT), sum).unwrap();
    assert_eq!(
        g.disconnect(sum),
        Ok(Cable {
            from: ep(a, ConstantSource::AUDIO_OUT),
            to: sum
        }),
        "returns the first removed cable"
    );
    assert_eq!(g.cables().filter(|c| c.to == sum).count(), 0);
    assert_eq!(g.disconnect(sum), Err(GraphError::NotConnected));
    assert!(g.compile().is_ok());
}

#[test]
fn topology_summing_rules_match_the_graph() {
    let mut t = Topology::new(&cfg(0, 2));
    let (a, b, bus) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    t.add_node(a, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(b, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    let sum = ep(bus, SummingBus::SUM_IN);
    t.connect(ep(b, ConstantSource::AUDIO_OUT), sum).unwrap();
    t.connect(ep(a, ConstantSource::AUDIO_OUT), sum).unwrap();
    assert_eq!(
        t.connect(ep(a, ConstantSource::AUDIO_OUT), sum),
        Err(GraphError::InputAlreadyConnected)
    );
    assert_eq!(t.cables().count(), 2);
    assert_eq!(
        t.disconnect_cable(ep(a, ConstantSource::AUDIO_OUT), sum),
        Ok(())
    );
    assert_eq!(
        t.disconnect_cable(ep(a, ConstantSource::AUDIO_OUT), sum),
        Err(GraphError::NotConnected)
    );
    assert_eq!(
        t.disconnect_cable(ep(DeviceInstanceId(9), SocketId(0)), sum),
        Err(GraphError::NotConnected)
    );
    t.connect(ep(a, ConstantSource::AUDIO_OUT), sum).unwrap();
    assert_eq!(t.disconnect(sum), Ok(()));
    assert_eq!(t.cables().count(), 0);
    t.connect(ep(a, ConstantSource::AUDIO_OUT), sum).unwrap();
    t.connect(ep(b, ConstantSource::AUDIO_OUT), sum).unwrap();
    t.remove_node(a).unwrap();
    assert_eq!(t.cables().count(), 1);
    assert!(t.compile().is_ok());
}

// ---------------------------------------------------------------------------------------------
// Summing
// ---------------------------------------------------------------------------------------------

/// Builds `values.len()` constant sources (ids 1..), connected to a bus summing input in the
/// order given by `connect_order`, with the bus routed to out_1.
fn constant_sum(values: &[f32], connect_order: &[usize]) -> RenderPlan {
    let mut g = Graph::new(&cfg(0, 1));
    let ids: Vec<_> = values
        .iter()
        .map(|v| g.add_device(ConstantSource::boxed(*v)))
        .collect();
    let bus = g.add_device(SummingBus::boxed());
    for &k in connect_order {
        g.connect(
            ep(ids[k], ConstantSource::AUDIO_OUT),
            ep(bus, SummingBus::SUM_IN),
        )
        .unwrap();
    }
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out(1)).unwrap();
    g.compile().unwrap()
}

#[test]
fn summing_is_a_left_fold_in_source_order_whatever_the_connect_order() {
    // In f32, (1e8 + 1) - 1e8 == 0 while (1e8 - 1e8) + 1 == 1: the order is observable.
    let values = [1.0e8f32, 1.0, -1.0e8, 0.1, 3.0e-3];
    let reference = values.iter().fold(0.0f32, |acc, v| acc + v);
    for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0], [2, 0, 4, 1, 3]] {
        let plan = constant_sum(&values, &order);
        let got = render_offline(plan, cfg(0, 1), 3 * BATCH, 64, &[], &[]);
        assert!(
            got[0].iter().all(|s| s.to_bits() == reference.to_bits()),
            "{order:?}: {} vs {reference}",
            got[0][0]
        );
    }
    // A different id order is a different (but equally deterministic) sum.
    let swapped = [1.0e8f32, -1.0e8, 1.0, 0.1, 3.0e-3];
    let reference2 = swapped.iter().fold(0.0f32, |acc, v| acc + v);
    assert_ne!(reference.to_bits(), reference2.to_bits());
    let got = render_offline(
        constant_sum(&swapped, &[0, 1, 2, 3, 4]),
        cfg(0, 1),
        BATCH,
        7,
        &[],
        &[],
    );
    assert_eq!(got[0][5].to_bits(), reference2.to_bits());
}

#[test]
fn cv_summing_and_note_merging_are_deterministic() {
    let mut g = Graph::new(&cfg(0, 2));
    let c1 = g.add_device(ConstantSource::boxed(0.25));
    let c2 = g.add_device(ConstantSource::boxed(-0.125));
    let thru_a = g.add_device(NoteThru::boxed());
    let thru_b = g.add_device(NoteThru::boxed());
    let bus = g.add_device(SummingBus::boxed());
    let (log, events) = NoteLog::boxed(16);
    let log_id = g.add_device(log);
    for c in [c2, c1] {
        g.connect(
            ep(c, ConstantSource::CV_OUT),
            ep(bus, SummingBus::CV_SUM_IN),
        )
        .unwrap();
    }
    for t in [thru_b, thru_a] {
        g.connect(
            ep(t, NoteThru::NOTES_OUT),
            ep(bus, SummingBus::NOTES_SUM_IN),
        )
        .unwrap();
    }
    g.connect(
        ep(bus, SummingBus::NOTES_OUT),
        ep(log_id, NoteLog::NOTES_IN),
    )
    .unwrap();
    let plan = g.compile().unwrap();
    let notes = [
        on(10, thru_b, NoteThru::NOTES_IN, 61),
        on(5, thru_b, NoteThru::NOTES_IN, 62),
        on(10, thru_a, NoteThru::NOTES_IN, 60),
        on(10, bus, SummingBus::NOTES_SUM_IN, 70),
        on(70, thru_a, NoteThru::NOTES_IN, 63),
    ];
    let _ = render_offline(plan, cfg(0, 2), 3 * BATCH, 13, &notes, &[]);
    let keys: Vec<(u64, u8)> = events
        .events()
        .into_iter()
        .map(|(f, k)| match k {
            NoteKind::On { key, .. } => (f, key),
            other => panic!("{other:?}"),
        })
        .collect();
    // Ties: scheduled notes first, then cables in source order (thru_a before thru_b).
    assert_eq!(keys, vec![(5, 62), (10, 70), (10, 60), (10, 61), (70, 63)]);

    // Cv: summed into the bus's cv_out, observed through a meter.
    let c = cfg(0, 1);
    let (mut engine, mut handle) = new_engine(c);
    let mut t = Topology::new(&c);
    let (c1, c2, bus) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    for (id, v) in [(c1, 0.25), (c2, -0.125)] {
        handle
            .insert_device(id, ConstantSource::boxed(v), &[])
            .unwrap();
        t.add_node(id, &atomik_testkit::fakes::CONSTANT_SOURCE)
            .unwrap();
        t.connect(
            ep(id, ConstantSource::CV_OUT),
            ep(bus, SummingBus::CV_SUM_IN),
        )
        .unwrap_err(); // the bus is not a node yet
    }
    handle.insert_device(bus, SummingBus::boxed(), &[]).unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    for id in [c2, c1] {
        t.connect(
            ep(id, ConstantSource::CV_OUT),
            ep(bus, SummingBus::CV_SUM_IN),
        )
        .unwrap();
    }
    t.add_meter(ep(bus, SummingBus::CV_OUT)).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    let mut outs = vec![0.0f32; BATCH];
    engine.render(&[], &mut [&mut outs]);
    let r = handle.meters().read(ep(bus, SummingBus::CV_OUT)).unwrap();
    assert_eq!((r.peak, r.rms), (0.125, 0.125));
}

// ---------------------------------------------------------------------------------------------
// Delay compensation
// ---------------------------------------------------------------------------------------------

/// One branch: a chain of LatencyPassthroughs from the click source into a summing bus.
type Branch = Vec<u32>;

struct Pdc {
    plan: RenderPlan,
    click: DeviceInstanceId,
    bus: DeviceInstanceId,
    /// The device feeding the bus, per branch (the click source itself for empty chains).
    tails: Vec<DeviceInstanceId>,
}

/// click (ImpulseOnNote) -> each branch chain -> bus.sum_in; bus -> out_1.
fn pdc_graph(c: &EngineConfig, branches: &[Branch], compensation: bool) -> Pdc {
    let mut g = Graph::new(c);
    let click = g.add_device(ImpulseOnNote::boxed());
    let bus = g.add_device(SummingBus::boxed());
    let mut tails = Vec::new();
    for chain in branches {
        let mut from = ep(click, ImpulseOnNote::AUDIO_OUT);
        let mut tail = click;
        for &latency in chain {
            let d = g.add_device(LatencyPassthrough::boxed(latency));
            g.connect(from, ep(d, LatencyPassthrough::AUDIO_IN))
                .unwrap();
            from = ep(d, LatencyPassthrough::AUDIO_OUT);
            tail = d;
        }
        // Two empty chains would be the same cable twice: skip duplicates.
        if g.connect(from, ep(bus, SummingBus::SUM_IN)).is_ok() {
            tails.push(tail);
        }
    }
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out(1)).unwrap();
    g.set_compensation(compensation);
    Pdc {
        plan: g.compile().unwrap(),
        click,
        bus,
        tails,
    }
}

fn click_render(c: &EngineConfig, pdc: Pdc, at: u64, frames: usize, block: usize) -> Vec<f32> {
    let note = on(at, pdc.click, ImpulseOnNote::NOTES_IN, 60);
    render_offline(pdc.plan, *c, frames, block, &[note], &[]).swap_remove(0)
}

#[test]
fn compensation_aligns_clicks_at_a_summing_input_and_off_shows_the_offsets() {
    let c = cfg(0, 1);
    let branches: Vec<Branch> = vec![vec![], vec![100], vec![30, 40], vec![7]];
    let on_plan = pdc_graph(&c, &branches, true);
    let report = on_plan.plan.pdc_report().clone();
    assert!(report.active && !report.limit_exceeded);
    let bus = on_plan.bus;
    let delays: Vec<(DeviceInstanceId, u32)> = report
        .cable_delays
        .iter()
        .map(|(from, to, d)| {
            assert_eq!(*to, ep(bus, SummingBus::SUM_IN));
            (from.device, *d)
        })
        .collect();
    let t = &on_plan.tails;
    // Sorted by source endpoint (device id).
    let mut want = vec![(t[0], 100), (t[1], 0), (t[2], 30), (t[3], 93)];
    want.sort();
    assert_eq!(delays, want);
    let latencies: Vec<_> = report
        .latencies
        .iter()
        .map(|(e, l)| (e.device, *l))
        .collect();
    let mut want = vec![(t[0], 0), (t[1], 100), (t[2], 70), (t[3], 7)];
    want.sort();
    assert_eq!(latencies, want);

    let aligned = click_render(&c, on_plan, 130, 8 * BATCH, 64);
    assert_eq!(
        nonzero(&aligned),
        vec![(230, 4.0)],
        "all four clicks at 130 + 100"
    );

    let off = pdc_graph(&c, &branches, false);
    let report = off.plan.pdc_report().clone();
    assert!(!report.active && !report.limit_exceeded);
    assert!(report.cable_delays.iter().all(|(_, _, d)| *d == 0));
    let offsets = click_render(&c, off, 130, 8 * BATCH, 64);
    assert_eq!(
        nonzero(&offsets),
        vec![(130, 1.0), (137, 1.0), (200, 1.0), (230, 1.0)],
        "off: each click arrives after its own path latency"
    );
}

#[test]
fn nested_buses_propagate_latency_and_align_at_every_summing_input() {
    // click -> L50 -> bus1; click -> bus1; bus1 -> bus2; click -> L10 -> bus2.
    // bus1 latency = 50; at bus2, bus1 (50) and L10 (10) are aligned to 50.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let click = g.add_device(ImpulseOnNote::boxed());
    let l50 = g.add_device(LatencyPassthrough::boxed(50));
    let l10 = g.add_device(LatencyPassthrough::boxed(10));
    let bus1 = g.add_device(SummingBus::boxed());
    let bus2 = g.add_device(SummingBus::boxed());
    let src = ep(click, ImpulseOnNote::AUDIO_OUT);
    g.connect(src, ep(l50, LatencyPassthrough::AUDIO_IN))
        .unwrap();
    g.connect(src, ep(l10, LatencyPassthrough::AUDIO_IN))
        .unwrap();
    g.connect(
        ep(l50, LatencyPassthrough::AUDIO_OUT),
        ep(bus1, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(src, ep(bus1, SummingBus::SUM_IN)).unwrap();
    g.connect(
        ep(bus1, SummingBus::AUDIO_OUT),
        ep(bus2, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(
        ep(l10, LatencyPassthrough::AUDIO_OUT),
        ep(bus2, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(ep(bus2, SummingBus::AUDIO_OUT), out(1)).unwrap();
    let plan = g.compile().unwrap();
    let report = plan.pdc_report().clone();
    let d: Vec<_> = report
        .cable_delays
        .iter()
        .map(|(f, t, d)| (f.device, t.device, *d))
        .collect();
    assert_eq!(
        d,
        vec![
            (click, bus1, 50),
            (l50, bus1, 0),
            (l10, bus2, 40),
            (bus1, bus2, 0)
        ]
    );
    let note = on(3, click, ImpulseOnNote::NOTES_IN, 60);
    let got = render_offline(plan, c, 4 * BATCH, 17, &[note], &[]);
    assert_eq!(nonzero(&got[0]), vec![(53, 3.0)]);
}

#[test]
fn split_paths_into_plain_inputs_are_not_compensated() {
    // click -> L100 -> bus.sum_in (alone); click -> bus.audio_in (plain). No summing input
    // has two contributions, so nothing is delayed; the bus reports the higher latency.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let click = g.add_device(ImpulseOnNote::boxed());
    let l = g.add_device(LatencyPassthrough::boxed(100));
    let bus = g.add_device(SummingBus::boxed());
    let bus2 = g.add_device(SummingBus::boxed());
    let src = ep(click, ImpulseOnNote::AUDIO_OUT);
    g.connect(src, ep(l, LatencyPassthrough::AUDIO_IN)).unwrap();
    g.connect(
        ep(l, LatencyPassthrough::AUDIO_OUT),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(src, ep(bus, SummingBus::AUDIO_IN)).unwrap();
    // A second level makes the bus latency observable in the report.
    g.connect(ep(bus, SummingBus::AUDIO_OUT), ep(bus2, SummingBus::SUM_IN))
        .unwrap();
    g.connect(src, ep(bus2, SummingBus::SUM_IN)).unwrap();
    g.connect(ep(bus2, SummingBus::AUDIO_OUT), out(1)).unwrap();
    let plan = g.compile().unwrap();
    let report = plan.pdc_report().clone();
    assert!(report.active);
    let lat: Vec<_> = report
        .latencies
        .iter()
        .map(|(e, l)| (e.device, *l))
        .collect();
    assert_eq!(lat, vec![(click, 0), (l, 100), (bus, 100)]);
    let note = on(0, click, ImpulseOnNote::NOTES_IN, 60);
    let got = render_offline(plan, c, 6 * BATCH, 64, &[note], &[]);
    // The plain (dry) branch inside `bus` is not delayed: it shows up 100 frames early. The
    // click straight into bus2 is aligned with the bus's reported latency (100).
    assert_eq!(nonzero(&got[0]), vec![(0, 1.0), (100, 2.0)]);
}

/// click -> L100 -> bus.sum_in; click -> bus.audio_in (plain); bus -> bus2.sum_in; click ->
/// bus2.sum_in; bus2 -> out_1. Returns the plan and the ids (click, bus).
fn split_plan(c: &EngineConfig, align: bool) -> (RenderPlan, DeviceInstanceId, DeviceInstanceId) {
    let mut g = Graph::new(c);
    let click = g.add_device(ImpulseOnNote::boxed());
    let l = g.add_device(LatencyPassthrough::boxed(100));
    let bus = g.add_device(SummingBus::boxed());
    let bus2 = g.add_device(SummingBus::boxed());
    let src = ep(click, ImpulseOnNote::AUDIO_OUT);
    g.connect(src, ep(l, LatencyPassthrough::AUDIO_IN)).unwrap();
    g.connect(
        ep(l, LatencyPassthrough::AUDIO_OUT),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(src, ep(bus, SummingBus::AUDIO_IN)).unwrap();
    g.connect(ep(bus, SummingBus::AUDIO_OUT), ep(bus2, SummingBus::SUM_IN))
        .unwrap();
    g.connect(src, ep(bus2, SummingBus::SUM_IN)).unwrap();
    g.connect(ep(bus2, SummingBus::AUDIO_OUT), out(1)).unwrap();
    g.set_align_inputs(align);
    (g.compile().unwrap(), click, bus)
}

#[test]
fn a_split_into_a_plain_input_is_reported_and_aligned_only_when_asked() {
    let c = cfg(0, 1);
    let (plan, click, bus) = split_plan(&c, false);
    let flagged: Vec<_> = plan
        .pdc_report()
        .misaligned
        .iter()
        .map(|(f, t, d)| (f.device, t.device, t.socket, *d))
        .collect();
    assert_eq!(
        flagged,
        vec![(click, bus, SummingBus::AUDIO_IN, 100)],
        "the early plain cable is listed with the frames it is early by"
    );
    let note = on(0, click, ImpulseOnNote::NOTES_IN, 60);
    let got = render_offline(plan, c, 6 * BATCH, 64, &[note], &[]);
    assert_eq!(nonzero(&got[0]), vec![(0, 1.0), (100, 2.0)], "unchanged");
    // Aligned: the plain cable is delayed to meet the other branch, nothing is flagged.
    let (plan, click, bus) = split_plan(&c, true);
    let report = plan.pdc_report().clone();
    assert!(report.misaligned.is_empty(), "{report:?}");
    assert!(
        report
            .cable_delays
            .iter()
            .any(|(f, t, d)| (f.device, t.device, t.socket, *d)
                == (click, bus, SummingBus::AUDIO_IN, 100)),
        "{report:?}"
    );
    let note = on(0, click, ImpulseOnNote::NOTES_IN, 60);
    let got = render_offline(plan, c, 6 * BATCH, 64, &[note], &[]);
    assert_eq!(nonzero(&got[0]), vec![(100, 3.0)]);
}

#[test]
fn aligned_plain_inputs_render_the_same_for_any_block_size() {
    let c = cfg(0, 1);
    let render = |block| {
        let (plan, click, _) = split_plan(&c, true);
        let note = on(3, click, ImpulseOnNote::NOTES_IN, 60);
        render_offline(plan, c, 6 * BATCH, block, &[note], &[])
    };
    let reference = render(64);
    for block in [1, 17, 333] {
        assert_eq!(bits(&render(block)), bits(&reference), "block {block}");
    }
}
#[test]
fn a_delay_beyond_the_limit_disables_compensation_and_is_reported() {
    let c = EngineConfig {
        sample_rate: SampleRate::new(8_000.0).unwrap(),
        ..cfg(0, 1)
    };
    let limit = (MAX_COMPENSATION_SECONDS * 8_000.0) as u32;
    // Exactly at the limit: compensated.
    let at = pdc_graph(&c, &[vec![], vec![limit]], true);
    let report = at.plan.pdc_report().clone();
    assert!(report.active && !report.limit_exceeded);
    assert_eq!(report.cable_delays[0].2, limit);
    // One frame more: disabled, reported, nothing delayed.
    let over = pdc_graph(&c, &[vec![], vec![limit + 1]], true);
    let report = over.plan.pdc_report().clone();
    assert!(!report.active && report.limit_exceeded, "{report:?}");
    assert!(report.cable_delays.iter().all(|(_, _, d)| *d == 0));
    let got = click_render(&c, over, 0, limit as usize + 2 * BATCH, 64);
    assert_eq!(
        nonzero(&got),
        vec![(0, 1.0), (limit as usize + 1, 1.0)],
        "uncompensated"
    );
    // Off: never reported as exceeded.
    let off = pdc_graph(&c, &[vec![], vec![limit + 1]], false);
    let report: &PdcReport = off.plan.pdc_report();
    assert!(!report.active && !report.limit_exceeded);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    /// Random path latencies (chains of LatencyPassthroughs) summed at one input align
    /// sample-exactly with compensation on, and arrive at their own latencies with it off.
    #[test]
    fn random_latencies_align_with_pdc_on_and_offset_with_it_off(
        branches in proptest::collection::vec(proptest::collection::vec(0u32..300, 0..3), 1..6),
        at in 0u64..200,
        block in prop_oneof![Just(1usize), Just(64), Just(333), 1usize..200],
    ) {
        let c = cfg(0, 1);
        let total: Vec<u32> = branches.iter().map(|b| b.iter().sum()).collect();
        let on_graph = pdc_graph(&c, &branches, true);
        let n = on_graph.tails.len();
        // Which branches survived (identical empty chains collapse into one cable).
        let mut kept = Vec::new();
        let mut empty_seen = false;
        for (k, b) in branches.iter().enumerate() {
            if b.is_empty() {
                if empty_seen { continue; }
                empty_seen = true;
            }
            kept.push(total[k]);
        }
        prop_assert_eq!(kept.len(), n);
        let max = *kept.iter().max().unwrap() as usize;
        let frames = at as usize + max + 2 * BATCH;
        let aligned = click_render(&c, on_graph, at, frames, block);
        prop_assert_eq!(nonzero(&aligned), vec![(at as usize + max, n as f32)]);

        let off_graph = pdc_graph(&c, &branches, false);
        let offsets = click_render(&c, off_graph, at, frames, block);
        let mut want: std::collections::BTreeMap<usize, f32> = Default::default();
        for l in kept {
            *want.entry(at as usize + l as usize).or_default() += 1.0;
        }
        prop_assert_eq!(nonzero(&offsets), want.into_iter().collect::<Vec<_>>());
    }
}

// ---------------------------------------------------------------------------------------------
// Hot routing swaps keep delay lines
// ---------------------------------------------------------------------------------------------

/// Two independent buses on out_1 / out_2. Bus 1: const 0.5 -> L100 -> sum, const 0.25 ->
/// sum (delayed 100). Bus 2: const 0.125 -> sum, plus `bus2_latency` on a second branch.
fn two_bus_topology(c: &EngineConfig, bus2_latency: Option<u32>, extra: bool) -> Topology {
    let mut t = Topology::new(c);
    let id = DeviceInstanceId;
    use atomik_testkit::fakes::{CONSTANT_SOURCE, LATENCY_PASSTHROUGH, SUMMING_BUS};
    for (n, d) in [
        (1, &CONSTANT_SOURCE),
        (2, &CONSTANT_SOURCE),
        (3, &LATENCY_PASSTHROUGH),
        (4, &SUMMING_BUS),
        (5, &CONSTANT_SOURCE),
        (6, &LATENCY_PASSTHROUGH),
        (7, &SUMMING_BUS),
        (8, &CONSTANT_SOURCE),
    ] {
        t.add_node(id(n), d).unwrap();
    }
    t.set_latency(id(3), 100).unwrap();
    t.set_latency(id(6), bus2_latency.unwrap_or(0)).unwrap();
    let audio = ConstantSource::AUDIO_OUT;
    t.connect(ep(id(1), audio), ep(id(3), LatencyPassthrough::AUDIO_IN))
        .unwrap();
    t.connect(
        ep(id(3), LatencyPassthrough::AUDIO_OUT),
        ep(id(4), SummingBus::SUM_IN),
    )
    .unwrap();
    t.connect(ep(id(2), audio), ep(id(4), SummingBus::SUM_IN))
        .unwrap();
    t.connect(ep(id(4), SummingBus::AUDIO_OUT), out(1)).unwrap();
    t.connect(ep(id(5), audio), ep(id(7), SummingBus::SUM_IN))
        .unwrap();
    if bus2_latency.is_some() {
        t.connect(ep(id(5), audio), ep(id(6), LatencyPassthrough::AUDIO_IN))
            .unwrap();
        t.connect(
            ep(id(6), LatencyPassthrough::AUDIO_OUT),
            ep(id(7), SummingBus::SUM_IN),
        )
        .unwrap();
    }
    if extra {
        t.connect(
            ep(id(8), ConstantSource::CV_OUT),
            ep(id(7), SummingBus::CV_SUM_IN),
        )
        .unwrap();
    }
    t.connect(ep(id(7), SummingBus::AUDIO_OUT), out(2)).unwrap();
    t
}

fn two_bus_engine(c: &EngineConfig) -> (Engine, EngineHandle) {
    let (engine, mut handle) = new_engine(*c);
    let values = [0.5, 0.25, 0.0, 0.0, 0.125, 0.0, 0.0, 0.0];
    for (k, v) in values.iter().enumerate() {
        let id = DeviceInstanceId(k as u32 + 1);
        let device = match k + 1 {
            3 => LatencyPassthrough::boxed(100),
            6 => LatencyPassthrough::boxed(0),
            4 | 7 => SummingBus::boxed(),
            _ => ConstantSource::boxed(*v),
        };
        handle.insert_device(id, device, &[]).unwrap();
    }
    (engine, handle)
}

fn run_two_bus(edits: Vec<(u64, Topology, Option<f32>)>, block: usize) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = two_bus_engine(&c);
    let mut timed = Vec::new();
    for (frame, topology, latency) in edits {
        if let Some(l) = latency {
            timed.push(TimedEdit {
                frame,
                edit: HotEdit::Param {
                    device: DeviceInstanceId(6),
                    param: LatencyPassthrough::LATENCY,
                    plain: l,
                },
            });
        }
        timed.push(TimedEdit {
            frame,
            edit: HotEdit::Routing(topology.compile().unwrap()),
        });
    }
    let job = OfflineJob {
        frames: 40 * BATCH,
        host_block: block,
        notes: &[],
        params: &[],
        inputs: &[],
    };
    render_offline_scripted(&mut engine, &mut handle, &job, timed, |f| {
        assert_no_alloc(f);
    })
}

#[test]
fn a_routing_swap_keeps_the_delay_lines_of_unchanged_cables() {
    let c = cfg(0, 2);
    let reference = run_two_bus(vec![(0, two_bus_topology(&c, None, false), None)], 64);
    // Bus 1 settles at 0.75 once both contributions came through their 100-frame paths.
    assert!(reference[0][..100].iter().all(|s| *s == 0.0));
    assert!(
        reference[0][100..]
            .iter()
            .all(|s| s.to_bits() == 0.75f32.to_bits())
    );
    assert!(
        reference[1]
            .iter()
            .all(|s| s.to_bits() == 0.125f32.to_bits())
    );
    // An unrelated edit (a Cv cable into bus 2) at frame 1280: bus 1 is bit-identical.
    let swapped = run_two_bus(
        vec![
            (0, two_bus_topology(&c, None, false), None),
            (1280, two_bus_topology(&c, None, true), None),
        ],
        64,
    );
    assert_eq!(bits(&swapped), bits(&reference));
}

#[test]
fn changing_one_bus_latency_resets_only_that_bus() {
    let c = cfg(0, 2);
    let reference = run_two_bus(vec![(0, two_bus_topology(&c, None, false), None)], 333);
    // At frame 1280, bus 2 gains a 200-frame branch: its delays change (reset), bus 1 does not.
    let changed = run_two_bus(
        vec![
            (0, two_bus_topology(&c, None, false), None),
            (1280, two_bus_topology(&c, Some(200), false), Some(200.0)),
        ],
        333,
    );
    assert_eq!(
        bits(&changed[..1]),
        bits(&reference[..1]),
        "bus 1 untouched"
    );
    // Bus 2: the direct cable now goes through a fresh 200-frame line (silence), and the new
    // branch through the device's 200-frame delay; both arrive 200 frames after the swap.
    assert!(
        changed[1][..1280]
            .iter()
            .all(|s| s.to_bits() == 0.125f32.to_bits())
    );
    assert!(changed[1][1280..1480].iter().all(|s| *s == 0.0));
    assert!(
        changed[1][1480..]
            .iter()
            .all(|s| s.to_bits() == 0.25f32.to_bits())
    );
}

// ---------------------------------------------------------------------------------------------
// Console bounce determinism and no allocation
// ---------------------------------------------------------------------------------------------

/// A small console: 6 click sources with different chains summed into 2 buses (with plain and
/// Cv/notes traffic), both buses summed into a master bus.
fn console_graph(c: &EngineConfig) -> (Graph, Vec<ScheduledNote>) {
    let mut g = Graph::new(c);
    let master = g.add_device(SummingBus::boxed());
    let buses = [
        g.add_device(SummingBus::boxed()),
        g.add_device(SummingBus::boxed()),
    ];
    let mut notes = Vec::new();
    for k in 0..6u32 {
        let click = g.add_device(ImpulseOnNote::boxed());
        let mut from = ep(click, ImpulseOnNote::AUDIO_OUT);
        for latency in [k * 37 % 101, k * 13 % 7] {
            let d = g.add_device(LatencyPassthrough::boxed(latency));
            g.connect(from, ep(d, LatencyPassthrough::AUDIO_IN))
                .unwrap();
            from = ep(d, LatencyPassthrough::AUDIO_OUT);
        }
        let bus = buses[(k % 2) as usize];
        g.connect(from, ep(bus, SummingBus::SUM_IN)).unwrap();
        for j in 0..12u64 {
            notes.push(on(
                j * 211 + u64::from(k) * 5,
                click,
                ImpulseOnNote::NOTES_IN,
                60,
            ));
        }
        let cv = g.add_device(ConstantSource::boxed(0.01 * k as f32));
        g.connect(
            ep(cv, ConstantSource::CV_OUT),
            ep(bus, SummingBus::CV_SUM_IN),
        )
        .unwrap();
    }
    for bus in buses {
        g.connect(
            ep(bus, SummingBus::AUDIO_OUT),
            ep(master, SummingBus::SUM_IN),
        )
        .unwrap();
    }
    let dry = g.add_device(ConstantSource::boxed(0.001));
    g.connect(
        ep(dry, ConstantSource::AUDIO_OUT),
        ep(master, SummingBus::AUDIO_IN),
    )
    .unwrap();
    g.connect(ep(master, SummingBus::AUDIO_OUT), out(1))
        .unwrap();
    g.connect(ep(buses[0], SummingBus::AUDIO_OUT), out(2))
        .unwrap();
    notes.sort_by_key(|n| n.frame);
    (g, notes)
}

#[test]
fn console_bounce_is_bit_identical_across_blocks_and_workers_without_allocation() {
    let c = cfg(0, 2);
    let frames = 3000;
    let mut results = Vec::new();
    // Engines stay alive until the end: a worker thread that exits frees its closure, which
    // the global worker counter would see.
    let mut engines = Vec::new();
    for threads in [0u16, 3] {
        for block in [1usize, 64, 333] {
            let (g, notes) = console_graph(&c);
            let plan = g.compile().unwrap();
            assert!(plan.pdc_report().active);
            let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
            handle.submit_plan(plan).unwrap();
            let job = OfflineJob {
                frames,
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
    assert!(first[0].iter().any(|b| *b != 0), "the bounce is not silent");
    for (key, r) in &results[1..] {
        assert_eq!(r, first, "{key:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Meters
// ---------------------------------------------------------------------------------------------

#[test]
fn add_meter_accepts_audio_and_cv_outputs_only() {
    let c = cfg(1, 1);
    let mut t = Topology::new(&c);
    let (a, bus) = (DeviceInstanceId(1), DeviceInstanceId(2));
    t.add_node(a, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    assert_eq!(t.add_meter(ep(a, ConstantSource::AUDIO_OUT)), Ok(()));
    assert_eq!(
        t.add_meter(ep(a, ConstantSource::AUDIO_OUT)),
        Ok(()),
        "idempotent"
    );
    assert_eq!(t.add_meter(ep(a, ConstantSource::CV_OUT)), Ok(()));
    assert_eq!(
        t.add_meter(ep(Graph::INTERFACE, SocketId(0))),
        Ok(()),
        "host in_1"
    );
    assert_eq!(
        t.add_meter(ep(bus, SummingBus::SUM_IN)),
        Err(GraphError::DirectionMismatch)
    );
    assert_eq!(
        t.add_meter(ep(bus, SummingBus::NOTES_OUT)),
        Err(GraphError::KindMismatch)
    );
    assert_eq!(
        t.add_meter(ep(DeviceInstanceId(9), SocketId(0))),
        Err(GraphError::UnknownDevice)
    );
    assert_eq!(
        t.add_meter(ep(a, SocketId(9))),
        Err(GraphError::UnknownSocket)
    );
    assert_eq!(t.remove_meter(ep(a, ConstantSource::CV_OUT)), Ok(()));
    assert_eq!(
        t.remove_meter(ep(a, ConstantSource::CV_OUT)),
        Err(GraphError::NotConnected)
    );
}

#[test]
fn meters_publish_peak_and_rms_lock_free_and_reset_the_peak_on_read() {
    let c = cfg(1, 1);
    let (mut engine, mut handle) = new_engine(c);
    let meters = handle.meters();
    let (neg, click, lat, bus) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
        DeviceInstanceId(4),
    );
    handle
        .insert_device(neg, ConstantSource::boxed(-0.75), &[])
        .unwrap();
    handle
        .insert_device(click, ImpulseOnNote::boxed(), &[])
        .unwrap();
    handle
        .insert_device(lat, LatencyPassthrough::boxed(10), &[])
        .unwrap();
    handle.insert_device(bus, SummingBus::boxed(), &[]).unwrap();
    let mut t = Topology::new(&c);
    t.add_node(neg, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(click, &atomik_testkit::fakes::IMPULSE_ON_NOTE)
        .unwrap();
    t.add_node(lat, &atomik_testkit::fakes::LATENCY_PASSTHROUGH)
        .unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    // Unconnected outputs can be metered too.
    let m_neg = ep(neg, ConstantSource::AUDIO_OUT);
    let m_cv = ep(neg, ConstantSource::CV_OUT);
    let m_click = ep(click, ImpulseOnNote::AUDIO_OUT);
    let m_host = ep(Graph::INTERFACE, SocketId(0));
    let m_bus = ep(bus, SummingBus::AUDIO_OUT);
    for m in [m_neg, m_cv, m_click, m_host, m_bus] {
        t.add_meter(m).unwrap();
    }
    t.connect(m_neg, ep(bus, SummingBus::SUM_IN)).unwrap();
    t.connect(m_host, ep(bus, SummingBus::SUM_IN)).unwrap();
    t.connect(m_bus, ep(Graph::INTERFACE, SocketId(1))).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    assert_eq!(
        meters.read(m_neg),
        None,
        "nothing published before the routing is live"
    );

    let input: Vec<f32> = (0..BATCH).map(|n| if n == 5 { 0.5 } else { 0.0 }).collect();
    let mut outs = vec![0.0f32; BATCH];
    assert_no_alloc(|| engine.render(&[&input], &mut [&mut outs]));
    // The FIFO's first batch reads silence; render a second one with the same input.
    handle
        .send_note(on(BATCH as u64 + 9, click, ImpulseOnNote::NOTES_IN, 60))
        .unwrap();
    assert_no_alloc(|| engine.render(&[&input], &mut [&mut outs]));
    let r = assert_no_alloc(|| meters.read(m_neg)).unwrap();
    assert_eq!((r.peak, r.rms), (0.75, 0.75));
    assert_eq!(meters.read(m_neg).unwrap().peak, 0.0, "peak resets on read");
    assert_eq!(
        meters.read(m_neg).unwrap().rms,
        0.75,
        "rms is the last batch's"
    );
    assert_eq!(meters.read(m_cv).unwrap().peak, 0.75);
    let r = meters.read(m_click).unwrap();
    assert_eq!(r.peak, 1.0);
    assert_eq!(r.rms, (1.0f32 / BATCH as f32).sqrt());
    let r = meters.read(m_host).unwrap();
    assert_eq!(r.peak, 0.5);
    assert_eq!(r.rms, (0.25f32 / BATCH as f32).sqrt());
    assert_eq!(meters.read(m_bus).unwrap().peak, 0.75);
    assert_eq!(meters.read(ep(lat, LatencyPassthrough::AUDIO_OUT)), None);

    // The peak holds the maximum over every batch since the last read.
    handle
        .send_note(on(2 * BATCH as u64 + 1, click, ImpulseOnNote::NOTES_IN, 60))
        .unwrap();
    for _ in 0..3 {
        engine.render(&[&input], &mut [&mut outs]);
    }
    let r = meters.read(m_click).unwrap();
    assert_eq!((r.peak, r.rms), (1.0, 0.0));

    // A routing without the meter unpublishes it.
    t.remove_meter(m_click).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    engine.render(&[&input], &mut [&mut outs]);
    handle.collect_garbage();
    assert_eq!(meters.read(m_click), None);
    assert!(meters.read(m_neg).is_some());
    assert!(format!("{meters:?}").contains("Meters"));
}

#[test]
fn meters_beyond_capacity_are_ignored() {
    let c = cfg(0, 1);
    let mut t = Topology::new(&c);
    let sources = METER_CAPACITY / 2 + 10;
    let mut endpoints = Vec::new();
    for k in 0..sources {
        let id = DeviceInstanceId(k as u32 + 1);
        t.add_node(id, &atomik_testkit::fakes::CONSTANT_SOURCE)
            .unwrap();
        for s in [ConstantSource::AUDIO_OUT, ConstantSource::CV_OUT] {
            t.add_meter(ep(id, s)).unwrap();
            endpoints.push(ep(id, s));
        }
    }
    let (mut engine, mut handle) = new_engine(c);
    for k in 0..sources {
        handle
            .insert_device(
                DeviceInstanceId(k as u32 + 1),
                ConstantSource::boxed(0.5),
                &[],
            )
            .unwrap();
    }
    handle.submit_routing(t.compile().unwrap()).unwrap();
    let mut outs = vec![0.0f32; BATCH];
    engine.render(&[], &mut [&mut outs]);
    endpoints.sort();
    for (k, e) in endpoints.iter().enumerate() {
        assert_eq!(
            meters_read(&handle, *e).is_some(),
            k < METER_CAPACITY,
            "{k}"
        );
    }
}

fn meters_read(handle: &EngineHandle, e: Endpoint) -> Option<atomik_engine::MeterReading> {
    handle.meters().read(e)
}

// ---------------------------------------------------------------------------------------------
// Device latency reaches the routing (latency.adjust and friends)
// ---------------------------------------------------------------------------------------------

#[test]
fn device_latency_is_published_by_the_engine_and_fills_the_topology() {
    let c = cfg(0, 1);
    let (mut engine, mut handle) = new_engine(c);
    let (src, lat, bus) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    handle
        .insert_device(src, ConstantSource::boxed(0.5), &[])
        .unwrap();
    handle
        .insert_device(
            lat,
            LatencyPassthrough::boxed(0),
            &[(LatencyPassthrough::LATENCY, 40.0)],
        )
        .unwrap();
    handle.insert_device(bus, SummingBus::boxed(), &[]).unwrap();
    // Known at insert time (prepared on the handle side), before the engine ran.
    assert_eq!(handle.device_latency(lat), Some(40));
    assert_eq!(handle.device_latency(src), Some(0));
    assert_eq!(handle.device_latency(DeviceInstanceId(77)), None);
    let mut t = Topology::new(&c);
    t.add_node(src, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(lat, &atomik_testkit::fakes::LATENCY_PASSTHROUGH)
        .unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    let a = ep(src, ConstantSource::AUDIO_OUT);
    t.connect(a, ep(lat, LatencyPassthrough::AUDIO_IN)).unwrap();
    t.connect(a, ep(bus, SummingBus::SUM_IN)).unwrap();
    t.connect(
        ep(lat, LatencyPassthrough::AUDIO_OUT),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    assert_eq!(t.latency(lat), Some(0));
    assert_eq!(
        t.set_latency(DeviceInstanceId(9), 1),
        Err(GraphError::UnknownDevice)
    );
    handle.fill_latencies(&mut t);
    assert_eq!(t.latency(lat), Some(40));
    let plan = t.compile().unwrap();
    assert_eq!(
        plan.pdc_report().cable_delays[0],
        (a, ep(bus, SummingBus::SUM_IN), 40)
    );
    handle.submit_routing(plan).unwrap();
    let epoch = handle.latency_epoch();
    let mut outs = vec![0.0f32; BATCH];
    engine.render(&[], &mut [&mut outs]);
    assert_eq!(handle.latency_epoch(), epoch, "no change, no new epoch");

    // A param that changes the device's latency is re-read by the engine at the next batch.
    handle
        .set_param(lat, LatencyPassthrough::LATENCY, 75.0)
        .unwrap();
    assert_eq!(handle.device_latency(lat), Some(40), "not applied yet");
    assert_no_alloc(|| engine.render(&[], &mut [&mut outs]));
    assert_eq!(handle.device_latency(lat), Some(75));
    assert!(
        handle.latency_epoch() > epoch,
        "the app recompiles when the epoch moves"
    );
    handle.fill_latencies(&mut t);
    assert_eq!(t.compile().unwrap().pdc_report().cable_delays[0].2, 75);
    // Removing the node forgets its latency.
    t.remove_node(lat).unwrap();
    assert_eq!(t.latency(lat), None);
}

#[test]
fn graph_compile_reads_latency_after_params_and_topology_compensation_can_be_switched_off() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let src = g.add_device(ConstantSource::boxed(0.5));
    let lat = g.add_device(LatencyPassthrough::boxed(5));
    let bus = g.add_device(SummingBus::boxed());
    g.set_param(lat, LatencyPassthrough::LATENCY, 12.0).unwrap();
    let a = ep(src, ConstantSource::AUDIO_OUT);
    g.connect(a, ep(lat, LatencyPassthrough::AUDIO_IN)).unwrap();
    g.connect(a, ep(bus, SummingBus::SUM_IN)).unwrap();
    g.connect(
        ep(lat, LatencyPassthrough::AUDIO_OUT),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    assert_eq!(g.compile().unwrap().pdc_report().cable_delays[0].2, 12);

    let mut t = Topology::new(&c);
    t.add_node(src, &atomik_testkit::fakes::CONSTANT_SOURCE)
        .unwrap();
    t.add_node(lat, &atomik_testkit::fakes::LATENCY_PASSTHROUGH)
        .unwrap();
    t.add_node(bus, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    t.set_latency(lat, 12).unwrap();
    t.connect(a, ep(lat, LatencyPassthrough::AUDIO_IN)).unwrap();
    t.connect(a, ep(bus, SummingBus::SUM_IN)).unwrap();
    t.connect(
        ep(lat, LatencyPassthrough::AUDIO_OUT),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    assert!(t.compensation());
    assert!(t.compile().unwrap().pdc_report().active);
    t.set_compensation(false);
    assert!(!t.compensation());
    let report = t.compile().unwrap().pdc_report().clone();
    assert!(!report.active);
    assert_eq!(
        report.latencies.len(),
        2,
        "latencies are reported either way"
    );
}
