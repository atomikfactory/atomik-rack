//! RPC: replies are routed by id, errors keep their kind, and a call that matters that is not
//! answered in time kills the child.

use super::*;
use crate::clock::now;
use crate::fake::{Action, FAKE_PID, fake_layout, start_with, test_timeouts};
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{EditorKind, NativeParent, PluginInstance};
use atomik_plugin_ipc::{FromChild, ToChild, WireErrorKind, WireParent};
use std::sync::atomic::Ordering;

#[test]
fn the_hello_exchange_and_instantiate_give_the_layout_and_the_facts() {
    let r = rig(FakeOptions::default());
    assert_eq!(r.instance.layout().name, "Fake Gain");
    assert_eq!(r.instance.layout().params.len(), 1);
    assert!(r.instance.has_gui());
    assert_eq!(r.instance.latency_frames(), 7);
    assert_eq!(r.instance.native_handle(), FAKE_PID as usize);
    assert_eq!(r.instance.editor_kind(), EditorKind::External);
    assert!(!r.instance.is_lost());
    assert_eq!(r.live.load(Ordering::SeqCst), 1);
    let requests = r.fake.ctl.requests();
    assert!(
        matches!(requests[0], ToChild::Hello { protocol: 1 }),
        "{requests:?}"
    );
    assert!(matches!(requests[1], ToChild::Instantiate { .. }));
}

#[test]
fn a_child_that_speaks_another_protocol_is_refused_with_a_clear_error() {
    let started = start(FakeOptions {
        protocol: 99,
        ..FakeOptions::default()
    });
    let err = started.instance.map(|_| ()).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("protocol 99"), "{text}");
    assert!(started.fake.ctl.was_killed());
}

#[test]
fn replies_are_routed_to_the_call_that_asked_by_their_id() {
    let mut r = rig(FakeOptions::default());
    assert_eq!(
        r.instance.param_text(1, 0.25).as_deref(),
        Some("0.25 units")
    );
    assert_eq!(r.instance.default_pins(), vec![1, 2]);
    assert!(!r.instance.take_layout_changed());
    let blob = r.instance.capture_state().unwrap();
    assert_eq!(blob, PluginStateBlob(b"fake state".to_vec()));
    r.instance
        .restore_state(&PluginStateBlob(vec![1, 2, 3]))
        .unwrap();
    assert_eq!(r.fake.ctl.restored(), vec![vec![1, 2, 3]]);
    r.instance.close_gui();
    assert!(
        r.fake
            .ctl
            .requests()
            .iter()
            .any(|q| matches!(q, ToChild::CloseGui { .. }))
    );
}

#[test]
fn a_reply_with_another_id_is_ignored() {
    let mut r = rig(FakeOptions::default().script(|req, _| match req {
        ToChild::ParamText { id, .. } => Some(Action::Reply(vec![
            (
                FromChild::Text {
                    id: id + 100,
                    text: Some("stale".to_string()),
                },
                None,
            ),
            (
                FromChild::Text {
                    id: *id,
                    text: Some("right".to_string()),
                },
                None,
            ),
        ])),
        _ => None,
    }));
    assert_eq!(r.instance.param_text(1, 0.5).as_deref(), Some("right"));
}

#[test]
fn an_error_reply_keeps_its_kind_and_text() {
    let mut r = rig(FakeOptions::default().script(|req, _| match req {
        ToChild::CaptureState { id } => Some(Action::Reply(vec![(
            FromChild::Err {
                id: *id,
                kind: WireErrorKind::Unsupported,
                text: "no state".to_string(),
            },
            None,
        )])),
        ToChild::RestoreState { id } => Some(Action::Reply(vec![(
            FromChild::Err {
                id: *id,
                kind: WireErrorKind::Failed,
                text: "bad blob".to_string(),
            },
            None,
        )])),
        _ => None,
    }));
    assert_eq!(
        r.instance.capture_state().unwrap_err(),
        BackendError::Unsupported("no state".to_string())
    );
    assert_eq!(
        r.instance
            .restore_state(&PluginStateBlob(vec![0]))
            .unwrap_err(),
        BackendError::Failed("bad blob".to_string())
    );
    // An error reply is not a lost child.
    assert!(!r.instance.is_lost());
    assert_eq!(r.instance.param_text(1, 0.5).as_deref(), Some("0.50 units"));
}

#[test]
fn a_changed_layout_replaces_the_layout_and_reports_true() {
    let mut changed = fake_layout();
    changed.name = "Fake Gain 2".to_string();
    let mut r = rig(FakeOptions::default().script(move |req, _| match req {
        ToChild::TakeLayoutChanged { id } => Some(Action::Reply(vec![(
            FromChild::LayoutChanged {
                id: *id,
                layout: Some(changed.clone()),
            },
            None,
        )])),
        _ => None,
    }));
    assert!(r.instance.take_layout_changed());
    assert_eq!(r.instance.layout().name, "Fake Gain 2");
}

#[test]
fn activate_reports_the_new_latency_and_a_second_activate_needs_a_deactivate() {
    let mut r = rig(FakeOptions::default());
    let p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .unwrap();
    assert_eq!(r.instance.latency_frames(), 9);
    let err = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .map(|_| ())
        .unwrap_err();
    assert!(err.to_string().contains("already active"), "{err}");
    r.instance.deactivate(p);
    assert!(
        r.fake
            .ctl
            .requests()
            .iter()
            .any(|q| matches!(q, ToChild::Deactivate { .. }))
    );
    let again = r
        .instance
        .activate(&cfg(44_100.0, RunMode::Offline))
        .unwrap();
    let activations: Vec<_> = r
        .fake
        .ctl
        .requests()
        .into_iter()
        .filter_map(|q| match q {
            ToChild::Activate {
                sample_rate,
                frames,
                offline,
                ..
            } => Some((sample_rate.to_bits(), frames, offline)),
            _ => None,
        })
        .collect();
    assert_eq!(
        activations,
        vec![
            (48_000.0f64.to_bits(), 64, false),
            (44_100.0f64.to_bits(), 64, true)
        ]
    );
    drop(again);
}

#[test]
fn non_finite_numbers_are_never_sent() {
    let mut r = rig(FakeOptions::default());
    assert_eq!(r.instance.param_text(1, f64::NAN), None);
    for rate in [f64::NAN, 0.0, f64::INFINITY] {
        let err = r
            .instance
            .activate(&cfg(rate, RunMode::Realtime))
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, BackendError::Failed(_)));
    }
    let sent = r.fake.ctl.requests();
    assert_eq!(sent.len(), 2, "only Hello and Instantiate: {sent:?}");
    assert!(!r.instance.is_lost());
}

#[test]
fn the_external_editor_opens_with_its_owner_and_the_embedded_one_is_unsupported() {
    let mut r = rig(FakeOptions::default());
    r.instance
        .open_external_gui(Some(NativeParent::Win32(7)))
        .unwrap();
    let opened = r
        .fake
        .ctl
        .requests()
        .into_iter()
        .find_map(|q| match q {
            ToChild::OpenGui { owner, scale, .. } => Some((owner, scale.to_bits())),
            _ => None,
        })
        .expect("OpenGui");
    assert_eq!(opened, (Some(WireParent::Win32(7)), 1.0f64.to_bits()));
    let err = r
        .instance
        .open_gui(NativeParent::Win32(1), 1.0)
        .unwrap_err();
    assert!(matches!(err, BackendError::Unsupported(_)));
}

#[test]
fn a_sixteen_mebibyte_state_round_trips() {
    let state: Vec<u8> = (0..16usize << 20).map(|i| (i % 251) as u8).collect();
    let stored = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let stored2 = Arc::clone(&stored);
    let mut r = rig(FakeOptions::default().script(move |req, blob| match req {
        ToChild::RestoreState { id } => {
            *stored2.lock().unwrap() = blob.unwrap_or_default().to_vec();
            Some(Action::Reply(vec![(FromChild::Ok { id: *id }, None)]))
        }
        ToChild::CaptureState { id } => Some(Action::Reply(vec![(
            FromChild::State { id: *id },
            Some(stored2.lock().unwrap().clone()),
        )])),
        _ => None,
    }));
    r.instance
        .restore_state(&PluginStateBlob(state.clone()))
        .unwrap();
    assert!(r.instance.capture_state().unwrap().0 == state);
}

// ----- timeouts ---------------------------------------------------------------------------------

#[test]
fn param_text_that_is_not_answered_does_not_kill_the_child() {
    let mut r = rig(FakeOptions::default()
        .script(|req, _| matches!(req, ToChild::ParamText { .. }).then_some(Action::Silent)));
    let started = now();
    assert_eq!(r.instance.param_text(1, 0.5), None);
    let took = now().saturating_duration_since(started);
    assert!(took >= test_timeouts().text, "{took:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert!(!r.instance.is_lost());
    assert!(!r.fake.ctl.was_killed());
}

#[test]
fn activate_that_is_not_answered_fails_after_the_create_timeout_and_frees_the_caller() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(200);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Activate { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    let t = now();
    let err = instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .map(|_| ())
        .unwrap_err();
    let took = now().saturating_duration_since(t);
    assert!(
        took >= Duration::from_millis(200) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert!(err.to_string().contains("Activate"), "{err}");
    assert!(started.fake.ctl.was_killed());
    assert!(instance.is_lost());
}

#[test]
fn opening_the_editor_waits_for_the_create_timeout_and_not_the_call_timeout() {
    let mut timeouts = test_timeouts();
    timeouts.create = Duration::from_millis(300);
    timeouts.call = Duration::from_secs(30);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::OpenGui { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    let t = now();
    let err = instance.open_external_gui(None).unwrap_err();
    let took = now().saturating_duration_since(t);
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_secs(10),
        "{took:?}"
    );
    assert!(err.to_string().contains("OpenGui"), "{err}");
    assert!(instance.is_lost());
}

#[test]
fn the_other_calls_use_the_call_timeout() {
    let mut timeouts = test_timeouts();
    timeouts.call = Duration::from_millis(150);
    let started = start_with(
        FakeOptions::default()
            .script(|req, _| matches!(req, ToChild::Deactivate { .. }).then_some(Action::Silent)),
        timeouts,
    );
    let mut instance = started.instance.map_err(|e| e.to_string()).unwrap();
    let processor = instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .map_err(|e| e.to_string())
        .unwrap();
    let t = now();
    instance.deactivate(processor);
    let took = now().saturating_duration_since(t);
    assert!(
        took >= Duration::from_millis(150) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert!(instance.is_lost());
    assert!(started.fake.ctl.was_killed());
}

#[test]
fn the_block_file_is_unlinked_once_the_child_has_greeted_and_before_any_activate() {
    let before = crate::instance::UNLINKS.with(std::cell::Cell::get);
    let mut r = rig(FakeOptions::default());
    assert_eq!(
        crate::instance::UNLINKS.with(std::cell::Cell::get),
        before + 1
    );
    let requests = r.fake.ctl.requests();
    assert!(matches!(requests[0], ToChild::Hello { .. }), "{requests:?}");
    #[cfg(unix)]
    assert!(!crate::fake::LAST_BLOCK.with(|p| p.borrow().exists()));
    // The mapping stays valid: the exchange still works.
    let mut p = r.instance.activate(&cfg(100.0, RunMode::Realtime)).unwrap();
    let mut b = super::processor::Bufs::new();
    assert_eq!(
        atomik_plugin_host::PluginProcessor::process(&mut *p, &mut b.io()),
        atomik_plugin_host::ProcessStatus::Continue
    );
}
