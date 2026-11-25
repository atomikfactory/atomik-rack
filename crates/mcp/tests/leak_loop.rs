//! A leak loop: opening and closing a headless session over and over leaves the process where it
//! started. Each cycle opens the factory `fx-demo` project, paints the main window on the CPU
//! canvas, and closes the session; at an early cycle and at the last one the test reads what the
//! operating system says about the process: the threads and the handles (file descriptors on
//! Linux) must be EQUAL, exactly; the private bytes are only reported.
//!
//! The ordinary test runs 20 cycles (compared at cycle 5 and cycle 20). The long one is
//! `#[ignore]`: 200 cycles, compared at cycle 10 and cycle 200.
//!
//! ```sh
//! cargo test -p atomik-mcp --test leak_loop -- --ignored --nocapture
//! ```
//!
//! `ATOMIK_LEAK_TABLE` names a file the table of the long run is also written to (default
//! `leak-table.txt` in the test folder of the build). Linux reads `/proc/self`, Windows asks
//! `Get-Process` through PowerShell (no `unsafe`); macOS has no reader here and the loop runs
//! without a comparison. No audio device is opened: the session has none. See "The leak loops"
//! in docs/TESTING.md.
// Justification: the helpers wait against the wall clock; this is a test, not the engine.
#![allow(clippy::disallowed_methods)]

use atomik_app::windows::WindowKind;
use atomik_mcp::{Args, Paths, Server, ToolOutput};
use atomik_ui::TextEngine;
use atomik_ui::cpu_canvas::CpuCanvas;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// What the operating system says about this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Usage {
    /// Private memory: anonymous resident memory on Linux, private bytes on Windows.
    private: u64,
    threads: u64,
    /// Open handles (file descriptors on Linux).
    handles: u64,
}

/// The usage of this process, or `None` where the platform has no reader here.
fn usage() -> Option<Usage> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        let kib = |key: &str| -> Option<u64> {
            let line = text.lines().find(|l| l.starts_with(key))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        };
        Some(Usage {
            private: kib("RssAnon:").or_else(|| kib("VmRSS:"))? * 1024,
            threads: kib("Threads:")?,
            handles: std::fs::read_dir("/proc/self/fd").ok()?.count() as u64,
        })
    }
    #[cfg(windows)]
    {
        let script = format!(
            "$p = Get-Process -Id {}; \"$($p.PrivateMemorySize64) $($p.Threads.Count) $($p.HandleCount)\"",
            std::process::id()
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut it = text.split_whitespace().map(|w| w.parse::<u64>().ok());
        Some(Usage {
            private: it.next()??,
            threads: it.next()??,
            handles: it.next()??,
        })
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

/// The usage once the threads and handles that the last cycle ended have gone: two reads in a
/// row that agree on both, or the last read after ten seconds.
fn settled() -> Option<Usage> {
    let limit = Instant::now() + Duration::from_secs(10);
    let mut last = usage()?;
    loop {
        let next = usage()?;
        if (next.threads == last.threads && next.handles == last.handles) || Instant::now() >= limit
        {
            return Some(next);
        }
        last = next;
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// One cycle: open the demo, paint a frame, close the session.
fn cycle(s: &mut Server, text: &mut TextEngine, n: usize) {
    let opened = ok(
        s,
        "session_open",
        json!({"path": "examples/projects/fx-demo/fx-demo.atomik"}),
    );
    assert!(
        opened["title"].as_str().unwrap().contains("fx-demo"),
        "cycle {n}: {opened}"
    );
    {
        let shell = s.session_mut().unwrap().shell_mut();
        shell.frame();
        let (w, h) = shell.physical_size_in(WindowKind::Main).unwrap();
        let mut canvas = CpuCanvas::new(u16::try_from(w).unwrap(), u16::try_from(h).unwrap(), text);
        shell.paint_in(WindowKind::Main, &mut canvas);
        let _ = canvas.finish();
    }
    ok(s, "session_close", json!({}));
}

/// Runs `cycles` cycles, reads the usage at cycle `first`, in the middle and at the end, asserts
/// that the threads and handles of the first and the last are equal, and returns the table.
fn run_loop(first: usize, cycles: usize) -> String {
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("mcp-leak");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let mut s = Server::new(Paths::new(repo(), std::slice::from_ref(&work)));
    let mut text = TextEngine::new();
    // A first round makes what lives as long as the process does (the text engine's fonts, the
    // lazily built tables, the first thread pool).
    cycle(&mut s, &mut text, 0);
    let mut rows: Vec<(usize, Usage)> = Vec::new();
    let marks = [first, cycles / 2, cycles];
    for n in 1..=cycles {
        cycle(&mut s, &mut text, n);
        if marks.contains(&n)
            && let Some(u) = settled()
        {
            rows.push((n, u));
        }
    }
    drop(s);

    let mut table = format!(
        "session_open / session_close on fx-demo ({cycles} cycles)\n cycle  threads  handles  private(MiB)\n"
    );
    for (n, u) in &rows {
        let _ = writeln!(
            table,
            "{n:6}  {:7}  {:7}  {:12.1}",
            u.threads,
            u.handles,
            mib(u.private)
        );
    }
    if let (Some((a, early)), Some((b, late))) = (rows.first(), rows.last())
        && a != b
    {
        assert_eq!(
            (early.threads, early.handles),
            (late.threads, late.handles),
            "{} cycles later the process holds other threads or handles than at cycle {a}\n{table}",
            b - a
        );
        let _ = writeln!(
            table,
            "equal at cycle {a} and cycle {b}: threads and handles; private bytes {:+.1} MiB (reported only)",
            mib(late.private) - mib(early.private)
        );
    } else {
        table.push_str("no reader on this platform: the loop ran, nothing was compared\n");
    }
    table
}

fn run_and_write(first: usize, cycles: usize) {
    let table = run_loop(first, cycles);
    println!("{table}");
    // The long run keeps its table; the ordinary one only prints it.
    if cycles >= 100 {
        let file = std::env::var_os("ATOMIK_LEAK_TABLE").map_or_else(
            || Path::new(env!("CARGO_TARGET_TMPDIR")).join("leak-table.txt"),
            PathBuf::from,
        );
        let mut all = std::fs::read_to_string(&file).unwrap_or_default();
        all.push_str(&table);
        all.push('\n');
        let _ = std::fs::write(file, all);
    }
}

#[test]
fn twenty_sessions_opened_and_closed_leave_the_process_where_it_started() {
    run_and_write(5, 20);
}

#[test]
#[ignore = "200 cycles: run it with --ignored"]
fn two_hundred_sessions_opened_and_closed_leave_the_process_where_it_started() {
    run_and_write(10, 200);
}
