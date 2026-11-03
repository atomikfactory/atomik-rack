//! A plugin with four stereo outputs in the real engine: each output pair reaches its own pair
//! of interface channels in a bounce, and a sandboxed plugin renders bit for bit like the same
//! plugin in process, across host blocks.
// Justification: copied samples are compared bit for bit and the pair gains are powers of two.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_core::{NoteKind, SampleRate, Tempo};
use atomik_device_sdk::{DeviceDescriptor, PluginFormat, PluginRef, RunMode, SocketId};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineOptions, Graph, OfflineJob, ScheduledNote,
    new_engine_with_options, render_offline_scripted,
};
use atomik_plugin_host::PluginManager;
use atomik_testkit::no_alloc::assert_no_alloc;
use sandbox_support::{Format, SR, Sandbox, in_process, request};

const CHANNELS: u16 = 8;
/// Gain of each pair against the main one (see the test plugin).
const GAINS: [f32; 4] = [1.0, 0.5, 0.25, 0.125];

fn plugin() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: "atomik.test.multiout".to_string(),
    }
}

fn library() -> std::path::PathBuf {
    Format::Clap.install("atomik-test-clap-variants")
}

fn cfg() -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: CHANNELS,
        seed: 1,
    }
}

fn socket(d: &'static DeviceDescriptor, key: &str) -> SocketId {
    d.socket_by_key(key)
        .unwrap_or_else(|| panic!("no socket {key}"))
        .id
}

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

/// The multi-output synth, every pair cabled to its own interface channels, playing a few notes.
fn render(manager: &mut PluginManager, mode: RunMode, block: usize) -> Vec<Vec<f32>> {
    let c = cfg();
    let mut g = Graph::new(&c);
    let h = manager.instantiate(&request(&plugin(), mode)).unwrap();
    let d = h.descriptor;
    assert!(d.socket_by_key("out_aux_3_r").is_some(), "three aux pairs");
    assert!(d.socket_by_key("out_aux_4_l").is_none());
    let synth = g.add_device(Box::new(h.device));
    let iface = g.descriptor(Graph::INTERFACE).unwrap();
    let pairs = [
        ("out_l", "out_r"),
        ("out_aux_1_l", "out_aux_1_r"),
        ("out_aux_2_l", "out_aux_2_r"),
        ("out_aux_3_l", "out_aux_3_r"),
    ];
    for (k, (l, r)) in pairs.into_iter().enumerate() {
        for (side, key) in [l, r].into_iter().enumerate() {
            let channel = format!("out_{}", 2 * k + side + 1);
            g.connect(
                Endpoint {
                    device: synth,
                    socket: socket(d, key),
                },
                Endpoint {
                    device: Graph::INTERFACE,
                    socket: socket(iface, &channel),
                },
            )
            .unwrap();
        }
    }
    let notes: Vec<ScheduledNote> = (0..8u64)
        .flat_map(|i| {
            let key = 52 + (i % 5) as u8 * 3;
            let t = 100 + i * 1_300;
            let notes_in = socket(d, "notes_in");
            [
                ScheduledNote {
                    frame: t,
                    device: synth as DeviceInstanceId,
                    socket: notes_in,
                    kind: NoteKind::On { key, velocity: 1.0 },
                },
                ScheduledNote {
                    frame: t + 700,
                    device: synth,
                    socket: notes_in,
                    kind: NoteKind::Off { key },
                },
            ]
        })
        .collect();
    let (mut e, mut handle) = new_engine_with_options(c, EngineOptions::default());
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let job = OfflineJob {
        frames: 12_000,
        host_block: block,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let rendered = render_offline_scripted(&mut e, &mut handle, &job, Vec::new(), |f| {
        assert_no_alloc(f)
    });
    drop((e, handle));
    rendered
}

fn reap(m: &mut PluginManager) {
    let events = m.poll();
    assert!(
        !events.iter().any(|e| matches!(
            e,
            atomik_plugin_host::ManagerEvent::Crashed { .. }
                | atomik_plugin_host::ManagerEvent::Failed { .. }
        )),
        "{events:?}"
    );
}

#[test]
fn each_output_pair_of_a_plugin_reaches_its_own_channels() {
    let mut local = in_process(Format::Clap, &[library()]);
    let out = render(&mut local, RunMode::Offline, 64);
    reap(&mut local);
    assert_eq!(out.len(), usize::from(CHANNELS));
    assert!(
        out[0].iter().any(|s| s.abs() > 0.01),
        "the synth makes sound"
    );
    for k in 1..4 {
        for (i, &main) in out[0].iter().enumerate() {
            assert_eq!(out[2 * k][i], main * GAINS[k], "pair {k} left, frame {i}");
            assert_eq!(
                out[2 * k + 1][i],
                -main * GAINS[k],
                "pair {k} right, frame {i}"
            );
        }
    }
}

#[test]
fn sandboxed_aux_outputs_are_bit_identical_to_in_process_across_blocks() {
    let mut local = in_process(Format::Clap, &[library()]);
    let reference = render(&mut local, RunMode::Offline, 64);
    reap(&mut local);
    let mut sb = Sandbox::new(
        PluginFormat::Clap,
        sandbox_support::config(),
        &[(library(), vec![plugin()])],
    );
    for block in [64usize, 1, 333, 512] {
        let got = render(&mut sb.manager, RunMode::Offline, block);
        reap(&mut sb.manager);
        assert_eq!(
            bits(&got),
            bits(&reference),
            "sandboxed, host block {block}"
        );
    }
    assert!(sb.wait_children(0, std::time::Duration::from_secs(5)));
    sb.finish();
}
