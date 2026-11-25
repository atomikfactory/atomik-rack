//! A headless soak: a session on a factory demo runs a scripted loop of user-like actions (add
//! and remove devices, turn parameters, scroll and zoom the arrangement, open and close the
//! panels, select tracks, undo and redo, save, paint a frame) for a set time while the memory of
//! the process is sampled, and the test fails on a clear leak only.
//!
//! The test is `#[ignore]`: it runs for minutes. Run it with
//!
//! ```sh
//! ATOMIK_SOAK_SECONDS=120 cargo test -p atomik-mcp --test soak -- --ignored --nocapture
//! ```
//!
//! `ATOMIK_SOAK_SECONDS` is the length of the run (default 120; the nightly job uses 900) and
//! `ATOMIK_SOAK_TABLE` a file the table of samples is also written to (default
//! `soak-table.txt` in the test folder of the build). No audio device is opened: the session has
//! none. See "The headless soak" in docs/TESTING.md for what fails it.

use atomik_app::windows::WindowKind;
use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_model::{Command, DeviceKey};
use atomik_ui::TextEngine;
use atomik_ui::cpu_canvas::CpuCanvas;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The first part of the run that is not measured: caches fill, the undo history grows to its
/// limit and the allocator settles.
const WARM_UP: f64 = 0.2;

/// The slope bound after the warm-up, in bytes per minute (2 MiB a minute). On this loop the
/// private memory is flat: 0.2 MiB a minute over ten minutes, and 0.4 to 1.0 over two or
/// three (a short run has few samples after its warm-up), see perf/BASELINE.md. The bound
/// sits about twice above the worst short run.
const MAX_SLOPE_PER_MINUTE: f64 = 2.0 * 1024.0 * 1024.0;

/// The memory at the end of the run may not be more than this many times the memory at the end
/// of the warm-up (the runs end at 1.0 to 1.12 times).
const MAX_GROWTH: f64 = 2.0;

/// Devices the loop adds to the rack, in turn (all of them effects that need no sample).
const ADDED: [&str; 5] = [
    "atomik.compressor",
    "atomik.eq",
    "atomik.reverb",
    "atomik.distortion",
    "atomik.modulation",
];

/// How many devices the loop keeps in the rack before it removes the oldest of them.
const KEPT: usize = 6;

/// The prefix of the keys of the devices the loop adds.
const ADDED_PREFIX: &str = "soak";

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn call(s: &mut Server, name: &str, args: Value) -> ToolOutput {
    s.call_tool(name, &Args::new(Some(&args)))
}

fn ok(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, name, args);
    assert!(!r.is_error, "{name}: {}", r.first_text());
    r.parsed()
}

/// Runs a menu command. A command that is disabled right now (nothing to undo yet, no track
/// selected) is a normal answer of the menu and does nothing; any other refusal fails the test.
fn command(s: &mut Server, id: &str) {
    let r = call(s, "session_command", json!({"id": id, "confirm": true}));
    assert!(
        !r.is_error || r.first_text().contains("is disabled right now"),
        "{id}: {}",
        r.first_text()
    );
}

/// A small deterministic generator: the script is the same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or(0) % bound.max(1)
    }
}

/// What the operating system says about this process.
#[derive(Clone, Copy, Default)]
struct Usage {
    /// Resident memory (the working set on Windows), bytes.
    resident: u64,
    /// Private memory: anonymous resident memory on Linux, private bytes on Windows, else the
    /// resident memory.
    private: u64,
    /// Threads of the process.
    threads: u64,
}

/// The memory and thread count of this process, or `None` where the platform has no reader here.
fn usage() -> Option<Usage> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        let kib = |key: &str| -> Option<u64> {
            let line = text.lines().find(|l| l.starts_with(key))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        };
        Some(Usage {
            resident: kib("VmRSS:")? * 1024,
            private: kib("RssAnon:").or_else(|| kib("VmRSS:"))? * 1024,
            threads: kib("Threads:")?,
        })
    }
    #[cfg(windows)]
    {
        let script = format!(
            "$p = Get-Process -Id {}; \"$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.Threads.Count)\"",
            std::process::id()
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut it = text.split_whitespace().map(|w| w.parse::<u64>().ok());
        Some(Usage {
            resident: it.next()??,
            private: it.next()??,
            threads: it.next()??,
        })
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

/// One row of the table.
struct Sample {
    /// Seconds since the start of the loop.
    at: f64,
    iteration: u64,
    usage: Usage,
    undo_depth: usize,
    /// Devices in the rack.
    devices: usize,
}

/// The slope, in bytes per minute, of the least squares line through `(minutes, bytes)`.
fn slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    if sxx <= f64::EPSILON { 0.0 } else { sxy / sxx }
}

fn mib(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

/// The text of the table and the verdict numbers.
struct Report {
    table: String,
    slope_per_minute: f64,
    growth: f64,
}

/// Builds the table and fits the slope of the private memory after the warm-up.
fn report(samples: &[Sample], total: f64) -> Report {
    let mut table =
        String::from("   t(s)  iteration  resident(MiB)  private(MiB)  threads  undo-depth\n");
    for s in samples {
        let _ = writeln!(
            table,
            "{:7.1}  {:9}  {:13.1}  {:12.1}  {:7}  {:10}  {:7}",
            s.at,
            s.iteration,
            mib(s.usage.resident as f64),
            mib(s.usage.private as f64),
            s.usage.threads,
            s.undo_depth,
            s.devices
        );
    }
    let after: Vec<&Sample> = samples.iter().filter(|s| s.at >= total * WARM_UP).collect();
    let points: Vec<(f64, f64)> = after
        .iter()
        .map(|s| (s.at / 60.0, s.usage.private as f64))
        .collect();
    let slope_per_minute = if points.len() >= 3 {
        slope(&points)
    } else {
        0.0
    };
    // The memory at the end of the warm-up and at the finish: the mean of three samples each.
    let mean = |rows: &[&Sample]| {
        rows.iter().map(|s| s.usage.private as f64).sum::<f64>() / rows.len().max(1) as f64
    };
    let growth = if after.len() >= 6 {
        mean(&after[after.len() - 3..]) / mean(&after[..3]).max(1.0)
    } else {
        1.0
    };
    let _ = writeln!(
        table,
        "\nwarm-up {:.0} s, {} samples after it\nprivate memory slope after warm-up: {:+.3} MiB/min (bound {:.1})\nprivate memory finish / end of warm-up: {:.3} (bound {:.1})",
        total * WARM_UP,
        after.len(),
        mib(slope_per_minute),
        mib(MAX_SLOPE_PER_MINUTE),
        growth,
        MAX_GROWTH
    );
    Report {
        table,
        slope_per_minute,
        growth,
    }
}

/// The keys of the devices of the demo that a track plays, in song order.
fn track_devices(s: &mut Server) -> Vec<DeviceKey> {
    let core = s.session_mut().unwrap().shell().core();
    core.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.clone())
        .collect()
}

/// The keys of the devices the loop has added to the rack and that are in it now, in rack order.
fn added_devices(doc: &atomik_model::RackDoc) -> Vec<DeviceKey> {
    doc.devices
        .iter()
        .filter(|d| d.key.0.starts_with(ADDED_PREFIX))
        .map(|d| d.key.clone())
        .collect()
}

/// The keys of the channel strips of the demo (they have the `input.gain` param).
fn channel_keys(s: &mut Server) -> Vec<DeviceKey> {
    let core = s.session_mut().unwrap().shell().core();
    core.doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.channel")
        .map(|d| d.key.clone())
        .collect()
}

/// Paints the main window on the CPU canvas, as a screenshot does, without encoding a PNG.
fn paint(s: &mut Server, text: &mut TextEngine) {
    let session = s.session_mut().unwrap();
    let shell = session.shell_mut();
    shell.frame();
    let (w, h) = shell.physical_size_in(WindowKind::Main).unwrap();
    let mut canvas = CpuCanvas::new(u16::try_from(w).unwrap(), u16::try_from(h).unwrap(), text);
    shell.paint_in(WindowKind::Main, &mut canvas);
    let _ = canvas.finish();
}

/// The group of actions an action number belongs to (the names of `ATOMIK_SOAK_ACTIONS`).
fn group_of(action: usize) -> &'static str {
    match action {
        0 | 1 => "devices",
        2..=4 => "params",
        5 | 6 => "zoom",
        7 | 8 => "panels",
        9 => "select",
        10 => "undo",
        _ => "save",
    }
}

/// The groups of actions the loop runs: `ATOMIK_SOAK_ACTIONS` is a comma separated list of
/// `devices`, `params`, `zoom`, `panels`, `select`, `undo` and `save` (default: all of them);
/// narrowing it finds which kind of action a growth comes from. A frame is painted either way.
fn groups_from_env() -> Vec<String> {
    let all = [
        "devices", "params", "zoom", "panels", "select", "undo", "save",
    ];
    match std::env::var("ATOMIK_SOAK_ACTIONS") {
        Ok(v) if !v.trim().is_empty() => {
            let wanted: Vec<String> = v.split(',').map(|w| w.trim().to_owned()).collect();
            for w in &wanted {
                assert!(
                    all.contains(&w.as_str()),
                    "ATOMIK_SOAK_ACTIONS: unknown group `{w}`"
                );
            }
            wanted
        }
        _ => all.iter().map(|w| (*w).to_owned()).collect(),
    }
}

fn seconds_from_env() -> f64 {
    std::env::var("ATOMIK_SOAK_SECONDS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v >= 10.0)
        .unwrap_or(120.0)
}

#[test]
#[ignore = "a soak of minutes: run it with --ignored (ATOMIK_SOAK_SECONDS sets the length)"]
#[allow(clippy::disallowed_methods, clippy::too_many_lines)] // Justification: the soak is timed by the wall clock; it is a test, not the engine.
fn the_headless_session_does_not_leak_under_a_long_edit_loop() {
    let total = seconds_from_env();
    let groups = groups_from_env();
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-soak");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let saved = out_dir.join("soak.atomik");

    let mut s = Server::new(Paths::new(repo(), std::slice::from_ref(&out_dir)));
    let opened = ok(
        &mut s,
        "session_open",
        json!({"path": "examples/projects/fx-demo/fx-demo.atomik"}),
    );
    assert!(
        opened["title"].as_str().unwrap().contains("fx-demo"),
        "{opened}"
    );
    let tracks = track_devices(&mut s);
    let channels = channel_keys(&mut s);
    assert!(tracks.len() >= 2 && !channels.is_empty());
    let step_limit = s
        .session_mut()
        .unwrap()
        .shell()
        .core()
        .document()
        .undo_limits()
        .0;

    // The undo history grows by a few steps a second and holds a thousand: filling it first keeps
    // the slope below from measuring a history that is still growing to its limit.
    {
        let core = s.session_mut().unwrap().shell_mut().core_mut();
        let mut n = 0u32;
        while core.document().undo_depth() < step_limit {
            n += 1;
            core.apply_command(Command::SetParam {
                key: channels[0].clone(),
                param: "input.gain".to_owned(),
                value: -6.0 + f32::from(u8::try_from(n % 12).unwrap_or(0)),
                merge: None,
            })
            .unwrap();
            assert!(n < 100_000, "the undo history does not fill");
        }
    }

    let mut text = TextEngine::new();
    let mut rng = Lcg(0x5eed);
    let mut next_type = 0usize;
    let mut max_added = 0usize;
    let mut samples: Vec<Sample> = Vec::new();
    let sample_every = Duration::from_secs_f64((total / 80.0).clamp(1.0, 10.0));
    let mut last_sample: Option<Instant> = None;
    let start = Instant::now();
    let mut iteration: u64 = 0;
    let mut max_depth = 0usize;
    let mut saved_once = false;

    while start.elapsed().as_secs_f64() < total {
        let action = rng.next(12);
        if !groups.iter().any(|g| g == group_of(action)) {
            paint(&mut s, &mut text);
            iteration += 1;
            continue;
        }
        match action {
            // Add a device at the end of the rack, or remove the oldest one that was added. The
            // devices the loop added are read from the document each time: an undo of a removal
            // brings one back and an undo of an add takes one away, so a list the loop kept
            // itself drifts and the rack grows without end (to over a hundred devices in a
            // few minutes, with the memory growing in step).
            0 | 1 => {
                let core = s.session_mut().unwrap().shell_mut().core_mut();
                let present = added_devices(core.doc());
                if present.len() >= KEPT {
                    let key = present[0].clone();
                    let _ = core.apply_command(Command::RemoveDevice { key });
                } else {
                    let before = core.doc().devices.len();
                    let key = DeviceKey(format!("{ADDED_PREFIX}{iteration}"));
                    let r = core.apply_command(Command::AddDevice {
                        index: before,
                        type_id: ADDED[next_type % ADDED.len()].to_owned(),
                        key: Some(key),
                    });
                    next_type += 1;
                    assert!(r.is_ok(), "adding a device: {r:?}");
                }
            }
            // Turn a knob.
            2..=4 => {
                let key = channels[rng.next(channels.len())].0.clone();
                let value = -12.0 + f64::from(u32::try_from(rng.next(25)).unwrap_or(0));
                ok(
                    &mut s,
                    "session_set_param",
                    json!({"device": key, "param": "input.gain", "value": value}),
                );
            }
            // Scroll and zoom the arrangement.
            5 | 6 => {
                let (dx, dy, mods): (f64, f64, &[&str]) = match rng.next(4) {
                    0 => (0.0, 1.0, &["ctrl"]),
                    1 => (0.0, -1.0, &["ctrl"]),
                    2 => (3.0, 0.0, &[]),
                    _ => (-3.0, 0.0, &[]),
                };
                ok(
                    &mut s,
                    "session_pointer",
                    json!({"action": "wheel", "x": 700, "y": 300, "dx": dx, "dy": dy, "modifiers": mods}),
                );
                command(
                    &mut s,
                    if action == 5 {
                        "view.zoom_in"
                    } else {
                        "view.zoom_out"
                    },
                );
            }
            // Open and close a panel.
            7 | 8 => {
                let panel = [
                    "view.toggle_rack",
                    "view.toggle_console",
                    "view.toggle_piano_roll",
                    "view.toggle_browser",
                    "view.toggle_song",
                    "view.toggle_inspector",
                ][rng.next(6)];
                command(&mut s, panel);
                paint(&mut s, &mut text);
                command(&mut s, panel);
            }
            // Select a track.
            9 => {
                let key = tracks[rng.next(tracks.len())].clone();
                s.session_mut()
                    .unwrap()
                    .shell_mut()
                    .core_mut()
                    .arrangement_mut()
                    .set_selected_track(Some(key));
            }
            // Undo and redo.
            10 => {
                for _ in 0..=rng.next(3) {
                    command(&mut s, "edit.undo");
                }
                for _ in 0..rng.next(3) {
                    command(&mut s, "edit.redo");
                }
            }
            // Save to a temporary file.
            _ => {
                if saved_once {
                    // The project has a folder of its own now: later saves are plain Saves.
                    command(&mut s, "file.save");
                } else {
                    // A project with audio is saved as `<name>/<name>.atomik` with its audio.
                    ok(
                        &mut s,
                        "session_save",
                        json!({"path": saved.to_string_lossy()}),
                    );
                    assert!(
                        out_dir.join("soak").join("soak.atomik").is_file(),
                        "the first save wrote no project"
                    );
                    saved_once = true;
                }
            }
        }
        paint(&mut s, &mut text);
        iteration += 1;
        max_added =
            max_added.max(added_devices(s.session_mut().unwrap().shell().core().doc()).len());

        let due = last_sample.is_none_or(|t| t.elapsed() >= sample_every);
        if due {
            let depth = s
                .session_mut()
                .unwrap()
                .shell()
                .core()
                .document()
                .undo_depth();
            max_depth = max_depth.max(depth);
            if let Some(usage) = usage() {
                samples.push(Sample {
                    at: start.elapsed().as_secs_f64(),
                    iteration,
                    usage,
                    undo_depth: depth,
                    devices: s.session_mut().unwrap().shell().core().doc().devices.len(),
                });
            }
            last_sample = Some(Instant::now());
        }
    }
    ok(&mut s, "session_close", json!({}));

    let elapsed = start.elapsed().as_secs_f64();
    let mut table = format!(
        "soak: {iteration} iterations in {elapsed:.0} s ({:.1} per second), deepest undo history {max_depth} of {step_limit}\n\n",
        iteration as f64 / elapsed
    );
    let verdict = if samples.is_empty() {
        table.push_str("no memory reader on this platform: the loop ran, nothing was measured\n");
        None
    } else {
        let r = report(&samples, elapsed);
        table.push_str(&r.table);
        Some((r.slope_per_minute, r.growth))
    };
    println!("{table}");
    let table_file = std::env::var_os("ATOMIK_SOAK_TABLE")
        .map_or_else(|| out_dir.join("soak-table.txt"), PathBuf::from);
    std::fs::write(&table_file, &table).unwrap();

    assert!(
        max_added <= KEPT,
        "the rack held {max_added} of the devices the loop adds, the loop keeps at most {KEPT}"
    );
    assert!(
        max_depth <= step_limit,
        "the undo history reached {max_depth} steps, the limit is {step_limit}"
    );
    if let Some((per_minute, growth)) = verdict {
        assert!(
            per_minute <= MAX_SLOPE_PER_MINUTE,
            "memory grows {:.2} MiB a minute after the warm-up (bound {:.1})\n{table}",
            mib(per_minute),
            mib(MAX_SLOPE_PER_MINUTE)
        );
        assert!(
            growth <= MAX_GROWTH,
            "memory at the finish is {growth:.2} times the memory at the end of the warm-up\n{table}"
        );
    }
}
