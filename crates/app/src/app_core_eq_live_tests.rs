//! The EQ analyser switch through the core: on for the panels in view, off for folded ones, sent
//! once per change and again after an engine restart.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::empty_doc;

const EQ: &str = "atomik.eq";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(core: &mut AppCore<RecordingSink>, k: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: EQ.into(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn live_calls(core: &mut AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, f32)> {
    let pid = core
        .registry()
        .get(EQ)
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
