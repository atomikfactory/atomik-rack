//! (host half) with the real CLAP and VST3 plugins in the real
//! engine: the output equals the reference DSP chain bit for bit and is identical across host
//! blocks 1, 64, 333, 480, 512 and worker counts 0, 1, 2, 4, with no allocation anywhere on the
//! audio path; PDC follows a plugin's latency, including a change at run time; plugins run on
//! any worker and pinned ones stay put; a restart in the middle of a render leaves a bounded gap.
// Justification: exact impulse amplitudes and copied samples are compared.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteKind, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{Device, DeviceDescriptor};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineHandle, EngineOptions, Graph,
    OfflineJob, ScheduledNote, Topology, WorkerConfig, new_engine_with_options,
    render_offline_scripted,
};
use atomik_plugin_host::{ManagerEvent, PluginManager};
use atomik_test_plugin_core as core;
use atomik_testkit::fakes::{ConstantSource, ImpulseOnNote, Passthrough, SummingBus};
use atomik_testkit::no_alloc::assert_no_alloc;
use support::host_support::{CoreEffect, CoreSynth, fx_layout, rig as ref_rig, synth_layout};
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

fn socket(d: &'static DeviceDescriptor, key: &str) -> atomik_device_sdk::SocketId {
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

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

fn setup(format: Format) -> (Kit, PluginManager) {
    let k = kit(format);
    let m = manager_for(format, std::slice::from_ref(&k.path));
    (k, m)
}

/// Where the plugin devices of a chain come from.
enum Source<'a> {
    /// The reference DSP behind `PluginDevice`.
    Reference,
    /// The real test plugins of a format through the manager.
    Real(&'a Kit, &'a mut PluginManager),
}

impl Source<'_> {
    fn synth(&mut self, tag: &str) -> (Box<dyn Device>, &'static DeviceDescriptor) {
        match self {
            Source::Reference => {
                let r = ref_rig(tag, &synth_layout(), Box::new(CoreSynth::new()));
                (Box::new(r.device), r.descriptor)
            }
            Source::Real(k, m) => {
                let h = m.instantiate(&request(&k.plugin(Plug::Synth))).unwrap();
                (Box::new(h.device), h.descriptor)
            }
        }
    }

    fn fx(&mut self, tag: &str) -> (Box<dyn Device>, &'static DeviceDescriptor) {
        match self {
            Source::Reference => {
                let r = ref_rig(tag, &fx_layout(), Box::new(CoreEffect::new()));
                (Box::new(r.device), r.descriptor)
            }
            Source::Real(k, m) => {
                let h = m.instantiate(&request(&k.plugin(Plug::Gain))).unwrap();
                (Box::new(h.device), h.descriptor)
            }
        }
    }

    /// The rack value of a gain in dB for this source (the reference takes plain values).
    fn gain(&self, db: f64) -> f32 {
        match self {
            Source::Reference => db as f32,
            Source::Real(k, _) => k.host_value(Plug::Gain, core::GainFx::GAIN, db).0 as f32,
        }
    }
}

struct Chain {
    plan: atomik_engine::RenderPlan,
    fx: DeviceInstanceId,
    notes: Vec<ScheduledNote>,
    gains: Vec<f32>,
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

/// synth, synth2 -> bus -> fx (sidechain from a constant source) -> out. Two parallel synths
/// make the level wide enough for workers.
fn chain(src: &mut Source<'_>) -> Chain {
    let c = cfg();
    let mut g = Graph::new(&c);
    let (s1, ds) = src.synth("chain.synth");
    let (s2, ds2) = src.synth("chain.synth2");
    let (f, df) = src.fx("chain.fx");
    let synth = g.add_device(s1);
    let synth2 = g.add_device(s2);
    let fx = g.add_device(f);
    let sc = g.add_device(ConstantSource::boxed(0.3));
    let bus = g.add_device(SummingBus::boxed());
    g.set_param(synth, ParamId(1), 1.0).unwrap();
    g.set_param(fx, ParamId(1), src.gain(-3.0)).unwrap();
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
        let (target, d) = if i % 2 == 0 {
            (synth, ds)
        } else {
            (synth2, ds2)
        };
        notes.push(note_on(t, target, socket(d, "notes_in"), key));
        notes.push(ScheduledNote {
            frame: t + 300,
            device: target,
            socket: socket(d, "notes_in"),
            kind: NoteKind::Off { key },
        });
    }
    let gains = (0..7).map(|i| src.gain(-6.0 + f64::from(i))).collect();
    Chain {
        plan: g.compile().unwrap(),
        fx,
        notes,
        gains,
    }
}

fn render_chain(mut src: Source<'_>, block: usize, workers: u16) -> Vec<Vec<f32>> {
    let ch = chain(&mut src);
    let (mut e, mut h) = engine(workers);
    h.submit_plan(ch.plan).unwrap();
    let params: Vec<(DeviceInstanceId, ParamId, f32)> = (0..60)
        .map(|i| (ch.fx, ParamId(1), ch.gains[i % 7]))
        .collect();
    let job = OfflineJob {
        frames: 24_000,
        host_block: block,
        notes: &ch.notes,
        params: &params,
        inputs: &[],
    };
    render_offline_scripted(&mut e, &mut h, &job, Vec::new(), |f| assert_no_alloc(f))
}

#[test]
fn real_plugins_equal_the_reference_dsp_and_are_bit_identical_across_blocks_and_workers() {
    let reference = render_chain(Source::Reference, 64, 0);
    assert!(
        reference[0].iter().any(|s| s.abs() > 0.01),
        "the chain makes sound"
    );
    for format in FORMATS {
        let (k, mut m) = setup(format);
        let real = render_chain(Source::Real(&k, &mut m), 64, 0);
        assert_eq!(
            bits(&real),
            bits(&reference),
            "real {} == reference DSP",
            format.name()
        );
        for block in [1usize, 333, 480, 512] {
            let got = render_chain(Source::Real(&k, &mut m), block, 0);
            assert_eq!(
                bits(&got),
                bits(&reference),
                "{} host block {block}",
                format.name()
            );
        }
        for workers in [1u16, 2, 4] {
            let got = render_chain(Source::Real(&k, &mut m), 512, workers);
            assert_eq!(
                bits(&got),
                bits(&reference),
                "{} workers {workers}",
                format.name()
            );
        }
    }
}

#[test]
fn two_runs_of_the_same_chain_are_identical() {
    for format in FORMATS {
        let (k, mut m) = setup(format);
        let a = render_chain(Source::Real(&k, &mut m), 64, 2);
        let b = render_chain(Source::Real(&k, &mut m), 64, 2);
        assert_eq!(bits(&a), bits(&b), "{}", format.name());
    }
}

// ----- PDC --------------------------------------------------------------------------------------

struct Pdc {
    g: Graph,
    click: DeviceInstanceId,
}

fn pdc_graph(
    k: &Kit,
    m: &mut PluginManager,
    state: Option<atomik_device_sdk::PluginStateBlob>,
) -> Pdc {
    let mut g = Graph::new(&cfg());
    let click = g.add_device(ImpulseOnNote::boxed());
    let dry = g.add_device(Passthrough::boxed(1.0));
    let bus = g.add_device(SummingBus::boxed());
    let mut req = request(&k.plugin(Plug::Gain));
    req.state = state;
    let h = m.instantiate(&req).unwrap();
    let d = h.descriptor;
    let fx = g.add_device(Box::new(h.device));
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

#[test]
fn the_latency_a_real_plugin_reports_makes_pdc_align_it_with_a_dry_path() {
    for format in FORMATS {
        let (k, mut m) = setup(format);
        let mut fx = core::GainFx::new(SR);
        fx.set_param(core::GainFx::LATENCY, 100.0);
        let p = pdc_graph(&k, &mut m, Some(wrap_state(format, &fx.save_state())));
        let note = note_on(130, p.click, ImpulseOnNote::NOTES_IN, 60);
        let plan = p.g.compile().unwrap();
        let aligned = atomik_engine::render_offline(plan, cfg(), 8 * BATCH, 64, &[note], &[]);
        assert_eq!(
            hits(&aligned[0]),
            vec![(230, 2.0)],
            "{}: both clicks at 130 + 100",
            format.name()
        );
    }
}

#[test]
fn a_latency_change_at_run_time_restarts_the_plugin_and_realigns_after_the_routing_is_recompiled() {
    for format in FORMATS {
        let c = cfg();
        let (k, mut m) = setup(format);
        let (mut e, mut h) = engine(0);
        let (click_id, dry_id, bus_id, fx_id) = (
            DeviceInstanceId(1),
            DeviceInstanceId(2),
            DeviceInstanceId(3),
            DeviceInstanceId(4),
        );
        let handle = m.instantiate(&request(&k.plugin(Plug::Gain))).unwrap();
        let (d, plugin) = (handle.descriptor, handle.id);
        h.insert_device(click_id, ImpulseOnNote::boxed(), &[])
            .unwrap();
        h.insert_device(dry_id, Passthrough::boxed(1.0), &[])
            .unwrap();
        h.insert_device(bus_id, SummingBus::boxed(), &[]).unwrap();
        h.insert_device(fx_id, Box::new(handle.device), &[])
            .unwrap();
        let mut topo = Topology::new(&c);
        topo.add_node(click_id, &atomik_testkit::fakes::IMPULSE_ON_NOTE)
            .unwrap();
        topo.add_node(dry_id, &atomik_testkit::fakes::PASSTHROUGH)
            .unwrap();
        topo.add_node(bus_id, &atomik_testkit::fakes::SUMMING_BUS)
            .unwrap();
        topo.add_node(fx_id, d).unwrap();
        let iface = Graph::new(&c);
        for (from, to) in [
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
        ] {
            topo.connect(from, to).unwrap();
        }
        h.submit_routing(topo.compile().unwrap()).unwrap();

        let pos = std::cell::Cell::new(0u64);
        let render = |e: &mut Engine,
                      m: &mut PluginManager,
                      blocks: usize|
         -> (Vec<f32>, Vec<ManagerEvent>) {
            let mut all = Vec::new();
            let mut events = Vec::new();
            for _ in 0..blocks {
                let (mut l, mut r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
                assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
                all.extend_from_slice(&l);
                pos.set(pos.get() + BATCH as u64);
                events.extend(m.poll());
            }
            (all, events)
        };
        // 1. No latency anywhere: one click of 2.0.
        h.send_note(note_on(
            pos.get() + 10,
            click_id,
            ImpulseOnNote::NOTES_IN,
            60,
        ))
        .unwrap();
        let (a, _) = render(&mut e, &mut m, 6);
        assert_eq!(hits(&a).len(), 1, "{}: {:?}", format.name(), hits(&a));
        assert_eq!(hits(&a)[0].1, 2.0);
        // 2. The plugin delays by 100 frames: it tells the host, the manager restarts it and
        // publishes the latency; the engine notices.
        let epoch = h.latency_epoch();
        h.set_param(fx_id, ParamId(3), 100.0).unwrap();
        let (_, mut events) = render(&mut e, &mut m, 2);
        events.extend(render(&mut e, &mut m, 8).1);
        assert!(
            events.contains(&ManagerEvent::LatencyChanged {
                instance: plugin,
                frames: 100
            }),
            "{}: {events:?}",
            format.name()
        );
        assert_ne!(
            h.latency_epoch(),
            epoch,
            "the engine published the new latency"
        );
        assert_eq!(h.device_latency(fx_id), Some(100));
        // 3. The app refills the latencies and resubmits the routing: aligned again.
        h.fill_latencies(&mut topo);
        h.submit_routing(topo.compile().unwrap()).unwrap();
        render(&mut e, &mut m, 4);
        h.send_note(note_on(
            pos.get() + 10,
            click_id,
            ImpulseOnNote::NOTES_IN,
            60,
        ))
        .unwrap();
        let (c3, _) = render(&mut e, &mut m, 6);
        let hc = hits(&c3);
        assert_eq!(hc.len(), 1, "{}: {hc:?}", format.name());
        assert_eq!(hc[0].1, 2.0);
    }
}

// ----- threads ---------------------------------------------------------------------------------------

#[test]
fn plugins_run_on_any_worker_and_pinned_plugins_stay_on_the_audio_thread() {
    for format in FORMATS {
        let lib = TestLib::open_path(&kit(format).path);
        for workers in [0u16, 1, 2, 4] {
            let c = cfg();
            let (k, mut m) = setup(format);
            let (mut e, mut h) = engine(workers);
            let mut topo = Topology::new(&c);
            let mut natives = Vec::new();
            for i in 0..6u32 {
                let mut req = request(&k.plugin(Plug::Gain));
                req.pinned = i % 2 == 0;
                let handle = m.instantiate(&req).unwrap();
                natives.push(m.native_handle(handle.id).unwrap());
                let id = DeviceInstanceId(i + 1);
                let src = DeviceInstanceId(100 + i);
                topo.add_node(id, handle.descriptor).unwrap();
                topo.add_node(src, &atomik_testkit::fakes::CONSTANT_SOURCE)
                    .unwrap();
                topo.connect(
                    ep(src, ConstantSource::AUDIO_OUT),
                    ep(id, socket(handle.descriptor, "in_l")),
                )
                .unwrap();
                h.insert_device(id, Box::new(handle.device), &[]).unwrap();
                h.insert_device(src, ConstantSource::boxed(0.3), &[])
                    .unwrap();
            }
            h.submit_routing(topo.compile().unwrap()).unwrap();
            // A signal in every effect: a VST3 effect with a zero tail is skipped without one.
            let rendered = std::thread::spawn(move || {
                for _ in 0..100 {
                    let (mut l, mut r) = ([0.0f32; BATCH], [0.0f32; BATCH]);
                    assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
                }
                (e, h)
            })
            .join()
            .unwrap();
            for (i, native) in natives.iter().enumerate() {
                assert_eq!(
                    lib.probe_raw(*native, probe::PROCESS_CALLS),
                    100,
                    "{} workers {workers} device {i}",
                    format.name()
                );
                if format == Format::Clap {
                    // CLAP asks the host which thread it is on; VST3 has no such question.
                    assert_eq!(lib.probe_raw(*native, probe::AUDIO_THREAD_TRUE), 100);
                    assert_eq!(lib.probe_raw(*native, probe::MAIN_THREAD_TRUE), 0);
                }
            }
            drop(rendered);
        }
    }
}

// ----- a restart in the middle of a render ---------------------------------------------------------------

#[test]
fn a_restart_in_the_middle_of_a_render_is_allocation_free_and_the_sound_resumes() {
    for format in FORMATS {
        for block in [1usize, 64, 333, 512] {
            let c = cfg();
            let (k, mut m) = setup(format);
            let (mut e, mut h) = engine(2);
            let hs = m.instantiate(&request(&k.plugin(Plug::Synth))).unwrap();
            let hf = m.instantiate(&request(&k.plugin(Plug::Gain))).unwrap();
            let (ds, df, fx_id_m) = (hs.descriptor, hf.descriptor, hf.id);
            let mut topo = Topology::new(&c);
            let (synth, fx) = (DeviceInstanceId(1), DeviceInstanceId(2));
            topo.add_node(synth, ds).unwrap();
            topo.add_node(fx, df).unwrap();
            h.insert_device(synth, Box::new(hs.device), &[]).unwrap();
            h.insert_device(fx, Box::new(hf.device), &[]).unwrap();
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
            let mut restarted = false;
            while done < total {
                let n = block.min(total - done);
                let mut l = vec![0.0f32; n];
                let mut r = vec![0.0f32; n];
                assert_no_alloc(|| e.render(&[], &mut [&mut l, &mut r]));
                heard.extend_from_slice(&l);
                done += n;
                for kk in 0..20 {
                    let gain = k
                        .host_value(Plug::Gain, core::GainFx::GAIN, -f64::from(kk))
                        .0;
                    let _ = h.set_param(fx, ParamId(1), gain as f32);
                }
                if restart_at.is_some_and(|t| done >= t) {
                    restart_at = None;
                    m.restart(fx_id_m).unwrap();
                }
                restarted |= m.poll().contains(&ManagerEvent::Restarted(fx_id_m));
                h.collect_garbage();
            }
            assert!(
                restarted,
                "{} block {block}: the restart completed",
                format.name()
            );
            assert!(
                heard[..12_000].iter().any(|s| s.abs() > 0.01),
                "{} block {block}: sound before",
                format.name()
            );
            assert!(
                heard[20_000..].iter().any(|s| s.abs() > 0.01),
                "{} block {block}: sound again after",
                format.name()
            );
            assert!(heard.iter().all(|s| s.is_finite()));
        }
    }
}
