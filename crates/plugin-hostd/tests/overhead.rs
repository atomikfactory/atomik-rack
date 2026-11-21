//! What the sandbox costs per audio batch, measured by hand (not part of CI): the same plugin
//! run in this process and through the child, at 48 kHz, with the app's 480-frame device
//! buffer (7.5 engine batches of 64 frames, one round trip each).
//!
//! ```text
//! cargo test -p atomik-plugin-hostd --test overhead -- --ignored --nocapture
//! ATOMIK_OVERHEAD_VST3=<.vst3 bundle> ATOMIK_OVERHEAD_ID=<32 hex> \
//!   cargo test -p atomik-plugin-hostd --test overhead -- --ignored --nocapture
//! ```
//!
//! Printed per plugin: the mean, median and 99th percentile of one batch in process and through
//! the child (the difference is the sandbox's round trip), and the CPU time each process used
//! during a paced run of 10 s of audio (one 480-frame buffer every 10 ms).
//!
//! A second probe measures the wait on a slow plugin (see
//! `the_wait_on_a_slow_plugin_with_and_without_the_callbacks_budget`).
#![allow(clippy::disallowed_methods)] // Justification: a manual benchmark; timing is its output.

atomik_testkit::install_alloc_guard!();

mod sandbox_support;
mod support;

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{
    ActivateConfig, AudioThreadGuard, EventList, HostHandle, InstanceId, PluginBackend,
    PluginEvent, PluginIo, PluginTransport, mark_main_thread, plugin_channel,
};
use atomik_plugin_ipc::AudioOp;
use atomik_plugin_vst3::Vst3Backend;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::{SR, Spawn, TestParent, note_on};

/// Batches measured back to back.
const BATCHES: usize = 20_000;
/// The device buffer of the user's setup.
const BUFFER: usize = 480;
/// 10 s of audio in device buffers.
const PACED_BUFFERS: usize = 1_000;

fn events_at(i: usize) -> Vec<PluginEvent> {
    // A held chord, restruck now and then, so an instrument has work to do.
    if i.is_multiple_of(2_000) {
        vec![
            note_on(0, 60, 0.8, 1),
            note_on(0, 64, 0.8, 2),
            note_on(0, 67, 0.8, 3),
        ]
    } else {
        Vec::new()
    }
}

struct Stats {
    mean: f64,
    p50: f64,
    p99: f64,
}

fn stats(mut v: Vec<f64>) -> Stats {
    v.sort_by(f64::total_cmp);
    let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    Stats {
        mean: v.iter().sum::<f64>() / v.len() as f64,
        p50: at(0.5),
        p99: at(0.99),
    }
}

/// CPU seconds a process has used, from the OS (`Get-Process`), to keep this test free of
/// `unsafe`.
fn cpu_seconds(pid: u32) -> f64 {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid}).TotalProcessorTime.TotalSeconds"),
        ])
        .output()
        .expect("powershell runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace(',', ".")
        .parse()
        .unwrap_or(f64::NAN)
}

fn in_process(file: &Path, id: &str) -> Stats {
    mark_main_thread();
    let backend = Vst3Backend::new();
    backend.register_plugin_file(id, file);
    let (_device_end, manager) = plugin_channel(InstanceId(1));
    let mut instance = backend
        .instantiate(
            &PluginRef {
                format: PluginFormat::Vst3,
                id: id.to_string(),
            },
            HostHandle::new(manager.messenger()),
        )
        .expect("the plugin is created");
    let mut processor = instance
        .activate(&ActivateConfig {
            sample_rate: SR,
            frames: BATCH as u32,
            mode: RunMode::Realtime,
        })
        .expect("the plugin activates");
    let guard = AudioThreadGuard::enter();
    assert!(processor.start_processing());
    let main_in: Vec<Batch> = vec![[0.0; BATCH]; 2];
    let mut times = Vec::with_capacity(BATCHES);
    let mut events_out = EventList::with_capacity(512);
    for i in 0..BATCHES {
        let events = events_at(i);
        let mut main_out = [[0.0; BATCH]; 2];
        events_out.clear();
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &main_in,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            aux_out: &mut [],
            events_in: &events,
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        let t = Instant::now();
        processor.process(&mut io);
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    processor.stop_processing();
    drop(guard);
    instance.deactivate(processor);
    stats(times)
}

struct Sandboxed {
    batch: Stats,
    parent_cpu: f64,
    child_cpu: f64,
    late_buffers: usize,
}

fn sandboxed(file: &Path, id: &str) -> Sandboxed {
    let mut p = TestParent::spawn(&Spawn::real("vst3", file, id));
    p.hello();
    p.instantiate();
    p.activate(false);
    assert!(p.op(AudioOp::Start).ok);
    let main_in: Vec<Batch> = vec![[0.0; BATCH]; 2];
    let mut times = Vec::with_capacity(BATCHES);
    for i in 0..BATCHES {
        let events = events_at(i);
        let t = Instant::now();
        p.process(&main_in, &events);
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    // Paced like a device callback: one 480-frame buffer (7.5 batches) every 10 ms.
    let period = Duration::from_secs_f64(BUFFER as f64 / SR);
    let (me, child) = (std::process::id(), p.pid());
    let (cpu_me, cpu_child) = (cpu_seconds(me), cpu_seconds(child));
    let start = Instant::now();
    let mut done = 0usize;
    let mut late_buffers = 0;
    for n in 0..PACED_BUFFERS {
        let due = start + period * n as u32;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let t = Instant::now();
        let want = (n + 1) * BUFFER / BATCH;
        while done < want {
            p.process(&main_in, &events_at(done));
            done += 1;
        }
        if t.elapsed() > period {
            late_buffers += 1;
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let (cpu_me2, cpu_child2) = (cpu_seconds(me), cpu_seconds(child));
    p.op(AudioOp::Stop);
    p.deactivate();
    Sandboxed {
        batch: stats(times),
        parent_cpu: (cpu_me2 - cpu_me) / wall * 100.0,
        child_cpu: (cpu_child2 - cpu_child) / wall * 100.0,
        late_buffers,
    }
}

fn report(name: &str, file: &Path, id: &str) {
    let local = in_process(file, id);
    let remote = sandboxed(file, id);
    let per_buffer = BUFFER as f64 / BATCH as f64;
    println!("== {name}");
    println!(
        "  one 64-frame batch in process : mean {:7.1} us  p50 {:7.1} us  p99 {:7.1} us",
        local.mean, local.p50, local.p99
    );
    println!(
        "  one 64-frame batch, sandboxed : mean {:7.1} us  p50 {:7.1} us  p99 {:7.1} us",
        remote.batch.mean, remote.batch.p50, remote.batch.p99
    );
    println!(
        "  sandbox round trip            : {:7.1} us per batch, {:7.1} us per 480-frame buffer (of 10 000 us)",
        remote.batch.p50 - local.p50,
        (remote.batch.p50 - local.p50) * per_buffer
    );
    println!(
        "  paced 10 s at 48 kHz / 480    : parent {:5.1} % CPU, child {:5.1} % CPU (of one core), {} late buffers of {PACED_BUFFERS}",
        remote.parent_cpu, remote.child_cpu, remote.late_buffers
    );
}

#[test]
#[ignore = "a benchmark, run by hand"]
fn the_sandbox_overhead_per_batch_and_its_cpu_at_48_khz_with_480_frame_buffers() {
    let file = vst3_test_plugin("atomik-test-vst3").expect("the test plugin builds");
    let synth = "a70f1c014a5b4c018d2e1f0000000002";
    report("test VST3 synth", &file, synth);
    if let (Some(path), Ok(id)) = (
        std::env::var_os("ATOMIK_OVERHEAD_VST3").map(PathBuf::from),
        std::env::var("ATOMIK_OVERHEAD_ID"),
    ) {
        report("installed VST3", &path, &id);
    }
}

// ----- the wait on a plugin that works in bursts -----------------------------------------------

/// What one scenario of the wait probe does: the work of the busy test plugin.
struct Work {
    name: &'static str,
    /// Microseconds of work per batch (every eighth batch when `burst`).
    busy_us: u64,
    burst: bool,
    /// Plugins in series, each doing this work.
    plugins: usize,
}

const WORKS: [Work; 7] = [
    Work {
        name: "steady 0.3 ms per batch",
        busy_us: 300,
        burst: false,
        plugins: 1,
    },
    Work {
        name: "burst: 3 ms every 8th batch",
        busy_us: 3_000,
        burst: true,
        plugins: 1,
    },
    Work {
        name: "burst: 6 ms every 8th batch",
        busy_us: 6_000,
        burst: true,
        plugins: 1,
    },
    Work {
        name: "steady 1.8 ms per batch",
        busy_us: 1_800,
        burst: false,
        plugins: 1,
    },
    Work {
        name: "steady 0.9 ms (at the budget)",
        busy_us: 900,
        burst: false,
        plugins: 1,
    },
    Work {
        name: "2 plugins, burst 4 ms each",
        busy_us: 4_000,
        burst: true,
        plugins: 2,
    },
    Work {
        name: "steady 20 ms per batch",
        busy_us: 20_000,
        burst: false,
        plugins: 1,
    },
];

/// Callbacks of one run (480 frames, one every 10 ms): 4 s of audio.
const WAIT_CALLBACKS: usize = 800;

/// The share of a callback the audio side publishes for its plugins (as the stream does).
const PUBLISHED: Duration = Duration::from_millis(7);

/// CPU seconds the processes started from `exe` have used so far.
fn cpu_of(exe: &str) -> f64 {
    let script = format!(
        "(Get-Process | Where-Object {{ $_.Path -eq '{exe}' }} |          ForEach-Object {{ $_.TotalProcessorTime.TotalSeconds }} | Measure-Object -Sum).Sum"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .expect("powershell runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .replace(',', ".")
        .parse()
        .unwrap_or(f64::NAN)
}

struct Wait {
    late: usize,
    batches: usize,
    /// Time spent inside the plugin calls, per callback, in microseconds.
    mean_us: f64,
    p99_us: f64,
    child_cpu: f64,
}

fn wait_run(work: &Work, publish: bool) -> Wait {
    use sandbox_support::{Sandbox, effect, host_command, null_plugin, request};
    let mut config =
        sandbox_support::config_with_env("ATOMIK_NULL_BUSY_US", &work.busy_us.to_string());
    if work.burst {
        config
            .command
            .envs
            .push(("ATOMIK_NULL_BUSY_BURST".into(), "1".into()));
    }
    let mut sb = Sandbox::null(config);
    let mut hs: Vec<_> = (0..work.plugins)
        .map(|_| {
            let handle = sb
                .manager
                .instantiate(&request(&null_plugin("null.busy"), RunMode::Realtime))
                .expect("the busy plugin starts");
            effect(handle, 0.25)
        })
        .collect();
    let lates = |hs: &[atomik_testkit::harness::DeviceHarness]| -> f32 {
        hs.iter()
            .map(|h| sandbox_support::meters(h)[sandbox_support::METER_LATE])
            .sum()
    };
    // Settle: the first batches pay for the child's start-up.
    for _ in 0..200 {
        hs.iter_mut().for_each(|h| h.process());
        std::thread::sleep(Duration::from_millis(1));
    }
    let exe = host_command().program.to_string_lossy().replace('\'', "''");
    let late_before = lates(&hs);
    let cpu_before = cpu_of(&exe);
    let period = Duration::from_secs_f64(BUFFER as f64 / SR);
    let start = Instant::now();
    let mut per_callback = Vec::with_capacity(WAIT_CALLBACKS);
    let mut done = 0usize;
    for n in 0..WAIT_CALLBACKS {
        let due = start + period * n as u32;
        // Sleep most of the way, spin the rest: a device callback is on time.
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            if wait > Duration::from_millis(2) {
                std::thread::sleep(wait - Duration::from_millis(2));
            }
            while Instant::now() < due {
                std::hint::spin_loop();
            }
        }
        if publish {
            atomik_core::CallbackDeadline::shared().publish(Instant::now() + PUBLISHED);
        }
        let want = (n + 1) * BUFFER / BATCH;
        let t = Instant::now();
        while done < want {
            hs.iter_mut().for_each(|h| h.process());
            done += 1;
        }
        per_callback.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let wall = start.elapsed().as_secs_f64();
    let cpu_after = cpu_of(&exe);
    let late = lates(&hs) - late_before;
    atomik_core::CallbackDeadline::shared().clear();
    drop(hs);
    sb.manager.poll();
    sb.finish();
    let s = stats(per_callback);
    Wait {
        late: late as usize,
        batches: done,
        mean_us: s.mean,
        p99_us: s.p99,
        child_cpu: (cpu_after - cpu_before) / wall * 100.0,
    }
}

/// Late batches, the time the app spends inside the plugin calls and the child's CPU, for a
/// plugin that works in bursts, with the wait of one batch period and with the wait up to the
/// share of the callback the audio side publishes. Run it in release for honest numbers:
///
/// ```text
/// cargo test --release -p atomik-plugin-hostd --test overhead -- --ignored --nocapture wait_on
/// ```
#[test]
#[ignore = "a benchmark, run by hand"]
fn the_wait_on_a_slow_plugin_with_and_without_the_callbacks_budget() {
    println!("480-frame callbacks at 48 kHz ({WAIT_CALLBACKS} of them), budget {PUBLISHED:?}");
    println!(
        "{:<30} {:<12} {:>12} {:>12} {:>13} {:>10}",
        "plugin work", "wait", "late/batches", "mean us/cb", "p99 us/cb", "child CPU"
    );
    for work in &WORKS {
        for (label, publish) in [("one batch", false), ("callback", true)] {
            let r = wait_run(work, publish);
            println!(
                "{:<30} {:<12} {:>5}/{:<6} {:>12.0} {:>12.0} {:>9.1} %",
                work.name, label, r.late, r.batches, r.mean_us, r.p99_us, r.child_cpu
            );
        }
    }
}
