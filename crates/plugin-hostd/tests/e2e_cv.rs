//! The CV programmer of a plugin device in the real engine: a CV cable moves a test plugin's
//! gain param from batch to batch, and a sandboxed plugin renders bit for bit like the same
//! plugin in process, across host blocks and worker counts (the pattern of `e2e_chain.rs`).
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_core::{BATCH, ParamId, SampleRate, Tempo};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, RunMode,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineOptions, Graph, OfflineJob, WorkerConfig,
    new_engine_with_options, render_offline_scripted,
};
use atomik_plugin_host::PluginManager;
use atomik_testkit::fakes::ConstantSource;
use atomik_testkit::no_alloc::assert_no_alloc;
use sandbox_support::{FORMATS, Format, Plug, SR, Sandbox, in_process, request};

/// Batches of the triangle the CV source repeats.
const STEPS: u64 = 8;

/// The CV of batch `n`: -1.0, -0.75, ... 0.75, then again.
fn cv_of(n: u64) -> f32 {
    (n % STEPS) as f32 / 4.0 - 1.0
}

static STEP_CV: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "e2e.step-cv",
    name: "Step CV",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[SocketDescriptor {
        id: SocketId(0),
        key: "cv_out",
        name: "CV out",
        kind: SocketKind::Cv,
        direction: Direction::Output,
    }],
};

/// A CV source that holds one level per 64-frame batch, stepping through [`cv_of`].
struct StepCv {
    batch: u64,
}

impl Device for StepCv {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &STEP_CV
    }

    fn prepare(&mut self, _: &PrepareInfo) {}

    fn reset(&mut self) {
        self.batch = 0;
    }

    fn set_param(&mut self, _: ParamId, _: f32) {}

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        *ctx.output(SocketId(0)) = [cv_of(self.batch); BATCH];
        self.batch += 1;
    }
}

fn cfg() -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(SR).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    }
}

fn ep(device: DeviceInstanceId, socket: SocketId) -> Endpoint {
    Endpoint { device, socket }
}

fn socket(d: &'static DeviceDescriptor, key: &str) -> SocketId {
    d.socket_by_key(key).unwrap().id
}

fn param(d: &'static DeviceDescriptor, key: &str) -> ParamId {
    d.param_by_key(key).unwrap().id
}

const BATCHES: usize = 96;
const AMOUNT: f32 = 0.5;
const INPUT: f32 = 0.5;

/// A constant 0.5 into the test gain effect, whose gain param is driven by the step CV.
fn render(
    manager: &mut PluginManager,
    format: Format,
    mode: RunMode,
    block: usize,
    workers: u16,
) -> Vec<Vec<f32>> {
    let c = cfg();
    let mut g = Graph::new(&c);
    let h = manager
        .instantiate(&request(&format.plugin(Plug::Gain), mode))
        .unwrap();
    let d = h.descriptor;
    let fx = g.add_device(Box::new(h.device));
    let src = g.add_device(ConstantSource::boxed(INPUT));
    let cv = g.add_device(Box::new(StepCv { batch: 0 }));
    // Gain is the first plugin param (descriptor index 1).
    g.set_param(fx, param(d, "cv.dest_1"), 1.0).unwrap();
    g.set_param(fx, param(d, "cv.amount_1"), AMOUNT).unwrap();
    g.connect(
        ep(src, ConstantSource::AUDIO_OUT),
        ep(fx, socket(d, "in_l")),
    )
    .unwrap();
    g.connect(
        ep(src, ConstantSource::AUDIO_OUT),
        ep(fx, socket(d, "in_r")),
    )
    .unwrap();
    g.connect(ep(cv, SocketId(0)), ep(fx, socket(d, "cv_in_1")))
        .unwrap();
    let iface = g.descriptor(Graph::INTERFACE).unwrap();
    for (from, to) in [("out_l", "out_1"), ("out_r", "out_2")] {
        g.connect(
            ep(fx, socket(d, from)),
            ep(Graph::INTERFACE, socket(iface, to)),
        )
        .unwrap();
    }
    let (mut e, mut handle) = new_engine_with_options(
        c,
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: None,
            },
            ..EngineOptions::default()
        },
    );
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let job = OfflineJob {
        frames: BATCHES * BATCH,
        host_block: block,
        notes: &[],
        params: &[],
        inputs: &[],
    };
    let rendered = render_offline_scripted(&mut e, &mut handle, &job, Vec::new(), |f| {
        assert_no_alloc(f)
    });
    drop((e, handle));
    rendered
}

fn bits(c: &[Vec<f32>]) -> Vec<Vec<u32>> {
    c.iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
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
fn a_cv_cable_moves_a_clap_plugin_param_batch_by_batch() {
    let format = Format::Clap;
    let mut local = in_process(format, &[format.main_file()]);
    let out = render(&mut local, format, RunMode::Offline, 64, 0);
    reap(&mut local);
    for n in 0..BATCHES {
        // The gain range is -60..12 dB and the rack value is 0 dB.
        let db = (f64::from(AMOUNT) * f64::from(cv_of(n as u64)) * 72.0).clamp(-60.0, 12.0);
        let want = f64::from(INPUT) * 10f64.powf(db / 20.0);
        let got = f64::from(out[0][n * BATCH + BATCH / 2]);
        assert!(
            (got - want).abs() <= 1e-5 * want.max(1.0),
            "batch {n}: {got} against {want} ({db} dB)"
        );
        assert!(
            out[0][n * BATCH..(n + 1) * BATCH]
                .iter()
                .all(|s| s.to_bits() == out[0][n * BATCH].to_bits()),
            "batch {n} holds one level"
        );
    }
}

#[test]
fn sandboxed_cv_is_bit_identical_to_in_process_across_blocks_and_workers() {
    for format in FORMATS {
        let mut local = in_process(format, &[format.main_file()]);
        let reference = render(&mut local, format, RunMode::Offline, 64, 0);
        reap(&mut local);
        let distinct: std::collections::BTreeSet<u32> =
            reference[0].iter().map(|s| s.to_bits()).collect();
        assert!(
            distinct.len() >= 4,
            "{}: the CV moves the output ({} levels)",
            format.name(),
            distinct.len()
        );
        let mut sb = Sandbox::real(format);
        let mut check = |block: usize, workers: u16, what: &str| {
            let got = render(&mut sb.manager, format, RunMode::Offline, block, workers);
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
        for workers in [1u16, 2] {
            check(512, workers, &format!("{workers} workers"));
        }
        assert!(sb.wait_children(0, std::time::Duration::from_secs(5)));
        sb.finish();
    }
}
