//! An offline render asks every sandboxed plugin for its notifications before it waits for any
//! answer: `begin_poll` sends the `Flush`, `poll` takes its answer.

use super::*;
use crate::fake::Action;
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{PluginInstance, PluginProcessor};
use atomik_plugin_ipc::{FromChild, ToChild};

/// A fake whose `Flush` is answered, with two notifications before the answer, only once the
/// test releases it.
fn held_flush() -> FakeOptions {
    FakeOptions::default().script(|req, _| match req {
        ToChild::Flush { id } => Some(Action::Blocked(vec![
            (FromChild::StateDirty, None),
            (FromChild::LatencyChanged, None),
            (FromChild::Ok { id: *id }, None),
        ])),
        _ => None,
    })
}

fn flushes(r: &Rig) -> usize {
    r.fake
        .ctl
        .requests()
        .iter()
        .filter(|q| matches!(q, ToChild::Flush { .. }))
        .count()
}

fn offline(r: &mut Rig) -> Box<dyn PluginProcessor> {
    r.instance
        .activate(&cfg(48_000.0, RunMode::Offline))
        .expect("activation")
}

fn fake_state() -> PluginStateBlob {
    PluginStateBlob(b"fake state".to_vec())
}

#[test]
fn begin_poll_sends_one_flush_and_returns_while_the_child_holds_the_answer() {
    let mut r = rig(held_flush());
    let _p = offline(&mut r);
    r.instance.begin_poll();
    eventually("the child to receive the flush", || flushes(&r) == 1);
    assert_eq!(r.manager.try_recv(), None, "nothing is answered yet");
    std::thread::scope(|s| {
        let ctl = &r.fake.ctl;
        s.spawn(move || ctl.release());
        r.instance.poll();
    });
    assert_eq!(flushes(&r), 1, "poll took the answer, it sent no other");
    assert_eq!(r.manager.try_recv(), Some(PluginMessage::StateDirty));
    assert_eq!(r.manager.try_recv(), Some(PluginMessage::LatencyChanged));
    assert_eq!(r.manager.try_recv(), None);
}

#[test]
fn a_poll_without_a_begin_still_makes_one_blocking_flush() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.fake.ctl.queue_notification(FromChild::LatencyChanged);
    r.instance.poll();
    assert_eq!(flushes(&r), 1);
    assert_eq!(r.manager.try_recv(), Some(PluginMessage::LatencyChanged));
}

#[test]
fn begin_twice_leaves_one_request_out_and_nothing_stale_behind() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.instance.begin_poll();
    r.instance.begin_poll();
    r.instance.poll();
    assert_eq!(flushes(&r), 1);
    // The next poll has nothing out and flushes by itself, answered by its own request.
    r.instance.poll();
    assert_eq!(flushes(&r), 2);
    assert!(!r.instance.is_lost());
}

#[test]
fn another_call_between_the_begin_and_the_poll_leaves_one_answered_flush_and_no_mix_up() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.instance.begin_poll();
    // A blocking call abandons the request that is out.
    assert_eq!(r.instance.capture_state().unwrap(), fake_state());
    r.fake.ctl.queue_notification(FromChild::StateDirty);
    r.instance.poll();
    assert_eq!(flushes(&r), 2, "the abandoned one and the one poll made");
    assert_eq!(r.manager.try_recv(), Some(PluginMessage::StateDirty));
    assert!(!r.instance.is_lost());
    r.instance.poll();
    assert_eq!(flushes(&r), 3);
}

#[test]
fn a_flush_that_is_out_is_not_taken_for_a_capture() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.instance.begin_poll();
    // A capture that was not begun asks for itself.
    assert_eq!(r.instance.capture_state().unwrap(), fake_state());
    // One that was begun after the flush took its place.
    r.instance.begin_poll();
    r.instance.begin_capture_state();
    assert_eq!(r.instance.capture_state().unwrap(), fake_state());
    r.instance.poll();
    assert!(!r.instance.is_lost());
}

#[test]
fn a_capture_that_is_out_is_not_taken_for_a_flush_and_its_collect_is_not_fooled() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.instance.begin_capture_state();
    r.instance.begin_poll();
    // The flush replaced the capture's request: the collect asks again and gets a state.
    assert_eq!(r.instance.capture_state().unwrap(), fake_state());
    r.instance.poll();
    assert!(!r.instance.is_lost());
    // The other way round: the capture is begun after the flush and the poll flushes by itself.
    r.instance.begin_poll();
    r.instance.begin_capture_state();
    r.instance.poll();
    assert_eq!(r.instance.capture_state().unwrap(), fake_state());
    assert!(!r.instance.is_lost());
}

#[test]
fn a_background_capture_and_a_begun_flush_are_answered_each_in_its_own_slot() {
    let mut r = rig(FakeOptions::default());
    let _p = offline(&mut r);
    r.instance.request_capture().unwrap();
    r.instance.begin_poll();
    r.instance.poll();
    let mut got = None;
    eventually("the background capture", || {
        got = r.instance.poll_capture();
        got.is_some()
    });
    assert_eq!(got.unwrap().unwrap(), fake_state());
    assert!(!r.instance.is_lost());
}

#[test]
fn a_live_or_inactive_instance_sends_nothing_for_begin_poll() {
    let mut r = rig(FakeOptions::default());
    r.instance.begin_poll();
    let p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .unwrap();
    r.instance.begin_poll();
    r.instance.poll();
    r.instance.deactivate(p);
    let p = offline(&mut r);
    r.instance.deactivate(p);
    r.instance.begin_poll();
    r.instance.poll();
    assert_eq!(flushes(&r), 0);
}

#[test]
fn a_child_that_ends_between_the_begin_and_the_poll_is_a_lost_link_and_nothing_hangs() {
    let mut r = rig(FakeOptions::default().script(|req, _| {
        matches!(req, ToChild::Flush { .. })
            .then(|| Action::Exit("the plugin process exited with code 9 (0x9)".to_string()))
    }));
    let _p = offline(&mut r);
    r.instance.begin_poll();
    r.instance.poll();
    assert!(r.instance.is_lost());
    let PluginMessage::ProcessExited { reason } = next_message(&r.manager) else {
        panic!("not an exit");
    };
    assert!(reason.contains("code 9"), "{reason}");
    let before = r.fake.ctl.requests().len();
    r.instance.begin_poll();
    r.instance.poll();
    assert_eq!(
        r.fake.ctl.requests().len(),
        before,
        "a lost link sends none"
    );
}

/// One manager, four instances in four children whose flushes are held: a single
/// `PluginManager::poll` has sent the flush to every child before any of them is answered.
#[test]
fn one_manager_poll_sends_every_childs_flush_before_it_waits_for_any_answer() {
    use atomik_device_sdk::{PluginFormat, PluginRef};
    use atomik_plugin_host::{
        HostHandle, InstantiateRequest, ManagerEvent, PluginBackend, PluginManager,
        mark_main_thread,
    };
    use std::sync::Mutex;

    type Parts = (crate::transport::Transport, atomik_plugin_ipc::ParentPort);
    struct Several(Mutex<Vec<Parts>>);
    impl PluginBackend for Several {
        fn format(&self) -> PluginFormat {
            PluginFormat::Clap
        }
        fn scan_file(
            &self,
            _: &std::path::Path,
        ) -> Result<Vec<atomik_plugin_host::ScannedPlugin>, BackendError> {
            Err(BackendError::Unsupported(String::new()))
        }
        fn instantiate(
            &self,
            _: &PluginRef,
            host: HostHandle,
        ) -> Result<Box<dyn PluginInstance>, BackendError> {
            let (transport, port) = self.0.lock().unwrap().remove(0);
            let live = Arc::new(AtomicUsize::new(0));
            let mut timeouts = crate::fake::test_timeouts();
            // Generous: the answers wait for the test thread, which is itself waiting for the
            // other children's requests.
            timeouts.call = Duration::from_secs(30);
            let instance = crate::instance::RemoteInstance::start(
                transport,
                port,
                timeouts,
                host.messenger().clone(),
                crate::link::LiveGuard::new(&live),
                host.budget().clone(),
            )?;
            Ok(Box::new(instance))
        }
    }

    const N: usize = 4;
    mark_main_thread();
    let mut parts = Vec::new();
    let mut fakes = Vec::new();
    for _ in 0..N {
        let (transport, port, fake) =
            crate::fake::fake_transport(held_flush(), crate::fake::make_ports());
        parts.push((transport, port));
        fakes.push(fake);
    }
    let mut m = PluginManager::new();
    m.add_backend(Box::new(Several(Mutex::new(parts))));
    let mut harnesses = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..N {
        let handle = m
            .instantiate(&InstantiateRequest {
                plugin: PluginRef {
                    format: PluginFormat::Clap,
                    id: "fake".to_string(),
                },
                state: None,
                sample_rate: 48_000.0,
                mode: RunMode::Offline,
                pinned: false,
                powered: true,
            })
            .map_err(|e| e.to_string())
            .unwrap();
        ids.push(handle.id);
        let mut h = atomik_testkit::harness::DeviceHarness::new(Box::new(handle.device), 48_000.0);
        h.process();
        harnesses.push(h);
    }
    let received = |f: &Fake| {
        f.ctl
            .requests()
            .iter()
            .any(|q| matches!(q, ToChild::Flush { .. }))
    };
    let (events, all_asked_first) = std::thread::scope(|s| {
        let fakes = &fakes;
        let watcher = s.spawn(move || {
            // The condition: every child has the request while none has been answered (they all
            // hold it). Released whatever happened, so the poll always ends.
            let mut all = false;
            let until = crate::clock::now() + Duration::from_secs(20);
            while crate::clock::now() < until {
                if fakes.iter().all(received) {
                    all = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            for f in fakes {
                f.ctl.release();
            }
            all
        });
        let events = m.poll();
        (events, watcher.join().unwrap())
    });
    assert!(
        all_asked_first,
        "every child received its flush while the first was still waiting for its answer"
    );
    for id in &ids {
        assert!(
            events.contains(&ManagerEvent::StateDirty(*id)),
            "the notifications that rode before the answer were drained by the same poll"
        );
    }
    drop(harnesses);
}
