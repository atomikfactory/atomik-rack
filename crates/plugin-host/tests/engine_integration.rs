//! (host half) with the real engine: delay compensation follows the
//! plugin latency, pinned devices stay on the audio thread, the thread-check answers per
//! thread for any worker count, the whole path is allocation free at several host blocks
//! (with notes, param bursts, a sidechain and a restart in the middle), and the output is
//! bit-identical across host blocks and worker counts.
// Justification: exact impulse amplitudes and copied samples are compared.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteKind, ParamId, SampleRate, Tempo};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineHandle, EngineOptions, Graph,
    OfflineJob, ScheduledNote, Topology, WorkerConfig, new_engine_with_options,
    render_offline_scripted,
};
use atomik_testkit::fakes::{ConstantSource, ImpulseOnNote, Passthrough, SummingBus};
use atomik_testkit::no_alloc::assert_no_alloc;
use support::*;

fn cfg() -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    }
}

fn ep(device: DeviceInstanceId, socket: atomik_device_sdk::SocketId) -> Endpoint {
    Endpoint { device, socket }
}

fn socket(
    d: &'static atomik_device_sdk::DeviceDescriptor,
    key: &str,
) -> atomik_device_sdk::SocketId {
    d.socket_by_key(key).unwrap().id
}

fn out(g: &Graph, key: &str) -> Endpoint {
    let d = g.descriptor(Graph::INTERFACE).unwrap();
    ep(Graph::INTERFACE, d.socket_by_key(key).unwrap().id)
}

fn engine(threads: u16) -> (Engine, EngineHandle) {
    new_engine_with_options(
        cfg(),
        EngineOptions {
            workers: WorkerConfig {
                threads,
                on_thread_start: None,
            },
            ..EngineOptions::default()
        },
    )
}

fn hits(signal: &[f32]) -> Vec<(usize, f32)> {
    signal
        .iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(i, s)| (i, *s))
        .collect()
}

fn note_on(
    frame: u64,
    device: DeviceInstanceId,
    socket: atomik_device_sdk::SocketId,
    key: u8,
) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket,
        kind: NoteKind::On { key, velocity: 1.0 },
    }
}

/// `click -> [passthrough | plugin fx] -> sum bus -> out_1`. Returns the plan pieces.
struct Pdc {
    g: Graph,
    click: DeviceInstanceId,
}

fn pdc_graph(plugin_latency_param: f32, slot_latency: u32) -> Pdc {
    let mut g = Graph::new(&cfg());
    let click = g.add_device(ImpulseOnNote::boxed());
    let dry = g.add_device(Passthrough::boxed(1.0));
    let bus = g.add_device(SummingBus::boxed());
    let r = rig("pdc.fx", &fx_layout(), Box::new(CoreEffect::new()));
    r.manager.slot().set_latency(slot_latency, SR);
    let d = r.descriptor;
    let fx = g.add_device(Box::new(r.device));
    g.set_param(fx, ParamId(3), plugin_latency_param).unwrap();
    g.connect(
        ep(click, ImpulseOnNote::AUDIO_OUT),
        ep(dry, Passthrough::AUDIO_IN),
    )
    .unwrap();
    g.connect(
        ep(click, ImpulseOnNote::AUDIO_OUT),
        ep(fx, socket(d, "in_l")),
    )
    .unwrap();
    g.connect(ep(dry, Passthrough::AUDIO_OUT), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(fx, socket(d, "out_l")), ep(bus, SummingBus::SUM_IN))
        .unwrap();
    g.connect(ep(bus, SummingBus::AUDIO_OUT), out(&g, "out_1"))
        .unwrap();
    Pdc { g, click }
}

fn render_pdc(p: Pdc) -> Vec<f32> {
    let note = note_on(130, p.click, ImpulseOnNote::NOTES_IN, 60);
    let plan = p.g.compile().unwrap();
    atomik_engine::render_offline(plan, cfg(), 8 * BATCH, 64, &[note], &[]).swap_remove(0)
}

#[test]
fn the_reported_latency_makes_pdc_align_a_plugin_path_with_a_dry_path() {
    let aligned = render_pdc(pdc_graph(100.0, 100));
    assert_eq!(hits(&aligned), vec![(230, 2.0)], "both clicks at 130 + 100");
    // Without the report the plugin's delay is not compensated: two clicks.
    let unaligned = render_pdc(pdc_graph(100.0, 0));
    assert_eq!(hits(&unaligned), vec![(130, 1.0), (230, 1.0)]);
}

#[test]
fn a_latency_change_realigns_after_the_routing_is_recompiled() {
    let c = cfg();
    let (mut e, mut h) = engine(0);
    let click_id = DeviceInstanceId(1);
    let dry_id = DeviceInstanceId(2);
    let bus_id = DeviceInstanceId(3);
    let fx_id = DeviceInstanceId(4);
    let r = rig("pdc.hot", &fx_layout(), Box::new(CoreEffect::new()));
    let (slot, d) = (r.manager.slot().clone(), r.descriptor);
    h.insert_device(click_id, ImpulseOnNote::boxed(), &[])
        .unwrap();
    h.insert_device(dry_id, Passthrough::boxed(1.0), &[])
        .unwrap();
    h.insert_device(bus_id, SummingBus::boxed(), &[]).unwrap();
    h.insert_device(fx_id, Box::new(r.device), &[]).unwrap();
    let mut topo = Topology::new(&c);
    topo.add_node(click_id, &atomik_testkit::fakes::IMPULSE_ON_NOTE)
        .unwrap();
    topo.add_node(dry_id, &atomik_testkit::fakes::PASSTHROUGH)
        .unwrap();
    topo.add_node(bus_id, &atomik_testkit::fakes::SUMMING_BUS)
        .unwrap();
    topo.add_node(fx_id, d).unwrap();
    let iface = Graph::new(&c);
    let cables = [
        (
            ep(click_id, ImpulseOnNote::AUDIO_OUT),
            ep(dry_id, Passthrough::AUDIO_IN),
        ),
        (
            ep(click_id, ImpulseOnNote::AUDIO_OUT),
            ep(fx_id, socket(d, "in_l")),
        ),
        (
            ep(dry_id, Passthrough::AUDIO_OUT),
            ep(bus_id, SummingBus::SUM_IN),
        ),
        (
            ep(fx_id, socket(d, "out_l")),
            ep(bus_id, SummingBus::SUM_IN),
        ),
        (ep(bus_id, SummingBus::AUDIO_OUT), out(&iface, "out_1")),
    ];
    for (from, to) in cables {
        topo.connect(from, to).unwrap();
    }
    h.submit_routing(topo.compile().unwrap()).unwrap();

    let pos = std::cell::Cell::new(0u64);
    let render = |e: &mut Engine, blocks: usize| -> Vec<f32> {
        let mut all = Vec::new();
        for _ in 0..blocks {
            let mut l = [0.0f32; BATCH];
            let mut r = [0.0f32; BATCH];
            assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
            all.extend_from_slice(&l);
            pos.set(pos.get() + BATCH as u64);
        }
        all
    };
    // 1. No latency anywhere: one click of 2.0.
    h.send_note(note_on(
        pos.get() + 10,
        click_id,
        ImpulseOnNote::NOTES_IN,
        60,
    ))
    .unwrap();
    let a = render(&mut e, 6);
    assert_eq!(hits(&a).len(), 1, "{:?}", hits(&a));
    assert_eq!(hits(&a)[0].1, 2.0);
    // 2. The plugin now delays by 100 frames and says so; the routing is not recompiled yet.
    h.set_param(fx_id, ParamId(3), 100.0).unwrap();
    slot.set_latency(100, SR);
    let epoch = h.latency_epoch();
    render(&mut e, 2);
    assert_ne!(
        h.latency_epoch(),
        epoch,
        "the engine published the new latency"
    );
    assert_eq!(h.device_latency(fx_id), Some(100));
    h.send_note(note_on(
        pos.get() + 10,
        click_id,
        ImpulseOnNote::NOTES_IN,
        60,
    ))
    .unwrap();
    let b = render(&mut e, 6);
    let hb = hits(&b);
    assert_eq!(hb.len(), 2, "misaligned until PDC recompiles: {hb:?}");
    assert_eq!(hb[1].0 - hb[0].0, 100);
    // 3. The app refills the latencies and resubmits the routing: aligned again.
    h.fill_latencies(&mut topo);
    h.submit_routing(topo.compile().unwrap()).unwrap();
    render(&mut e, 4);
    h.send_note(note_on(
        pos.get() + 10,
        click_id,
        ImpulseOnNote::NOTES_IN,
        60,
    ))
    .unwrap();
    let c = render(&mut e, 6);
    let hc = hits(&c);
    assert_eq!(hc.len(), 1, "{hc:?}");
    assert_eq!(hc[0].1, 2.0);
}

// ----- threads -----------------------------------------------------------------------------

#[test]
fn pinned_plugin_devices_stay_on_the_audio_thread_and_the_thread_check_answers_per_thread() {
    for workers in [0u16, 1, 2, 4] {
        let c = cfg();
        let (mut e, mut h) = engine(workers);
        let mut topo = Topology::new(&c);
        let mut shared = Vec::new();
        for i in 0..6u32 {
            let (probe, s) = Probe::new();
            let r = rig(&format!("thr.{i}"), &fx_layout(), Box::new(probe));
            r.manager.slot().set_pinned(i % 2 == 0);
            let id = DeviceInstanceId(i + 1);
            topo.add_node(id, r.descriptor).unwrap();
            h.insert_device(id, Box::new(r.device), &[]).unwrap();
            shared.push((s, r.manager));
        }
        h.submit_routing(topo.compile().unwrap()).unwrap();
        for _ in 0..100 {
            let (mut l, mut r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
            assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
        }
        let me = std::thread::current().id();
        let mut off_thread = false;
        for (i, (s, _m)) in shared.iter().enumerate() {
            let log = s.log.lock().unwrap();
            assert_eq!(log.calls, 100, "workers {workers} device {i}");
            assert!(
                log.audio_thread_flags.iter().all(|f| *f),
                "audio-thread check"
            );
            assert!(
                log.main_thread_flags.iter().all(|f| !*f),
                "never the main thread"
            );
            if i % 2 == 0 {
                assert!(
                    log.thread_ids.iter().all(|t| *t == me),
                    "workers {workers}: pinned device {i} left the audio thread"
                );
            } else {
                off_thread |= log.thread_ids.iter().any(|t| *t != me);
            }
        }
        if workers > 0 {
            assert!(
                off_thread,
                "workers {workers}: unpinned devices use the workers"
            );
        }
        drop(h);
    }
}

// ----- the full path: allocation, determinism ----------------------------------------------

struct Chain {
    plan: atomik_engine::RenderPlan,
    synth: DeviceInstanceId,
    fx: DeviceInstanceId,
    notes: Vec<ScheduledNote>,
}

/// synth -> fx (sidechain from a constant source) -> out. Two parallel synths make the level
/// wide enough for workers.
fn chain() -> Chain {
    let c = cfg();
    let mut g = Graph::new(&c);
    let rs = rig("chain.synth", &synth_layout(), Box::new(CoreSynth::new()));
    let rs2 = rig("chain.synth2", &synth_layout(), Box::new(CoreSynth::new()));
    let rf = rig("chain.fx", &fx_layout(), Box::new(CoreEffect::new()));
    let (ds, ds2, df) = (rs.descriptor, rs2.descriptor, rf.descriptor);
    let synth = g.add_device(Box::new(rs.device));
    let synth2 = g.add_device(Box::new(rs2.device));
    let fx = g.add_device(Box::new(rf.device));
    let sc = g.add_device(ConstantSource::boxed(0.3));
    let bus = g.add_device(SummingBus::boxed());
    g.set_param(synth, ParamId(1), 1.0).unwrap();
    g.set_param(fx, ParamId(1), -3.0).unwrap();
    g.set_param(fx, ParamId(3), 64.0).unwrap();
    for (s, d) in [(synth, ds), (synth2, ds2)] {
        g.connect(ep(s, socket(d, "out_l")), ep(bus, SummingBus::SUM_IN))
            .unwrap();
    }
    g.connect(ep(bus, SummingBus::AUDIO_OUT), ep(fx, socket(df, "in_l")))
        .unwrap();
    g.connect(
        ep(sc, ConstantSource::AUDIO_OUT),
        ep(fx, socket(df, "sc_in_l")),
    )
    .unwrap();
    g.connect(ep(fx, socket(df, "out_l")), out(&g, "out_1"))
        .unwrap();
    g.connect(ep(fx, socket(df, "out_r")), out(&g, "out_2"))
        .unwrap();
    let mut notes = Vec::new();
    for i in 0..40u64 {
        let key = 48 + (i % 24) as u8;
        let t = 100 + i * 510;
        let target = if i % 2 == 0 { synth } else { synth2 };
        notes.push(note_on(t, target, socket(ds, "notes_in"), key));
        notes.push(ScheduledNote {
            frame: t + 300,
            device: target,
            socket: socket(ds, "notes_in"),
            kind: NoteKind::Off { key },
        });
    }
    Chain {
        plan: g.compile().unwrap(),
        synth,
        fx,
        notes,
    }
}

fn render_chain(block: usize, workers: u16) -> Vec<Vec<f32>> {
    let ch = chain();
    let (mut e, mut h) = engine(workers);
    h.submit_plan(ch.plan).unwrap();
    let params: Vec<(DeviceInstanceId, ParamId, f32)> = (0..60)
        .map(|i| (ch.fx, ParamId(1), -6.0 + (i % 7) as f32))
        .collect();
    let job = OfflineJob {
        frames: 24_000,
        host_block: block,
        notes: &ch.notes,
        params: &params,
        inputs: &[],
    };
    let _ = ch.synth;
    render_offline_scripted(&mut e, &mut h, &job, Vec::new(), |f| assert_no_alloc(f))
}

#[test]
fn the_whole_path_is_allocation_free_and_bit_identical_across_host_blocks_and_workers() {
    let reference = render_chain(64, 0);
    let sound = reference[0].iter().any(|s| s.abs() > 0.01);
    assert!(sound, "the chain makes sound");
    assert!(reference.iter().flatten().all(|s| s.is_finite()));
    for block in [1usize, 333, 480, 512] {
        let got = render_chain(block, 0);
        assert_eq!(bits(&got), bits(&reference), "host block {block}");
    }
    for workers in [1u16, 2, 4] {
        let got = render_chain(512, workers);
        assert_eq!(bits(&got), bits(&reference), "workers {workers}");
    }
}

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

#[test]
fn a_restart_in_the_middle_of_a_render_is_allocation_free_and_the_sound_resumes() {
    for block in [1usize, 64, 333, 512] {
        let c = cfg();
        let (mut e, mut h) = engine(2);
        let rs = rig("mid.synth", &synth_layout(), Box::new(CoreSynth::new()));
        let Rig {
            device: fx_device,
            mut manager,
            descriptor: df,
            ..
        } = rig("mid.fx", &fx_layout(), Box::new(CoreEffect::new()));
        let ds = rs.descriptor;
        let mut topo = Topology::new(&c);
        let (synth, fx) = (DeviceInstanceId(1), DeviceInstanceId(2));
        topo.add_node(synth, ds).unwrap();
        topo.add_node(fx, df).unwrap();
        h.insert_device(synth, Box::new(rs.device), &[]).unwrap();
        h.insert_device(fx, Box::new(fx_device), &[]).unwrap();
        let iface = Graph::new(&c);
        topo.connect(ep(synth, socket(ds, "out_l")), ep(fx, socket(df, "in_l")))
            .unwrap();
        topo.connect(ep(fx, socket(df, "out_l")), out(&iface, "out_1"))
            .unwrap();
        h.submit_routing(topo.compile().unwrap()).unwrap();
        for i in 0..30u64 {
            h.send_note(note_on(
                64 + i * 700,
                synth,
                socket(ds, "notes_in"),
                50 + (i % 12) as u8,
            ))
            .unwrap();
        }
        let mut heard = Vec::new();
        let total = 30_000usize;
        let mut done = 0usize;
        let mut restart_at = Some(12_000usize);
        let mut resume_at = None;
        while done < total {
            let n = block.min(total - done);
            let mut l = vec![0.0f32; n];
            let mut r = vec![0.0f32; n];
            assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
            heard.extend_from_slice(&l);
            done += n;
            // Param bursts between renders reach the next batches.
            for k in 0..20 {
                let _ = h.set_param(fx, ParamId(1), -(k as f32));
            }
            if restart_at.is_some_and(|t| done >= t) {
                restart_at = None;
                manager.request_stop();
                resume_at = Some(done + 3_000);
            }
            if resume_at.is_some_and(|t| done >= t) {
                resume_at = None;
                let old = manager
                    .take_processor()
                    .expect("handed back within a few batches");
                assert!(manager.resume(Box::new(CoreEffect::new())).is_ok());
                drop(old);
            }
            h.collect_garbage();
        }
        assert!(
            heard[..12_000].iter().any(|s| s.abs() > 0.01),
            "block {block}: sound before"
        );
        assert!(
            heard[20_000..].iter().any(|s| s.abs() > 0.01),
            "block {block}: sound again after the restart"
        );
        assert!(heard.iter().all(|s| s.is_finite()));
    }
}
