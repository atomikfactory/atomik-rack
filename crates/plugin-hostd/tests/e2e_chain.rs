//! The sandboxed CLAP and VST3 test plugins in the real engine: the same chain (two synths, a
//! bus, an effect with a sidechain, notes and param changes) renders bit for bit like the same
//! plugins in process, across host blocks and worker counts, offline (AC-2), and live (the
//! one-batch deadline must be met), with no allocation anywhere on the audio path.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_core::{NoteKind, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{Device, DeviceDescriptor, RunMode};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineOptions, Graph, OfflineJob, ScheduledNote,
    WorkerConfig, new_engine_with_options, render_offline_scripted,
};
use atomik_plugin_host::{ManagerEvent, PluginManager};
use atomik_testkit::fakes::{ConstantSource, SummingBus};
use atomik_testkit::no_alloc::assert_no_alloc;
use sandbox_support::{FORMATS, Format, Plug, SR, Sandbox, in_process, request};

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

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

/// Where a chain gets its plugins: a manager (in process or sandboxed) and the mode it asks.
struct Source<'a> {
    format: Format,
    manager: &'a mut PluginManager,
    mode: RunMode,
}

impl Source<'_> {
    fn make(&mut self, plug: Plug) -> (Box<dyn Device>, &'static DeviceDescriptor) {
        let h = self
            .manager
            .instantiate(&request(&self.format.plugin(plug), self.mode))
            .unwrap();
        (Box::new(h.device), h.descriptor)
    }
}

struct Chain {
    plan: atomik_engine::RenderPlan,
    fx: DeviceInstanceId,
    notes: Vec<ScheduledNote>,
    gains: Vec<f32>,
}

fn note(
    frame: u64,
    device: DeviceInstanceId,
    socket: atomik_device_sdk::SocketId,
    kind: NoteKind,
) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket,
        kind,
    }
}

/// synth, synth2 -> bus -> fx (sidechain from a constant source) -> out.
fn chain(src: &mut Source<'_>) -> Chain {
    let c = cfg();
    let mut g = Graph::new(&c);
    let (s1, ds) = src.make(Plug::Synth);
    let (s2, ds2) = src.make(Plug::Synth);
    let (f, df) = src.make(Plug::Gain);
    let synth = g.add_device(s1);
    let synth2 = g.add_device(s2);
    let fx = g.add_device(f);
    let sc = g.add_device(ConstantSource::boxed(0.3));
    let bus = g.add_device(SummingBus::boxed());
    g.set_param(synth, ParamId(1), 1.0).unwrap();
    g.set_param(fx, ParamId(1), src.format.gain_value(-3.0))
        .unwrap();
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
        let notes_in = socket(d, "notes_in");
        notes.push(note(
            t,
            target,
            notes_in,
            NoteKind::On { key, velocity: 1.0 },
        ));
        notes.push(note(t + 300, target, notes_in, NoteKind::Off { key }));
    }
    let gains = (0..7)
        .map(|i| src.format.gain_value(-6.0 + f64::from(i)))
        .collect();
    Chain {
        plan: g.compile().unwrap(),
        fx,
        notes,
        gains,
    }
}

fn render_chain(mut src: Source<'_>, block: usize, workers: u16) -> Vec<Vec<f32>> {
    let ch = chain(&mut src);
    let (mut e, mut h) = new_engine_with_options(
        cfg(),
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: None,
            },
            ..EngineOptions::default()
        },
    );
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
    let rendered =
        render_offline_scripted(&mut e, &mut h, &job, Vec::new(), |f| assert_no_alloc(f));
    drop((e, h));
    rendered
}

/// Reaps the devices a finished render dropped.
fn reap(m: &mut PluginManager) {
    let events = m.poll();
    assert!(
        !events.iter().any(|e| matches!(
            e,
            ManagerEvent::Crashed { .. } | ManagerEvent::Failed { .. }
        )),
        "{events:?}"
    );
}

#[test]
fn sandboxed_plugins_in_the_engine_are_bit_identical_to_in_process_across_blocks_and_workers() {
    for format in FORMATS {
        let mut local = in_process(format, &[format.main_file()]);
        let reference = render_chain(
            Source {
                format,
                manager: &mut local,
                mode: RunMode::Offline,
            },
            64,
            0,
        );
        reap(&mut local);
        assert!(
            reference[0].iter().any(|s| s.abs() > 0.01),
            "the chain makes sound"
        );
        let mut sb = Sandbox::real(format);
        let mut check = |block: usize, workers: u16, what: &str| {
            let got = render_chain(
                Source {
                    format,
                    manager: &mut sb.manager,
                    mode: RunMode::Offline,
                },
                block,
                workers,
            );
            reap(&mut sb.manager);
            assert_eq!(
                bits(&got),
                bits(&reference),
                "{} sandboxed, {what}",
                format.name()
            );
        };
        check(64, 0, "host block 64");
        for block in [1usize, 333, 512] {
            check(block, 0, &format!("host block {block}"));
        }
        for workers in [1u16, 2, 4] {
            check(512, workers, &format!("{workers} workers"));
        }
        assert!(sb.wait_children(0, std::time::Duration::from_secs(5)));
        sb.finish();
    }
}

#[test]
#[cfg_attr(
    coverage,
    ignore = "a live deadline is not met by an instrumented build on a shared runner"
)]
fn a_live_render_of_sandboxed_plugins_meets_every_batch_deadline_and_equals_in_process() {
    for format in FORMATS {
        let mut local = in_process(format, &[format.main_file()]);
        let reference = render_chain(
            Source {
                format,
                manager: &mut local,
                mode: RunMode::Realtime,
            },
            64,
            0,
        );
        reap(&mut local);
        let mut sb = Sandbox::real(format);
        let got = render_chain(
            Source {
                format,
                manager: &mut sb.manager,
                mode: RunMode::Realtime,
            },
            64,
            0,
        );
        reap(&mut sb.manager);
        assert_eq!(
            bits(&got),
            bits(&reference),
            "{}: a late batch would show here",
            format.name()
        );
        sb.finish();
    }
}
