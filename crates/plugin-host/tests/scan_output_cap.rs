//! A scanner that prints more than the parent accepts is stopped and reported as such, instead
//! of being left blocked on a full pipe until its timeout.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_host::{ChildScanner, FailureKind, ScanRunner, ScannerCommand};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const ROLE: &str = "ATOMIK_SCAN_FLOOD";
const TEST: &str = "a_scanner_that_prints_too_much_is_stopped_and_reported_at_once";

#[test]
fn a_scanner_that_prints_too_much_is_stopped_and_reported_at_once() {
    if let Ok(mib) = std::env::var(ROLE) {
        // The scanner role: print that much, in one unbroken line, and end normally.
        let chunk = vec![b'x'; 1 << 20];
        let mut out = std::io::stdout().lock();
        for _ in 0..mib.parse::<usize>().unwrap() {
            let _ = out.write_all(&chunk);
        }
        let _ = out.flush();
        return;
    }
    let mut command = ScannerCommand::new(std::env::current_exe().unwrap());
    command.args = ["--exact", TEST, "--nocapture", "--test-threads=1", "--"]
        .map(Into::into)
        .into();
    command.envs = vec![(ROLE.into(), "20".into())];
    let err = ChildScanner::new(command)
        .scan(
            PluginFormat::Clap,
            Path::new("flood.clap"),
            Duration::from_secs(20),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::BadOutput, "{}", err.reason);
    assert!(err.reason.contains("too large"), "{}", err.reason);
}
