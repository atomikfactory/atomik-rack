//! Leak loops: creating and destroying plugin instances over and over leaves this process, and
//! everything below it, where it started.
//!
//! Three shells run the same loop: the built-in `null` plugin in a child process of its own (the
//! sandbox in its default one-child-per-instance mode), the test CLAP plugin in a child process,
//! and the test CLAP plugin in this process (the sandbox off). Each cycle creates an instance,
//! runs some batches through it, drops it and polls the manager; after every cycle the manager
//! holds no instance. At an early cycle and at the last one the test reads the process table:
//! the threads, the handles (file descriptors on Linux) and the child processes must be EQUAL,
//! exactly; the private bytes are only reported.
//!
//! The ordinary tests run 20 cycles (compared at cycle 5 and cycle 20). The long ones are
//! `#[ignore]`: 200 cycles, compared at cycle 10 and cycle 200.
//!
//! ```sh
//! cargo test -p atomik-plugin-hostd --test leak_loops -- --ignored --nocapture
//! ```
//!
//! `ATOMIK_LEAK_TABLE` names a file the table of the long runs is also written to (default
//! `leak-table.txt` in the test folder of the build). The numbers come from the operating
//! system's process table through `powershell` (Windows), `/proc` (Linux) or `ps` (macOS), which
//! keeps this test free of `unsafe`; macOS reports resident bytes and no thread or handle counts,
//! so there only the children are compared. See "The leak loops" in docs/TESTING.md.
// Justification: the helpers wait for processes against the wall clock; nothing here is in a
// signal path.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{ManagerEvent, PluginManager};
use sandbox_support::{
    Format, Plug, SETTLE, Sandbox, config, effect, in_process, null_plugin, process_settled,
    request,
};
use std::fmt::Write as _;
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};
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
}

/// This process and everything below it, read at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Tally {
    /// The processes below this one, not counting console hosts.
    children: usize,
    /// The console hosts below this one: on Windows the system starts a hidden one for each
    /// child that has no window.
    consoles: usize,
    /// The threads of this process and of everything below it.
    threads: Option<u64>,
    /// The open handles (file descriptors on Linux) of this process and of everything below it.
    handles: Option<u64>,
    /// The private bytes of this process and of everything below it.
    private: u64,
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
fn processes() -> Option<(Vec<Proc>, u32)> {
    let script = "Get-CimInstance Win32_Process | ForEach-Object { \
        '{0},{1},{2},{3},{4},{5}' -f $_.ProcessId, $_.ParentProcessId, \
        $_.ThreadCount, $_.HandleCount, $_.PrivatePageCount, $_.Name }";
    let (text, probe) = run_probe(Command::new("powershell").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]))?;
    let mut table = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split(',').collect();
        if let [pid, ppid, threads, handles, private, name] = f[..] {
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
            });
        }
    }
    Some((table, probe))
}

#[cfg(target_os = "linux")]
fn processes() -> Option<(Vec<Proc>, u32)> {
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
        let (Some(ppid), Some(threads)) = (field(1), field(17)) else {
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
            ppid: u32::try_from(ppid).unwrap_or(0),
            name: name.to_string(),
            threads: Some(threads),
            handles: std::fs::read_dir(dir.join("fd"))
                .ok()
                .map(|d| d.count() as u64),
            private,
        });
    }
    Some((table, 0))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn processes() -> Option<(Vec<Proc>, u32)> {
    let (text, probe) = run_probe(Command::new("ps").args(["-axo", "pid=,ppid=,rss=,comm="]))?;
    let mut table = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if let [pid, ppid, rss, name, ..] = f[..] {
            let (Ok(pid), Ok(ppid), Ok(rss)) = (pid.parse(), ppid.parse(), rss.parse::<u64>())
            else {
                continue;
            };
            table.push(Proc {
                pid,
                ppid,
                name: name.to_string(),
                threads: None,
                handles: None,
                private: rss * 1024,
            });
        }
    }
    Some((table, probe))
}

/// Whether a process is the system's console host (the hidden console of a child without a
/// window), which is not a plugin process.
fn is_console_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("conhost.exe") || name.eq_ignore_ascii_case("OpenConsole.exe")
}

/// This process and everything below it now, or `None` where the table cannot be read.
fn tally() -> Option<Tally> {
    let (table, probe) = processes()?;
    let me = std::process::id();
    let mine = table.iter().find(|p| p.pid == me)?.clone();
    let mut t = Tally {
        threads: mine.threads,
        handles: mine.handles,
        private: mine.private,
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
            }
            t.threads = t.threads.zip(p.threads).map(|(a, b)| a + b);
            t.handles = t.handles.zip(p.handles).map(|(a, b)| a + b);
            t.private += p.private;
            frontier.push(p.pid);
        }
    }
    Some(t)
}

/// The tally once what the processes started or stopped has settled: two reads in a row that
/// agree on the counts, and `done` true, or the last read after ten seconds. A child that has
/// just ended can show in the table for a moment, and threads end a moment after the call that
/// ended them.
fn settled(mut done: impl FnMut() -> bool) -> Option<Tally> {
    let limit = Instant::now() + Duration::from_secs(10);
    let mut last = tally()?;
    loop {
        let next = tally()?;
        let same = next.children == last.children
            && next.consoles == last.consoles
            && next.threads == last.threads
            && next.handles == last.handles;
        if (same && done()) || Instant::now() >= limit {
            return Some(next);
        }
        last = next;
    }
}

// ----- the loop --------------------------------------------------------------------------------

/// Which plugin shell the loop creates and destroys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shell {
    /// The child's built-in `null.gain`, in a child process of its own.
    NullChild,
    /// The test CLAP plugin, in a child process of its own.
    ClapChild,
    /// The test CLAP plugin in this process.
    ClapHere,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Shell::NullChild => "null shell, one child per instance",
            Shell::ClapChild => "test CLAP plugin, one child per instance",
            Shell::ClapHere => "test CLAP plugin, in this process",
        }
    }
}

/// What the loop drives: a manager, and the sandbox that counts the children when there are any.
enum Rig {
    Child(Sandbox),
    Here(PluginManager),
}

impl Rig {
    fn new(shell: Shell) -> Self {
        match shell {
            Shell::NullChild => Rig::Child(Sandbox::null(config())),
            Shell::ClapChild => Rig::Child(Sandbox::real(Format::Clap)),
            Shell::ClapHere => Rig::Here(in_process(Format::Clap, &[Format::Clap.main_file()])),
        }
    }

    fn manager(&mut self) -> &mut PluginManager {
        match self {
            Rig::Child(sb) => &mut sb.manager,
            Rig::Here(m) => m,
        }
    }

    /// The children the sandbox backend counts alive.
    fn live_children(&self) -> usize {
        match self {
            Rig::Child(sb) => sb.backend.live_children(),
            Rig::Here(_) => 0,
        }
    }

    /// Ends the rig: no child may be left behind.
    fn finish(self) {
        match self {
            Rig::Child(sb) => sb.finish(),
            Rig::Here(m) => drop(m),
        }
    }
}

/// One cycle: create an instance, run batches through it, drop it, poll until the manager says
/// it is gone.
fn cycle(rig: &mut Rig, shell: Shell, n: usize) {
    let plugin = match shell {
        Shell::NullChild => null_plugin("null.gain"),
        Shell::ClapChild | Shell::ClapHere => Format::Clap.plugin(Plug::Gain),
    };
    let handle = rig
        .manager()
        .instantiate(&request(&plugin, RunMode::Realtime))
        .unwrap_or_else(|e| panic!("cycle {n}: the instance does not start: {e:?}"));
    let id = handle.id;
    let mut h = effect(handle, 0.25);
    process_settled(&mut h, 8, SETTLE);
    drop(h);
    let limit = Instant::now() + Duration::from_secs(10);
    let mut gone = false;
    while !gone {
        gone = rig.manager().poll().contains(&ManagerEvent::Destroyed(id));
        assert!(
            gone || Instant::now() < limit,
            "cycle {n}: the instance is not destroyed"
        );
        if !gone {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(
        rig.manager().instance_count(),
        0,
        "cycle {n}: the manager still holds an instance"
    );
    // The children of a sandbox end asynchronously, across polls.
    let limit = Instant::now() + Duration::from_secs(10);
    while rig.live_children() != 0 {
        assert!(
            Instant::now() < limit,
            "cycle {n}: {} children are left",
            rig.live_children()
        );
        rig.manager().poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn show(v: Option<u64>) -> String {
    v.map_or_else(|| "-".to_owned(), |n| n.to_string())
}

/// Runs `cycles` cycles of `shell`, reads the table at cycle `first`, in the middle and at the
/// end, asserts that the threads, handles and children of the first and the last are equal, and
/// returns the table.
fn run_loop(shell: Shell, first: usize, cycles: usize) -> String {
    let mut rig = Rig::new(shell);
    // A first round starts what lives as long as the process does (the library is loaded, the
    // plugin's descriptor is made, the child's first start-up).
    cycle(&mut rig, shell, 0);
    let mut rows: Vec<(usize, Tally)> = Vec::new();
    let marks = [first, cycles / 2, cycles];
    for n in 1..=cycles {
        cycle(&mut rig, shell, n);
        if marks.contains(&n) {
            let live = &mut rig;
            let t = settled(|| {
                live.manager().poll();
                live.live_children() == 0
            });
            if let Some(t) = t {
                rows.push((n, t));
            }
        }
    }
    rig.finish();

    let mut table = format!(
        "{} ({cycles} cycles)\n cycle  children  consoles  threads  handles  private(MiB)\n",
        shell.name()
    );
    for (n, t) in &rows {
        let _ = writeln!(
            table,
            "{n:6}  {:8}  {:8}  {:>7}  {:>7}  {:12.1}",
            t.children,
            t.consoles,
            show(t.threads),
            show(t.handles),
            mib(t.private)
        );
    }
    if let (Some((a, early)), Some((b, late))) = (rows.first(), rows.last())
        && a != b
    {
        assert_eq!(
            (early.children, early.threads, early.handles),
            (late.children, late.threads, late.handles),
            "{} cycles later the process holds different children, threads or handles than at \
             cycle {a}\n{table}",
            b - a
        );
        let _ = writeln!(
            table,
            "equal at cycle {a} and cycle {b}: children, threads and handles; private bytes {:+.1} MiB (reported only)",
            mib(late.private) - mib(early.private)
        );
    } else {
        table.push_str(
            "the process table cannot be read here: the loop ran, nothing was compared\n",
        );
    }
    table
}

fn run_and_write(shell: Shell, first: usize, cycles: usize) {
    let table = run_loop(shell, first, cycles);
    println!("{table}");
    // The long runs keep their table; the ordinary ones only print it.
    if cycles >= 100 {
        let file = std::env::var_os("ATOMIK_LEAK_TABLE").map_or_else(
            || std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("leak-table.txt"),
            std::path::PathBuf::from,
        );
        let mut all = std::fs::read_to_string(&file).unwrap_or_default();
        all.push_str(&table);
        all.push('\n');
        let _ = std::fs::write(file, all);
    }
}

// ----- the tests -------------------------------------------------------------------------------

const SHELLS: [Shell; 3] = [Shell::NullChild, Shell::ClapChild, Shell::ClapHere];

/// The loops of one run are one test, not one test for each shell: the test harness gives each
/// test a thread of its own, and a test that waits for its turn is a thread too, so the thread
/// count of the process would change with the order the tests finish in.
#[test]
fn twenty_instances_of_each_shell_leave_the_process_where_it_started() {
    for shell in SHELLS {
        run_and_write(shell, 5, 20);
    }
}

#[test]
#[ignore = "200 cycles of each shell: run it with --ignored"]
fn two_hundred_instances_of_each_shell_leave_the_process_where_it_started() {
    for shell in SHELLS {
        run_and_write(shell, 10, 200);
    }
}
