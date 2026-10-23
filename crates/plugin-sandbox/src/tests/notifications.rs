//! The child's notifications reach the host's messenger; the end of the child is reported once.

use super::*;
use crate::fake::Action;
use atomik_plugin_host::{LogLevel, ParamPhase, PluginInstance};
use atomik_plugin_ipc::{FromChild, ToChild, WireLevel, WirePhase};
use std::sync::atomic::Ordering;

#[test]
fn every_notification_becomes_the_matching_plugin_message_in_order() {
    let r = rig(FakeOptions::default());
    let sent = [
        FromChild::ParamEdit {
            param: 7,
            value: 0.25,
            phase: WirePhase::Begin,
        },
        FromChild::ParamEdit {
            param: 7,
            value: 0.5,
            phase: WirePhase::Change,
        },
        FromChild::ParamEdit {
            param: 7,
            value: 0.5,
            phase: WirePhase::End,
        },
        FromChild::StateDirty,
        FromChild::LatencyChanged,
        FromChild::RescanParams,
        FromChild::RequestRestart,
        FromChild::GuiResize { w: 800, h: 600 },
        FromChild::GuiClosed,
        FromChild::GuiRequestShow,
        FromChild::GuiRequestHide,
        FromChild::Log {
            level: WireLevel::Warning,
            text: "hot".to_string(),
        },
    ];
    for m in &sent {
        r.fake.ctl.send(m, None);
    }
    let want = [
        PluginMessage::ParamEdit {
            id: 7,
            value: 0.25,
            phase: ParamPhase::Begin,
        },
        PluginMessage::ParamEdit {
            id: 7,
            value: 0.5,
            phase: ParamPhase::Change,
        },
        PluginMessage::ParamEdit {
            id: 7,
            value: 0.5,
            phase: ParamPhase::End,
        },
        PluginMessage::StateDirty,
        PluginMessage::LatencyChanged,
        PluginMessage::RescanParams,
        PluginMessage::RequestRestart,
        PluginMessage::GuiResize { w: 800, h: 600 },
        PluginMessage::GuiClosed,
        PluginMessage::GuiRequestShow,
        PluginMessage::GuiRequestHide,
        PluginMessage::Log {
            level: LogLevel::Warning,
            text: "hot".to_string(),
        },
    ];
    for expected in want {
        assert_eq!(next_message(&r.manager), expected);
    }
}

#[test]
fn a_notification_between_a_request_and_its_reply_does_not_disturb_the_call() {
    let mut r = rig(FakeOptions::default().script(|req, _| match req {
        ToChild::ParamText { id, .. } => Some(Action::Reply(vec![
            (FromChild::StateDirty, None),
            (
                FromChild::Text {
                    id: *id,
                    text: Some("t".to_string()),
                },
                None,
            ),
        ])),
        _ => None,
    }));
    assert_eq!(r.instance.param_text(1, 0.5).as_deref(), Some("t"));
    assert_eq!(next_message(&r.manager), PluginMessage::StateDirty);
}

#[test]
fn the_end_of_the_child_posts_its_exit_and_its_last_stderr_lines_once() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.stderr("first line\nthe plugin said: boom\n");
    r.fake
        .ctl
        .exit("the plugin process exited with code 3 (0x3)");
    let PluginMessage::ProcessExited { reason } = next_message(&r.manager) else {
        panic!("not an exit");
    };
    assert!(reason.contains("code 3"), "{reason}");
    assert!(reason.contains("the plugin said: boom"), "{reason}");
    assert!(reason.starts_with("the plugin process exited"), "{reason}");
    assert!(r.instance.is_lost());
    eventually("the child to be counted gone", || {
        r.live.load(Ordering::SeqCst) == 0
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(r.manager.try_recv(), None, "exactly one exit message");
}

#[test]
fn only_the_last_two_kibibytes_of_stderr_are_kept() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.stderr(&format!("OLD-{}\n", "x".repeat(4000)));
    r.fake.ctl.stderr("the end\n");
    r.fake
        .ctl
        .exit("the plugin process exited with code 1 (0x1)");
    let PluginMessage::ProcessExited { reason } = next_message(&r.manager) else {
        panic!("not an exit");
    };
    assert!(reason.contains("the end"));
    assert!(!reason.contains("OLD-"), "the start is cut");
    assert!(reason.len() < 2048 + 100, "{}", reason.len());
}

#[test]
fn dropping_the_instance_posts_no_exit_message() {
    let Rig {
        instance,
        manager,
        live,
        fake,
    } = rig(FakeOptions::default());
    drop(instance);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert_eq!(manager.try_recv(), None);
    drop(fake);
}

fn flushes(r: &Rig) -> usize {
    r.fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, atomik_plugin_ipc::ToChild::Flush { .. }))
        .count()
}

#[test]
fn an_offline_poll_flushes_the_childs_notifications_before_it_returns() {
    let mut r = rig(FakeOptions::default());
    let _p = r
        .instance
        .activate(&super::cfg(48_000.0, RunMode::Offline))
        .unwrap();
    r.fake.ctl.queue_notification(FromChild::LatencyChanged);
    r.instance.poll();
    assert_eq!(flushes(&r), 1);
    assert_eq!(r.manager.try_recv(), Some(PluginMessage::LatencyChanged));
}

#[test]
fn a_live_or_inactive_poll_sends_no_flush() {
    let mut r = rig(FakeOptions::default());
    r.instance.poll();
    let p = r
        .instance
        .activate(&super::cfg(48_000.0, RunMode::Realtime))
        .unwrap();
    r.instance.poll();
    r.instance.deactivate(p);
    let p = r
        .instance
        .activate(&super::cfg(48_000.0, RunMode::Offline))
        .unwrap();
    r.instance.deactivate(p);
    r.instance.poll();
    assert_eq!(flushes(&r), 0);
}

#[test]
fn an_unanswered_flush_kills_the_child_and_a_lost_instance_sends_none() {
    use crate::fake::{Action, start_with, test_timeouts};
    let mut timeouts = test_timeouts();
    timeouts.call = Duration::from_millis(150);
    let started = start_with(
        FakeOptions::default().script(|req, _| {
            matches!(req, atomik_plugin_ipc::ToChild::Flush { .. }).then_some(Action::Silent)
        }),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    let _p = instance
        .activate(&super::cfg(48_000.0, RunMode::Offline))
        .unwrap();
    instance.poll();
    assert!(instance.is_lost());
    assert!(started.fake.ctl.was_killed());
    let before = started.fake.ctl.requests().len();
    instance.poll();
    assert_eq!(started.fake.ctl.requests().len(), before);
}
