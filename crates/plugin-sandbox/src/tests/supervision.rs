//! Process supervision with a real process: the test binary run again with an environment
//! variable, acting as a child. One stub exits with code 3 after the Hello; the other hangs and
//! must be killed when its call times out.

use super::*;
use crate::fake::{make_ports, test_timeouts};
use crate::instance::RemoteInstance;
use crate::link::{LiveGuard, Timeouts};
use crate::transport::{Transport, spawn_process};
use atomik_plugin_host::{InstanceId, plugin_channel};
use atomik_plugin_ipc::{FromChild, PROTOCOL_VERSION, ToChild, read_frame, write_frame};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;

const STUB_ENV: &str = "ATOMIK_SANDBOX_STUB";

/// What the re-executed test binary prints before the test runs: "running 1 test", then the
/// name of the test and " ... " (the test's output is not captured, so the name comes first).
const PREAMBLE: &[u8] = b" ... ";

/// Drops the test harness's own stdout lines before the stub's frames.
struct SkipPreamble<R> {
    inner: R,
    skipped: bool,
}

impl<R: Read> Read for SkipPreamble<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.skipped {
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while !seen.ends_with(PREAMBLE) {
                if self.inner.read(&mut byte)? == 0 {
                    return Ok(0);
                }
                seen.push(byte[0]);
                assert!(seen.len() < 4096, "no test header in the stub's output");
            }
            self.skipped = true;
        }
        self.inner.read(buf)
    }
}

/// The child side: runs only when the parent test re-executes this binary with the variable.
#[test]
fn child_stub() {
    let Some(mode) = std::env::var_os(STUB_ENV) else {
        return;
    };
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let _hello: (ToChild, _) = read_frame(&mut stdin).expect("the parent's Hello");
    write_frame(
        &mut stdout,
        &FromChild::Hello {
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
        },
        None,
    )
    .expect("our Hello");
    let _ = stdout.flush();
    if mode == "hang" {
        // Bounded, so that a failed test cannot leave this process behind for long.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_secs(1));
        }
        std::process::exit(4);
    }
    let _next: (ToChild, _) = read_frame(&mut stdin).expect("the parent's request");
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(b"the stub plugin died on purpose\n");
    let _ = stderr.flush();
    std::process::exit(3);
}

fn real_transport(mode: &str) -> Transport {
    let exe = std::env::current_exe().expect("this test binary");
    let (_, test_name) = module_path!().split_once("::").expect("a module path");
    let args: Vec<OsString> = vec![
        "--exact".into(),
        format!("{test_name}::child_stub").into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ];
    let envs = vec![(OsString::from(STUB_ENV), OsString::from(mode))];
    let mut transport = spawn_process(&exe, &args, &envs).expect("the test binary starts");
    transport.reader = Box::new(SkipPreamble {
        inner: transport.reader,
        skipped: false,
    });
    transport
}

fn start_real(
    mode: &str,
    timeouts: Timeouts,
) -> (
    Result<RemoteInstance, atomik_plugin_host::BackendError>,
    ManagerEnd,
    Arc<AtomicUsize>,
) {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let live = Arc::new(AtomicUsize::new(0));
    let (port, _child_port) = make_ports();
    let instance = RemoteInstance::start(
        real_transport(mode),
        port,
        timeouts,
        manager.messenger(),
        LiveGuard::new(&live),
    );
    (instance, manager, live)
}

#[test]
fn a_child_that_exits_with_code_3_after_the_hello_is_reported_with_its_code_and_stderr() {
    let (instance, manager, live) = start_real("exit3", test_timeouts());
    let err = instance.map(|_| ()).unwrap_err().to_string();
    assert!(err.contains("code 3"), "{err}");
    assert!(err.contains("the stub plugin died on purpose"), "{err}");
    let PluginMessage::ProcessExited { reason } = next_message(&manager) else {
        panic!("not an exit message");
    };
    assert!(reason.contains("code 3"), "{reason}");
    assert!(
        reason.contains("the stub plugin died on purpose"),
        "{reason}"
    );
    assert_eq!(live.load(Ordering::SeqCst), 0, "the child is reaped");
}

#[test]
fn a_child_that_hangs_is_killed_when_its_call_times_out() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(500);
    let (instance, manager, live) = start_real("hang", timeouts);
    let err = instance.map(|_| ()).unwrap_err().to_string();
    assert!(err.contains("did not answer Instantiate"), "{err}");
    eventually("the killed child to be reaped", || {
        live.load(Ordering::SeqCst) == 0
    });
    // The failed start dropped the instance: nothing is reported, and nothing is left.
    assert_eq!(manager.try_recv(), None);
}
