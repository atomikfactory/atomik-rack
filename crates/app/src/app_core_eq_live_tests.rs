//! The EQ analyser switch through the core: on for the panels in view, off for folded ones, sent
//! once per change and again after an engine restart.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::empty_doc;

const EQ: &str = "atomik.eq";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

const DYNAMIC_EQ: &str = "atomik.dynamic-eq";

fn add(core: &mut AppCore<RecordingSink>, k: &str) {
    add_of(core, EQ, k);
}

fn add_of(core: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn live_calls(core: &mut AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, f32)> {
    live_calls_of(core, EQ)
}

fn live_calls_of(core: &mut AppCore<RecordingSink>, type_id: &str) -> Vec<(DeviceInstanceId, f32)> {
    let pid = core
        .registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key("internal.analyser_live")
        .unwrap()
        .id;
    core.sink_mut()
        .take()
        .into_iter()
        .filter_map(|c| match c {
            SinkCall::Param { id, param, plain } if param == pid => Some((id, plain)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_eq_in_view_runs_its_analyser_and_a_folded_one_stops_it() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    let id = core.device_id(&key("eq")).unwrap();
    core.frame();
    assert_eq!(live_calls(&mut core), vec![(id, 1.0)]);
    // Nothing is sent again while nothing changes.
    core.frame();
    core.frame();
    assert!(live_calls(&mut core).is_empty());
    // Folded: the display is not drawn.
    core.apply_command(Command::SetFolded {
        key: key("eq"),
        folded: true,
    })
    .unwrap();
    core.frame();
    assert_eq!(live_calls(&mut core), vec![(id, 0.0)]);
    core.frame();
    assert!(live_calls(&mut core).is_empty());
    // It is UI state: never in the document.
    assert!(
        !core
            .doc()
            .device(&key("eq"))
            .unwrap()
            .params
            .contains_key("internal.analyser_live")
    );
}

#[test]
fn the_switch_is_sent_again_after_an_engine_restart() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    core.frame();
    live_calls(&mut core);
    core.rebuild_engine();
    core.frame();
    let id = core.device_id(&key("eq")).unwrap();
    assert_eq!(live_calls(&mut core), vec![(id, 1.0)]);
}

#[test]
fn a_rack_without_an_eq_sends_nothing() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    core.frame();
    assert!(live_calls(&mut core).is_empty());
}

#[test]
fn a_dynamic_eq_in_view_runs_its_analysers_and_a_folded_one_stops_them() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add_of(&mut core, DYNAMIC_EQ, "dyn");
    let id = core.device_id(&key("dyn")).unwrap();
    core.frame();
    assert_eq!(live_calls_of(&mut core, DYNAMIC_EQ), vec![(id, 1.0)]);
    core.frame();
    assert!(live_calls_of(&mut core, DYNAMIC_EQ).is_empty());
    core.apply_command(Command::SetFolded {
        key: key("dyn"),
        folded: true,
    })
    .unwrap();
    core.frame();
    assert_eq!(live_calls_of(&mut core, DYNAMIC_EQ), vec![(id, 0.0)]);
    assert!(
        !core
            .doc()
            .device(&key("dyn"))
            .unwrap()
            .params
            .contains_key("internal.analyser_live"),
        "UI state, never in the document"
    );
}

#[test]
fn an_eq_and_a_dynamic_eq_are_switched_independently() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    add_of(&mut core, DYNAMIC_EQ, "dyn");
    core.frame();
    let calls = core.sink_mut().take();
    let sent = |type_id: &str| {
        let pid = core_param(type_id);
        calls
            .iter()
            .filter(|c| matches!(c, SinkCall::Param { param, .. } if *param == pid))
            .count()
    };
    assert_eq!(sent(EQ), 1);
    assert_eq!(sent(DYNAMIC_EQ), 1);
    core.rebuild_engine();
    core.frame();
    let calls = core.sink_mut().take();
    let again = |type_id: &str| {
        let pid = core_param(type_id);
        calls
            .iter()
            .filter(|c| matches!(c, SinkCall::Param { param, .. } if *param == pid))
            .count()
    };
    assert_eq!(again(EQ), 1, "an engine restart sends it again");
    assert_eq!(again(DYNAMIC_EQ), 1);
}

fn core_param(type_id: &str) -> atomik_core::ParamId {
    let mut r = atomik_device_sdk::Registry::new();
    atomik_devices::register_all(&mut r).unwrap();
    r.get(type_id)
        .unwrap()
        .descriptor
        .param_by_key("internal.analyser_live")
        .unwrap()
        .id
}
