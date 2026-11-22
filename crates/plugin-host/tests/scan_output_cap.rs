//! A scanner that prints more than the parent accepts is stopped and reported as such, instead
//! of being left blocked on a full pipe until its timeout, and is reported as such also when it
//! has already ended normally by the time the parent looks.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_host::{ChildScanner, FailureKind, ScanRunner, ScannerCommand};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const ROLE: &str = "ATOMIK_SCAN_FLOOD";
const TEST: &str = "a_scanner_that_prints_too_much_is_stopped_and_reported_at_once";

/// The scanner role: print that much, in one unbroken line, and end normally. Returns whether
/// this process is the scanner.
fn flood_if_scanner() -> bool {
    let Ok(mib) = std::env::var(ROLE) else {
        return false;
    };
    let chunk = vec![b'x'; 1 << 20];
    let mut out = std::io::stdout().lock();
    for _ in 0..mib.parse::<usize>().unwrap() {
        let _ = out.write_all(&chunk);
    }
    let _ = out.flush();
    true
}

fn flooding_scanner() -> ChildScanner {
    let mut command = ScannerCommand::new(std::env::current_exe().unwrap());
    command.args = ["--exact", TEST, "--nocapture", "--test-threads=1", "--"]
        .map(Into::into)
        .into();
    command.envs = vec![(ROLE.into(), "20".into())];
    ChildScanner::new(command)
}

fn scan(runner: &ChildScanner) -> atomik_plugin_host::ScanFailure {
    runner
        .scan(
            PluginFormat::Clap,
            Path::new("flood.clap"),
            Duration::from_secs(20),
            &AtomicBool::new(false),
        )
        .unwrap_err()
}

#[test]
fn a_scanner_that_prints_too_much_is_stopped_and_reported_at_once() {
    if flood_if_scanner() {
        return;
    }
    let err = scan(&flooding_scanner());
    assert_eq!(err.kind, FailureKind::BadOutput, "{}", err.reason);
    assert!(err.reason.contains("too large"), "{}", err.reason);
}

#[test]
fn a_scanner_that_printed_too_much_and_ended_before_the_parent_looked_is_still_too_large() {
    // The parent looks three seconds late: the scanner has long ended with exit code 0, and the
    // clean exit must not hide what it printed.
    let runner = flooding_scanner().with_first_look_delay(Duration::from_secs(3));
    let err = scan(&runner);
    assert_eq!(err.kind, FailureKind::BadOutput, "{}", err.reason);
    assert!(err.reason.contains("too large"), "{}", err.reason);
}
