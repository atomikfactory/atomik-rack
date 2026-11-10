//! Loading and opening the editor take as long as the plugin needs. A plugin may check its
//! licence online while it loads or before its editor opens, and show its own dialogs; a
//! firewall may ask the user first. A live child that has not answered yet is not dead: only the
//! (long) load timeout stops it, never the create timeout of the other calls, and the app is not
//! blocked meanwhile.

use super::*;
use crate::clock::now;
use crate::fake::{Action, fake_layout, fake_transport, make_ports, start_with, test_timeouts};
use crate::link::{LiveGuard, Timeouts};
use atomik_plugin_host::{
    InstanceId, NativeParent, PendingInstance, PluginInstance, plugin_channel,
};
use atomik_plugin_ipc::{FromChild, ToChild};

/// The create timeout short, the load timeout long: what the app has, scaled down.
fn slow_load_timeouts() -> Timeouts {
    let mut t = test_timeouts();
    t.create = Duration::from_millis(100);
    t.load = Duration::from_secs(10);
    t
}

fn instantiated(id: u64) -> Vec<(FromChild, Option<Vec<u8>>)> {
    vec![(
        FromChild::Instantiated {
            id,
            layout: fake_layout(),
            has_gui: true,
            latency: 7,
        },
        None,
    )]
}

fn slow_instantiate(delay: Duration) -> FakeOptions {
    FakeOptions::default().script(move |req, _| match req {
        ToChild::Instantiate { id } => Some(Action::Delayed(delay, instantiated(*id))),
        _ => None,
    })
}

#[test]
fn a_plugin_that_loads_for_longer_than_the_create_timeout_is_not_stopped() {
    let started = start_with(
        slow_instantiate(Duration::from_millis(600)),
        slow_load_timeouts(),
    );
    let instance = started.instance.map_err(|e| e.to_string()).unwrap();
    assert!(!instance.is_lost());
    assert!(!started.fake.ctl.was_killed());
    assert_eq!(instance.layout().name, "Fake Gain");
}

#[test]
fn a_pending_load_is_polled_past_the_create_timeout_without_blocking() {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let live = Arc::new(AtomicUsize::new(0));
    let (transport, port, fake) =
        fake_transport(slow_instantiate(Duration::from_millis(600)), make_ports());
    let mut pending = RemoteInstance::begin(
        transport,
        port,
        slow_load_timeouts(),
        manager.messenger(),
        LiveGuard::new(&live),
    )
    .map_err(|e| e.to_string())
    .unwrap();
    let t = now();
    let mut polls = 0;
    let instance = loop {
        let before = now();
        let answer = pending.poll();
        assert!(
            now().saturating_duration_since(before) < Duration::from_millis(100),
            "a poll does not wait"
        );
        polls += 1;
        if let Some(answer) = answer {
            break answer.map_err(|e| e.to_string()).unwrap();
        }
        assert!(now().saturating_duration_since(t) < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(polls > 10, "the load went on across many polls ({polls})");
    assert!(!instance.is_lost());
    assert!(!fake.ctl.was_killed());
}

#[test]
fn a_load_that_outlasts_the_load_timeout_stops_the_child() {
    let mut timeouts = slow_load_timeouts();
    timeouts.load = Duration::from_millis(300);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Instantiate { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let err = started.instance.map(|_| ()).unwrap_err().to_string();
    assert!(
        err.contains("did not answer Instantiate within 300 ms"),
        "{err}"
    );
    assert!(started.fake.ctl.was_killed());
}

fn slow_editor(delay: Duration) -> FakeOptions {
    FakeOptions::default().script(move |req, _| match req {
        ToChild::OpenGui { id, .. } => Some(Action::Delayed(
            delay,
            vec![(
                FromChild::GuiOpened {
                    id: *id,
                    w: 640,
                    h: 480,
                },
                None,
            )],
        )),
        _ => None,
    })
}

#[test]
fn an_editor_that_takes_long_to_open_is_waited_for_and_other_calls_do_not_disturb_it() {
    let started = start_with(
        slow_editor(Duration::from_millis(700)),
        slow_load_timeouts(),
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    let ctl = &started.fake.ctl;
    let t = now();
    assert_eq!(
        instance.begin_open_external_gui(Some(NativeParent::Win32(7))),
        Ok(false),
        "not open yet"
    );
    assert!(now().saturating_duration_since(t) < Duration::from_millis(100));
    assert_eq!(instance.poll_open_external_gui(), None);
    // Asking again while it opens sends nothing more.
    assert_eq!(instance.begin_open_external_gui(None), Ok(false));
    // Whatever else the app asks meanwhile fails at once and is not sent: the busy plugin is
    // neither disturbed nor counted as missing calls.
    eventually("the fake to receive the OpenGui", || {
        ctl.requests()
            .iter()
            .any(|r| matches!(r, ToChild::OpenGui { .. }))
    });
    let sent = ctl.requests().len();
    for _ in 0..20 {
        assert_eq!(instance.param_text(1, 0.5), None);
        let _ = instance.default_pins();
    }
    let err = instance.capture_state().unwrap_err().to_string();
    assert!(err.contains("busy with OpenGui"), "{err}");
    assert_eq!(ctl.requests().len(), sent, "nothing was sent");
    let answer = loop {
        if let Some(answer) = instance.poll_open_external_gui() {
            break answer;
        }
        assert!(now().saturating_duration_since(t) < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(answer, Ok(()));
    assert!(now().saturating_duration_since(t) >= Duration::from_millis(600));
    assert!(!instance.is_lost());
    assert!(!ctl.was_killed());
    let opens = ctl
        .requests()
        .iter()
        .filter(|r| matches!(r, ToChild::OpenGui { .. }))
        .count();
    assert_eq!(opens, 1);
    // The plugin answers again once its editor is open.
    assert_eq!(instance.param_text(1, 0.5), Some("0.50 units".to_string()));
    assert_eq!(instance.poll_open_external_gui(), None, "reported once");
}

#[test]
fn an_editor_that_never_opens_is_stopped_after_the_load_timeout() {
    let mut timeouts = slow_load_timeouts();
    timeouts.load = Duration::from_millis(300);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::OpenGui { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    assert_eq!(instance.begin_open_external_gui(None), Ok(false));
    let t = now();
    let answer = loop {
        if let Some(answer) = instance.poll_open_external_gui() {
            break answer;
        }
        assert!(now().saturating_duration_since(t) < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(5));
    };
    let err = answer.unwrap_err().to_string();
    assert!(err.contains("OpenGui"), "{err}");
    assert!(instance.is_lost());
    assert!(started.fake.ctl.was_killed());
}

#[test]
fn a_stopped_child_is_reported_with_why_and_how_it_ended() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(100);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Activate { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    started.fake.ctl.stderr("last words of the plugin\n");
    let _ = instance.activate(&ActivateConfig {
        sample_rate: 48_000.0,
        frames: atomik_core::BATCH as u32,
        mode: RunMode::Realtime,
    });
    let reason = loop {
        if let PluginMessage::ProcessExited { reason } = next_message(&started.manager) {
            break reason;
        }
    };
    assert!(reason.contains("did not answer Activate"), "{reason}");
    assert!(
        reason.contains("(the fake process was killed)"),
        "how the process ended follows: {reason}"
    );
    assert!(reason.contains("last words of the plugin"), "{reason}");
}

fn opens(ctl: &crate::fake::Control) -> usize {
    ctl.requests()
        .iter()
        .filter(|r| matches!(r, ToChild::OpenGui { .. }))
        .count()
}

/// Two instances of one plugin are two children with two links: the editor of the second is
/// asked of the second child only, answered by it alone (whatever the first is doing), and
/// neither is asked twice.
#[test]
fn two_instances_open_their_editors_each_in_its_own_child() {
    let a_started = start_with(
        slow_editor(Duration::from_millis(600)),
        slow_load_timeouts(),
    );
    let b_started = start_with(
        slow_editor(Duration::from_millis(200)),
        slow_load_timeouts(),
    );
    let mut a = a_started.instance.map_err(|e| e.to_string()).unwrap();
    let mut b = b_started.instance.map_err(|e| e.to_string()).unwrap();
    let (a_ctl, b_ctl) = (&a_started.fake.ctl, &b_started.fake.ctl);
    let t = now();
    assert_eq!(
        a.begin_open_external_gui(Some(NativeParent::Win32(7))),
        Ok(false)
    );
    assert_eq!(
        b.begin_open_external_gui(Some(NativeParent::Win32(7))),
        Ok(false)
    );
    // Asking again while they open sends nothing more, to either child.
    assert_eq!(a.begin_open_external_gui(None), Ok(false));
    assert_eq!(b.begin_open_external_gui(None), Ok(false));
    let b_answer = loop {
        // A is not answered yet: B's answer is not A's.
        if let Some(answer) = b.poll_open_external_gui() {
            break answer;
        }
        assert!(now().saturating_duration_since(t) < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(b_answer, Ok(()));
    assert_eq!(
        a.poll_open_external_gui(),
        None,
        "B's answer is not A's, A still opens"
    );
    let a_answer = loop {
        if let Some(answer) = a.poll_open_external_gui() {
            break answer;
        }
        assert!(now().saturating_duration_since(t) < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(a_answer, Ok(()));
    assert_eq!(
        (opens(a_ctl), opens(b_ctl)),
        (1, 1),
        "one request per child"
    );
    assert_eq!(b.poll_open_external_gui(), None, "reported once");
    assert_eq!(a.poll_open_external_gui(), None, "reported once");
    // Raising B's open editor asks B again and A not at all.
    assert_eq!(b.begin_open_external_gui(None), Ok(false));
    eventually("B to be asked again", || opens(b_ctl) == 2);
    assert_eq!(opens(a_ctl), 1);
}
