//! The spectrum behind the EQ's curve through the core: the scope tap of an EQ is live only while
//! its panel is on screen with the spectrum on and the device is not bypassed, the switch is sent
//! once per change and again after an engine restart, and the FFT of the tap reaches the view.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::empty_doc;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(core: &mut AppCore<RecordingSink>, k: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.eq".into(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn set(core: &mut AppCore<RecordingSink>, k: &str, param: &str, value: f32) {
    core.apply_command(Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

fn live(core: &mut AppCore<RecordingSink>, id: DeviceInstanceId) -> bool {
    core.sink_mut().scopes_live.contains(&id)
}

fn tone(freq: f32, frames: usize, head: u64) -> (Vec<f32>, Vec<f32>, u64) {
    let l: Vec<f32> = (0..frames)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / 48_000.0).sin())
        .collect();
    (l.clone(), l, head)
}

#[test]
fn the_tap_is_live_only_while_the_panel_shows_the_spectrum() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    let id = core.device_id(&key("eq")).unwrap();
    assert!(!live(&mut core, id), "off until a frame sees the panel");
    core.frame();
    assert!(live(&mut core, id), "on screen with the spectrum on");
    assert!(core.eq_spectrum_live(&key("eq")));
    // The spectrum switch.
    set(&mut core, "eq", "spectrum.on", 0.0);
    core.frame();
    assert!(!live(&mut core, id), "spectrum off");
    assert!(!core.eq_spectrum_live(&key("eq")));
    set(&mut core, "eq", "spectrum.on", 1.0);
    core.frame();
    assert!(live(&mut core, id));
    // Bypassed: the device writes nothing, so the tap is off.
    set(&mut core, "eq", "bypass", 1.0);
    core.frame();
    assert!(!live(&mut core, id), "bypassed");
    set(&mut core, "eq", "bypass", 0.0);
    core.frame();
    assert!(live(&mut core, id));
    // Folded: the panel is not drawn.
    core.apply_command(Command::SetFolded {
        key: key("eq"),
        folded: true,
    })
    .unwrap();
    core.frame();
    assert!(!live(&mut core, id), "folded");
    core.apply_command(Command::SetFolded {
        key: key("eq"),
        folded: false,
    })
    .unwrap();
    core.frame();
    assert!(live(&mut core, id), "unfolded");
}

#[test]
fn a_rack_without_an_eq_never_touches_a_tap() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    core.frame();
    assert!(core.sink_mut().scopes_live.is_empty());
}

#[test]
fn the_switch_is_sent_again_after_an_engine_restart() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    core.frame();
    core.sink_mut().scopes_live.clear();
    core.frame();
    assert!(
        core.sink_mut().scopes_live.is_empty(),
        "nothing is sent while nothing changes"
    );
    core.rebuild_engine();
    core.frame();
    let id = core.device_id(&key("eq")).unwrap();
    assert!(live(&mut core, id));
}

#[test]
fn the_fft_of_the_tap_reaches_the_graph_and_is_cleared_with_the_switch() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "eq");
    let id = core.device_id(&key("eq")).unwrap();
    core.frame();
    let trace = |core: &AppCore<RecordingSink>| {
        core.view()
            .analyser_data(&key("eq"))
            .expect("the EQ has analyser data")
            .trace
            .clone()
    };
    assert!(trace(&core).is_empty(), "nothing before the tap has frames");
    core.sink_mut()
        .device_scopes
        .insert(id, tone(1000.0, 8192, 8192));
    core.frame();
    let t = trace(&core);
    assert_eq!(t.len(), atomik_ui::analyser::TRACE_COLUMNS);
    // The loudest column is the one at 1 kHz: 20 Hz to 20 kHz is three decades.
    let loudest = t
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap();
    let hz = 20.0 * 1000f64.powf((loudest as f64 + 0.5) / t.len() as f64);
    assert!((hz / 1000.0 - 1.0).abs() < 0.1, "{hz}");
    // The same write position is not analysed again.
    core.sink_mut()
        .device_scopes
        .insert(id, tone(3000.0, 8192, 8192));
    core.frame();
    assert_eq!(trace(&core), t, "a tap that did not move costs nothing");
    // Spectrum off: the trace is cleared, so nothing stale shows when it comes back.
    set(&mut core, "eq", "spectrum.on", 0.0);
    core.frame();
    assert!(trace(&core).is_empty());
}
