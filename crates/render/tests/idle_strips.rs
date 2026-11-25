//! A channel or bus strip that has gone silent is not called, and the render is the render it
//! would have been. Two levels: an engine of one strip (and a bus it plays into) fed noise for
//! a second, left to run dry (the engine's counter says for how many batches it was skipped),
//! with a parameter, a mute and a tempo change in the silence and noise again after it, every
//! output metered batch by batch; and whole projects, the example projects and a generated song
//! of two dozen strips with sends, bounced with the skip on and off.
atomik_testkit::install_alloc_guard!();

use atomik_core::{BATCH, PPQ, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, Topology, new_engine, set_default_idle_skip,
};
use atomik_model::stress::large_song_doc;
use atomik_model::{Command, ConsoleCommand, DeviceKey, Document, RackDoc, SongCommand};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc, render_project_file};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const SR: u64 = 48_000;
/// Seconds of audio rendered by the engine scenario.
const SECONDS: u64 = 20;
/// Batches in a second.
const PER_SECOND: u64 = SR / BATCH as u64;

static NOISE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.timed-noise",
    name: "Timed noise",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[SocketDescriptor {
        id: SocketId(0),
        key: "audio_out",
        name: "audio_out",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    }],
};

/// Noise during two one-second windows of the timeline (a function of the frame, so every
/// render of it is the same), exact `+0.0` otherwise. A source: it never says it is quiescent.
struct TimedNoise {
    windows: [(u64, u64); 2],
    state: u32,
}

impl Device for TimedNoise {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &NOISE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let frame = ctx.time.frame;
        let sounding = self.windows.iter().any(|&(a, b)| frame >= a && frame < b);
        let mut out = [0.0f32; BATCH];
        if sounding {
            for x in &mut out {
                self.state = self
                    .state
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                *x = (self.state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            }
        }
        *ctx.output(SocketId(0)) = out;
    }
}

/// A strip with every section on and something to do in every stage.
const FULL: [(&str, f32); 26] = [
    ("input.gain", 3.0),
    ("hpf.on", 1.0),
    ("lpf.on", 1.0),
    ("lpf.freq", 9_000.0),
    ("comp.on", 1.0),
    ("comp.threshold", -30.0),
    ("comp.ratio", 6.0),
    ("comp.release", 120.0),
    ("comp.auto_makeup", 1.0),
    ("eq.on", 1.0),
    ("eq.hf.gain", 5.0),
    ("eq.hmf.gain", -4.0),
    ("eq.lmf.gain", 3.0),
    ("eq.lf.gain", 4.0),
    ("pcomp.on", 1.0),
    ("pcomp.mix", 0.4),
    ("latency.adjust", 120.0),
    ("width", 0.7),
    ("pan", -0.4),
    ("fader", 0.6),
    ("send1.on", 1.0),
    ("send2.on", 1.0),
    ("send2.pre", 1.0),
    ("send3.on", 1.0),
    ("send3.level", -6.0),
    ("send8.on", 1.0),
];

/// What one run of the engine scenario saw.
struct Seen {
    /// Peak and RMS of every metered output of every batch, as bits.
    meters: Vec<u32>,
    /// Batches the strip and the bus were skipped for.
    skipped: (u64, u64),
}

fn param(registry: &atomik_device_sdk::Registry, type_id: &str, key: &str) -> ParamId {
    registry
        .factory(type_id)
        .unwrap()
        .descriptor
        .param_by_key(key)
        .unwrap_or_else(|| panic!("{type_id} has no {key}"))
        .id
}

/// Noise -> channel strip (-> output bus, with the follow link), each output metered, rendered
/// with the skip on or off.
fn scenario(skip: bool, with_bus: bool) -> Seen {
    let c = EngineConfig {
        sample_rate: SampleRate::new(SR as f64).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 1,
        seed: 7,
    };
    let registry = default_registry();
    let (mut engine, mut handle) = new_engine(c);
    engine.set_idle_skip(skip);
    let mut t = Topology::new(&c);
    let (src, strip, bus) = (
        DeviceInstanceId(1),
        DeviceInstanceId(2),
        DeviceInstanceId(3),
    );
    let channel = registry.factory("atomik.channel").unwrap();
    let bus_factory = registry.factory("atomik.bus").unwrap();
    let ep = |d: DeviceInstanceId, desc: &DeviceDescriptor, key: &str| Endpoint {
        device: d,
        socket: desc.socket_by_key(key).unwrap().id,
    };
    handle
        .insert_device(
            src,
            Box::new(TimedNoise {
                windows: [(0, SR), (14 * SR, 15 * SR)],
                state: 0x1234_5678,
            }),
            &[],
        )
        .unwrap();
    t.add_node(src, &NOISE).unwrap();
    let params: Vec<_> = FULL
        .iter()
        .map(|&(k, v)| (param(&registry, "atomik.channel", k), v))
        .collect();
    handle
        .insert_device(strip, (channel.create)(), &params)
        .unwrap();
    t.add_node(strip, channel.descriptor).unwrap();
    let from = Endpoint {
        device: src,
        socket: SocketId(0),
    };
    for key in ["in_l", "in_r"] {
        t.connect(from, ep(strip, channel.descriptor, key)).unwrap();
    }
    let mut metered = vec![
        ep(strip, channel.descriptor, "out_l"),
        ep(strip, channel.descriptor, "out_r"),
        ep(strip, channel.descriptor, "send1_l"),
        ep(strip, channel.descriptor, "send2_r"),
        ep(strip, channel.descriptor, "insert_send_l"),
        ep(strip, channel.descriptor, "gr_cv_out"),
    ];
    if with_bus {
        let bus_params = [
            ("fader", 0.7),
            ("hpf.on", 1.0),
            ("comp.on", 1.0),
            ("comp.threshold", -24.0),
            ("send1.on", 1.0),
        ]
        .map(|(k, v)| (param(&registry, "atomik.bus", k), v));
        handle
            .insert_device(bus, (bus_factory.create)(), &bus_params)
            .unwrap();
        t.add_node(bus, bus_factory.descriptor).unwrap();
        for (out, input) in [("out_l", "bus_l"), ("out_r", "bus_r")] {
            t.connect(
                ep(strip, channel.descriptor, out),
                ep(bus, bus_factory.descriptor, input),
            )
            .unwrap();
        }
        t.connect(
            ep(bus, bus_factory.descriptor, "follow_out"),
            ep(strip, channel.descriptor, "follow_in"),
        )
        .unwrap();
        metered.push(ep(bus, bus_factory.descriptor, "out_l"));
        metered.push(ep(bus, bus_factory.descriptor, "send1_r"));
        metered.push(ep(bus, bus_factory.descriptor, "follow_out"));
    }
    for &endpoint in &metered {
        t.add_meter(endpoint).unwrap();
    }
    handle.submit_routing(t.compile().unwrap()).unwrap();
    let meters = handle.meters();
    let (fader, hpf, mute, threshold) = (
        param(&registry, "atomik.channel", "fader"),
        param(&registry, "atomik.channel", "hpf.freq"),
        param(&registry, "atomik.channel", "mute"),
        param(&registry, "atomik.channel", "comp.threshold"),
    );
    let mut out = [0.0f32; BATCH];
    let mut seen = Vec::new();
    for n in 0..SECONDS * PER_SECOND {
        match n {
            x if x == 9 * PER_SECOND => handle.set_param(strip, hpf, 140.0).unwrap(),
            x if x == 10 * PER_SECOND => handle.set_param(strip, mute, 1.0).unwrap(),
            x if x == 10 * PER_SECOND + 300 => handle.set_param(strip, mute, 0.0).unwrap(),
            x if x == 11 * PER_SECOND => handle.set_param(strip, fader, 0.45).unwrap(),
            x if x == 12 * PER_SECOND => handle.set_tempo(Tempo::new(140.0).unwrap()).unwrap(),
            x if x == 13 * PER_SECOND => handle.set_param(strip, threshold, -24.0).unwrap(),
            _ => {}
        }
        assert_no_alloc(|| engine.render(&[], &mut [&mut out]));
        for &endpoint in &metered {
            let r = meters.read(endpoint).expect("a metered output");
            seen.push(r.peak.to_bits());
            seen.push(r.rms.to_bits());
        }
    }
    Seen {
        meters: seen,
        skipped: (
            engine.idle_skipped(strip).unwrap(),
            if with_bus {
                engine.idle_skipped(bus).unwrap()
            } else {
                0
            },
        ),
    }
}

fn compare(with_bus: bool) -> (u64, u64) {
    let off = scenario(false, with_bus);
    let on = scenario(true, with_bus);
    assert_eq!(off.skipped, (0, 0), "the switch is off");
    assert!(on.meters.iter().any(|&b| b != 0), "the project does sound");
    assert!(
        on.meters == off.meters,
        "the skip changed what the meters show"
    );
    on.skipped
}

#[test]
fn a_strip_that_ran_dry_is_skipped_and_its_meters_read_the_same() {
    let (strip, _) = compare(false);
    // Noise for the first second, the compressors' release stages and the latency ring run dry
    // about 5 s in, the noise comes back at 14 s (and the edits wake it for a batch or two).
    assert!(strip >= 4_000, "{strip} batches skipped");
}

#[test]
fn a_strip_and_the_bus_it_plays_into_are_skipped_and_the_meters_read_the_same() {
    let (strip, bus) = compare(true);
    assert!(strip >= 4_000, "{strip} batches skipped");
    assert!(bus >= 4_000, "{bus} batches skipped");
}

fn bits_of(rendered: &atomik_render::Rendered) -> Vec<u32> {
    rendered
        .channels
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect()
}

fn options() -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(256).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn examples() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "atomik")
                && !path.to_string_lossy().contains("plugin-demo")
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects"),
        &mut files,
    );
    files.sort();
    files
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// `tracks` synth tracks of which only the first `playing` hold a clip (one bar, at the start),
/// every strip with its sections on and its first two sends on, a reverb on the first return, a
/// bus the odd strips play into (send follow on), and a song of eight bars.
fn generated(tracks: usize, playing: usize) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(large_song_doc(&registry, tracks, playing, playing * 2));
    let apply = |d: &mut Document, c: Command| {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    let set = |k: &DeviceKey, param: &str, value: f32| Command::SetParam {
        key: k.clone(),
        param: param.into(),
        value,
        merge: None,
    };
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.reverb".into(),
            key: Some(key("hall")),
        },
    );
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("hall")),
        }),
    );
    apply(
        &mut d,
        Command::Console(ConsoleCommand::CreateBus {
            name: "Group".into(),
            dest: atomik_model::console::OutputDest::Master,
        }),
    );
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetSendFollow(true)),
    );
    let channels: Vec<DeviceKey> = d
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .collect();
    assert_eq!(channels.len(), tracks);
    for (i, ch) in channels.iter().enumerate() {
        for (k, v) in [
            ("hpf.on", 1.0),
            ("comp.on", 1.0),
            ("comp.threshold", -28.0),
            ("eq.on", 1.0),
            ("eq.hf.gain", 3.0),
            ("pcomp.on", 1.0),
            ("send1.on", 1.0),
            ("send2.on", 1.0),
            ("send2.pre", 1.0),
        ] {
            apply(&mut d, set(ch, k, v));
        }
        if i % 2 == 1 {
            apply(
                &mut d,
                Command::Console(ConsoleCommand::SetOutput {
                    strip: ch.clone(),
                    dest: atomik_model::console::OutputDest::Bus(key("bus")),
                }),
            );
        }
    }
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(8 * 4 * PPQ)));
    d.doc().clone()
}

/// Every render in one test (the switch is process-wide): the example projects and the generated
/// song, with the skip on and off, byte for byte.
#[test]
fn projects_with_silent_strips_render_the_same_bytes_skipped_or_not() {
    let registry = default_registry();
    let render_both = |render: &dyn Fn() -> Vec<u32>, what: &str| {
        set_default_idle_skip(false);
        let off = render();
        set_default_idle_skip(true);
        let on = render();
        assert!(!off.is_empty(), "{what}: renders something");
        assert!(on == off, "{what}: the skip changed the render");
    };
    for path in examples() {
        render_both(
            &|| {
                bits_of(
                    &render_project_file(&path, &registry, &options())
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
                )
            },
            &path.display().to_string(),
        );
    }
    for (tracks, playing) in [(24, 6), (24, 0), (8, 8)] {
        let doc = generated(tracks, playing);
        render_both(
            &|| bits_of(&render_doc(&doc, &registry, &options()).expect("it renders")),
            &format!("{tracks} tracks, {playing} playing"),
        );
    }
    set_default_idle_skip(true);
}
