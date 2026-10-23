//! A plugin that prints to stdout must not corrupt the control channel.

mod support;

use atomik_plugin_host::ProcessStatus;
use atomik_plugin_ipc::{FromChild, ToChild};
use std::time::Duration;
use support::{Spawn, TestParent, ramp};

#[test]
fn a_plugin_that_prints_to_stdout_leaves_every_frame_intact_and_its_text_on_stderr() {
    let mut p = TestParent::spawn(&Spawn::null("null.noisy"));
    p.hello();
    p.instantiate();
    p.activate(false);
    for _ in 0..50 {
        let out = p.process(&[ramp(1.0), ramp(-1.0)], &[]);
        assert_eq!(out.status, ProcessStatus::Continue);
    }
    // Let a few ticks run so poll prints too, then check the channel still parses.
    std::thread::sleep(Duration::from_millis(100));
    assert!(matches!(
        p.call(|id| ToChild::LatencyFrames { id }, None).0,
        FromChild::Latency { .. }
    ));
    let said = p.stderr();
    for what in [
        "noisy instantiate",
        "noisy activate",
        "noisy process",
        "noisy poll",
    ] {
        assert!(said.contains(what), "{what} missing from stderr: {said}");
    }
    assert!(
        said.contains("raw instantiate") && said.contains("raw process"),
        "{said}"
    );
}
