//! The observed flag of the mastering device and the phase meter: nothing is sent by default,
//! and with the windowed app's setting the devices are told whether a panel shows them.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{Command, DeviceKey};

const MASTERING: &str = "atomik.mastering";
const PHASE: &str = "atomik.phase-meter";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

fn fold<S: EngineSink>(c: &mut AppCore<S>, k: &str, folded: bool) {
    c.apply_command(Command::SetFolded {
        key: key(k),
        folded,
    })
    .unwrap_or_else(|e| panic!("fold {k}: {e}"));
}

fn rack(only_shown: bool) -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    c.set_analyse_only_shown(only_shown);
    add(&mut c, MASTERING, "master-chain");
    add(&mut c, PHASE, "phase");
    c
}

fn unobserved(c: &AppCore<RecordingSink>, k: &str) -> bool {
    let id = c.device_id(&key(k)).expect("a device id");
    c.sink().unobserved.contains(&id)
}

#[test]
fn nothing_is_sent_unless_the_app_asked_for_it() {
    let mut c = rack(false);
    fold(&mut c, "master-chain", true);
    c.set_rack_window_shown(false);
    c.frame();
    assert_eq!(c.sink().observed_calls, 0);
    assert!(c.device_observed(&key("master-chain")));
    assert!(c.device_observed(&key("phase")));
    assert_eq!(c.observed_devices().len(), 2);
}

#[test]
fn a_device_is_told_when_its_panel_leaves_the_screen_and_when_it_comes_back() {
    let mut c = rack(true);
    c.frame();
    assert!(!unobserved(&c, "master-chain") && !unobserved(&c, "phase"));
    assert_eq!(c.sink().observed_calls, 0, "shown from the start: no call");
    // Folded: only that device is told.
    fold(&mut c, "master-chain", true);
    c.frame();
    assert!(unobserved(&c, "master-chain"));
    assert!(!unobserved(&c, "phase"));
    assert!(!c.device_observed(&key("master-chain")));
    // Told once, not every frame.
    let calls = c.sink().observed_calls;
    c.frame();
    c.frame();
    assert_eq!(c.sink().observed_calls, calls);
    fold(&mut c, "master-chain", false);
    c.frame();
    assert!(!unobserved(&c, "master-chain"));
    assert!(c.device_observed(&key("master-chain")));
}

#[test]
fn minimizing_the_window_tells_every_device_and_restoring_it_tells_them_back() {
    let mut c = rack(true);
    c.set_rack_window_shown(false);
    c.frame();
    assert!(unobserved(&c, "master-chain") && unobserved(&c, "phase"));
    c.set_rack_window_shown(true);
    c.frame();
    assert!(!unobserved(&c, "master-chain") && !unobserved(&c, "phase"));
}
