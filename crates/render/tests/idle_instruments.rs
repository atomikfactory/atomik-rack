//! An instrument with no sounding voice is not called, and the render is the render it would have
//! been: a chord for a second, silence long enough for the instrument to be skipped (the engine's
//! counter says for how many batches), then a parameter change, a tempo change and controller
//! messages in the silence, then a chord again, rendered with the skip on and off through the
//! offline path. One table row per instrument; the effects have `idle_effects.rs`.
atomik_testkit::install_alloc_guard!();

use atomik_core::{ExprKind, NoteKind, SampleRate, Tempo};
use atomik_device_sdk::{
    DeviceDescriptor, LoopMode, SampleBuffer, SampleMap, SampleMapRef, TriggerMode, Zone, ZoneExt,
};
use atomik_engine::{
    DeviceInstanceId, EngineConfig, Graph, HotEdit, OfflineJob, ScheduledNote, TimedEdit,
    new_engine, render_offline_scripted,
};
use atomik_render::default_registry;
use atomik_testkit::no_alloc::assert_no_alloc;

const SR: u64 = 48_000;
/// Seconds of audio rendered.
const SECONDS: u64 = 20;

/// One instrument of the table.
struct Case {
    name: &'static str,
    type_id: &'static str,
    /// The audio outputs, cabled to the two interface channels.
    outs: [&'static str; 2],
    /// Parameters set before the render: the sound must make the release audible, the modulation
    /// swept (a skip that misplaces a free-running part is heard after the silence).
    params: &'static [(&'static str, f32)],
    /// The chord played at the start (0.2 s to 1.2 s) and again at 14 s (to 15 s).
    keys: &'static [u8],
    /// The parameter changed in the silence, and its new value.
    change: (&'static str, f32),
    /// Whether the instrument plays a sample map (given at the start of the render).
    samples: bool,
    /// Batches the instrument must have been skipped for in each scenario (see `scenarios`).
    min_skipped: [u64; 2],
}

const CASES: &[Case] = &[
    Case {
        name: "organ",
        type_id: "atomik.organ",
        outs: ["out_l", "out_r"],
        params: &[
            ("scanner.type", 6.0),
            ("scanner.rate", 7.3),
            ("out.drive", 0.4),
            ("key.click", 0.5),
            ("perc.mode", 1.0),
            ("perc.decay", 1.0),
            ("wheel.target", 1.0),
            ("drawbar.4", 5.0),
        ],
        keys: &[48, 55, 64],
        change: ("scanner.rate", 8.1),
        samples: false,
        // The release and the click are spent a few hundredths of a second after the chord; it
        // is skipped until the chord returns at 14 s (13,400 of the 15,000 batches).
        min_skipped: [10_000, 10_000],
    },
    Case {
        name: "sampler",
        type_id: "atomik.sampler",
        outs: ["out_l", "out_r"],
        params: &[
            ("lfo.wave", 4.0),
            ("lfo.rate", 7.3),
            ("lfo.to_pitch", 4.0),
            ("lfo.to_amp", 0.8),
            ("filter.enabled", 1.0),
            ("filter.cutoff", 3_000.0),
            ("lfo.to_cutoff", 0.5),
        ],
        keys: &[48, 55, 64],
        change: ("master.level", -3.0),
        samples: true,
        // The voices release over 150 ms after each chord; skipped otherwise (13,270 of 15,000).
        min_skipped: [10_000, 10_000],
    },
    Case {
        name: "granular",
        type_id: "atomik.granular",
        outs: ["out_l", "out_r"],
        params: &[
            ("lfo.wave", 4.0),
            ("lfo.rate", 7.3),
            ("lfo.to_position", 0.3),
            ("lfo.to_size", 0.5),
            ("lfo.to_pitch", 3.0),
            ("lfo.to_density", 0.4),
            ("pos.spray", 0.3),
            ("pitch.random", 20.0),
            ("grain.reverse", 0.3),
            ("filter.enabled", 1.0),
            ("filter.cutoff", 3_000.0),
            ("lfo.to_cutoff", 0.5),
        ],
        keys: &[48, 55, 64],
        change: ("master.level", -3.0),
        samples: false,
        // The built-in source plays; the voices release over 400 ms after each chord (12,890 of 15,000).
        min_skipped: [10_000, 10_000],
    },
];

/// One mono zone over every key: 8,192 frames of a wave built from two parabolas (no libm call),
/// looped over its middle while the key is held.
fn sample_map() -> SampleMapRef {
    let wave: Vec<f32> = (0..8_192)
        .map(|i| {
            let x = (i % 100) as f32 / 100.0;
            if x < 0.5 {
                16.0 * x * (0.5 - x)
            } else {
                -16.0 * (x - 0.5) * (1.0 - x)
            }
        })
        .collect();
    let zone = Zone {
        buffer: 0,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 60,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start: 0,
        end: 8_192,
        loop_mode: LoopMode::Forward,
        loop_start: 1_000,
        loop_end: 7_000,
        crossfade: 200,
        trigger: TriggerMode::Gate,
        reverse: false,
        ext: ZoneExt::NONE,
    };
    let buffer = std::sync::Arc::new(SampleBuffer::new(vec![wave].into()).unwrap());
    std::sync::Arc::new(SampleMap::new(vec![buffer], vec![zone]).unwrap())
}

struct Rendered {
    bits: Vec<Vec<u32>>,
    skipped: u64,
}

fn note(
    seconds: f64,
    device: DeviceInstanceId,
    socket: atomik_device_sdk::SocketId,
    kind: NoteKind,
) -> ScheduledNote {
    ScheduledNote {
        frame: (seconds * SR as f64) as u64,
        device,
        socket,
        kind,
    }
}

/// Renders the project (`instrument -> out`) with the skip on or off. `edits` get the
/// instrument's id and descriptor and return the script of the render, `controllers` the
/// controller messages sent in the silence.
fn render(
    case: &Case,
    skip: bool,
    edits: impl FnOnce(DeviceInstanceId, &DeviceDescriptor) -> Vec<TimedEdit>,
    controllers: &[(f64, NoteKind)],
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
    let fx = g.add_device(registry.create(case.type_id).unwrap());
    let socket = |g: &Graph, id: DeviceInstanceId, key: &str| atomik_engine::Endpoint {
        device: id,
        socket: g.descriptor(id).unwrap().socket_by_key(key).unwrap().id,
    };
    for (key, out) in case.outs.iter().zip(["out_1", "out_2"]) {
        let (from, to) = (socket(&g, fx, key), socket(&g, Graph::INTERFACE, out));
        g.connect(from, to).unwrap();
    }
    let notes_in = socket(&g, fx, "notes_in").socket;
    let descriptor = g.descriptor(fx).unwrap();
    let params: Vec<_> = case
        .params
        .iter()
        .map(|&(key, v)| (fx, descriptor.param_by_key(key).unwrap().id, v))
        .collect();
    let mut script = edits(fx, descriptor);
    if case.samples {
        script.push(at(
            0,
            HotEdit::Samples {
                device: fx,
                map: Some(sample_map()),
            },
        ));
    }
    let mut notes = Vec::new();
    for (on, off) in [(0.2, 1.2), (14.0, 15.0)] {
        for (i, &key) in case.keys.iter().enumerate() {
            let velocity = 0.6 + 0.1 * i as f32;
            notes.push(note(on, fx, notes_in, NoteKind::On { key, velocity }));
            notes.push(note(off, fx, notes_in, NoteKind::Off { key }));
        }
    }
    for &(seconds, kind) in controllers {
        notes.push(note(seconds, fx, notes_in, kind));
    }
    let (mut engine, mut handle) = new_engine(c);
    engine.set_idle_skip(skip);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let job = OfflineJob {
        frames: (SECONDS * SR) as usize,
        host_block: 256,
        notes: &notes,
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
    controllers: &[(f64, NoteKind)],
) -> u64 {
    let off = render(case, false, &script, controllers);
    let on = render(case, true, &script, controllers);
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

fn cc(controller: u8, value: f32) -> NoteKind {
    NoteKind::Cc { controller, value }
}

/// The two scripts: a parameter change and a tempo change in the silence; then tempo changes and
/// every kind of controller message in it (mod wheel, volume, expression, sustain, bend,
/// pressure and a note expression), which the instrument holds for the chord that follows.
fn scenarios(case: &Case) {
    let skipped = compare(
        case,
        |fx, d| {
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
        },
        &[],
    );
    assert!(
        skipped >= case.min_skipped[0],
        "{}: {skipped} batches skipped",
        case.name
    );
    let controllers = [
        (5.0, cc(1, 0.8)),
        (6.0, cc(7, 0.6)),
        (7.0, cc(11, 0.9)),
        (8.0, cc(64, 1.0)),
        (9.0, cc(64, 0.0)),
        (10.0, NoteKind::PitchBend { value: 0.5 }),
        (11.0, NoteKind::ChannelPressure { value: 0.4 }),
        (
            11.5,
            NoteKind::NoteExpr {
                key: case.keys[0],
                kind: ExprKind::Tuning,
                value: 0.5,
            },
        ),
        (12.5, cc(1, 0.0)),
    ];
    let skipped = compare(
        case,
        |_, _| {
            vec![
                at(9, HotEdit::Tempo(Tempo::new(90.0).unwrap())),
                at(12, HotEdit::Tempo(Tempo::new(150.0).unwrap())),
            ]
        },
        &controllers,
    );
    assert!(
        skipped >= case.min_skipped[1],
        "{}: {skipped} batches skipped (controllers)",
        case.name
    );
}

fn case(name: &str) -> &'static Case {
    CASES.iter().find(|c| c.name == name).unwrap()
}

#[test]
fn the_organ_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("organ"));
}

#[test]
fn the_sampler_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("sampler"));
}

#[test]
fn the_granular_synth_renders_the_same_bytes_skipped_or_not() {
    scenarios(case("granular"));
}
