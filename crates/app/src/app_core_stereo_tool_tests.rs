//! The stereo tool's panel in the app: the five published values reach the view in the units the
//! panel draws (the two correlations signed, the three flags 0 or 1).

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{Command, DeviceKey};

const STEREO_TOOL: &str = "atomik.stereo-tool";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: STEREO_TOOL.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {STEREO_TOOL}: {e}"));
}

#[test]
fn the_stereo_tool_values_reach_the_view_in_the_units_of_its_panel() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "s");
    let id = c.device_id(&key("s")).unwrap();
    assert_eq!(
        atomik_ui::fx::FxKind::StereoTool.meter_count(),
        atomik_devices::stereo_tool::METER_COUNT
    );
    // Low and high correlation, the two ACTIVE flags and the split flag.
    c.sink_mut()
        .device_values
        .insert(id, (vec![-0.347, 0.62, 1.0, 0.0, 1.0], 1));
    c.frame();
    let shown = c.view().device_meters(&key("s")).to_vec();
    assert_eq!(shown, vec![-0.35, 0.62, 1.0, 0.0, 1.0], "{shown:?}");
    // A later frame replaces them (a negative correlation keeps its sign).
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.0, -1.0, 0.0, 1.0, 0.0], 2));
    c.frame();
    let shown = c.view().device_meters(&key("s")).to_vec();
    assert_eq!(shown, vec![0.0, -1.0, 0.0, 1.0, 0.0], "{shown:?}");
}
