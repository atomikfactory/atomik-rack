//! The gain tool's panel in the app: the eight published values reach the view in the units the
//! panel draws, and RESET (`UiAction::PulseParam`) sends the device 1 and then 0 through the
//! engine without touching the project or the undo history.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::{Command, DeviceKey};

const GAIN_TOOL: &str = "atomik.gain-tool";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: GAIN_TOOL.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {GAIN_TOOL}: {e}"));
}

/// The values the engine was sent for `param` of device `k`, in order.
fn sent(c: &AppCore<RecordingSink>, k: &str, param: &str) -> Vec<f32> {
    let id = c.device_id(&key(k)).unwrap();
    let pid = atomik_devices::gain_tool::DESCRIPTOR
        .param_by_key(param)
        .unwrap()
        .id;
    c.sink()
        .calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::Param {
                id: i,
                param: p,
                plain,
            } if *i == id && *p == pid => Some(*plain),
            _ => None,
        })
        .collect()
}

#[test]
fn the_gain_tool_values_reach_the_view_in_the_units_of_its_panel() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "g");
    let id = c.device_id(&key("g")).unwrap();
    assert_eq!(
        atomik_ui::fx::FxKind::GainTool.meter_count(),
        atomik_devices::gain_tool::METER_COUNT
    );
    // Peaks in, peaks out (linear), the held peak (dB), then the three flags.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.25, 0.5, 0.25, -3.2, 1.0, 0.0, 1.0], 1));
    c.frame();
    let shown = c.view().device_meters(&key("g")).to_vec();
    assert_eq!(shown.len(), 8, "{shown:?}");
    assert!((shown[0] - (-6.0)).abs() < 0.6, "{shown:?}");
    assert!((shown[1] - (-12.0)).abs() < 0.6, "{shown:?}");
    assert!(
        (shown[4] - (-3.2)).abs() < 0.11,
        "held peak as published: {shown:?}"
    );
    assert_eq!(&shown[5..], &[1.0, 0.0, 1.0], "flags: {shown:?}");
}

#[test]
fn reset_sends_one_then_zero_and_leaves_the_project_and_the_history_alone() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "g");
    let before = c.doc().clone();
    c.handle_action(UiAction::PulseParam {
        device: key("g"),
        param: "peak_reset".into(),
    });
    assert_eq!(sent(&c, "g", "peak_reset"), vec![1.0], "up at once");
    c.frame();
    assert_eq!(
        sent(&c, "g", "peak_reset"),
        vec![1.0],
        "still up for a frame"
    );
    c.frame();
    assert_eq!(sent(&c, "g", "peak_reset"), vec![1.0, 0.0], "then down");
    c.frame();
    assert_eq!(sent(&c, "g", "peak_reset"), vec![1.0, 0.0], "once");
    assert_eq!(c.doc(), &before, "the project is untouched");
    // The only undo step is the one that added the device.
    assert!(c.undo());
    assert!(!c.undo(), "the pulse added no step of its own");
}

#[test]
fn a_pulse_of_something_that_is_not_a_switch_is_ignored() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "g");
    c.handle_action(UiAction::PulseParam {
        device: key("g"),
        param: "gain".into(),
    });
    c.handle_action(UiAction::PulseParam {
        device: key("ghost"),
        param: "peak_reset".into(),
    });
    c.frame();
    c.frame();
    assert!(sent(&c, "g", "gain").is_empty());
}
