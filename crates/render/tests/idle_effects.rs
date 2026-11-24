//! An effect that has run dry is not called, and the render is the render it would have been:
//! noise for a second, silence long enough for the effect to be skipped (the engine's counter
//! says for how many batches), then a parameter change and a tempo change in the silence, then
//! noise again, rendered with the skip on and off through the offline path. The same with the
//! effect bypassed and switched back on during the silence (the bypass wrapper stops the effect
//! after two seconds and is quiescent behind it). One table row per effect; the stereo delay,
//! the first one, has a file of its own (`idle_stereo_delay.rs`).
atomik_testkit::install_alloc_guard!();

use atomik_core::{BATCH, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{
    DeviceInstanceId, EngineConfig, Graph, HotEdit, OfflineJob, TimedEdit, new_engine,
    render_offline_scripted,
};
use atomik_render::default_registry;
use atomik_testkit::no_alloc::assert_no_alloc;

const SR: u64 = 48_000;
/// Seconds of audio rendered.
const SECONDS: u64 = 20;

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

/// One effect of the table.
struct Case {
    name: &'static str,
    type_id: &'static str,
    /// Parameters set before the render: the sound must make the tail audible, the modulation
    /// swept (a skip that misplaces a free-running part is heard after the silence).
    params: &'static [(&'static str, f32)],
    /// The parameter changed in the silence, and its new value.
    change: (&'static str, f32),
    /// Batches the effect must have been skipped for in each scenario (see `scenarios`).
    min_skipped: [u64; 3],
}

const CASES: &[Case] = &[
    Case {
        name: "echo",
        type_id: "atomik.echo",
        params: &[
            ("time.mode", 1.0),
            ("time.sync", 0.0), // 1/32: a tempo change moves the time
            ("feedback", 0.3),
            ("mix", 0.5),
            ("color.drive", 0.5),
            ("color.high", 6_000.0),
            ("diffusion", 0.5),
            ("wobble.depth", 0.8),
            ("wobble.rate", 3.0),
            ("duck", 0.5),
        ],
        change: ("feedback", 0.2),
        // The ring (5.5 s) and the repeats run dry about 7 s in; the noise comes back at 14 s.
        min_skipped: [3_000, 1_500, 2_000],
    },
    Case {
        name: "reverb",
        type_id: "atomik.reverb",
        params: &[
            ("algorithm", 1.0),
            ("decay", 0.3),
            ("damping", 0.4),
            ("modulation", 1.0),
            ("mix", 0.5),
            ("predelay", 20.0),
            ("early", 0.5),
        ],
        change: ("decay", 0.4),
        // The pre-delay ring (a third of a second) and the tail (400 dB down in about 2 s) run dry
        // about 3.5 s in; the noise comes back at 14 s.
        min_skipped: [8_000, 7_500, 7_500],
    },
    Case {
        name: "space-reverb",
        type_id: "atomik.space-reverb",
        params: &[
            ("algorithm", 1.0),
            ("decay", 0.3),
            ("damping", 0.4),
            ("modulation", 1.0),
            ("mix", 0.5),
            ("predelay", 20.0),
            ("highcut", 8_000.0),
            ("gate", 150.0),
            ("gate_level", -30.0),
        ],
        change: ("highcut", 5_000.0),
        // The early-reflection ring (1.4 s) and the tail (400 dB down in about 2 s) run dry about
        // 4.5 s in; the noise comes back at 14 s.
        min_skipped: [7_000, 6_000, 6_000],
    },
    Case {
        name: "chorus",
        type_id: "atomik.modulation",
        params: &[
            ("mode", 0e+00),
            ("rate", 3.7),
            ("depth", 1.0),
            ("feedback", 0.5),
            ("delay", 12.0),
            ("spread", 90.0),
            ("mix", 0.6),
            ("stages", 8.0),
        ],
        change: ("feedback", 0.3),
        // Dry about 50 ms after the noise; it is skipped until the noise returns at 14 s.
        min_skipped: [10_000, 9_000, 9_000],
    },
    Case {
        name: "flanger",
        type_id: "atomik.modulation",
        params: &[
            ("mode", 1e+00),
            ("rate", 3.7),
            ("depth", 1.0),
            ("feedback", 0.5),
            ("delay", 12.0),
            ("spread", 90.0),
            ("mix", 0.6),
            ("stages", 8.0),
        ],
        change: ("feedback", 0.3),
        // Dry about 50 ms after the noise; it is skipped until the noise returns at 14 s.
        min_skipped: [10_000, 9_000, 9_000],
    },
    Case {
        name: "phaser",
        type_id: "atomik.modulation",
        params: &[
            ("mode", 2e+00),
            ("rate", 3.7),
            ("depth", 1.0),
            ("feedback", 0.5),
            ("delay", 12.0),
            ("spread", 90.0),
            ("mix", 0.6),
            ("stages", 8.0),
        ],
        change: ("feedback", 0.3),
        // Dry about 50 ms after the noise; it is skipped until the noise returns at 14 s.
        min_skipped: [10_000, 9_000, 9_000],
    },
    // Locked to the transport: a rendered song plays, so the sweep takes its phase from the frame and
    // the effect is processed throughout (and renders the same bytes trivially).
    Case {
        name: "chorus-synced",
        type_id: "atomik.modulation",
        params: &[
            ("mode", 0.0),
            ("sync", 3.0),
            ("depth", 1.0),
            ("feedback", 0.5),
            ("mix", 0.6),
        ],
        change: ("feedback", 0.3),
        min_skipped: [0, 0, 0],
    },
];

struct Rendered {
    bits: Vec<Vec<u32>>,
    skipped: u64,
}

/// Renders the project (`noise -> effect -> out`) with the skip on or off. `edits` get the
/// effect's id and descriptor and return the script of the render.
fn render(
    case: &Case,
    skip: bool,
    edits: impl FnOnce(DeviceInstanceId, &DeviceDescriptor) -> Vec<TimedEdit>,
) -> Rendered {
    let c = EngineConfig {
        sample_rate: SampleRate::new(SR as f64).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 7,
    };
    let registry = default_registry();
    let mut g = Graph::new(&c);
    let src = g.add_device(Box::new(TimedNoise {
        windows: [(0, SR), (14 * SR, 15 * SR)],
        state: 0x1234_5678,
    }));
    let fx = g.add_device(registry.create(case.type_id).unwrap());
    let socket = |g: &Graph, id: DeviceInstanceId, key: &str| atomik_engine::Endpoint {
        device: id,
        socket: g.descriptor(id).unwrap().socket_by_key(key).unwrap().id,
    };
    let from = socket(&g, src, "audio_out");
    for key in ["in_l", "in_r"] {
        let to = socket(&g, fx, key);
        g.connect(from, to).unwrap();
    }
    for (key, out) in [("out_l", "out_1"), ("out_r", "out_2")] {
        let (from, to) = (socket(&g, fx, key), socket(&g, Graph::INTERFACE, out));
        g.connect(from, to).unwrap();
    }
    let descriptor = g.descriptor(fx).unwrap();
    let params: Vec<_> = case
        .params
        .iter()
        .map(|&(key, v)| (fx, descriptor.param_by_key(key).unwrap().id, v))
        .collect();
    let script = edits(fx, descriptor);
    let (mut engine, mut handle) = new_engine(c);
    engine.set_idle_skip(skip);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let job = OfflineJob {
        frames: (SECONDS * SR) as usize,
        host_block: 256,
        notes: &[],
        params: &params,
        inputs: &[],
    };
    let out = render_offline_scripted(&mut engine, &mut handle, &job, script, |f| {
        assert_no_alloc(f)
    });
    Rendered {
        bits: out
            .iter()
            .map(|c| c.iter().map(|s| s.to_bits()).collect())
            .collect(),
        skipped: engine.idle_skipped(fx).unwrap(),
    }
}

fn at(seconds: u64, edit: HotEdit) -> TimedEdit {
    TimedEdit {
        frame: seconds * SR,
        edit,
    }
}

/// Both renders of a script, and the equality of their bytes; returns the batches skipped.
fn compare(
    case: &Case,
    script: impl Fn(DeviceInstanceId, &DeviceDescriptor) -> Vec<TimedEdit>,
) -> u64 {
    let off = render(case, false, &script);
    let on = render(case, true, &script);
    assert_eq!(off.skipped, 0, "{}: the switch is off", case.name);
    assert!(
        on.bits.iter().flatten().any(|&b| b != 0),
        "{}: the project does sound",
        case.name
    );
    assert!(
        on.bits == off.bits,
        "{}: the skip changed the render",
        case.name
    );
    on.skipped
}

/// The three scripts: a parameter and a tempo change in the silence, a bypass and its release in
/// it, and a bypass with tempo changes around it.
fn scenarios(case: &Case) {
    let skipped = compare(case, |fx, d| {
        let p = d.param_by_key(case.change.0).unwrap().id;
        vec![
            at(
                11,
                HotEdit::Param {
                    device: fx,
                    param: p,
                    plain: case.change.1,
                },
            ),
            at(12, HotEdit::Tempo(Tempo::new(140.0).unwrap())),
        ]
    });
    assert!(
        skipped >= case.min_skipped[0],
        "{}: {skipped} batches skipped",
        case.name
    );
    let bypass = |fx, d: &DeviceDescriptor, plain| HotEdit::Param {
        device: fx,
        param: d.param_by_key("bypass").unwrap().id,
        plain,
    };
    let skipped = compare(case, |fx, d| {
        vec![at(10, bypass(fx, d, 1.0)), at(13, bypass(fx, d, 0.0))]
    });
    assert!(
        skipped >= case.min_skipped[1],
        "{}: {skipped} batches skipped (bypass)",
        case.name
    );
    let skipped = compare(case, |fx, d| {
        vec![
            at(9, HotEdit::Tempo(Tempo::new(90.0).unwrap())),
            at(10, bypass(fx, d, 1.0)),
            at(12, HotEdit::Tempo(Tempo::new(150.0).unwrap())),
            at(13, bypass(fx, d, 0.0)),
        ]
    });
    assert!(
        skipped >= case.min_skipped[2],
        "{}: {skipped} batches skipped (bypass and tempo)",
        case.name
    );
}

fn case(name: &str) -> &'static Case {
    CASES.iter().find(|c| c.name == name).unwrap()
}

#[test]
fn the_echo_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("echo"));
}

#[test]
fn the_reverb_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("reverb"));
}

#[test]
fn the_space_reverb_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("space-reverb"));
}

#[test]
fn the_modulation_effect_renders_the_same_bytes_skipped_or_not() {
    for name in ["chorus", "flanger", "phaser", "chorus-synced"] {
        scenarios(case(name));
    }
}
