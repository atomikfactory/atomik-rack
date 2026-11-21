//! What the app does when the child is slow, hung or hostile: cosmetic requests that are late
//! never stop it, unanswered important calls do, late batches in a row make it hung,
//! notifications out of bounds are refused, edit bursts are coalesced, and many children end
//! together.

use super::*;
use crate::clock::now;
use crate::fake::{Action, AudioMode, fake_layout, start, start_with, test_timeouts};
use atomik_plugin_host::{HUNG, LogLevel, ParamPhase, PluginInstance, PluginIo, ProcessStatus};
use atomik_plugin_ipc::{FromChild, ToChild, WireLevel, WirePhase};
use std::sync::atomic::Ordering;

pub(super) fn text(id: u64, text: &str) -> Vec<(FromChild, Option<Vec<u8>>)> {
    vec![(
        FromChild::Text {
            id,
            text: Some(text.to_string()),
        },
        None,
    )]
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

pub(super) fn exit_reason(r: &Rig) -> String {
    loop {
        if let PluginMessage::ProcessExited { reason } = next_message(&r.manager) {
            return reason;
        }
    }
}

// ----- cosmetic requests ----------------------------------------------------------------------------

/// Everything the instance has answered late since the last call.
fn late_answers(r: &mut Rig) -> Vec<atomik_plugin_host::SoftAnswer> {
    r.instance.poll();
    r.instance.take_soft_answers()
}

/// Polls frame after frame (each begins a new frame of the allowance) until `f` is true. The
/// bound is generous: a slow runner needs seconds, not milliseconds.
fn frames_until(r: &mut Rig, what: &str, mut f: impl FnMut(&mut Rig) -> bool) {
    let budget = r.instance.budget();
    let limit = now() + Duration::from_secs(60);
    while now() < limit {
        budget.begin_frame();
        if f(r) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

/// A fake that holds the texts of the requests that `held` picks until the test releases them.
fn texts_held_when(held: impl Fn(usize) -> bool + Send + 'static) -> FakeOptions {
    let calls = Arc::new(AtomicUsize::new(0));
    FakeOptions::default().script(move |req, _| match req {
        ToChild::ParamText { id, plain, .. } if held(calls.fetch_add(1, Ordering::SeqCst)) => {
            Some(Action::Blocked(text(*id, &format!("late {plain}"))))
        }
        _ => None,
    })
}

/// Asks for the text of `plain` while the plugin holds the answer, then lets it go and waits
/// until the table is empty again (a stalled answer, then a plugin that is free).
fn stall_once(r: &mut Rig, param: u32, plain: f64) {
    r.fake.ctl.hold();
    r.instance.budget().begin_frame();
    let _ = r.instance.param_text(param, plain);
    r.fake.ctl.release();
    frames_until(r, "the answer", |r| {
        r.instance.poll();
        !r.instance.soft_out()
    });
    let _ = r.instance.take_soft_answers();
}

#[test]
fn a_late_text_is_delivered_when_it_arrives_and_the_next_call_gets_its_own_answer() {
    let mut r = started(texts_held_when(|n| n == 0), test_timeouts());
    assert_eq!(r.instance.param_text(1, 0.5), None, "held by the plugin");
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    r.fake.ctl.release();
    let mut late = Vec::new();
    frames_until(&mut r, "the late text", |r| {
        late.extend(late_answers(r));
        !late.is_empty()
    });
    assert_eq!(
        late,
        vec![atomik_plugin_host::SoftAnswer::ParamText {
            param: 1,
            plain: 0.5,
            text: "late 0.5".to_string()
        }],
        "the text, for the value it was asked for"
    );
    // The next request is answered at once by the plugin: its text reaches the caller either in
    // the call or, when the wait of the frame missed it, as a late answer.
    let mut got = None;
    frames_until(&mut r, "the text of the next value", |r| {
        got = got.take().or_else(|| r.instance.param_text(1, 0.6));
        for a in late_answers(r) {
            if let atomik_plugin_host::SoftAnswer::ParamText { plain, text, .. } = a {
                assert_eq!(plain, 0.6);
                got = Some(text);
            }
        }
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some("0.60 units"));
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
}

#[test]
fn a_busy_plugin_gives_no_text_for_a_value_it_has_not_answered_and_the_right_one_later() {
    let mut r = started(texts_held_when(|n| n > 0), test_timeouts());
    // The first text comes at once (in the call or as a late answer).
    let mut first = None;
    frames_until(&mut r, "the first text", |r| {
        first = first.take().or_else(|| r.instance.param_text(1, 0.5));
        for a in late_answers(r) {
            if let atomik_plugin_host::SoftAnswer::ParamText { text, .. } = a {
                first = Some(text);
            }
        }
        first.is_some()
    });
    assert_eq!(first.as_deref(), Some("0.50 units"));
    r.instance.budget().begin_frame();
    assert_eq!(
        r.instance.param_text(1, 0.9),
        None,
        "the plugin holds it: the old text does not stand in for the new value"
    );
    r.fake.ctl.release();
    let mut late = Vec::new();
    frames_until(&mut r, "the text of the new value", |r| {
        late.extend(late_answers(r));
        !late.is_empty()
    });
    assert_eq!(
        late,
        vec![atomik_plugin_host::SoftAnswer::ParamText {
            param: 1,
            plain: 0.9,
            text: "late 0.9".to_string()
        }]
    );
    assert!(!r.instance.is_lost());
}

#[test]
fn an_instance_with_no_sign_of_life_is_marked_slow_before_it_is_stopped() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_secs(3);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::ParamText { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let budget = r.instance.budget();
    let mut was_slow_alive = false;
    let limit = now() + Duration::from_secs(60);
    let mut n = 0u32;
    while now() < limit && !r.instance.is_lost() {
        budget.begin_frame();
        n += 1;
        let _ = r.instance.param_text(1, f64::from(n));
        r.instance.poll();
        was_slow_alive |= r.instance.is_slow() && !r.instance.is_lost();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(was_slow_alive, "slow before it was stopped");
    assert!(r.instance.is_lost());
    assert!(r.fake.ctl.was_killed());
    let reason = exit_reason(&r);
    assert!(reason.contains("did not answer ParamText"), "{reason}");
}

#[test]
fn a_slow_instance_is_not_waited_for_between_its_retries_and_the_user_is_told_once() {
    let mut timeouts = test_timeouts();
    timeouts.slow_retry = Duration::from_secs(600);
    timeouts.slow_window = Duration::from_secs(600);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::ParamText { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let budget = r.instance.budget();
    for n in 0..3 {
        budget.begin_frame();
        assert_eq!(r.instance.param_text(1, f64::from(n)), None);
        assert_eq!(budget.waits(), 1, "waited for in vain");
    }
    assert!(r.instance.is_slow());
    for n in 3..60 {
        budget.begin_frame();
        assert_eq!(r.instance.param_text(1, f64::from(n)), None);
        assert_eq!(budget.waits(), 0, "a slow instance is not waited for");
    }
    let mut told = 0;
    while let Some(m) = r.manager.try_recv() {
        told += usize::from(matches!(
            m,
            PluginMessage::Log { level: LogLevel::Warning, text } if text.contains("slow")
        ));
    }
    assert_eq!(told, 1, "one warning for the whole slow episode");
    assert!(!r.instance.is_lost());
}

#[test]
fn answers_that_alternate_between_fast_and_stalled_keep_the_instance_slow() {
    // Every other request is held by the plugin and the others are answered at once: with the
    // old rule one success reset the count and every stall cost a full wait again.
    let mut timeouts = test_timeouts();
    timeouts.slow_retry = Duration::from_secs(600);
    timeouts.slow_window = Duration::from_secs(600);
    let mut r = started(texts_held_when(|n| n % 2 == 1), timeouts);
    let budget = r.instance.budget();
    let mut waits_when_slow = 0;
    for n in 0..14u32 {
        let slow = r.instance.is_slow();
        if n % 2 == 1 {
            r.fake.ctl.hold();
        }
        budget.begin_frame();
        let _ = r.instance.param_text(1, f64::from(n));
        if slow {
            waits_when_slow += budget.waits();
        }
        r.fake.ctl.release();
        frames_until(&mut r, "the answer", |r| {
            r.instance.poll();
            !r.instance.soft_out()
        });
        let _ = r.instance.take_soft_answers();
    }
    assert!(r.instance.is_slow(), "the stalls kept it in the cheap path");
    assert_eq!(
        waits_when_slow, 0,
        "once slow it was never waited for again"
    );
    assert!(!r.instance.is_lost());
}

#[test]
fn slowness_ends_with_time_not_with_one_fast_answer() {
    let mut timeouts = test_timeouts();
    timeouts.slow_retry = Duration::ZERO;
    timeouts.slow_window = Duration::from_secs(4);
    let mut r = started(texts_held_when(|n| n < 3), timeouts);
    for n in 0..3u32 {
        stall_once(&mut r, 1, f64::from(n));
    }
    let became_slow = now();
    assert!(r.instance.is_slow(), "three late answers");
    // A fast answer (the plugin answers at once from now on): asked until one gets through.
    let mut fast = false;
    frames_until(&mut r, "an answer that is in time", |r| {
        let n = 10.0 + f64::from(r.instance.soft_out() as u8);
        fast = r.instance.param_text(1, n).is_some();
        if !fast {
            r.instance.poll();
            let _ = r.instance.take_soft_answers();
        }
        fast
    });
    if now().saturating_duration_since(became_slow) < Duration::from_secs(2) {
        assert!(r.instance.is_slow(), "one fast answer does not end it");
    }
    frames_until(&mut r, "the window to pass", |r| !r.instance.is_slow());
}

#[test]
fn a_layout_change_that_comes_after_its_call_gave_up_is_not_lost() {
    let first = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&first);
    let mut changed = fake_layout();
    changed.name = "Changed".to_string();
    let mut r = started(
        FakeOptions::default().script(move |req, _| match req {
            ToChild::TakeLayoutChanged { id } if seen.fetch_add(1, Ordering::SeqCst) == 0 => {
                Some(Action::Blocked(vec![(
                    FromChild::LayoutChanged {
                        id: *id,
                        layout: Some(changed.clone()),
                    },
                    None,
                )]))
            }
            _ => None,
        }),
        test_timeouts(),
    );
    assert!(!r.instance.take_layout_changed(), "held by the plugin");
    r.fake.ctl.release();
    eventually("the late layout", || r.instance.take_layout_changed());
    assert_eq!(r.instance.layout().name, "Changed");
    assert!(!r.instance.is_lost());
}

#[test]
fn a_child_that_never_answers_capture_state_is_still_stopped() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(200);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::CaptureState { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let err = r.instance.capture_state().unwrap_err();
    assert!(err.to_string().contains("CaptureState"), "{err}");
    assert!(r.instance.is_lost());
    assert!(r.fake.ctl.was_killed());
    let reason = exit_reason(&r);
    assert!(reason.contains("did not answer CaptureState"), "{reason}");
}

// ----- a hung child ---------------------------------------------------------------------------------

fn bufs_io(b: &mut super::processor::Bufs) -> PluginIo<'_> {
    b.io()
}

#[test]
fn late_batches_in_a_row_make_the_child_hung_and_the_main_thread_ends_it_within_a_frame() {
    let mut timeouts = test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = started(FakeOptions::default(), timeouts);
    let mut p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .expect("activation");
    let mut b = super::processor::Bufs::new();
    assert_eq!(p.process(&mut bufs_io(&mut b)), ProcessStatus::Continue);
    r.fake.ctl.set_audio(AudioMode::Stall);
    let mut calls = 0;
    loop {
        calls += 1;
        match p.process(&mut bufs_io(&mut b)) {
            ProcessStatus::Late => assert!(calls < 100, "never hung"),
            ProcessStatus::Lost => break,
            other => panic!("{other:?}"),
        }
    }
    // 60 ms of 64-frame batches at 48 kHz is 45 of them.
    assert!((40..=50).contains(&calls), "{calls}");
    assert!(r.instance.is_lost(), "the block is dead");
    assert!(!r.fake.ctl.was_killed(), "the audio side never kills");
    let t = now();
    r.instance.poll();
    assert!(
        now().saturating_duration_since(t) < Duration::from_millis(50),
        "the main thread does not wait"
    );
    assert!(r.fake.ctl.was_killed());
    let reason = exit_reason(&r);
    assert!(reason.starts_with(HUNG), "{reason}");
}

#[test]
fn a_child_that_answers_every_batch_late_is_never_declared_hung() {
    let mut timeouts = test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = started(FakeOptions::default(), timeouts);
    let mut p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .expect("activation");
    let mut b = super::processor::Bufs::new();
    // Every answer takes several batch periods (one is 1.3 ms), yet each one comes.
    r.fake.ctl.set_audio_delay(Duration::from_millis(8));
    let hang_after = 45; // 60 ms of 64-frame batches at 48 kHz
    // A batch is late when the answer lands after its deadline. On a machine where the sleep
    // below overshoots, an answer may be there in time: that batch is simply taken. What must
    // never happen, however long the run of late batches, is the child being declared lost.
    let mut late = 0;
    for _ in 0..3 * hang_after {
        // The audio callback comes once per batch period.
        std::thread::sleep(Duration::from_micros(1_300));
        match p.process(&mut bufs_io(&mut b)) {
            ProcessStatus::Late => late += 1,
            ProcessStatus::Lost => panic!("declared lost after {late} late batches"),
            _ => {}
        }
    }
    assert!(
        r.fake.ctl.audio_ops().len() >= 3,
        "the child kept completing requests"
    );
    assert!(!r.instance.is_lost(), "the port is not marked dead");
    r.instance.poll();
    assert!(!r.fake.ctl.was_killed());
}

#[test]
fn late_batches_that_are_not_in_a_row_never_make_the_child_hung() {
    let mut timeouts = test_timeouts();
    timeouts.hung = Duration::from_millis(60);
    let mut r = started(FakeOptions::default(), timeouts);
    let mut p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .expect("activation");
    let mut b = super::processor::Bufs::new();
    for _ in 0..3 {
        r.fake.ctl.set_audio(AudioMode::Stall);
        for _ in 0..30 {
            assert_eq!(p.process(&mut bufs_io(&mut b)), ProcessStatus::Late);
        }
        r.fake.ctl.set_audio(AudioMode::Echo);
        eventually("an answered batch", || {
            p.process(&mut bufs_io(&mut b)) == ProcessStatus::Continue
        });
    }
    assert!(!r.instance.is_lost());
}

#[test]
fn an_offline_batch_never_counts_as_hung_it_waits_for_the_watchdog() {
    let mut timeouts = test_timeouts();
    timeouts.hung = Duration::from_millis(1);
    timeouts.watchdog = Duration::from_millis(300);
    let mut r = started(FakeOptions::default(), timeouts);
    let mut p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Offline))
        .expect("activation");
    let mut b = super::processor::Bufs::new();
    r.fake.ctl.set_audio(AudioMode::Stall);
    let t = now();
    assert_eq!(p.process(&mut bufs_io(&mut b)), ProcessStatus::Lost);
    assert!(now().saturating_duration_since(t) >= Duration::from_millis(300));
    r.instance.poll();
    let reason = exit_reason(&r);
    assert!(!reason.starts_with(HUNG), "{reason}");
    assert!(reason.contains("stopped answering audio"), "{reason}");
}

// ----- hostile output -------------------------------------------------------------------------------

#[test]
fn notifications_out_of_bounds_are_refused_without_harm() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.send(
        &FromChild::Log {
            level: WireLevel::Info,
            text: "x".repeat(5000),
        },
        None,
    );
    r.fake
        .ctl
        .send(&FromChild::GuiResize { w: 20_000, h: 10 }, None);
    r.fake
        .ctl
        .send(&FromChild::GuiResize { w: 10, h: 20_000 }, None);
    r.fake
        .ctl
        .send(&FromChild::GuiResize { w: 16_384, h: 100 }, None);
    r.fake.ctl.send(
        &FromChild::Log {
            level: WireLevel::Info,
            text: "fine".to_string(),
        },
        None,
    );
    assert_eq!(
        next_message(&r.manager),
        PluginMessage::GuiResize { w: 16_384, h: 100 }
    );
    assert!(matches!(
        next_message(&r.manager),
        PluginMessage::Log { text, .. } if text == "fine"
    ));
    assert_eq!(r.instance.rejected_frames(), 3);
    assert!(!r.instance.is_lost());
}

#[test]
fn a_blob_on_a_reply_that_carries_none_is_refused_and_the_child_is_stopped() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.send(
        &FromChild::Log {
            level: WireLevel::Info,
            text: "hi".to_string(),
        },
        Some(&[1, 2, 3]),
    );
    let reason = exit_reason(&r);
    assert!(reason.contains("cannot read"), "{reason}");
    assert!(r.instance.is_lost());
}

// ----- edit bursts ----------------------------------------------------------------------------------

fn edit(param: u32, value: f64, phase: WirePhase) -> FromChild {
    FromChild::ParamEdit {
        param,
        value,
        phase,
    }
}

#[test]
fn a_burst_of_edits_reaches_the_app_as_one_per_parameter() {
    let mut r = rig(FakeOptions::default());
    for i in 0..1000 {
        r.fake
            .ctl
            .send(&edit(1, f64::from(i), WirePhase::Change), None);
    }
    r.fake.ctl.send(&edit(2, 5.0, WirePhase::Change), None);
    r.fake.ctl.send(&FromChild::StateDirty, None);
    // Everything before it has been read; the edits are held for the main thread.
    assert_eq!(next_message(&r.manager), PluginMessage::StateDirty);
    assert_eq!(r.manager.try_recv(), None);
    r.instance.poll();
    let mut got = Vec::new();
    while let Some(m) = r.manager.try_recv() {
        got.push(m);
    }
    assert_eq!(
        got,
        vec![
            PluginMessage::ParamEdit {
                id: 1,
                value: 999.0,
                phase: ParamPhase::Change
            },
            PluginMessage::ParamEdit {
                id: 2,
                value: 5.0,
                phase: ParamPhase::Change
            },
        ]
    );
}

#[test]
fn a_gesture_keeps_its_begin_its_last_value_and_its_end_in_order() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.send(&edit(1, 0.0, WirePhase::Begin), None);
    for i in 1..=5 {
        r.fake
            .ctl
            .send(&edit(1, f64::from(i), WirePhase::Change), None);
    }
    r.fake.ctl.send(&edit(1, 5.0, WirePhase::End), None);
    let mut got = Vec::new();
    while got.len() < 3 {
        got.push(next_message(&r.manager));
    }
    let phases: Vec<_> = got
        .iter()
        .map(|m| match m {
            PluginMessage::ParamEdit { value, phase, .. } => (*value, *phase),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            (0.0, ParamPhase::Begin),
            (5.0, ParamPhase::Change),
            (5.0, ParamPhase::End)
        ]
    );
}

// ----- shutting many down -----------------------------------------------------------------------------

#[test]
fn forty_instances_with_five_slow_children_close_together_within_five_seconds() {
    let mut instances = Vec::new();
    let mut keep = Vec::new();
    for i in 0..40 {
        let options = if i % 8 == 0 {
            // A child that ignores `Shutdown` and its closed input: stopped at the deadline.
            FakeOptions::default()
                .script(|req, _| matches!(req, ToChild::Shutdown).then_some(Action::Silent))
        } else {
            FakeOptions::default()
        };
        let s = start(options);
        instances.push(
            s.instance
                .map_err(|e| e.to_string())
                .expect("the fake greets"),
        );
        keep.push((s.fake, s.manager, s.live));
    }
    let t = now();
    for instance in &mut instances {
        instance.begin_shutdown();
    }
    drop(instances);
    let took = now().saturating_duration_since(t);
    assert!(took < Duration::from_secs(5), "{took:?}");
    let killed = keep.iter().filter(|(f, _, _)| f.ctl.was_killed()).count();
    assert_eq!(killed, 5, "only the slow ones were killed");
    assert!(keep.iter().all(|(_, _, live)| {
        eventually("the child to be counted gone", || {
            live.load(Ordering::SeqCst) == 0
        });
        true
    }));
}

#[test]
fn a_drop_without_begin_shutdown_still_ends_the_child() {
    let r = rig(FakeOptions::default());
    let fake = r.fake;
    drop(r.instance);
    eventually("the fake to exit", || {
        fake.ctl
            .requests()
            .iter()
            .any(|q| matches!(q, ToChild::Shutdown))
    });
}
