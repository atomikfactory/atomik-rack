//! What loading many plugin instances costs, through the real host path: a plugin manager over
//! the sandbox backend (one child per instance, one per file, one shared) or over the plugin
//! formats in this process, with the built-in `null` shell and the test CLAP and VST3 shells.
//!
//! The sweep is a manual benchmark (not part of CI):
//!
//! ```text
//! cargo test --release -p atomik-plugin-hostd --test sweep -- --ignored --nocapture
//! ATOMIK_SWEEP_COUNTS=1,3 cargo test -p atomik-plugin-hostd --test sweep -- --ignored --nocapture
//! ```
//!
//! One row per shell, sandbox mode and instance count N (1, 10 and 30 unless
//! `ATOMIK_SWEEP_COUNTS` names others):
//!
//! * `load` is the time of the whole run of N blocking `instantiate` calls, and `per` the median
//!   of one call, in milliseconds;
//! * `children` is the number of processes below this one, `MB`, `thr` and `hdl` are what N
//!   instances added to this process and everything below it (private bytes, threads, open
//!   handles; file descriptors on Linux), per instance;
//! * `cpu` is the share of one core this process and the children used while the N instances
//!   processed silence for a few seconds, one 256-frame callback (four 64-frame batches of
//!   every instance) every 5.3 ms at 48 kHz, through the manager's normal block path.
//!
//! The library of a plugin format is loaded by its first instance, so the first instance pays
//! for the file once: the figures per instance of N = 1 are higher than those of N = 30 by that
//! share. The numbers of the process come from the operating system's process table through
//! `powershell` (Windows), `/proc` (Linux) or `ps` (macOS), which keeps this test free of
//! `unsafe`; macOS reports resident bytes and no thread or handle counts.
//!
//! The ordinary test of this file, `three_instances_...`, asserts the counts that do not depend
//! on the machine: the processes below this one and the threads of each child.
// Justification: wall time is this file's output, and the helpers wait for processes against
// the wall clock; nothing here is in a signal path.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_core::BATCH;
use atomik_device_sdk::{PluginRef, RunMode};
use atomik_plugin_host::{PluginManager, ProcessMode};
use atomik_plugin_sandbox::SandboxBackend;
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    Format, Plug, SETTLE, SR, Sandbox, config, effect, in_process, null_plugin, process_settled,
    request,
};
use std::collections::BTreeMap;
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ----- the process table -----------------------------------------------------------------------

/// One process of the table.
#[derive(Debug, Clone)]
struct Proc {
    pid: u32,
    ppid: u32,
    name: String,
    threads: Option<u64>,
    handles: Option<u64>,
    /// Private bytes (resident bytes where the system has no cheaper figure).
    private: u64,
    cpu_secs: f64,
}

/// This process and everything below it, read at one moment.
#[derive(Debug, Clone, Default)]
struct Tally {
    /// Seconds on a clock that is shared by two tallies.
    at: f64,
    /// The processes below this one, not counting console hosts.
    children: usize,
    /// The console hosts below this one: on Windows the system starts a hidden one for each
    /// child that has no window.
    consoles: usize,
    /// Their names.
    names: Vec<String>,
    /// The thread count of each of them, in order.
    child_threads: Vec<u64>,
    /// The threads of this process alone.
    parent_threads: Option<u64>,
    threads: Option<u64>,
    handles: Option<u64>,
    private: u64,
    cpu_parent: f64,
    cpu_children: f64,
}

#[cfg(not(target_os = "linux"))]
/// Runs the probe command and returns its output and its process id (so the probe is not
/// counted among the children).
fn run_probe(command: &mut Command) -> Option<(String, u32)> {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let pid = child.id();
    let out = child.wait_with_output().ok()?;
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), pid))
}

#[cfg(windows)]
fn processes() -> Option<(Vec<Proc>, f64, u32)> {
    let script = "$p = Get-CimInstance Win32_Process; \
        $t = [DateTime]::UtcNow.Ticks; \
        $p | ForEach-Object { '{0},{1},{2},{3},{4},{5},{6}' -f $_.ProcessId, $_.ParentProcessId, \
        $_.ThreadCount, $_.HandleCount, $_.PrivatePageCount, \
        ($_.KernelModeTime + $_.UserModeTime), $_.Name }; \
        'T,' + $t";
    let (text, probe) = run_probe(Command::new("powershell").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]))?;
    let mut at = f64::NAN;
    let mut table = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split(',').collect();
        if f.first() == Some(&"T") {
            at = f.get(1)?.parse::<f64>().ok()? / 1e7;
            continue;
        }
        if let [pid, ppid, threads, handles, private, cpu, name] = f[..] {
            let (Ok(pid), Ok(ppid)) = (pid.parse(), ppid.parse()) else {
                continue;
            };
            table.push(Proc {
                pid,
                ppid,
                name: name.to_string(),
                threads: threads.parse().ok(),
                handles: handles.parse().ok(),
                private: private.parse().unwrap_or(0),
                cpu_secs: cpu.parse::<f64>().unwrap_or(0.0) / 1e7,
            });
        }
    }
    Some((table, at, probe))
}

#[cfg(target_os = "linux")]
fn processes() -> Option<(Vec<Proc>, f64, u32)> {
    let mut table = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        let Ok(stat) = std::fs::read_to_string(dir.join("stat")) else {
            continue;
        };
        // "pid (comm) state ppid ..." and comm may hold spaces and parentheses.
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
            continue;
        };
        let (name, rest) = (&stat[open + 1..close], &stat[close + 1..]);
        let f: Vec<&str> = rest.split_whitespace().collect();
        let field = |i: usize| f.get(i).and_then(|s| s.parse::<u64>().ok());
        let (Some(ppid), Some(utime), Some(stime), Some(threads)) =
            (field(1), field(11), field(12), field(17))
        else {
            continue;
        };
        let private = std::fs::read_to_string(dir.join("status"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("RssAnon:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            })
            .map_or(0, |kib| kib * 1024);
        table.push(Proc {
            pid,
            ppid: ppid as u32,
            name: name.to_string(),
            threads: Some(threads),
            handles: std::fs::read_dir(dir.join("fd"))
                .ok()
                .map(|d| d.count() as u64),
            private,
            // The kernel counts in clock ticks, 100 a second on every supported system.
            cpu_secs: (utime + stime) as f64 / 100.0,
        });
    }
    Some((table, clock(), 0))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn processes() -> Option<(Vec<Proc>, f64, u32)> {
    let (text, probe) =
        run_probe(Command::new("ps").args(["-axo", "pid=,ppid=,rss=,time=,comm="]))?;
    let at = clock();
    let mut table = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if let [pid, ppid, rss, time, name, ..] = f[..] {
            let (Ok(pid), Ok(ppid), Ok(rss)) = (pid.parse(), ppid.parse(), rss.parse::<u64>())
            else {
                continue;
            };
            // "[[h:]m:]s.cc"
            let cpu_secs = time
                .split(':')
                .try_fold(0.0, |acc, part| part.parse::<f64>().map(|v| acc * 60.0 + v))
                .unwrap_or(0.0);
            table.push(Proc {
                pid,
                ppid,
                name: name.to_string(),
                threads: None,
                handles: None,
                private: rss * 1024,
                cpu_secs,
            });
        }
    }
    Some((table, at, probe))
}

/// Seconds on a clock the tallies share.
#[cfg(not(windows))]
fn clock() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Whether a process is the system's console host (the hidden console of a child without a
/// window), which is not a plugin process.
fn is_console_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("conhost.exe") || name.eq_ignore_ascii_case("OpenConsole.exe")
}

/// This process and everything below it now, or `None` where the table cannot be read.
fn tally() -> Option<Tally> {
    let (table, at, probe) = processes()?;
    let me = std::process::id();
    let mine = table.iter().find(|p| p.pid == me)?.clone();
    let mut t = Tally {
        at,
        parent_threads: mine.threads,
        threads: mine.threads,
        handles: mine.handles,
        private: mine.private,
        cpu_parent: mine.cpu_secs,
        ..Tally::default()
    };
    // Everything below this process except the probe that read the table.
    let mut frontier = vec![me];
    while let Some(parent) = frontier.pop() {
        for p in table
            .iter()
            .filter(|p| p.ppid == parent && p.pid != probe && p.pid != me)
        {
            if is_console_host(&p.name) {
                t.consoles += 1;
            } else {
                t.children += 1;
                t.names.push(format!("{} ({})", p.name, p.pid));
                t.child_threads.extend(p.threads);
            }
            t.threads = t.threads.zip(p.threads).map(|(a, b)| a + b);
            t.handles = t.handles.zip(p.handles).map(|(a, b)| a + b);
            t.private += p.private;
            t.cpu_children += p.cpu_secs;
            frontier.push(p.pid);
        }
    }
    t.child_threads.sort_unstable();
    Some(t)
}

/// A tally once what the processes started or stopped has settled: two reads in a row that
/// agree on the counts, within `within`. Threads end a moment after the call that ended them.
fn settled_tally(within: Duration) -> Option<Tally> {
    let limit = Instant::now() + within;
    let mut last = tally()?;
    loop {
        let next = tally()?;
        let same = next.children == last.children
            && next.consoles == last.consoles
            && next.threads == last.threads
            && next.child_threads == last.child_threads;
        if same || Instant::now() >= limit {
            return Some(next);
        }
        last = next;
    }
}

/// A settled tally that satisfies `ok`, or the last one after ten seconds.
fn settled_until(ok: impl Fn(&Tally) -> bool) -> Option<Tally> {
    let limit = Instant::now() + Duration::from_secs(10);
    loop {
        let t = settled_tally(Duration::from_secs(10))?;
        if ok(&t) || Instant::now() >= limit {
            return Some(t);
        }
    }
}

// ----- the rigs --------------------------------------------------------------------------------

/// Which plugin shell a row loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shell {
    /// The child's built-in test plugins (sandbox only: they exist only in the child).
    Null,
    Real(Format),
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Shell::Null => "null",
            Shell::Real(f) => f.name(),
        }
    }

    fn plugin(self) -> PluginRef {
        match self {
            Shell::Null => null_plugin("null.gain"),
            Shell::Real(f) => f.plugin(Plug::Gain),
        }
    }
}

/// Where the plugins live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// In this process (the sandbox is off).
    Here,
    /// In child processes, shared as the mode says.
    Children(ProcessMode),
}

impl Where {
    fn name(self) -> &'static str {
        match self {
            Where::Here => "in process",
            Where::Children(ProcessMode::PerPlugin) => "child each",
            Where::Children(ProcessMode::PerFile) => "child per file",
            Where::Children(ProcessMode::Shared) => "one shared child",
        }
    }
}

/// The manager of a row and, when there are children, the backend that counts them.
struct Rig {
    manager: PluginManager,
    backend: Option<Arc<SandboxBackend>>,
    sandbox: Option<Sandbox>,
}

impl Rig {
    fn new(shell: Shell, at: Where) -> Option<Self> {
        match (shell, at) {
            (Shell::Null, Where::Here) => None,
            (Shell::Real(f), Where::Here) => Some(Self {
                manager: in_process(f, &[f.main_file()]),
                backend: None,
                sandbox: None,
            }),
            (shell, Where::Children(mode)) => {
                let sb = match shell {
                    Shell::Null => Sandbox::null(config()),
                    Shell::Real(f) => Sandbox::real(f),
                };
                sb.backend.set_grouping(mode, BTreeMap::new());
                Some(Self {
                    manager: PluginManager::new(),
                    backend: Some(Arc::clone(&sb.backend)),
                    sandbox: Some(sb),
                })
            }
        }
    }

    fn manager(&mut self) -> &mut PluginManager {
        match &mut self.sandbox {
            Some(sb) => &mut sb.manager,
            None => &mut self.manager,
        }
    }

    /// The children the backend counts alive.
    fn live_children(&self) -> usize {
        self.backend.as_ref().map_or(0, |b| b.live_children())
    }

    /// Ends the rig: no child may be left behind.
    fn finish(mut self) {
        self.manager().poll();
        if let Some(sb) = self.sandbox.take() {
            sb.finish();
        }
    }
}

/// Instantiates `n` instances one after the other, as the blocking path does, and starts each
/// (the first batches pay for the child's start-up). Returns the harnesses and the time each
/// `instantiate` took.
fn load(rig: &mut Rig, shell: Shell, n: usize) -> (Vec<DeviceHarness>, Vec<Duration>) {
    let plugin = shell.plugin();
    let mut harnesses = Vec::with_capacity(n);
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let handle = rig
            .manager()
            .instantiate(&request(&plugin, RunMode::Realtime))
            .expect("the plugin loads");
        times.push(t.elapsed());
        // Silence in: the instances are idle.
        harnesses.push(effect(handle, 0.0));
    }
    for h in &mut harnesses {
        process_settled(h, 20, SETTLE);
    }
    (harnesses, times)
}

// ----- the sweep -------------------------------------------------------------------------------

/// Frames of one callback of the paced run.
const CALLBACK: usize = 256;

/// Seconds the paced run lasts.
const IDLE_SECS: f64 = 4.0;

/// One row of the table.
struct Row {
    shell: Shell,
    at: Where,
    n: usize,
    total: Duration,
    median: Duration,
    children: usize,
    names: Vec<String>,
    consoles: usize,
    live: usize,
    mb: f64,
    threads: Option<f64>,
    handles: Option<f64>,
    cpu_parent: f64,
    cpu_children: f64,
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn measure(shell: Shell, at: Where, n: usize) -> Option<Row> {
    let mut rig = Rig::new(shell, at)?;
    // The row before may still be ending its processes: its figures are not this row's.
    let before = settled_until(|t| t.children == 0)?;
    let started = Instant::now();
    let (mut harnesses, times) = load(&mut rig, shell, n);
    let total = started.elapsed();
    let live = rig.live_children();
    let after = settled_until(|t| t.children == live)?;

    // Idle: N instances process silence at 48 kHz in 256-frame callbacks.
    let period = Duration::from_secs_f64(CALLBACK as f64 / SR);
    let callbacks = (IDLE_SECS / period.as_secs_f64()) as u32;
    let first = tally()?;
    let start = Instant::now();
    for k in 0..callbacks {
        if let Some(wait) = (start + period * k).checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        for _ in 0..CALLBACK / BATCH {
            harnesses.iter_mut().for_each(DeviceHarness::process);
        }
    }
    let last = tally()?;
    let wall = (last.at - first.at).max(f64::EPSILON);

    drop(harnesses);
    rig.finish();
    let per =
        |a: Option<u64>, b: Option<u64>| a.zip(b).map(|(a, b)| (a as f64 - b as f64) / n as f64);
    Some(Row {
        shell,
        at,
        n,
        total,
        median: median(times),
        children: after.children,
        consoles: after.consoles,
        names: after.names.clone(),
        live,
        mb: (after.private as f64 - before.private as f64) / n as f64 / (1024.0 * 1024.0),
        threads: per(after.threads, before.threads),
        handles: per(after.handles, before.handles),
        cpu_parent: (last.cpu_parent - first.cpu_parent) / wall * 100.0,
        cpu_children: (last.cpu_children - first.cpu_children) / wall * 100.0,
    })
}

fn counts() -> Vec<usize> {
    std::env::var("ATOMIK_SWEEP_COUNTS")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .filter(|v: &Vec<usize>| !v.is_empty())
        .unwrap_or_else(|| vec![1, 10, 30])
}

const SHELLS: [Shell; 3] = [
    Shell::Null,
    Shell::Real(Format::Clap),
    Shell::Real(Format::Vst3),
];

const PLACES: [Where; 4] = [
    Where::Here,
    Where::Children(ProcessMode::PerPlugin),
    Where::Children(ProcessMode::PerFile),
    Where::Children(ProcessMode::Shared),
];

fn opt(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v:.1}"))
}

/// The load time, the memory, the threads, the handles and the idle CPU of N instances of each
/// shell in each sandbox mode.
#[test]
#[ignore = "a benchmark, run by hand"]
fn the_load_and_the_idle_cost_of_one_ten_and_thirty_instances() {
    println!(
        "{:<6} {:<17} {:>3} {:>9} {:>8} {:>8} {:>8} {:>7} {:>6} {:>6} {:>10} {:>10}",
        "shell",
        "where",
        "N",
        "load ms",
        "per ms",
        "children",
        "consoles",
        "MB",
        "thr",
        "hdl",
        "cpu parent",
        "cpu child"
    );
    let only = std::env::var("ATOMIK_SWEEP_ONLY").ok();
    for shell in SHELLS {
        for at in PLACES {
            let label = format!("{} {}", shell.name(), at.name());
            if only.as_ref().is_some_and(|o| !label.contains(o.as_str())) {
                continue;
            }
            for n in counts() {
                let Some(r) = measure(shell, at, n) else {
                    continue;
                };
                println!(
                    "{:<6} {:<17} {:>3} {:>9.0} {:>8.1} {:>8} {:>8} {:>7.1} {:>6} {:>6} {:>9.1}% {:>9.1}%",
                    r.shell.name(),
                    r.at.name(),
                    r.n,
                    r.total.as_secs_f64() * 1e3,
                    r.median.as_secs_f64() * 1e3,
                    r.children,
                    r.consoles,
                    r.mb,
                    opt(r.threads),
                    opt(r.handles),
                    r.cpu_parent,
                    r.cpu_children
                );
                if r.children != r.live {
                    println!(
                        "       the table shows {} children, the backend counts {}: {:?}",
                        r.children, r.live, r.names
                    );
                }
            }
        }
    }
}

// ----- the exact counts ------------------------------------------------------------------------

/// The instances of the ordinary test.
const THREE: usize = 3;

/// A rig of `mode` over the null shell.
fn null_rig(mode: ProcessMode) -> Rig {
    Rig::new(Shell::Null, Where::Children(mode)).expect("a null rig")
}

/// The processes and the threads that three instances cost, which are the same on every
/// machine: how many children a sandbox mode makes, that the children of one program have as
/// many threads as each other, that a shared child gains exactly one thread per instance, and
/// that nothing stays behind (no child, no thread of this process) once the instances are gone.
/// The absolute thread count of a process depends on the machine (the system's loader starts
/// helper threads by the number of cores), so only differences and equalities are asserted.
#[test]
fn three_instances_have_the_expected_child_processes_and_threads() {
    // Where the table cannot be read, the backend's own count still stands.
    let table = tally().is_some();
    // A child that has just ended can show in the table for a moment: read until the count is
    // the one expected, and the assertion reports what was seen if it never is.
    let read =
        |children: usize| settled_until(|t| t.children == children).expect("the table reads");

    // The first sandbox of a process starts what lives as long as the process does.
    let mut warm = null_rig(ProcessMode::PerPlugin);
    let (h, _) = load(&mut warm, Shell::Null, 1);
    drop(h);
    warm.finish();
    let base = table.then(|| read(0));

    // One child for each instance.
    let mut rig = null_rig(ProcessMode::PerPlugin);
    let (harnesses, _) = load(&mut rig, Shell::Null, THREE);
    assert_eq!(
        rig.live_children(),
        THREE,
        "one child each: counted by the backend"
    );
    if table {
        let t = read(THREE);
        assert_eq!(
            t.children, THREE,
            "one child each: processes below this one"
        );
        assert!(
            t.consoles <= t.children,
            "at most one console host per child: {t:?}"
        );
        assert!(
            t.child_threads.windows(2).all(|w| w[0] == w[1]),
            "the children of one program have as many threads as each other: {:?}",
            t.child_threads
        );
    }
    drop(harnesses);
    rig.finish();

    // One child for the instances of a file.
    let mut rig = null_rig(ProcessMode::PerFile);
    let (harnesses, _) = load(&mut rig, Shell::Null, THREE);
    assert_eq!(
        rig.live_children(),
        1,
        "one child per file: counted by the backend"
    );
    if table {
        assert_eq!(
            read(1).children,
            1,
            "one child per file: processes below this one"
        );
    }
    drop(harnesses);
    rig.finish();

    // One child for all, which gains one thread per instance.
    let mut rig = null_rig(ProcessMode::Shared);
    let (mut harnesses, _) = load(&mut rig, Shell::Null, 1);
    let one = table.then(|| read(1));
    harnesses.extend(load(&mut rig, Shell::Null, THREE - 1).0);
    assert_eq!(
        rig.live_children(),
        1,
        "a shared child: counted by the backend"
    );
    if let Some(one) = one {
        let three = read(1);
        assert_eq!(
            three.children, 1,
            "a shared child: processes below this one"
        );
        // The process table of macOS has no thread counts: the lists are empty there.
        if let (Some(&before), Some(&after)) =
            (one.child_threads.first(), three.child_threads.first())
        {
            assert_eq!(
                after,
                before + (THREE as u64 - 1),
                "a shared child gains one thread per instance ({:?} then {:?})",
                one.child_threads,
                three.child_threads
            );
        }
    }
    drop(harnesses);
    rig.finish();

    // Everything the instances made is gone.
    if let Some(base) = base {
        let after = read(0);
        assert_eq!(
            (after.children, after.consoles),
            (0, 0),
            "no process left: {after:?}"
        );
        assert_eq!(
            after.parent_threads, base.parent_threads,
            "this process has the threads it started with"
        );
    }
}
