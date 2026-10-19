//! The CLAP and the VST3 test plugins are one DSP behind two formats, so the same
//! input, params, sidechain and restored state give the same bits, and one rack that mixes both
//! formats (one manager holding both backends) bounces identically across host blocks and
//! workers.
// Justification: both formats run the same arithmetic; their outputs are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, NoteKind, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{Device, DeviceDescriptor};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineOptions, Graph, OfflineJob, ScheduledNote,
    WorkerConfig, new_engine_with_options, render_offline_scripted,
};
use atomik_plugin_host::PluginManager;
use atomik_test_plugin_core as core;
use atomik_testkit::fakes::{ConstantSource, SummingBus};
use atomik_testkit::no_alloc::assert_no_alloc;
use support::host_support::{CoreEffect, CoreSynth, fx_layout, rig as ref_rig, synth_layout};
use support::*;

fn ramp(scale: f32) -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 100.0;
    }
    b
}

fn both() -> (Kit, Kit) {
    (kit(Format::Clap), kit(Format::Vst3))
}

/// Runs the gain effect of `k` over a fixed script of params and signals; returns every output
/// batch of both channels.
fn run_effect(k: &Kit, state: Option<&[u8]>) -> Vec<[Batch; 2]> {
    let mut m = manager_for(k.format, std::slice::from_ref(&k.path));
    let mut req = request(&k.plugin(Plug::Gain));
    req.state = state.map(|s| wrap_state(k.format, s));
    let handle = m.instantiate(&req).unwrap();
    let mut h = atomik_testkit::harness::DeviceHarness::new(Box::new(handle.device), SR);
    let mut out = Vec::new();
    for n in 0..40u32 {
        if n % 5 == 0 {
            for (id, plain) in [
                (core::GainFx::GAIN, -((n % 13) as f64)),
                (core::GainFx::MUTE, f64::from((n / 10) % 2)),
                (core::GainFx::DUCK, 0.25 * f64::from(n % 4)),
            ] {
                let (host, _) = k.host_value(Plug::Gain, id, plain);
                h.set_param(&format!("p{id}"), host as f32);
            }
        }
        let sc = ramp(if n % 4 == 0 { 3.0 } else { 0.0 });
        *h.input_mut("in_l") = ramp(1.0 + n as f32 * 0.07);
        *h.input_mut("in_r") = ramp(-0.4 * n as f32);
        *h.input_mut("sc_in_l") = sc;
        *h.input_mut("sc_in_r") = sc;
        h.process();
        out.push([*h.output("out_l"), *h.output("out_r")]);
    }
    out
}

#[test]
fn the_clap_and_the_vst3_effect_give_the_same_bits_for_the_same_input_params_and_sidechain() {
    let (clap, vst3) = both();
    let a = run_effect(&clap, None);
    let b = run_effect(&vst3, None);
    assert!(
        a.iter().any(|o| o[0].iter().any(|s| s.abs() > 0.1)),
        "it sounds"
    );
    for (n, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(
            x[0].map(f32::to_bits),
            y[0].map(f32::to_bits),
            "batch {n} left"
        );
        assert_eq!(
            x[1].map(f32::to_bits),
            y[1].map(f32::to_bits),
            "batch {n} right"
        );
    }
}

#[test]
fn a_restored_state_gives_the_same_bits_in_both_formats() {
    let (clap, vst3) = both();
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::GAIN, -7.0);
    fx.set_param(core::GainFx::LATENCY, 33.0);
    fx.set_param(core::GainFx::DUCK, 0.5);
    let state = fx.save_state();
    let a = run_effect(&clap, Some(&state));
    let b = run_effect(&vst3, Some(&state));
    for (n, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(
            x[0].map(f32::to_bits),
            y[0].map(f32::to_bits),
            "batch {n} left"
        );
        assert_eq!(
            x[1].map(f32::to_bits),
            y[1].map(f32::to_bits),
            "batch {n} right"
        );
    }
}

// ----- one rack with both formats -----------------------------------------------------------------

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

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

/// A manager with both backends and both main test libraries.
fn mixed_manager(clap: &Kit, vst3: &Kit) -> PluginManager {
    let mut m = PluginManager::new();
    for k in [clap, vst3] {
        let backend = k.format.new_backend();
        backend.scan_file(&k.path).unwrap();
        m.add_backend(backend);
    }
    m
}

/// Where a chain gets its synth and effect: the reference DSP (`None`) or a format.
fn device(
    m: &mut PluginManager,
    kits: [&Kit; 2],
    format: Option<Format>,
    plug: Plug,
    tag: &str,
) -> (Box<dyn Device>, &'static DeviceDescriptor) {
    match format {
        None if plug == Plug::Synth => {
            let r = ref_rig(tag, &synth_layout(), Box::new(CoreSynth::new()));
            (Box::new(r.device), r.descriptor)
        }
        None => {
            let r = ref_rig(tag, &fx_layout(), Box::new(CoreEffect::new()));
            (Box::new(r.device), r.descriptor)
        }
        Some(f) => {
            let k = kits[usize::from(f == Format::Vst3)];
            let h = m.instantiate(&request(&k.plugin(plug))).unwrap();
            (Box::new(h.device), h.descriptor)
        }
    }
}

/// synth -> fx (sidechain from a constant source) -> out, the synth and the effect from the
/// given formats.
fn render_mixed(
    m: &mut PluginManager,
    kits: [&Kit; 2],
    synth: Option<Format>,
    fx: Option<Format>,
    block: usize,
    workers: u16,
) -> Vec<Vec<f32>> {
    let c = cfg();
    let mut g = Graph::new(&c);
    let (s, ds) = device(m, kits, synth, Plug::Synth, "mix.synth");
    let (f, df) = device(m, kits, fx, Plug::Gain, "mix.fx");
    let synth_id = g.add_device(s);
    let fx_id = g.add_device(f);
    let sc = g.add_device(ConstantSource::boxed(0.3));
    let bus = g.add_device(SummingBus::boxed());
    g.set_param(synth_id, ParamId(1), 1.0).unwrap();
    // -3 dB is an exact value in both formats (the VST3 gain spans -64 to 64 dB).
    let gain = match fx {
        Some(Format::Vst3) => kits[1].host_value(Plug::Gain, core::GainFx::GAIN, -3.0).0 as f32,
        _ => -3.0,
    };
    g.set_param(fx_id, ParamId(1), gain).unwrap();
    g.connect(
        ep(synth_id, socket(ds, "out_l")),
        ep(bus, SummingBus::SUM_IN),
    )
    .unwrap();
    g.connect(
        ep(bus, SummingBus::AUDIO_OUT),
        ep(fx_id, socket(df, "in_l")),
    )
    .unwrap();
    g.connect(
        ep(sc, ConstantSource::AUDIO_OUT),
        ep(fx_id, socket(df, "sc_in_l")),
    )
    .unwrap();
    let iface = g.descriptor(Graph::INTERFACE).unwrap();
    let out1 = ep(Graph::INTERFACE, iface.socket_by_key("out_1").unwrap().id);
    let out2 = ep(Graph::INTERFACE, iface.socket_by_key("out_2").unwrap().id);
    g.connect(ep(fx_id, socket(df, "out_l")), out1).unwrap();
    g.connect(ep(fx_id, socket(df, "out_r")), out2).unwrap();
    let mut notes = Vec::new();
    for i in 0..30u64 {
        let key = 50 + (i % 12) as u8;
        let t = 100 + i * 600;
        notes.push(ScheduledNote {
            frame: t,
            device: synth_id,
            socket: socket(ds, "notes_in"),
            kind: NoteKind::On { key, velocity: 1.0 },
        });
        notes.push(ScheduledNote {
            frame: t + 350,
            device: synth_id,
            socket: socket(ds, "notes_in"),
            kind: NoteKind::Off { key },
        });
    }
    let plan = g.compile().unwrap();
    let (mut e, mut h) = new_engine_with_options(
        c,
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: None,
            },
            ..EngineOptions::default()
        },
    );
    h.submit_plan(plan).unwrap();
    let job = OfflineJob {
        frames: 20_000,
        host_block: block,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    render_offline_scripted(&mut e, &mut h, &job, Vec::new(), |f| assert_no_alloc(f))
}

#[test]
fn a_rack_that_mixes_both_formats_bounces_the_same_across_blocks_and_workers() {
    let (clap, vst3) = both();
    let kits = [&clap, &vst3];
    let mut m = mixed_manager(&clap, &vst3);
    let reference = render_mixed(&mut m, kits, None, None, 64, 0);
    assert!(
        reference[0].iter().any(|s| s.abs() > 0.01),
        "the chain makes sound"
    );
    for (synth, fx) in [
        (Format::Clap, Format::Clap),
        (Format::Clap, Format::Vst3),
        (Format::Vst3, Format::Clap),
        (Format::Vst3, Format::Vst3),
    ] {
        let a = render_mixed(&mut m, kits, Some(synth), Some(fx), 64, 0);
        assert_eq!(
            bits(&a),
            bits(&reference),
            "{} synth into {} effect equals the reference",
            synth.name(),
            fx.name()
        );
        for (block, workers) in [(1usize, 0u16), (333, 2), (480, 4), (512, 1)] {
            let b = render_mixed(&mut m, kits, Some(synth), Some(fx), block, workers);
            assert_eq!(
                bits(&b),
                bits(&reference),
                "{} into {}: block {block} workers {workers}",
                synth.name(),
                fx.name()
            );
        }
    }
}

#[test]
fn one_manager_serves_both_formats_by_the_format_of_the_plugin() {
    let (clap, vst3) = both();
    let mut m = mixed_manager(&clap, &vst3);
    let a = m.instantiate(&request(&clap.plugin(Plug::Gain))).unwrap();
    let b = m.instantiate(&request(&vst3.plugin(Plug::Gain))).unwrap();
    assert_ne!(
        a.descriptor.name, "",
        "both descriptors are built (one per plugin)"
    );
    assert!(!std::ptr::eq(a.descriptor, b.descriptor));
    assert_eq!(m.layout(a.id).unwrap().name, m.layout(b.id).unwrap().name);
    assert_eq!(m.instance_count(), 2);
}
