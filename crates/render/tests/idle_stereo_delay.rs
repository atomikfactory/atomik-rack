//! A stereo delay that has run dry is not called, and the render is the render it would have
//! been: noise for a second, silence long enough for the delay to be skipped (the engine's
//! counter says for how many batches), then a parameter change and a tempo change in the
//! silence, then noise again, rendered with the skip on and off through the offline path. The
//! same with the delay bypassed and switched back on during the silence (the bypass wrapper
//! stops the effect after two seconds and is quiescent behind it).
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

struct Rendered {
    bits: Vec<Vec<u32>>,
    skipped: u64,
}

/// Renders the project (`noise -> stereo delay -> out`) with the skip on or off. `edits` get the
/// delay's id and return the script of the render.
fn render(
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
    let delay = g.add_device(registry.create("atomik.stereo-delay").unwrap());
    let socket = |g: &Graph, id: DeviceInstanceId, key: &str| atomik_engine::Endpoint {
        device: id,
        socket: g.descriptor(id).unwrap().socket_by_key(key).unwrap().id,
    };
    let from = socket(&g, src, "audio_out");
    for key in ["in_l", "in_r"] {
        let to = socket(&g, delay, key);
        g.connect(from, to).unwrap();
    }
    for (key, out) in [("out_l", "out_1"), ("out_r", "out_2")] {
        let (from, to) = (socket(&g, delay, key), socket(&g, Graph::INTERFACE, out));
        g.connect(from, to).unwrap();
    }
    let descriptor = g.descriptor(delay).unwrap();
    // Synced to 1/32 so a tempo change moves the delay time; the repeats die away in a few
    // seconds and the ring in four more.
    let param = |key: &str| descriptor.param_by_key(key).unwrap().id;
    let params = [
        (delay, param("time.mode"), 1.0),
        (delay, param("time.sync"), 0.0),
        (delay, param("feedback"), 0.3),
        (delay, param("mix"), 0.5),
    ];
    let script = edits(delay, descriptor);
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
        skipped: engine.idle_skipped(delay).unwrap(),
    }
}

fn at(seconds: u64, edit: HotEdit) -> TimedEdit {
    TimedEdit {
        frame: seconds * SR,
        edit,
    }
}

/// Both renders, the skipped batches of each, and the equality of their bytes.
fn compare(script: fn(DeviceInstanceId, &DeviceDescriptor) -> Vec<TimedEdit>) -> u64 {
    let off = render(false, script);
    let on = render(true, script);
    assert_eq!(off.skipped, 0, "the switch is off");
    assert!(
        on.bits.iter().flatten().any(|&b| b != 0),
        "and the project does sound"
    );
    assert!(on.bits == off.bits, "the skip changed the render");
    on.skipped
}

#[test]
fn a_parameter_and_a_tempo_change_in_the_silence_render_the_same_bytes() {
    let skipped = compare(|delay, d| {
        let feedback = d.param_by_key("feedback").unwrap().id;
        vec![
            at(
                11,
                HotEdit::Param {
                    device: delay,
                    param: feedback,
                    plain: 0.2,
                },
            ),
            at(12, HotEdit::Tempo(Tempo::new(140.0).unwrap())),
        ]
    });
    // Dry from about 9 s to the noise at 14 s: well over 3,000 batches of 750 a second.
    assert!(skipped > 3_000, "{skipped} batches skipped");
}

#[test]
fn a_bypass_and_its_release_in_the_silence_render_the_same_bytes() {
    let skipped = compare(|delay, d| {
        let bypass = d.param_by_key("bypass").unwrap().id;
        let edit = |plain| HotEdit::Param {
            device: delay,
            param: bypass,
            plain,
        };
        vec![at(10, edit(1.0)), at(13, edit(0.0))]
    });
    // Skipped while dry before the bypass (about 9 s to 10 s), behind the stopped effect (about
    // 12 s to 13 s) and after the release, until the noise at 14 s.
    assert!(skipped > 1_500, "{skipped} batches skipped");
}

#[test]
fn bypassing_and_changing_the_tempo_while_the_delay_is_dry_renders_the_same_bytes() {
    let skipped = compare(|delay, d| {
        let bypass = d.param_by_key("bypass").unwrap().id;
        let edit = |plain| HotEdit::Param {
            device: delay,
            param: bypass,
            plain,
        };
        vec![
            at(9, HotEdit::Tempo(Tempo::new(90.0).unwrap())),
            at(10, edit(1.0)),
            at(12, HotEdit::Tempo(Tempo::new(150.0).unwrap())),
            at(13, edit(0.0)),
        ]
    });
    assert!(skipped > 2_000, "{skipped} batches skipped");
}
