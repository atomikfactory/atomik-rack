//! The background capture: the state of a plugin is asked for without waiting, so a plugin
//! whose main thread is busy holds nothing up, is not stopped for it, and a call that needs
//! the state now takes the request that is out.

use super::*;
use crate::fake::{Action, FakeOptions, start_with, test_timeouts};
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{PluginInstance, StallOutcome};
use atomik_plugin_ipc::{FromChild, ToChild};

/// A fake whose main thread takes `delay` to answer a capture (busy, not hung).
fn busy_capture(delay: Duration) -> FakeOptions {
    FakeOptions::default().script(move |req, _| match req {
        ToChild::CaptureState { id } => Some(Action::Delayed(
            delay,
            vec![(FromChild::State { id: *id }, Some(b"late state".to_vec()))],
        )),
        _ => None,
    })
}

fn capture_requests(r: &Rig) -> usize {
    r.fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, ToChild::CaptureState { .. }))
        .count()
}

fn stall_calls(m: &ManagerEnd) -> Vec<(String, StallOutcome)> {
    let mut out = Vec::new();
    while let Some(msg) = m.try_recv() {
        if let PluginMessage::Stall { call, outcome, .. } = msg {
            out.push((call, outcome));
        }
    }
    out
}

#[test]
fn a_background_capture_returns_at_once_and_is_collected_when_it_arrives() {
    let mut r = rig(busy_capture(Duration::from_millis(1500)));
    let _ = stall_calls(&r.manager);
    let began = crate::clock::now();
    r.instance.request_capture().unwrap();
    assert!(r.instance.poll_capture().is_none());
    assert!(
        crate::clock::now().saturating_duration_since(began) < Duration::from_millis(1000),
        "asking and polling never wait"
    );
    let mut got = None;
    eventually("the late answer", || {
        got = r.instance.poll_capture();
        got.is_some()
    });
    assert_eq!(
        got.unwrap().unwrap(),
        PluginStateBlob(b"late state".to_vec())
    );
    assert!(r.instance.poll_capture().is_none(), "taken once");
    assert!(!r.instance.is_lost());
    assert!(!r.fake.ctl.was_killed());
    let stalls = stall_calls(&r.manager);
    assert!(
        !stalls.iter().any(|(call, _)| call == "CaptureState"),
        "nothing waited, so nothing stalled: {stalls:?}"
    );
}

#[test]
fn only_one_background_capture_is_out_at_a_time() {
    let mut r = rig(busy_capture(Duration::from_millis(150)));
    r.instance.request_capture().unwrap();
    r.instance.request_capture().unwrap();
    r.instance.request_capture().unwrap();
    eventually("the answer", || r.instance.poll_capture().is_some());
    assert_eq!(capture_requests(&r), 1);
    // Once collected, the next request goes out.
    r.instance.request_capture().unwrap();
    eventually("the second answer", || r.instance.poll_capture().is_some());
    assert_eq!(capture_requests(&r), 2);
}

#[test]
fn a_background_capture_that_is_never_answered_does_not_stop_the_child() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(100);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::CaptureState { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    instance.request_capture().unwrap();
    // Several times the capture timeout of the waiting calls.
    let until = crate::clock::now() + Duration::from_millis(500);
    while crate::clock::now() < until {
        assert!(instance.poll_capture().is_none());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!instance.is_lost());
    assert!(!started.fake.ctl.was_killed());
}

#[test]
fn a_call_that_needs_the_state_now_waits_for_the_request_that_is_out() {
    let mut r = rig(busy_capture(Duration::from_millis(200)));
    let _ = stall_calls(&r.manager);
    r.instance.request_capture().unwrap();
    let blob = r.instance.capture_state().unwrap();
    assert_eq!(blob, PluginStateBlob(b"late state".to_vec()));
    assert_eq!(capture_requests(&r), 1, "no second request");
    assert!(
        r.instance.poll_capture().is_none(),
        "nothing left to collect"
    );
    let got = stall_calls(&r.manager);
    assert_eq!(
        got,
        vec![("CaptureState".to_string(), StallOutcome::Answered)]
    );
    // A pipelined begin does not send a second request while one is out either.
    r.instance.request_capture().unwrap();
    r.instance.begin_capture_state();
    assert!(r.instance.capture_state().is_ok());
    assert_eq!(capture_requests(&r), 2);
}

#[test]
fn a_background_capture_does_not_disturb_the_calls_around_it() {
    let mut r = rig(FakeOptions::default());
    r.instance.request_capture().unwrap();
    assert_eq!(
        r.instance.param_text(1, 0.25).as_deref(),
        Some("0.25 units")
    );
    let mut got = None;
    eventually("the answer", || {
        got = r.instance.poll_capture();
        got.is_some()
    });
    assert_eq!(
        got.unwrap().unwrap(),
        PluginStateBlob(b"fake state".to_vec())
    );
}

#[test]
fn a_background_capture_on_a_lost_child_fails() {
    let mut r = rig(busy_capture(Duration::from_millis(500)));
    r.instance.request_capture().unwrap();
    r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    let mut got = None;
    eventually("the failure", || {
        got = r.instance.poll_capture();
        got.is_some()
    });
    assert_eq!(got.unwrap().unwrap_err(), lost_error());
    assert!(r.instance.request_capture().is_err());
}
