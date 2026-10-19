//! `AppCore` console sync: solo silence, compensation, meters, latency
//! epochs and PDC readouts, with the recording sink and the full device registry (automatic
//! channels on).

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_engine::MeterReading;
use atomik_model::console::SOLO_SILENCE_PARAM;
use atomik_model::{Command, ConsoleCommand, DeviceKey};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add_synth(c: &mut AppCore<RecordingSink>, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.into(),
        key: Some(key(k)),
    })
    .expect("instrument added");
}

/// Two synths `a` and `b`, each with its automatic channel, and the master.
fn two_channels() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    add_synth(&mut c, "a");
    add_synth(&mut c, "b");
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

fn master(c: &AppCore<RecordingSink>) -> DeviceKey {
    c.doc().console.master.clone().expect("master")
}

fn pid(c: &AppCore<RecordingSink>, type_id: &str, param: &str) -> ParamId {
    c.registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
}

fn endpoint_of(c: &AppCore<RecordingSink>, device: &str, socket: &str) -> Endpoint {
    let socket = SocketRef {
        device: key(device),
        socket: socket.to_string(),
    };
    c.endpoint(c.topology(), &socket).unwrap()
}

fn endpoint(c: &AppCore<RecordingSink>, device: &DeviceKey, socket: &str) -> Endpoint {
    let id = c.device_id(device).unwrap();
    Endpoint {
        device: id,
        socket: c
            .topology()
            .descriptor(id)
            .unwrap()
            .socket_by_key(socket)
            .unwrap()
            .id,
    }
}

fn silence_calls(c: &AppCore<RecordingSink>, calls: &[SinkCall]) -> Vec<(DeviceInstanceId, f32)> {
    let p = pid(c, CHANNEL_TYPE, SOLO_SILENCE_PARAM);
    calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::Param { id, param, plain } if *param == p => Some((*id, *plain)),
            _ => None,
        })
        .collect()
}

fn set_param(c: &mut AppCore<RecordingSink>, device: &DeviceKey, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: device.clone(),
        param: param.into(),
        value,
        merge: None,
    })
    .expect("param set");
}

#[test]
fn every_routing_meters_the_channel_and_master_outputs() {
    let c = two_channels();
    // The Audio I/O sockets are metered too: the host inputs at the
    // interface and, for the outputs, the sources of the cables that end there (the master's
    // `main`, already metered for the strip). Nothing else is.
    let interface = |c: &AppCore<RecordingSink>| -> Vec<Endpoint> {
        (1..=c.doc().input_channels)
            .map(|k| endpoint_of(c, "interface", &format!("in_{k}")))
            .collect()
    };
    let meters: Vec<Endpoint> = c
        .topology()
        .meters()
        .filter(|e| !interface(&c).contains(e))
        .collect();
    let mut want = Vec::new();
    for ch in [channel_of(&c, "a"), channel_of(&c, "b")] {
        for s in ["out_l", "out_r", "gr_cv_out"] {
            want.push(endpoint(&c, &ch, s));
        }
    }
    for s in ["main_l", "main_r", "gr_cv_out"] {
        want.push(endpoint(&c, &master(&c), s));
    }
    want.sort();
    assert_eq!(meters, want);
}

#[test]
fn soloing_a_channel_silences_the_others_through_the_engine_only() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    set_param(&mut c, &a, "solo", 1.0);
    let calls = c.sink_mut().take();
    let (ia, ib) = (c.device_id(&a).unwrap(), c.device_id(&b).unwrap());
    assert_eq!(silence_calls(&c, &calls), vec![(ia, 0.0), (ib, 1.0)]);
    // Never stored in the document (not saved, not undoable).
    assert!(
        c.doc()
            .devices
            .iter()
            .all(|d| !d.params.contains_key(SOLO_SILENCE_PARAM))
    );
    // Undo un-solos: everybody plays again.
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(silence_calls(&c, &calls), vec![(ia, 0.0), (ib, 0.0)]);
}

#[test]
fn a_rebuilt_engine_gets_the_solo_silence_after_the_routing() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    set_param(&mut c, &a, "solo", 1.0);
    c.sink_mut().take();
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let b = c.device_id(&channel_of(&c, "b")).unwrap();
    let routing = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Routing(_)))
        .unwrap();
    let silence = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Param { id, plain, .. } if *id == b && *plain > 0.5))
        .expect("b silenced after the rebuild");
    assert!(routing < silence, "{calls:?}");
}

#[test]
fn compensation_off_recompiles_the_routing_without_compensation() {
    let mut c = two_channels();
    assert!(c.topology().compensation());
    c.apply_command(Command::Console(ConsoleCommand::SetCompensation(false)))
        .unwrap();
    let calls = c.sink_mut().take();
    assert!(
        calls.iter().any(|x| matches!(x, SinkCall::Routing(_))),
        "{calls:?}"
    );
    assert!(!c.topology().compensation());
    assert!(!c.sink().pdc.as_ref().unwrap().active);
    // A solo change alone does not recompile.
    let a = channel_of(&c, "a");
    set_param(&mut c, &a, "solo", 1.0);
    let calls = c.sink_mut().take();
    assert!(
        !calls.iter().any(|x| matches!(x, SinkCall::Routing(_))),
        "{calls:?}"
    );
}

#[test]
fn a_latency_epoch_change_refills_latencies_and_resubmits_the_routing() {
    let mut c = two_channels();
    c.frame();
    assert!(
        !c.sink_mut()
            .take()
            .iter()
            .any(|x| matches!(x, SinkCall::Routing(_))),
        "no change, no routing"
    );
    let a = c.device_id(&channel_of(&c, "a")).unwrap();
    c.sink_mut().latencies.insert(a, 480);
    c.sink_mut().epoch += 1;
    c.frame();
    let calls = c.sink_mut().take();
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::Routing(_)))
            .count(),
        1,
        "{calls:?}"
    );
    assert_eq!(c.topology().latency(a), Some(480));
    c.frame();
    assert!(
        !c.sink_mut()
            .take()
            .iter()
            .any(|x| matches!(x, SinkCall::Routing(_))),
        "the epoch was seen: no second routing"
    );
}

#[test]
fn latency_readouts_are_each_channels_output_latency() {
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let a = c.device_id(&ka).unwrap();
    c.sink_mut().latencies.insert(a, 480);
    c.sink_mut().epoch += 1;
    c.frame();
    assert_eq!(c.latency_readouts(), &[(ka, 480), (kb, 0)][..]);
    assert!(!c.pdc_limit_exceeded());
    assert_eq!(c.pdc_alert(), None);
}

#[test]
fn a_delay_over_ten_seconds_disables_compensation_with_an_alert() {
    let mut c = two_channels();
    let a = c.device_id(&channel_of(&c, "a")).unwrap();
    c.sink_mut().latencies.insert(a, 10 * 48_000 + 1);
    c.sink_mut().epoch += 1;
    c.frame();
    assert!(c.pdc_limit_exceeded());
    let alert = c.pdc_alert().expect("alert");
    assert!(alert.contains("10 s"), "{alert}");
    c.sink_mut().latencies.insert(a, 0);
    c.sink_mut().epoch += 1;
    c.frame();
    assert_eq!(c.pdc_alert(), None);
}

#[test]
fn meters_read_peak_rms_and_gain_reduction_per_strip() {
    let mut c = two_channels();
    let ka = channel_of(&c, "a");
    let m = master(&c);
    let reading = |peak, rms| MeterReading { peak, rms };
    let (l, r, gr) = (
        endpoint(&c, &ka, "out_l"),
        endpoint(&c, &ka, "out_r"),
        endpoint(&c, &ka, "gr_cv_out"),
    );
    let ml = endpoint(&c, &m, "main_l");
    let sink = c.sink_mut();
    sink.meters.insert(l, reading(0.5, 0.25));
    sink.meters.insert(r, reading(0.4, 0.2));
    sink.meters.insert(gr, reading(0.75, 0.5));
    sink.meters.insert(ml, reading(0.9, 0.3));
    let got = c.console_meter_readings();
    let find = |k: &DeviceKey| got.iter().find(|(key, _)| key == k).unwrap().1;
    let a = find(&ka);
    assert_eq!(
        (a.out_peak_l, a.out_peak_r, a.out_rms_l, a.out_rms_r),
        (0.5, 0.4, 0.25, 0.2)
    );
    // gr_cv_out = 1 − gain; its peak is the deepest reduction since the last frame.
    assert!((a.gr - 0.25).abs() < 1e-6, "{a:?}");
    let b = find(&channel_of(&c, "b"));
    assert_eq!(b, atomik_ui::ChannelMeters::SILENT, "nothing published yet");
    let mm = find(&m);
    assert_eq!((mm.out_peak_l, mm.gr), (0.9, 1.0));
    assert_eq!(got.len(), 3, "two channels and the master");
    // Feeding the console view advances its ballistics.
    c.update_console_meters(0.0);
    c.update_console_meters(0.1);
    assert!(c.console().meter(&ka).is_some());
}

#[test]
fn the_console_pane_routes_events_and_paints() {
    let mut c = two_channels();
    c.console_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 300.0, 1200.0, 800.0));
    let mut list = atomik_ui::DisplayList::new();
    c.paint_pane(Pane::Console, &mut list);
    assert_eq!(c.console().strip_keys().len(), 2);
    let actions = c.handle_pane_event(
        Pane::Console,
        &UiEvent::Key {
            key: atomik_ui::Key::F(5),
            modifiers: atomik_ui::Modifiers::NONE,
            pressed: true,
        },
    );
    assert!(actions.contains(&UiAction::ToggleConsole), "{actions:?}");
}

#[test]
fn device_meter_values_are_read_by_key_through_the_sink() {
    let mut c = two_channels();
    let mut out = [0.0f32; 4];
    assert_eq!(c.device_meter_values(&key("a"), &mut out), None);
    let id = c.device_id(&key("a")).unwrap();
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.25, 0.125], 7));
    assert_eq!(c.device_meter_values(&key("a"), &mut out), Some((3, 7)));
    assert_eq!(out, [0.5, 0.25, 0.125, 0.0]);
    let mut two = [0.0f32; 2];
    assert_eq!(c.device_meter_values(&key("a"), &mut two), Some((2, 7)));
    assert_eq!(c.device_meter_values(&key("nope"), &mut out), None);
    assert_eq!(c.device_meter_values(&key("b"), &mut out), None);
}
