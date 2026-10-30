//! What the app does when the child is slow, hung or hostile: cosmetic calls that miss never
//! stop it, repeated misses and unanswered important calls do, late batches in a row make it
//! hung, notifications out of bounds are refused, edit bursts are coalesced, and many children
//! end together.

use super::*;
use crate::clock::now;
use crate::fake::{Action, AudioMode, fake_layout, start, start_with, test_timeouts};
use atomik_plugin_host::{HUNG, LogLevel, ParamPhase, PluginInstance, PluginIo, ProcessStatus};
use atomik_plugin_ipc::{FromChild, ToChild, WireLevel, WirePhase};
use std::sync::atomic::Ordering;

fn text(id: u64, text: &str) -> Vec<(FromChild, Option<Vec<u8>>)> {
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

fn exit_reason(r: &Rig) -> String {
    loop {
        if let PluginMessage::ProcessExited { reason } = next_message(&r.manager) {
            return reason;
        }
    }
}

// ----- cosmetic calls -----------------------------------------------------------------------------

#[test]
fn a_late_text_is_dropped_and_the_next_call_gets_its_own_answer() {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(100);
    let first = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&first);
    let mut r = started(
        FakeOptions::default().script(move |req, _| match req {
            ToChild::ParamText { id, .. } if seen.fetch_add(1, Ordering::SeqCst) == 0 => Some(
                Action::Delayed(Duration::from_millis(400), text(*id, "late")),
            ),
            _ => None,
        }),
        timeouts,
    );
    assert_eq!(r.instance.param_text(1, 0.5), None, "no answer in time");
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
    let mut got = None;
    eventually("the child to answer again", || {
        got = r.instance.param_text(1, 0.6);
        got.is_some()
    });
    assert_eq!(got.as_deref(), Some("0.60 units"), "not the late text");
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
}

#[test]
fn a_busy_plugin_keeps_showing_the_last_text_it_gave() {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(100);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let mut r = started(
        FakeOptions::default().script(move |req, _| match req {
            ToChild::ParamText { id, .. } if seen.fetch_add(1, Ordering::SeqCst) > 0 => Some(
                Action::Delayed(Duration::from_millis(500), text(*id, "late")),
            ),
            _ => None,
        }),
        timeouts,
    );
    assert_eq!(r.instance.param_text(1, 0.5).as_deref(), Some("0.50 units"));
    assert_eq!(
        r.instance.param_text(1, 0.9).as_deref(),
        Some("0.50 units"),
        "the child is busy: the last text stays"
    );
    assert!(!r.instance.is_lost());
}

#[test]
fn a_cosmetic_call_that_keeps_missing_marks_the_instance_slow_and_then_stops_the_child() {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(30);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::ParamText { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut was_slow_alive = false;
    for _ in 0..100 {
        let _ = r.instance.param_text(1, 0.5);
        if r.instance.is_lost() {
            break;
        }
        was_slow_alive |= r.instance.is_slow();
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(was_slow_alive, "slow before it was stopped");
    assert!(r.instance.is_lost());
    assert!(r.fake.ctl.was_killed());
    let reason = exit_reason(&r);
    assert!(reason.contains("12 times in a row"), "{reason}");
}

#[test]
fn a_slow_instance_is_left_alone_between_its_retries_and_the_user_is_told() {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(30);
    timeouts.slow_retry = Duration::from_secs(30);
    let mut r = started(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::ParamText { .. }).then_some(Action::Silent)),
        timeouts,
    );
    for _ in 0..3 {
        assert_eq!(r.instance.param_text(1, 0.5), None);
    }
    assert!(r.instance.is_slow());
    let asked = r
        .fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, ToChild::ParamText { .. }))
        .count();
    let t = now();
    for _ in 0..50 {
        assert_eq!(r.instance.param_text(1, 0.5), None);
    }
    assert!(now().saturating_duration_since(t) < Duration::from_millis(30));
    let after = r
        .fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, ToChild::ParamText { .. }))
        .count();
    assert_eq!(after, asked, "nothing more was sent");
    let mut told = false;
    while let Some(m) = r.manager.try_recv() {
        told |= matches!(
            m,
            PluginMessage::Log { level: LogLevel::Warning, text } if text.contains("slow")
        );
    }
    assert!(told, "a warning says the plugin is slow");
    assert!(!r.instance.is_lost());
}

#[test]
fn a_layout_change_that_comes_after_its_call_gave_up_is_not_lost() {
    let mut timeouts = test_timeouts();
    timeouts.text = Duration::from_millis(100);
    let first = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&first);
    let mut changed = fake_layout();
    changed.name = "Changed".to_string();
    let mut r = started(
        FakeOptions::default().script(move |req, _| match req {
            ToChild::TakeLayoutChanged { id } if seen.fetch_add(1, Ordering::SeqCst) == 0 => {
                Some(Action::Delayed(
                    Duration::from_millis(300),
                    vec![(
                        FromChild::LayoutChanged {
                            id: *id,
                            layout: Some(changed.clone()),
                        },
                        None,
                    )],
                ))
            }
            _ => None,
        }),
        timeouts,
    );
    assert!(!r.instance.take_layout_changed(), "not in time");
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
