//! A call that times out stops the plugin's process only when the process shows no sign of
//! life. A plugin whose main thread is busy but whose audio thread runs is left running: the
//! call fails, the instance is stuck (every later call fails at once without waiting), and it
//! recovers by itself when the late answer arrives. A bounce keeps stopping on a timeout.

use super::*;
use crate::clock::now;
use crate::fake::{Action, FakeOptions, start_with, test_timeouts};
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{BackendError, PluginInstance};
use atomik_plugin_ipc::{FromChild, ToChild};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Timeouts where a call that waits is clearly told from one that does not.
fn slow_timeouts() -> crate::link::Timeouts {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(400);
    timeouts.call = Duration::from_millis(400);
    timeouts
}

fn started(options: FakeOptions, timeouts: crate::link::Timeouts) -> Rig {
    let s = start_with(options, timeouts);
    Rig {
        instance: s
            .instance
            .map_err(|e| e.to_string())
            .expect("the fake greets"),
        fake: s.fake,
        manager: s.manager,
        live: s.live,
    }
}

/// A fake whose main thread does not answer the first request that `matches` until the test
/// releases it; its audio thread goes on.
fn blocked_once(matches: fn(&ToChild) -> bool) -> FakeOptions {
    let seen = Arc::new(AtomicUsize::new(0));
    FakeOptions::default().script(move |req, _| {
        if !matches(req) || seen.fetch_add(1, Ordering::SeqCst) > 0 {
            return None;
        }
        Some(Action::Blocked(match req {
            ToChild::CaptureState { id } => {
                vec![(FromChild::State { id: *id }, Some(b"late state".to_vec()))]
            }
            ToChild::Activate { id, .. } => vec![(
                FromChild::Activated {
                    id: *id,
                    latency: 9,
                },
                None,
            )],
            ToChild::ListPresets { id } => vec![(
                FromChild::Presets {
                    id: *id,
                    presets: Vec::new(),
                },
                None,
            )],
            ToChild::ParamText { id, .. } => vec![(
                FromChild::Text {
                    id: *id,
                    text: Some("late".to_string()),
                },
                None,
            )],
            _ => Vec::new(),
        }))
    })
}

fn is_capture(req: &ToChild) -> bool {
    matches!(req, ToChild::CaptureState { .. })
}

fn requests_of(r: &Rig, f: fn(&ToChild) -> bool) -> usize {
    r.fake.ctl.requests().iter().filter(|q| f(q)).count()
}

fn assert_busy(err: &BackendError) {
    assert!(err.is_not_responding(), "{err}");
    assert!(!err.to_string().contains("was stopped"), "{err}");
}

#[test]
fn a_capture_that_times_out_while_the_plugin_is_alive_does_not_stop_it() {
    let mut r = started(blocked_once(is_capture), slow_timeouts());
    assert!(r.instance.not_responding().is_none());
    let err = r.instance.capture_state().unwrap_err();
    assert_busy(&err);
    assert!(err.to_string().contains("CaptureState"), "{err}");
    assert!(!r.instance.is_lost());
    assert!(!r.fake.ctl.was_killed());
    assert_eq!(r.instance.not_responding().as_deref(), Some("CaptureState"));
}

#[test]
fn while_the_plugin_is_stuck_every_call_fails_at_once_and_sends_nothing() {
    let mut r = started(blocked_once(is_capture), slow_timeouts());
    assert_busy(&r.instance.capture_state().unwrap_err());
    let sent = r.fake.ctl.requests().len();
    let began = now();
    assert_busy(&r.instance.capture_state().unwrap_err());
    assert_busy(
        &r.instance
            .restore_state(&PluginStateBlob(b"x".to_vec()))
            .unwrap_err(),
    );
    assert_busy(&r.instance.load_preset("a").unwrap_err());
    assert_busy(&r.instance.request_capture().unwrap_err());
    assert!(
        r.instance
            .activate(&cfg(48_000.0, RunMode::Realtime))
            .err()
            .is_some_and(|e| e.is_not_responding()),
        "activate fails fast too"
    );
    assert!(r.instance.list_presets().is_empty());
    assert!(r.instance.read_params().is_empty());
    assert_eq!(
        r.instance.param_text(1, 0.5),
        None,
        "a soft call is neutral"
    );
    assert!(
        now().saturating_duration_since(began) < Duration::from_millis(1500),
        "no call waited for the plugin (a wait would take 400 ms each)"
    );
    assert_eq!(r.fake.ctl.requests().len(), sent, "nothing was sent to it");
    assert!(!r.fake.ctl.was_killed());
}

#[test]
fn a_stuck_plugin_recovers_by_itself_when_the_late_answer_arrives() {
    let mut r = started(blocked_once(is_capture), slow_timeouts());
    assert_busy(&r.instance.capture_state().unwrap_err());
    assert!(r.instance.not_responding().is_some());
    r.fake.ctl.release();
    // No call is made on the instance: the reader thread sees the answer by itself.
    eventually("the plugin to answer again", || {
        r.instance.not_responding().is_none()
    });
    assert!(!r.instance.is_lost());
    let state = r.instance.capture_state().expect("it works again");
    assert_eq!(state, PluginStateBlob(b"fake state".to_vec()));
    assert!(!r.fake.ctl.was_killed());
}

#[test]
fn an_activation_that_times_out_while_the_plugin_is_alive_does_not_stop_it() {
    let mut r = started(
        blocked_once(|req| matches!(req, ToChild::Activate { .. })),
        slow_timeouts(),
    );
    let err = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .err()
        .expect("no answer in time");
    assert_busy(&err);
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    assert_eq!(r.instance.not_responding().as_deref(), Some("Activate"));
    r.fake.ctl.release();
    eventually("recovery", || r.instance.not_responding().is_none());
    assert!(
        r.instance
            .activate(&cfg(48_000.0, RunMode::Realtime))
            .is_ok(),
        "the plugin activates once it is free"
    );
}

#[test]
fn a_preset_list_that_times_out_while_the_plugin_is_alive_does_not_stop_it() {
    let mut r = started(
        blocked_once(|req| matches!(req, ToChild::ListPresets { .. })),
        slow_timeouts(),
    );
    assert!(r.instance.list_presets().is_empty());
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    assert_eq!(r.instance.not_responding().as_deref(), Some("ListPresets"));
    r.fake.ctl.release();
    eventually("recovery", || r.instance.not_responding().is_none());
    assert_eq!(r.instance.list_presets().len(), 2, "answers again");
}

/// Soft calls on `r` until the twelfth real miss leaves the instance stuck, the child is lost, or
/// ten seconds have passed (the number of calls that really wait does not depend on the clock).
fn miss_until_stuck(r: &mut Rig) {
    let limit = now() + Duration::from_secs(10);
    while now() < limit && !r.instance.is_lost() && r.instance.not_responding().is_none() {
        assert_eq!(r.instance.param_text(1, 0.5), None);
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Timeouts for the twelve-misses tests: a short soft wait, and no back-off between the calls so
/// that every call really waits.
fn miss_timeouts() -> crate::link::Timeouts {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(30);
    timeouts.slow_retry = Duration::ZERO;
    timeouts
}

#[test]
fn a_child_whose_beat_is_rarer_than_a_short_wait_is_still_alive_over_the_run_of_misses() {
    // One beat every ~40 ms against a 30 ms wait (whose own window is 15 ms): most waits see no
    // beat at all, the run of misses does. A live plugin must not be stopped for that.
    let mut r = started(
        blocked_once(|req| matches!(req, ToChild::ParamText { .. })),
        miss_timeouts(),
    );
    r.fake.ctl.set_audio_idle(Duration::from_millis(40));
    miss_until_stuck(&mut r);
    assert!(!r.instance.is_lost(), "a live plugin was stopped");
    assert!(!r.fake.ctl.was_killed());
    assert_eq!(r.instance.not_responding().as_deref(), Some("ParamText"));
}

#[test]
fn twelve_missed_soft_calls_do_not_stop_a_plugin_that_is_alive() {
    let mut r = started(
        blocked_once(|req| matches!(req, ToChild::ParamText { .. })),
        miss_timeouts(),
    );
    miss_until_stuck(&mut r);
    assert!(!r.instance.is_lost(), "a live plugin was stopped");
    assert_eq!(r.instance.not_responding().as_deref(), Some("ParamText"));
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    let asked = requests_of(&r, |q| matches!(q, ToChild::ParamText { .. }));
    assert_eq!(r.instance.param_text(1, 0.6), None);
    assert_eq!(
        requests_of(&r, |q| matches!(q, ToChild::ParamText { .. })),
        asked,
        "a stuck instance is not asked"
    );
    // The fake answers what it was sent, in order: the last answer ends the state.
    r.fake.ctl.release();
    eventually("recovery", || r.instance.not_responding().is_none());
    assert!(!r.fake.ctl.was_killed());
}

#[test]
fn an_editor_that_does_not_open_in_time_is_left_running_while_the_plugin_is_alive() {
    let mut timeouts = test_timeouts();
    timeouts.load = Duration::from_millis(300);
    let mut r = started(
        blocked_once(|req| matches!(req, ToChild::OpenGui { .. })),
        timeouts,
    );
    assert_eq!(r.instance.begin_open_external_gui(None).ok(), Some(false));
    let mut answer = None;
    eventually("the answer of the poll", || {
        answer = r.instance.poll_open_external_gui();
        answer.is_some()
    });
    let err = answer.unwrap().unwrap_err();
    assert_busy(&err);
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    assert_eq!(r.instance.not_responding().as_deref(), Some("OpenGui"));
}

#[test]
fn a_synchronous_capture_that_waits_for_the_background_request_does_not_stop_a_live_plugin() {
    let mut r = started(blocked_once(is_capture), slow_timeouts());
    r.instance.request_capture().unwrap();
    let err = r.instance.capture_state().unwrap_err();
    assert_busy(&err);
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    assert!(!r.instance.background_capture() || r.instance.poll_capture().is_none());
    r.fake.ctl.release();
    eventually("recovery", || r.instance.not_responding().is_none());
    assert_eq!(
        r.instance.capture_state().unwrap(),
        PluginStateBlob(b"fake state".to_vec())
    );
}

#[test]
fn a_child_with_no_sign_of_life_is_still_stopped_at_the_timeout() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(300);
    let mut r = started(
        FakeOptions::default().script(|req, _| is_capture(req).then_some(Action::Silent)),
        timeouts,
    );
    let err = r.instance.capture_state().unwrap_err();
    assert!(!err.is_not_responding(), "{err}");
    assert!(
        err.to_string().contains("did not answer CaptureState"),
        "{err}"
    );
    assert!(r.instance.is_lost());
    assert!(r.fake.ctl.was_killed());
    assert!(r.instance.not_responding().is_none());
}

#[test]
fn a_bounce_stops_a_plugin_that_does_not_answer_even_when_it_is_alive() {
    let mut timeouts = slow_timeouts();
    timeouts.create = Duration::from_millis(300);
    timeouts.bounce = true;
    let mut r = started(blocked_once(is_capture), timeouts);
    let err = r.instance.capture_state().unwrap_err();
    assert!(!err.is_not_responding(), "{err}");
    assert!(r.instance.is_lost());
    assert!(r.fake.ctl.was_killed());
}

#[test]
fn a_stuck_plugin_that_then_dies_is_lost_and_no_longer_stuck() {
    let mut r = started(blocked_once(is_capture), slow_timeouts());
    assert_busy(&r.instance.capture_state().unwrap_err());
    r.fake
        .ctl
        .exit("the plugin process exited with code 1 (0x1)");
    eventually("the loss", || r.instance.is_lost());
    assert!(r.instance.not_responding().is_none());
    let err = r.instance.capture_state().unwrap_err();
    assert!(!err.is_not_responding(), "{err}");
}
