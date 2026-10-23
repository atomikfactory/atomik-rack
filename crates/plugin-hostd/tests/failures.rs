//! What happens when the plugin crashes or hangs: the child dies or stays stuck, the test does
//! not (AC-5, AC-7, AC-8 child side).

mod support;

use atomik_plugin_host::ProcessStatus;
use atomik_plugin_ipc::{Deadline, FromChild, Poll, ToChild};
use std::time::Duration;
use support::{PATIENCE, Spawn, TestParent, ramp};

fn started(spec: &Spawn) -> TestParent {
    let mut p = TestParent::spawn(spec);
    p.hello();
    p.instantiate();
    p
}

fn stereo() -> [atomik_core::Batch; 2] {
    [ramp(1.0), ramp(-1.0)]
}

#[test]
fn a_crashed_plugin_ends_the_child_and_not_the_test() {
    let mut p = started(&Spawn::null("null.crash"));
    p.activate(false);
    let t = p.submit_process(&stereo());
    let status = p.wait_exit(PATIENCE).expect("the child ends by itself");
    assert!(!status.success(), "{status:?}");
    // What the parent's reader thread does when it sees the child end.
    p.port.mark_dead();
    assert_eq!(p.port.wait_done(t, Deadline::after(PATIENCE)), Poll::Dead);
}

#[test]
fn a_crashed_child_closes_the_control_channel() {
    let mut p = started(&Spawn::null("null.crash"));
    p.activate(false);
    let _ = p.try_process(&stereo(), &[], 2, Duration::from_millis(50));
    p.wait_exit(PATIENCE).expect("the child ends");
    // Hello was read; the stream now only ends.
    assert!(matches!(p.recv_or_end(PATIENCE), Err(true)));
}

#[test]
fn the_crash_comes_after_the_batches_the_environment_names() {
    let mut p = started(&Spawn::null("null.crash").env("ATOMIK_NULL_CRASH_AFTER", "2"));
    p.activate(false);
    for _ in 0..2 {
        let out = p.process(&stereo(), &[]);
        assert_eq!(out.status, ProcessStatus::Continue);
    }
    let r = p.try_process(&stereo(), &[], 2, Duration::from_millis(50));
    assert!(r.is_err() || r.is_ok_and(|o| o.status != ProcessStatus::Continue));
    let status = p.wait_exit(PATIENCE).expect("the child ends");
    assert!(!status.success());
}

#[test]
fn a_hung_plugin_leaves_the_request_pending_and_the_control_channel_free() {
    let mut p = started(&Spawn::null("null.hang"));
    p.activate(false);
    let r = p.try_process(&stereo(), &[], 2, Duration::from_millis(300));
    assert!(matches!(r, Err(Poll::TimedOut)), "{r:?}");
    assert!(
        p.port.outstanding().is_some(),
        "the request is still pending"
    );
    assert!(
        p.wait_exit(Duration::from_millis(100)).is_none(),
        "the child is still there"
    );
    // The main thread is not the audio thread: it still answers.
    assert!(matches!(
        p.call(|id| ToChild::LatencyFrames { id }, None).0,
        FromChild::Latency { frames: 0, .. }
    ));
    p.kill();
    p.wait_exit(PATIENCE).expect("a killed child ends");
    p.port.mark_dead();
    let t = p.port.outstanding().unwrap();
    assert_eq!(p.port.wait_done(t, Deadline::Never), Poll::Dead);
}

#[test]
fn a_closed_stdin_ends_a_child_whose_plugin_hangs_within_two_seconds() {
    let mut p = started(&Spawn::null("null.hang"));
    p.activate(false);
    let _ = p.try_process(&stereo(), &[], 2, Duration::from_millis(100));
    p.close_stdin();
    let status = p
        .wait_exit(Duration::from_secs(2))
        .expect("it gives up on the plugin");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn a_hang_in_activate_sends_no_answer_and_the_child_can_be_killed() {
    let mut p = started(&Spawn::null("null.hang-activate"));
    let id = p.id();
    p.send(&ToChild::Activate {
        id,
        sample_rate: support::SR,
        frames: 64,
        offline: false,
    });
    assert!(p.recv(Duration::from_millis(800)).is_none(), "no Activated");
    assert!(p.wait_exit(Duration::from_millis(50)).is_none());
    p.kill();
    assert!(p.wait_exit(PATIENCE).is_some());
}
