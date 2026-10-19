//! The engine side of the container device, with hand-built flat graphs.
//!
//! The model flattens a container into ordinary devices and cables before the engine sees it,
//! so the engine needs nothing new. These tests build by hand the graphs the expansion
//! will produce (a `atomik.note-split` in front of inner instruments, an inner compressor with
//! look-ahead, inner parallel paths) and prove what the design relies on: delay compensation over
//! the flat graph, bit-identical output across host blocks and worker counts, the arena headroom for 256 inner devices and the note split routing
//! notes between flat devices.

mod common;

use atomik_core::{NoteKind, ParamId};
use atomik_device_sdk::SocketId;
use atomik_devices::{compressor, line_mixer, note_split, poly_synth, reverb, stereo_delay};
use atomik_engine::{DeviceInstanceId, Endpoint, Graph, RenderPlan, ScheduledNote, render_offline};
use atomik_testkit::fakes::{ImpulseOnNote, Passthrough, SummingBus};
use common::{bits, cfg, engine_with, ep, iface, render_blocks};

atomik_testkit::install_alloc_guard!();

const SR_FRAMES_PER_MS: f32 = 48.0;

fn dev(g: &mut Graph, f: atomik_device_sdk::DeviceFactory) -> DeviceInstanceId {
    g.add_device((f.create)())
}

fn sock(g: &Graph, d: DeviceInstanceId, key: &str) -> Endpoint {
    ep(d, g.descriptor(d).unwrap().socket_by_key(key).unwrap().id)
}

fn set(g: &mut Graph, d: DeviceInstanceId, key: &str, v: f32) {
    let p = g.descriptor(d).unwrap().param_by_key(key).unwrap().id;
    g.set_param(d, p, v).unwrap();
}

/// Configures slot `n` (1-based) of a split device.
fn zone(g: &mut Graph, split: DeviceInstanceId, n: usize, keys: (u8, u8), transpose: i8) {
    set(g, split, &format!("s{n}.on"), 1.0);
    set(g, split, &format!("s{n}.key_lo"), f32::from(keys.0));
    set(g, split, &format!("s{n}.key_hi"), f32::from(keys.1));
    set(g, split, &format!("s{n}.transpose"), f32::from(transpose));
}

fn note_on(frame: u64, device: DeviceInstanceId, key: u8) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: SocketId(0),
        kind: NoteKind::On { key, velocity: 0.8 },
    }
}

fn note_off(frame: u64, device: DeviceInstanceId, key: u8) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: SocketId(0),
        kind: NoteKind::Off { key },
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

fn out1(g: &Graph) -> Endpoint {
    iface(g, "out_1")
}

// ---------------------------------------------------------------------------------------------
// PDC over the flat graph (engine side)
// ---------------------------------------------------------------------------------------------

/// `split -> impulse A -> inner compressor (look-ahead) -> bus` and `split -> impulse B -> bus`,
/// the bus being a summing input (a console channel in the real rack). Returns the plan, the
/// split and the number of look-ahead frames.
fn lookahead_pair(lookahead_ms: f32) -> (RenderPlan, DeviceInstanceId, u32) {
    let mut g = Graph::new(&cfg(0, 1));
    let split = dev(&mut g, note_split::factory());
    zone(&mut g, split, 1, (0, 59), 0);
    zone(&mut g, split, 2, (60, 127), 0);
    let a = g.add_device(ImpulseOnNote::boxed());
    let b = g.add_device(ImpulseOnNote::boxed());
    let comp = dev(&mut g, compressor::factory());
    set(&mut g, comp, "lookahead", lookahead_ms);
    set(&mut g, comp, "mix", 0.0); // the delayed input, bit for bit
    let bus = g.add_device(SummingBus::boxed());
    g.connect(sock(&g, split, "out_1"), ep(a, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(sock(&g, split, "out_2"), ep(b, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(ep(a, ImpulseOnNote::AUDIO_OUT), sock(&g, comp, "in_l"))
        .unwrap();
    g.connect(sock(&g, comp, "out_l"), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(b, ImpulseOnNote::AUDIO_OUT), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out1(&g)).unwrap();
    let frames = (lookahead_ms * SR_FRAMES_PER_MS).round() as u32;
    (g.compile().unwrap(), split, frames)
}

#[test]
fn an_inner_lookahead_is_reported_and_aligned_against_a_dry_inner_path() {
    for ms in [1.0f32, 2.0, 4.0] {
        let (plan, split, frames) = lookahead_pair(ms);
        let report = plan.pdc_report();
        assert!(report.active && !report.limit_exceeded);
        // One real delay line: the dry cable into the summing input, by exactly the look-ahead
        // (the compressor's own cable is listed with a zero delay).
        let mut delays: Vec<u32> = report.cable_delays.iter().map(|d| d.2).collect();
        delays.sort_unstable();
        assert_eq!(delays, [0, frames], "look-ahead {ms} ms");
        assert!(report.latencies.iter().any(|(_, l)| *l == frames));
        // Both keys sound at frame 500; the compressor path arrives `frames` later and the dry
        // path is delayed to match, so the impulses coincide at 500 + look-ahead. (The engine's
        // own latency is the fixed batch FIFO, which the offline render removes.)
        let notes = [note_on(500, split, 40), note_on(500, split, 70)];
        let out = render_offline(plan, cfg(0, 1), 1024, 64, &notes, &[]);
        assert_eq!(
            nonzero(&out[0]),
            [(500 + frames as usize, 2.0)],
            "look-ahead {ms} ms"
        );
    }
}

#[test]
fn changing_the_lookahead_recompiles_and_realigns() {
    let (short, split, f1) = lookahead_pair(1.0);
    let (long, _, f2) = lookahead_pair(3.0);
    assert_ne!(f1, f2);
    let notes = [note_on(300, split, 40), note_on(300, split, 70)];
    let a = render_offline(short, cfg(0, 1), 700, 480, &notes, &[]);
    let b = render_offline(long, cfg(0, 1), 700, 480, &notes, &[]);
    assert_eq!(nonzero(&a[0]), [(300 + f1 as usize, 2.0)]);
    assert_eq!(nonzero(&b[0]), [(300 + f2 as usize, 2.0)]);
}

#[test]
fn parallel_inner_paths_that_meet_in_a_plain_input_are_not_compensated() {
    // The documented limit: only a summing input is aligned. Two inner paths into a plain mixer
    // input stay `look-ahead` frames apart.
    let mut g = Graph::new(&cfg(0, 2));
    let split = dev(&mut g, note_split::factory());
    zone(&mut g, split, 1, (0, 59), 0);
    zone(&mut g, split, 2, (60, 127), 0);
    let a = g.add_device(ImpulseOnNote::boxed());
    let b = g.add_device(ImpulseOnNote::boxed());
    let comp = dev(&mut g, compressor::factory());
    set(&mut g, comp, "lookahead", 2.0);
    set(&mut g, comp, "mix", 0.0);
    let mixer = dev(&mut g, line_mixer::factory());
    g.connect(sock(&g, split, "out_1"), ep(a, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(sock(&g, split, "out_2"), ep(b, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(ep(a, ImpulseOnNote::AUDIO_OUT), sock(&g, comp, "in_l"))
        .unwrap();
    g.connect(sock(&g, comp, "out_l"), sock(&g, mixer, "ch1_in_l"))
        .unwrap();
    g.connect(ep(b, ImpulseOnNote::AUDIO_OUT), sock(&g, mixer, "ch2_in_l"))
        .unwrap();
    g.connect(sock(&g, mixer, "master_l"), out1(&g)).unwrap();
    let plan = g.compile().unwrap();
    assert!(plan.pdc_report().cable_delays.is_empty());
    let notes = [note_on(500, split, 40), note_on(500, split, 70)];
    let out = render_offline(plan, cfg(0, 2), 1024, 64, &notes, &[]);
    let hits = nonzero(&out[0]);
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(hits[1].0 - hits[0].0, 96, "{hits:?}");
}

// ---------------------------------------------------------------------------------------------
// The split device between flat devices
// ---------------------------------------------------------------------------------------------

#[test]
fn the_split_routes_notes_between_flat_devices_and_an_off_follows_its_on() {
    let mut g = Graph::new(&cfg(0, 1));
    let split = dev(&mut g, note_split::factory());
    zone(&mut g, split, 1, (0, 59), 0);
    zone(&mut g, split, 2, (60, 127), 0);
    let a = g.add_device(ImpulseOnNote::boxed());
    let b = g.add_device(ImpulseOnNote::boxed());
    let bus = g.add_device(SummingBus::boxed());
    g.connect(sock(&g, split, "out_1"), ep(a, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(sock(&g, split, "out_2"), ep(b, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(ep(a, ImpulseOnNote::AUDIO_OUT), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(b, ImpulseOnNote::AUDIO_OUT), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out1(&g)).unwrap();
    let plan = g.compile().unwrap();
    let notes = [
        note_on(100, split, 40),
        note_on(200, split, 72),
        note_off(300, split, 40),
        note_off(310, split, 72),
    ];
    let out = render_offline(plan, cfg(0, 1), 512, 100, &notes, &[]);
    // Each On is one impulse, from whichever side owns the key; the Offs are silent.
    assert_eq!(nonzero(&out[0]), [(100, 1.0), (200, 1.0)]);
}

// ---------------------------------------------------------------------------------------------
// Bit-exactness over host blocks and workers (engine half)
// ---------------------------------------------------------------------------------------------

/// A flattened "layered instrument with an effect container": the split, two synths, an inner
/// compressor and delay, a nested effect container's reverb, all summed by a line mixer. `ids`
/// follows the expansion order (split first, then the inner devices in document order).
fn flat_rack(c: &atomik_engine::EngineConfig) -> (RenderPlan, DeviceInstanceId) {
    let mut g = Graph::new(c);
    let split = dev(&mut g, note_split::factory());
    let synth_a = dev(&mut g, poly_synth::factory());
    let synth_b = dev(&mut g, poly_synth::factory());
    let comp = dev(&mut g, compressor::factory());
    let delay = dev(&mut g, stereo_delay::factory());
    let verb = dev(&mut g, reverb::factory());
    let mixer = dev(&mut g, line_mixer::factory());
    zone(&mut g, split, 1, (0, 59), 0);
    zone(&mut g, split, 2, (60, 127), -12);
    set(&mut g, synth_a, "osc1.wave", 1.0);
    set(&mut g, synth_b, "osc1.wave", 2.0);
    // The macro fan-out of the real rack is plain `set_param`s on the flat devices.
    set(&mut g, synth_a, "filter.cutoff", 1200.0);
    set(&mut g, synth_b, "filter.cutoff", 1200.0);
    set(&mut g, comp, "lookahead", 2.0);
    set(&mut g, comp, "threshold", -24.0);
    set(&mut g, delay, "time.mode", 0.0);
    set(&mut g, delay, "time.ms", 41.0);
    set(&mut g, delay, "feedback", 0.5);
    g.connect(sock(&g, split, "out_1"), sock(&g, synth_a, "notes_in"))
        .unwrap();
    g.connect(sock(&g, split, "out_2"), sock(&g, synth_b, "notes_in"))
        .unwrap();
    for side in ["l", "r"] {
        g.connect(
            sock(&g, synth_a, &format!("out_{side}")),
            sock(&g, comp, &format!("in_{side}")),
        )
        .unwrap();
        g.connect(
            sock(&g, comp, &format!("out_{side}")),
            sock(&g, mixer, &format!("ch1_in_{side}")),
        )
        .unwrap();
        g.connect(
            sock(&g, synth_b, &format!("out_{side}")),
            sock(&g, delay, &format!("in_{side}")),
        )
        .unwrap();
        g.connect(
            sock(&g, delay, &format!("out_{side}")),
            sock(&g, verb, &format!("in_{side}")),
        )
        .unwrap();
        g.connect(
            sock(&g, verb, &format!("out_{side}")),
            sock(&g, mixer, &format!("ch2_in_{side}")),
        )
        .unwrap();
    }
    g.connect(sock(&g, mixer, "master_l"), iface(&g, "out_1"))
        .unwrap();
    g.connect(sock(&g, mixer, "master_r"), iface(&g, "out_2"))
        .unwrap();
    (g.compile().unwrap(), split)
}

fn script(split: DeviceInstanceId) -> Vec<ScheduledNote> {
    let mut n = Vec::new();
    for (i, key) in [48u8, 64, 55, 72, 67, 36, 80].into_iter().enumerate() {
        let at = 200 + i as u64 * 1300;
        n.push(note_on(at, split, key));
        n.push(note_off(at + 1900, split, key));
    }
    n.sort_by_key(|e| e.frame);
    n
}

fn render_flat(threads: u16, block: usize) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let (plan, split) = flat_rack(&c);
    let (mut engine, mut handle) = engine_with(c, threads, None);
    for n in script(split) {
        handle.send_note(n).unwrap();
    }
    handle.submit_plan(plan).unwrap();
    let out = render_blocks(&mut engine, &[], 2, 14_000, block);
    assert_eq!(handle.dropped_events(), 0);
    out
}

#[test]
fn the_flat_rack_is_audible_and_bit_identical_across_blocks_and_workers() {
    let reference = render_flat(0, 64);
    assert!(
        reference.iter().any(|c| c.iter().any(|s| s.abs() > 1.0e-4)),
        "the rack is silent"
    );
    assert!(reference.iter().all(|c| c.iter().all(|s| s.is_finite())));
    let want = bits(&reference);
    for threads in [0u16, 1, 2, 4] {
        for block in [64usize, 480, 512, 1, 333] {
            assert_eq!(
                bits(&render_flat(threads, block)),
                want,
                "threads {threads}, block {block}"
            );
        }
    }
}

#[test]
fn a_twin_built_in_the_same_order_renders_identically() {
    // The expansion theorem's engine half: ids (and with them the device seeds) come from the
    // flat order, so a hand-built twin in that order is the same sound.
    assert_eq!(bits(&render_flat(1, 480)), bits(&render_flat(2, 512)));
    let c = cfg(0, 2);
    let (p1, s1) = flat_rack(&c);
    let (p2, s2) = flat_rack(&c);
    assert_eq!(s1, s2);
    let a = render_offline(p1, c, 9000, 480, &script(s1), &[]);
    let b = render_offline(p2, c, 9000, 64, &script(s2), &[]);
    assert_eq!(bits(&a), bits(&b));
}

// ---------------------------------------------------------------------------------------------
// Arena and levels (256 inner devices)
// ---------------------------------------------------------------------------------------------

/// Eight "containers" of 31 passthroughs behind a split and an impulse source: 264 flat devices
/// plus the bus, past the 256 inner-device limit and far below the 4096-slot arena.
fn big_flat(c: &atomik_engine::EngineConfig) -> (RenderPlan, Vec<DeviceInstanceId>) {
    let mut g = Graph::new(c);
    let bus = g.add_device(SummingBus::boxed());
    let mut splits = Vec::new();
    for _ in 0..8 {
        let split = dev(&mut g, note_split::factory());
        zone(&mut g, split, 1, (0, 127), 0);
        let src = g.add_device(ImpulseOnNote::boxed());
        g.connect(sock(&g, split, "out_1"), ep(src, ImpulseOnNote::NOTES_IN))
            .unwrap();
        let mut from = ep(src, ImpulseOnNote::AUDIO_OUT);
        for _ in 0..31 {
            let p = g.add_device(Passthrough::boxed(0.99));
            g.connect(from, ep(p, Passthrough::AUDIO_IN)).unwrap();
            from = ep(p, Passthrough::AUDIO_OUT);
        }
        g.connect(from, ep(bus, SummingBus::SUM_IN)).unwrap();
        splits.push(split);
    }
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out1(&g)).unwrap();
    (g.compile().unwrap(), splits)
}

#[test]
fn two_hundred_and_fifty_six_inner_devices_fit_the_arena_and_run_in_parallel() {
    let c = cfg(0, 1);
    let (plan, splits) = big_flat(&c);
    assert!(plan.order().len() > 256 + 8, "{}", plan.order().len());
    // Eight independent chains: the levels are wide, so the workers have something to share.
    assert!(plan.levels().any(|l| l.len() >= 8));
    let notes: Vec<_> = splits
        .iter()
        .enumerate()
        .map(|(i, s)| note_on(10 + i as u64 * 7, *s, 60))
        .collect();
    let want = render_offline(plan, c, 1024, 64, &notes, &[]);
    let hits = nonzero(&want[0]);
    assert_eq!(hits.len(), 8, "{hits:?}");
    let expect = 0.99f32.powi(31);
    assert!(hits.iter().all(|(_, v)| (v - expect).abs() < 1.0e-4));
    let raw = |threads: u16| {
        let (plan, splits) = big_flat(&c);
        let (mut engine, mut handle) = engine_with(c, threads, None);
        for (i, s) in splits.iter().enumerate() {
            handle
                .send_note(note_on(10 + i as u64 * 7, *s, 60))
                .unwrap();
        }
        handle.submit_plan(plan).unwrap();
        bits(&render_blocks(&mut engine, &[], 1, 1024, 480))
    };
    let want_bits = raw(0);
    for threads in [1u16, 4] {
        assert_eq!(raw(threads), want_bits, "threads {threads}");
    }
}

#[test]
fn split_params_set_through_the_engine_param_path_take_effect() {
    // The app sends `~split` params with the ordinary param queue right after the insert.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let split = dev(&mut g, note_split::factory());
    let src = g.add_device(ImpulseOnNote::boxed());
    g.connect(sock(&g, split, "out_1"), ep(src, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), out1(&g))
        .unwrap();
    let plan = g.compile().unwrap();
    let notes = [note_on(100, split, 60)];
    let silent = render_offline(plan, c, 256, 64, &notes, &[]);
    assert!(nonzero(&silent[0]).is_empty(), "slot 1 starts off");

    let mut g = Graph::new(&c);
    let split = dev(&mut g, note_split::factory());
    let src = g.add_device(ImpulseOnNote::boxed());
    g.connect(sock(&g, split, "out_1"), ep(src, ImpulseOnNote::NOTES_IN))
        .unwrap();
    g.connect(ep(src, ImpulseOnNote::AUDIO_OUT), out1(&g))
        .unwrap();
    let plan = g.compile().unwrap();
    let params = [(split, ParamId(0), 1.0)];
    let heard = render_offline(plan, c, 256, 64, &notes, &params);
    assert_eq!(nonzero(&heard[0]), [(100, 1.0)]);
}
