//! The start, the control channel and the exit rules of the child (AC-12, AC-18).

mod support;

use atomik_plugin_ipc::{FromChild, PROTOCOL_VERSION, ToChild, WireErrorKind};
use std::process::Command;
use std::time::Duration;
use support::{PATIENCE, Spawn, TestParent, host_binary};

#[test]
fn the_child_says_hello_first_with_its_protocol_and_pid() {
    let mut p = TestParent::spawn(&Spawn::null("null.gain"));
    let hello = p.hello();
    assert_eq!(
        hello,
        FromChild::Hello {
            protocol: PROTOCOL_VERSION,
            pid: p.pid()
        }
    );
}

#[test]
fn bad_arguments_print_the_usage_and_exit_with_2() {
    let out = Command::new(host_binary())
        .arg("vst2")
        .output()
        .expect("the child runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout is for frames only");
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn a_block_of_another_protocol_version_ends_the_child_with_exit_3_and_a_clear_line() {
    let mut p =
        TestParent::spawn(&Spawn::null("null.gain").with_block_version(PROTOCOL_VERSION + 1));
    let status = p.wait_exit(PATIENCE).expect("the child ends");
    assert_eq!(status.code(), Some(3));
    let said = p.stderr();
    assert!(said.contains("protocol"), "{said}");
    assert!(
        p.recv(Duration::from_millis(100)).is_none(),
        "no frame is sent"
    );
}

#[test]
fn a_parent_that_speaks_another_protocol_ends_the_child_with_exit_3() {
    let mut p = TestParent::spawn(&Spawn::null("null.gain"));
    let _ = p.recv(PATIENCE).expect("hello");
    p.send(&ToChild::Hello {
        protocol: PROTOCOL_VERSION + 1,
    });
    let status = p.wait_exit(PATIENCE).expect("the child ends");
    assert_eq!(status.code(), Some(3));
    assert!(p.stderr().contains("protocol"), "{}", p.stderr());
}

#[test]
fn a_closed_stdin_ends_the_child_with_exit_0_within_a_second() {
    let mut p = TestParent::spawn(&Spawn::null("null.gain"));
    p.hello();
    p.close_stdin();
    let status = p
        .wait_exit(Duration::from_secs(1))
        .expect("it ends in time");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn shutdown_ends_the_child_with_exit_0() {
    let mut p = TestParent::spawn(&Spawn::null("null.gain"));
    p.hello();
    p.send(&ToChild::Shutdown);
    let status = p
        .wait_exit(Duration::from_secs(1))
        .expect("it ends in time");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn a_request_before_instantiate_is_answered_with_an_error() {
    let mut p = TestParent::spawn(&Spawn::null("null.gain"));
    p.hello();
    let (reply, _) = p.call(|id| ToChild::LatencyFrames { id }, None);
    match reply {
        FromChild::Err { kind, text, .. } => {
            assert_eq!(kind, WireErrorKind::Failed);
            assert!(text.contains("instantiate"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unknown_plugin_fails_instantiate_with_unknown_plugin() {
    let mut p = TestParent::spawn(&Spawn::null("null.nothing"));
    p.hello();
    let (reply, _) = p.call(|id| ToChild::Instantiate { id }, None);
    match reply {
        FromChild::Err { kind, text, .. } => {
            assert_eq!(kind, WireErrorKind::UnknownPlugin);
            assert!(text.contains("null.nothing"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}
