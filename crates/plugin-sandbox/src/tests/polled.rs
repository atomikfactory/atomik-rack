//! Restoring a state, activating and ending a plugin do not hold the caller: the live app starts
//! them and looks again on a later frame. A plugin that is busy but alive is not stopped for
//! being slow (only the long load timeout judges it), the answers and errors are the ones the
//! blocking calls give, and a bounce keeps waiting.

use super::*;
use crate::clock::now;
use crate::fake::{Action, FakeOptions, start_with, test_timeouts};
use crate::link::Timeouts;
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{BackendError, PluginInstance, StallOutcome};
use atomik_plugin_ipc::{FromChild, ToChild, WireErrorKind};

/// The create timeout short and the load timeout long: what the app has, scaled down.
fn timeouts() -> Timeouts {
    let mut t = test_timeouts();
    t.create = Duration::from_millis(100);
    t.call = Duration::from_millis(100);
    t.load = Duration::from_secs(10);
    t
}

fn started(options: FakeOptions, timeouts: Timeouts) -> Rig {
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

fn ok(id: u64) -> Vec<(FromChild, Option<Vec<u8>>)> {
    vec![(FromChild::Ok { id }, None)]
}

fn activated(id: u64) -> Vec<(FromChild, Option<Vec<u8>>)> {
    vec![(FromChild::Activated { id, latency: 9 }, None)]
}

/// A fake whose main thread holds `RestoreState` and `Activate` until the test releases it.
fn blocked_load() -> FakeOptions {
    FakeOptions::default().script(|req, _| match req {
        ToChild::RestoreState { id } => Some(Action::Blocked(ok(*id))),
        ToChild::Activate { id, .. } => Some(Action::Blocked(activated(*id))),
        _ => None,
    })
}

fn stalls(r: &Rig) -> Vec<String> {
    let mut calls = Vec::new();
    while let Some(m) = r.manager.try_recv() {
        if let PluginMessage::Stall { call, .. } = m {
            calls.push(call);
        }
    }
    calls
}

/// Polls `f` until it answers, with a generous bound.
fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let t = now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            now().saturating_duration_since(t) < Duration::from_secs(8),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_restore_that_the_plugin_holds_is_polled_without_a_blocking_wait() {
    let mut r = started(blocked_load(), timeouts());
    let blob = PluginStateBlob(vec![4, 5, 6]);
    assert!(!r.instance.begin_restore_state(&blob).unwrap());
    // Held past the create timeout: nothing stops the child, nothing is waited for.
    std::thread::sleep(Duration::from_millis(250));
    assert!(r.instance.poll_restore_state().is_none());
    assert!(!r.instance.is_lost());
    assert!(!r.fake.ctl.was_killed());
    r.fake.ctl.release();
    until("the restore", || r.instance.poll_restore_state()).unwrap();
    assert!(
        r.fake
            .ctl
            .requests()
            .iter()
            .any(|q| matches!(q, ToChild::RestoreState { .. })),
        "the child was asked"
    );
    assert!(
        !stalls(&r).iter().any(|c| c == "RestoreState"),
        "no wait on the main thread names RestoreState"
    );
}

#[test]
fn a_restore_that_fails_says_what_the_blocking_call_says() {
    let script = || {
        FakeOptions::default().script(|req, _| match req {
            ToChild::RestoreState { id } => Some(Action::Reply(vec![(
                FromChild::Err {
                    id: *id,
                    kind: WireErrorKind::Failed,
                    text: "bad blob".to_string(),
                },
                None,
            )])),
            _ => None,
        })
    };
    let mut blocking = started(script(), timeouts());
    let wanted = blocking
        .instance
        .restore_state(&PluginStateBlob(vec![0]))
        .unwrap_err();
    let mut polled = started(script(), timeouts());
    let got = match polled
        .instance
        .begin_restore_state(&PluginStateBlob(vec![0]))
    {
        Err(e) => e,
        Ok(_) => until("the restore", || polled.instance.poll_restore_state()).unwrap_err(),
    };
    assert_eq!(got, wanted);
    assert_eq!(got, BackendError::Failed("bad blob".to_string()));
}

#[test]
fn an_activation_that_the_plugin_holds_is_polled_and_gives_the_processor() {
    let mut r = started(blocked_load(), timeouts());
    let begun = r
        .instance
        .begin_activate(&cfg(48_000.0, RunMode::Realtime))
        .map_err(|e| e.to_string())
        .unwrap();
    assert!(begun.is_none(), "the answer is not there yet");
    std::thread::sleep(Duration::from_millis(250));
    assert!(r.instance.poll_activate().is_none());
    assert!(!r.fake.ctl.was_killed());
    // Another call is refused at once, not queued behind it.
    let busy = r.instance.capture_state().unwrap_err().to_string();
    assert!(busy.contains("busy with Activate"), "{busy}");
    r.fake.ctl.release();
    let mut processor = until("the activation", || r.instance.poll_activate())
        .map_err(|e| e.to_string())
        .unwrap();
    assert_eq!(r.instance.latency_frames(), 9);
    assert!(processor.start_processing());
    processor.stop_processing();
    assert!(
        !stalls(&r).iter().any(|c| c == "Activate"),
        "no wait on the main thread names Activate"
    );
}

#[test]
fn an_activation_that_goes_unanswered_for_the_load_timeout_stops_a_child_without_a_sign_of_life() {
    let mut t = timeouts();
    t.load = Duration::from_millis(300);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Activate { .. }).then_some(Action::Silent)),
        t,
    );
    assert!(
        r.instance
            .begin_activate(&cfg(48_000.0, RunMode::Realtime))
            .unwrap()
            .is_none()
    );
    let err = until("the verdict", || r.instance.poll_activate())
        .err()
        .expect("an error");
    assert!(
        err.to_string()
            .contains("did not answer Activate within 300 ms"),
        "{err}"
    );
    assert!(r.fake.ctl.was_killed());
}

#[test]
fn a_bounce_restores_and_activates_with_the_blocking_calls() {
    let mut t = timeouts();
    t.bounce = true;
    let mut r = started(FakeOptions::default(), t);
    assert!(
        r.instance
            .begin_restore_state(&PluginStateBlob(vec![7]))
            .unwrap(),
        "restored at once"
    );
    assert_eq!(r.fake.ctl.restored(), vec![vec![7]]);
    let processor = r
        .instance
        .begin_activate(&cfg(48_000.0, RunMode::Offline))
        .map_err(|e| e.to_string())
        .unwrap();
    assert!(processor.is_some(), "activated at once");
}

#[test]
fn the_end_of_a_child_is_polled_and_a_drop_after_it_does_not_wait() {
    let r = started(FakeOptions::default(), timeouts());
    let mut instance = r.instance;
    instance.begin_shutdown();
    until("the end of the child", || {
        instance.poll_shutdown().then_some(())
    });
    let t = now();
    drop(instance);
    assert!(
        now().saturating_duration_since(t) < Duration::from_secs(1),
        "the child is gone: the drop has nothing to wait for"
    );
}

#[test]
fn a_child_that_ignores_the_shutdown_is_stopped_after_the_grace_and_polled_to_the_end() {
    // It answers nothing, not even the Shutdown: the grace runs out, the child is stopped.
    let r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Shutdown).then_some(Action::Silent)),
        timeouts(),
    );
    let mut instance = r.instance;
    let t = now();
    let mut polls = 0;
    until("the child to be stopped", || {
        polls += 1;
        instance.poll_shutdown().then_some(())
    });
    assert!(r.fake.ctl.was_killed(), "stopped after the grace");
    assert!(polls > 5, "the end was polled across many calls ({polls})");
    assert!(now().saturating_duration_since(t) >= Duration::from_millis(900));
    let t = now();
    drop(instance);
    assert!(
        now().saturating_duration_since(t) < Duration::from_secs(1),
        "the drop does not wait again"
    );
}

#[test]
fn the_blocking_calls_still_name_themselves_in_a_stall_record() {
    // The records the polled path must not produce are the ones the blocking path does.
    let mut r = started(
        FakeOptions::default().script(|req, _| match req {
            ToChild::Activate { id, .. } => {
                Some(Action::Delayed(Duration::from_millis(300), activated(*id)))
            }
            _ => None,
        }),
        {
            let mut t = timeouts();
            t.create = Duration::from_secs(5);
            t
        },
    );
    let _processor = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .map_err(|e| e.to_string())
        .unwrap();
    let mut found = Vec::new();
    while let Some(m) = r.manager.try_recv() {
        if let PluginMessage::Stall { call, outcome, .. } = m {
            found.push((call, outcome));
        }
    }
    assert!(
        found.contains(&("Activate".to_string(), StallOutcome::Answered)),
        "{found:?}"
    );
}
