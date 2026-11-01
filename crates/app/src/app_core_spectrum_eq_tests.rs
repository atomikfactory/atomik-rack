//! The Spectrum EQ window in the core: the tap switch follows the window and the strip, the
//! trace is the analysis of the tap, a drag of a point is one undo step, the master has no EQ.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_dsp::spectrum::{Settings, Spectrum};
use atomik_model::console::TAP_LIVE_PARAM;
use atomik_model::{Command, DeviceKey};
use atomik_ui::spectrum_eq::PointId;
use atomik_ui::{Modifiers, PointerButton, UiEvent};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn two_channels() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .expect("instrument added");
    }
    c.sink_mut().take();
    c
}

fn channel_of(c: &AppCore<RecordingSink>, source: &str) -> DeviceKey {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("automatic channel")
        .channel
        .clone()
}

/// The `internal.tap_live` calls in `calls` as (device, value).
fn tap_calls_of(
    c: &AppCore<RecordingSink>,
    type_id: &str,
    calls: &[SinkCall],
) -> Vec<(DeviceInstanceId, f32)> {
    let p = c
        .registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(TAP_LIVE_PARAM)
        .unwrap()
        .id;
    calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::Param { id, param, plain } if *param == p => Some((*id, *plain)),
            _ => None,
        })
        .collect()
}

fn tap_calls(c: &AppCore<RecordingSink>, calls: &[SinkCall]) -> Vec<(DeviceInstanceId, f32)> {
    tap_calls_of(c, CHANNEL_TYPE, calls)
}

fn tone(hz: f64, frames: usize) -> (Vec<f32>, Vec<f32>, u64) {
    let l: Vec<f32> = (0..frames)
        .map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin()) as f32)
        .collect();
    (l.clone(), l, frames as u64)
}

#[test]
fn the_tap_is_on_only_for_the_shown_strip_and_only_while_the_window_is_open() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let (ia, ib) = (c.device_id(&a).unwrap(), c.device_id(&b).unwrap());
    c.frame();
    assert!(
        taken(&mut c).is_empty(),
        "a closed window costs the engine nothing"
    );
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.set_spectrum_eq_shown(true);
    assert_eq!(c.spectrum_eq_strip(), Some(&a));
    assert_eq!(taken(&mut c), [(ia, 1.0)]);
    // A frame with nothing new sends nothing again.
    c.frame();
    assert!(taken(&mut c).is_empty());
    // Following the selection moves the tap with it: off for the old strip, on for the new one.
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&b));
    c.frame();
    assert_eq!(c.spectrum_eq_strip(), Some(&b));
    assert_eq!(taken(&mut c), [(ia, 0.0), (ib, 1.0)]);
    // Closing switches it off.
    c.set_spectrum_eq_shown(false);
    assert_eq!(taken(&mut c), [(ib, 0.0)]);
    c.frame();
    assert!(taken(&mut c).is_empty());
}

#[test]
fn the_arrows_pick_a_strip_and_stop_following() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.set_spectrum_eq_shown(true);
    assert!(c.spectrum_eq_follows());
    c.handle_action(UiAction::SpectrumEqStep(1));
    assert_eq!(c.spectrum_eq_strip(), Some(&b));
    assert!(!c.spectrum_eq_follows());
    // The selection no longer moves it.
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.frame();
    assert_eq!(c.spectrum_eq_strip(), Some(&b));
    c.handle_action(UiAction::SpectrumEqFollow(true));
    assert_eq!(c.spectrum_eq_strip(), Some(&a));
    // Stepping wraps through the channels and the master.
    c.handle_action(UiAction::SpectrumEqStep(-1));
    assert_eq!(c.spectrum_eq_strip(), c.doc().console.master.as_ref());
}

#[test]
fn the_tap_switch_is_never_stored_and_never_an_undo_step() {
    let mut c = two_channels();
    let depth = c.document().undo_depth();
    c.set_spectrum_eq_shown(true);
    c.frame();
    assert_eq!(c.document().undo_depth(), depth);
    for dev in &c.doc().devices {
        assert!(!dev.params.contains_key(TAP_LIVE_PARAM), "{}", dev.key);
    }
}

#[test]
fn the_tap_is_switched_on_again_after_the_engine_is_rebuilt() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.set_spectrum_eq_shown(true);
    c.sink_mut().take();
    c.rebuild_engine();
    c.frame();
    let ia = c.device_id(&a).unwrap();
    assert!(taken(&mut c).contains(&(ia, 1.0)));
}

#[test]
fn the_trace_is_the_offline_analysis_of_the_taps_frames() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    let ia = c.device_id(&a).unwrap();
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.set_spectrum_eq_shown(true);
    let frames = tone(1_000.0, 4096);
    c.sink_mut().device_scopes.insert(ia, frames.clone());
    c.frame();
    let shown = c.spectrum_eq().trace().to_vec();
    assert_eq!(shown.len(), atomik_ui::analyser::TRACE_COLUMNS);
    // The same frames through the same analysis, offline.
    let mut offline = Spectrum::new();
    offline.update(
        &frames.0,
        &frames.1,
        frames.2,
        48_000.0,
        &Settings::default(),
        atomik_ui::analyser::TRACE_COLUMNS,
    );
    assert_eq!(shown, offline.trace());
    // And it finds the tone: the loudest column sits at 1 kHz on the log axis.
    let loudest = shown
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .unwrap()
        .0;
    let hz = 20.0 * 1000f64.powf((loudest as f64 + 0.5) / shown.len() as f64);
    assert!((hz / 1_000.0 - 1.0).abs() < 0.05, "{hz}");
}

fn press(c: &mut AppCore<RecordingSink>, pos: atomik_ui::kurbo::Point) {
    c.handle_pane_event(
        Pane::SpectrumEq,
        &UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
            clicks: 1,
        },
    );
}

fn drag(c: &mut AppCore<RecordingSink>, pos: atomik_ui::kurbo::Point) {
    c.handle_pane_event(
        Pane::SpectrumEq,
        &UiEvent::PointerMove {
            pos,
            modifiers: Modifiers::default(),
        },
    );
}

fn release(c: &mut AppCore<RecordingSink>, pos: atomik_ui::kurbo::Point) {
    c.handle_pane_event(
        Pane::SpectrumEq,
        &UiEvent::PointerUp {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        },
    );
}

fn param(c: &AppCore<RecordingSink>, dev: &DeviceKey, p: &str) -> f32 {
    c.doc().param_value(dev, p, c.registry()).unwrap()
}

#[test]
fn dragging_the_hmf_point_edits_the_strip_in_one_undo_step() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&a));
    c.set_spectrum_eq_shown(true);
    c.frame();
    let depth = c.document().undo_depth();
    let (f0, g0) = (param(&c, &a, "eq.hmf.freq"), param(&c, &a, "eq.hmf.gain"));
    let start = c.spectrum_eq().point(PointId::band(1)).unwrap();
    press(&mut c, start);
    for i in 1..=6 {
        let t = f64::from(i) / 6.0;
        drag(
            &mut c,
            atomik_ui::kurbo::Point::new(start.x + 70.0 * t, start.y - 40.0 * t),
        );
        c.frame();
    }
    release(
        &mut c,
        atomik_ui::kurbo::Point::new(start.x + 70.0, start.y - 40.0),
    );
    assert!(param(&c, &a, "eq.hmf.freq") > f0);
    assert!(param(&c, &a, "eq.hmf.gain") > g0);
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    // The view follows the document: the point moved with the params.
    let moved = c.spectrum_eq().point(PointId::band(1)).unwrap();
    assert!(moved.x > start.x && moved.y < start.y);
    c.undo();
    assert_eq!(param(&c, &a, "eq.hmf.freq"), f0);
    assert_eq!(param(&c, &a, "eq.hmf.gain"), g0);
}

#[test]
fn the_master_shows_the_analyser_and_no_points() {
    let mut c = two_channels();
    let master = c.doc().console.master.clone().unwrap();
    // The master is no selectable strip: the arrows reach it.
    let a = channel_of(&c, "a");
    c.console_mut().set_selected_strips(&[a]);
    c.set_spectrum_eq_shown(true);
    c.handle_action(UiAction::SpectrumEqStep(-1));
    assert_eq!(c.spectrum_eq_strip(), Some(&master));
    assert!(c.spectrum_eq().state().unwrap().master);
    assert!(c.spectrum_eq().point(PointId::band(1)).is_none());
    // The master's tap is switched on like a channel's.
    let calls = c.sink_mut().take();
    let id = c.device_id(&master).unwrap();
    assert!(tap_calls_of(&c, "atomik.master", &calls).contains(&(id, 1.0)));
}

/// The `internal.tap_live` calls made since the last look.
fn taken(c: &mut AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, f32)> {
    let calls = c.sink_mut().take();
    tap_calls(c, &calls)
}
