//! `AppCore` console sync: solo silence, compensation, meters, latency
//! epochs and PDC readouts, with the recording sink and the full device registry (automatic
//! channels on).

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_engine::MeterReading;
use atomik_model::console::{SOLO_SILENCE_PARAM, VCA_DB_PARAM, VCA_MUTE_PARAM, VCA_SOLO_PARAM};
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

fn create_bus(c: &mut AppCore<RecordingSink>, name: &str) -> DeviceKey {
    let bus = atomik_model::console::new_bus_key(c.doc());
    c.apply_command(Command::Console(ConsoleCommand::CreateBus {
        name: name.into(),
        dest: atomik_model::console::OutputDest::Master,
    }))
    .expect("bus created");
    bus
}

#[test]
fn buses_are_metered_like_channels() {
    let mut c = two_channels();
    let bus = create_bus(&mut c, "Drums");
    let reading = |peak, rms| MeterReading { peak, rms };
    let (l, r, gr) = (
        endpoint(&c, &bus, "out_l"),
        endpoint(&c, &bus, "out_r"),
        endpoint(&c, &bus, "gr_cv_out"),
    );
    let sink = c.sink_mut();
    sink.meters.insert(l, reading(0.5, 0.25));
    sink.meters.insert(r, reading(0.4, 0.2));
    sink.meters.insert(gr, reading(0.75, 0.5));
    let got = c.console_meter_readings();
    assert_eq!(got.len(), 4, "two channels, the bus and the master");
    let b = got.iter().find(|(k, _)| *k == bus).expect("bus reading").1;
    assert_eq!((b.out_peak_l, b.out_peak_r), (0.5, 0.4));
    assert!((b.gr - 0.25).abs() < 1e-6, "{b:?}");
    c.update_console_meters(0.0);
    c.update_console_meters(0.1);
    assert!(
        c.console().meter(&bus).is_some(),
        "the console view meters the bus"
    );
}

#[test]
fn a_bus_has_a_latency_readout_like_a_channel() {
    let mut c = two_channels();
    let bus = create_bus(&mut c, "Drums");
    assert!(
        c.latency_readouts().iter().any(|(k, _)| *k == bus),
        "{:?}",
        c.latency_readouts()
    );
}

fn dest_of(c: &AppCore<RecordingSink>, strip: &DeviceKey) -> atomik_model::console::OutputDest {
    atomik_model::console::output_dest(c.doc(), strip)
}

#[test]
fn the_selected_instruments_are_routed_to_a_new_bus_in_one_undo_step() {
    use atomik_model::console::OutputDest;
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("b")));
    let new_bus = atomik_model::console::new_bus_key(c.doc());
    assert!(c.route_selection_to_new_bus(false));
    assert_eq!(c.doc().console.buses.len(), 1);
    assert_eq!(c.doc().console.buses[0].bus, new_bus);
    assert_eq!(c.doc().console.buses[0].name, "Bus 1");
    assert_eq!(dest_of(&c, &ka), OutputDest::Bus(new_bus.clone()));
    assert_eq!(dest_of(&c, &kb), OutputDest::Bus(new_bus.clone()));
    assert_eq!(dest_of(&c, &new_bus), OutputDest::Master);
    assert_eq!(
        c.console().selected_strips(),
        std::slice::from_ref(&new_bus)
    );
    assert!(c.undo());
    assert!(c.doc().console.buses.is_empty(), "one undo step");
    assert_eq!(dest_of(&c, &ka), OutputDest::Master);
    // A second route makes the next default name.
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(c.route_selection_to_new_bus(false));
    assert!(c.route_selection_to_new_bus(false));
    let names: Vec<&str> = c
        .doc()
        .console
        .buses
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(names, ["Bus 1", "Bus 2"]);
}

#[test]
fn the_console_selection_is_routed_when_the_console_has_the_focus() {
    use atomik_model::console::OutputDest;
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&kb));
    assert!(c.route_selection_to_new_bus(true));
    let bus = c.doc().console.buses[0].bus.clone();
    assert_eq!(dest_of(&c, &kb), OutputDest::Bus(bus));
    assert_eq!(
        dest_of(&c, &ka),
        OutputDest::Master,
        "the rack selection was not used"
    );
    // With nothing selected in the console, the rack selection is used.
    let mut c = two_channels();
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(c.route_selection_to_new_bus(true));
    assert!(matches!(
        dest_of(&c, &channel_of(&c, "a")),
        OutputDest::Bus(_)
    ));
}

#[test]
fn a_selected_bus_can_be_routed_to_a_new_bus_too() {
    use atomik_model::console::OutputDest;
    let mut c = two_channels();
    let first = create_bus(&mut c, "Drums");
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&first));
    assert!(c.route_selection_to_new_bus(true));
    let second = c.doc().console.buses[1].bus.clone();
    assert_eq!(dest_of(&c, &first), OutputDest::Bus(second));
}

#[test]
fn routing_without_a_selection_says_what_to_select() {
    let mut c = two_channels();
    c.view_mut().set_selection(&[], None);
    assert!(!c.route_selection_to_new_bus(false));
    assert!(c.doc().console.buses.is_empty());
    let msg = c.last_rejection().expect("a message");
    assert!(msg.contains("Select") && msg.contains("channel"), "{msg}");
}

#[test]
fn a_rejected_console_command_is_shown_in_the_console_too() {
    let mut c = two_channels();
    create_bus(&mut c, "Drums");
    let second = create_bus(&mut c, "Synths");
    assert!(
        c.apply_command(Command::Console(ConsoleCommand::RenameBus {
            bus: second,
            name: "drums".into(),
        }))
        .is_err()
    );
    let notice = c.console().notice().expect("a console notice").to_string();
    assert!(notice.contains("already called"), "{notice}");
}

// ---- VCA groups ----------------------------------------------------------------------------

fn vca_calls(
    c: &AppCore<RecordingSink>,
    calls: &[SinkCall],
    param: &str,
) -> Vec<(DeviceInstanceId, f32)> {
    let p = pid(c, CHANNEL_TYPE, param);
    calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::Param { id, param, plain } if *param == p => Some((*id, *plain)),
            _ => None,
        })
        .collect()
}

fn vca(c: &mut AppCore<RecordingSink>, command: ConsoleCommand) {
    c.apply_command(Command::Console(command))
        .expect("VCA command applied");
}

fn create_vca(c: &mut AppCore<RecordingSink>, members: &[&DeviceKey]) {
    vca(
        c,
        ConsoleCommand::CreateVca {
            name: "Group".into(),
            members: members.iter().map(|k| (*k).clone()).collect(),
        },
    );
}

#[test]
fn a_group_level_reaches_only_its_members_through_the_engine_only() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    create_vca(&mut c, &[&a]);
    let calls = c.sink_mut().take();
    assert!(
        vca_calls(&c, &calls, VCA_DB_PARAM).is_empty(),
        "a group at 0 dB sends nothing: {calls:?}"
    );
    vca(
        &mut c,
        ConsoleCommand::SetVcaLevel {
            vca: 1,
            db: -6.0,
            merge: None,
        },
    );
    let calls = c.sink_mut().take();
    let ia = c.device_id(&a).unwrap();
    assert_eq!(vca_calls(&c, &calls, VCA_DB_PARAM), vec![(ia, -6.0)]);
    // Never stored in the document.
    assert!(
        c.doc()
            .devices
            .iter()
            .all(|d| { d.params.keys().all(|k| !k.starts_with("internal.")) })
    );
    // A drag is one param per move, the other strip is left alone.
    for db in [-7.0, -8.0] {
        vca(
            &mut c,
            ConsoleCommand::SetVcaLevel {
                vca: 1,
                db,
                merge: Some(1),
            },
        );
    }
    let calls = c.sink_mut().take();
    assert_eq!(
        vca_calls(&c, &calls, VCA_DB_PARAM),
        vec![(ia, -7.0), (ia, -8.0)]
    );
    // Undo puts the strip back to the start of the drag, and a removal from the group resets it.
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(vca_calls(&c, &calls, VCA_DB_PARAM), vec![(ia, -6.0)]);
    vca(
        &mut c,
        ConsoleCommand::AssignToVca {
            vca: None,
            strips: vec![a.clone()],
        },
    );
    let calls = c.sink_mut().take();
    assert_eq!(vca_calls(&c, &calls, VCA_DB_PARAM), vec![(ia, 0.0)]);
    assert!(vca_calls(&c, &calls, VCA_MUTE_PARAM).is_empty());
    let _ = b;
}

#[test]
fn a_group_mute_and_solo_reach_the_members_and_the_solo_silences_the_others() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    create_vca(&mut c, &[&a]);
    c.sink_mut().take();
    let (ia, ib) = (c.device_id(&a).unwrap(), c.device_id(&b).unwrap());
    vca(&mut c, ConsoleCommand::SetVcaMute { vca: 1, on: true });
    let calls = c.sink_mut().take();
    assert_eq!(vca_calls(&c, &calls, VCA_MUTE_PARAM), vec![(ia, 1.0)]);
    vca(&mut c, ConsoleCommand::SetVcaSolo { vca: 1, on: true });
    let calls = c.sink_mut().take();
    assert_eq!(vca_calls(&c, &calls, VCA_SOLO_PARAM), vec![(ia, 1.0)]);
    assert_eq!(silence_calls(&c, &calls), vec![(ia, 0.0), (ib, 1.0)]);
    // Clearing the solos clears the group's solo too.
    vca(&mut c, ConsoleCommand::ClearSolos);
    let calls = c.sink_mut().take();
    assert_eq!(vca_calls(&c, &calls, VCA_SOLO_PARAM), vec![(ia, 0.0)]);
    assert_eq!(silence_calls(&c, &calls), vec![(ia, 0.0), (ib, 0.0)]);
}

#[test]
fn a_rebuilt_engine_and_a_restored_strip_get_the_feed_again() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    create_vca(&mut c, &[&a]);
    vca(
        &mut c,
        ConsoleCommand::SetVcaLevel {
            vca: 1,
            db: -3.0,
            merge: None,
        },
    );
    c.sink_mut().take();
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let ia = c.device_id(&a).unwrap();
    assert_eq!(vca_calls(&c, &calls, VCA_DB_PARAM), vec![(ia, -3.0)]);
    // Removing the instrument removes its channel and its membership; undo brings the
    // strip back as a new engine device that must get the offset again.
    c.apply_command(Command::Workflow(
        atomik_model::WorkflowCommand::RemoveDevices {
            keys: vec![key("a")],
        },
    ))
    .unwrap();
    assert!(c.doc().console.vca(1).unwrap().members.is_empty());
    c.sink_mut().take();
    assert!(c.undo());
    let calls = c.sink_mut().take();
    let ia = c.device_id(&a).unwrap();
    assert_eq!(vca_calls(&c, &calls, VCA_DB_PARAM), vec![(ia, -3.0)]);
}

#[test]
fn create_vca_from_selection_uses_the_focused_selection_in_one_undo_step() {
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    // The console's selection wins when the console has the focus.
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    c.console_mut()
        .set_selected_strips(std::slice::from_ref(&kb));
    assert!(c.create_vca_from_selection(Pane::Console));
    let g = c.doc().console.vca(1).expect("a group");
    assert_eq!(
        (g.name.as_str(), g.members.clone()),
        ("VCA 1", vec![kb.clone()])
    );
    assert!(c.undo());
    assert!(c.doc().console.vcas.is_empty(), "one undo step");
    // From the rack, an instrument stands for its channel, and strips leave their groups.
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("a")));
    assert!(c.create_vca_from_selection(Pane::Rack));
    assert_eq!(c.doc().console.vca(1).unwrap().members, vec![ka, kb]);
    assert_eq!(c.doc().console.default_vca_name(), "VCA 2");
}

#[test]
fn the_vca_commands_say_what_to_select_and_what_is_missing() {
    let mut c = two_channels();
    c.view_mut().set_selection(&[], None);
    assert!(!c.create_vca_from_selection(Pane::Rack));
    assert!(c.last_rejection().unwrap().contains("Select"));
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(!c.add_selection_to_vca(Pane::Rack, None));
    assert!(c.last_rejection().unwrap().contains("no VCA group"));
    assert!(
        !c.remove_selection_from_vca(Pane::Rack),
        "a strip in no group changes nothing"
    );
    assert!(!c.delete_vca(Pane::Rack, None));
    assert!(c.last_rejection().unwrap().contains("Select a channel"));
    assert!(!c.vca_menu(Pane::Rack).can_add && c.vca_menu(Pane::Rack).can_create);
    assert!(!c.vca_menu(Pane::Rack).can_delete);
}

#[test]
fn add_remove_and_delete_follow_the_selection_and_the_named_group() {
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    create_vca(&mut c, &[&ka]);
    vca(
        &mut c,
        ConsoleCommand::CreateVca {
            name: "Other".into(),
            members: Vec::new(),
        },
    );
    c.view_mut().set_selection(&[key("b")], Some(&key("b")));
    let menu = c.vca_menu(Pane::Rack);
    assert!(menu.can_create && menu.can_add && menu.can_delete && !menu.can_remove);
    // No id: the last group; a named group: that one.
    assert!(c.add_selection_to_vca(Pane::Rack, None));
    assert_eq!(c.doc().console.vca(2).unwrap().members, vec![kb.clone()]);
    assert!(c.add_selection_to_vca(Pane::Rack, Some(1)));
    assert_eq!(
        c.doc().console.vca(1).unwrap().members,
        vec![ka.clone(), kb.clone()]
    );
    assert!(c.vca_menu(Pane::Rack).can_remove);
    assert!(c.remove_selection_from_vca(Pane::Rack));
    assert!(c.doc().console.vca_of(&kb).is_none());
    // Delete takes the groups of the selection, or the named one.
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(c.delete_vca(Pane::Rack, None));
    assert!(c.doc().console.vca(1).is_none() && c.doc().console.vca(2).is_some());
    assert!(c.delete_vca(Pane::Rack, Some(2)));
    assert!(c.doc().console.vcas.is_empty());
    assert!(!c.delete_vca(Pane::Rack, Some(2)), "gone");
}

#[test]
fn the_arrangement_selection_is_used_through_its_tracks_channels() {
    let mut c = two_channels();
    for k in ["a", "b"] {
        c.apply_command(Command::Song(atomik_model::SongCommand::AddTrack {
            device: key(k),
        }))
        .unwrap();
    }
    c.arrangement_mut().set_selected_track(Some(key("b")));
    assert!(c.create_vca_from_selection(Pane::Arrangement));
    assert_eq!(
        c.doc().console.vca(1).unwrap().members,
        vec![channel_of(&c, "b")]
    );
}

// ------------------------------------------------------------------ channel strip upgrades

fn phase_of(c: &AppCore<RecordingSink>, strip: &DeviceKey) -> f32 {
    c.doc()
        .device(strip)
        .and_then(|d| d.params.get("input.phase").copied())
        .unwrap_or(0.0)
}

#[test]
fn toggle_polarity_flips_the_selected_strips_in_one_undo_step() {
    let mut c = two_channels();
    let (ka, kb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("a")));
    assert!(c.strip_menu(Pane::Rack).can_polarity);
    assert!(c.toggle_polarity(Pane::Rack));
    assert_eq!((phase_of(&c, &ka), phase_of(&c, &kb)), (1.0, 1.0));
    assert!(c.undo());
    assert_eq!(
        (phase_of(&c, &ka), phase_of(&c, &kb)),
        (0.0, 0.0),
        "both strips in one step"
    );
    assert!(c.redo());
    // All inverted: back to normal. Mixed: all become inverted.
    assert!(c.toggle_polarity(Pane::Rack));
    assert_eq!((phase_of(&c, &ka), phase_of(&c, &kb)), (0.0, 0.0));
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(c.toggle_polarity(Pane::Rack));
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("a")));
    assert!(c.toggle_polarity(Pane::Rack));
    assert_eq!((phase_of(&c, &ka), phase_of(&c, &kb)), (1.0, 1.0));
}

#[test]
fn toggle_polarity_reaches_the_engine_and_says_what_to_select() {
    let mut c = two_channels();
    c.view_mut().set_selection(&[], None);
    assert!(!c.strip_menu(Pane::Rack).can_polarity);
    assert!(!c.toggle_polarity(Pane::Rack));
    assert!(c.last_rejection().unwrap().contains("Select"));
    let ka = channel_of(&c, "a");
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    c.sink_mut().take();
    assert!(c.toggle_polarity(Pane::Rack));
    let calls = c.sink_mut().take();
    let id = c.device_id(&ka).unwrap();
    assert_eq!(vca_calls(&c, &calls, "input.phase"), vec![(id, 1.0)]);
}

/// `a`'s channel with two reverbs as inserts.
fn with_two_inserts() -> (AppCore<RecordingSink>, DeviceKey) {
    let mut c = two_channels();
    for k in ["fx1", "fx2"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.reverb".into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    let ka = channel_of(&c, "a");
    for (slot, k) in ["fx1", "fx2"].into_iter().enumerate() {
        c.apply_command(Command::Console(ConsoleCommand::SetInsert {
            channel: ka.clone(),
            slot,
            effect: Some(atomik_model::InsertEffect::Existing(key(k))),
        }))
        .unwrap();
    }
    (c, ka)
}

fn bypass_of(c: &AppCore<RecordingSink>, effect: &str) -> f32 {
    c.doc()
        .device(&key(effect))
        .and_then(|d| d.params.get("bypass").copied())
        .unwrap_or(0.0)
}

#[test]
fn bypass_all_inserts_bypasses_every_effect_and_restores_them_in_one_undo_step() {
    let (mut c, _) = with_two_inserts();
    c.view_mut().set_selection(&[key("a")], Some(&key("a")));
    assert!(c.strip_menu(Pane::Rack).can_bypass);
    assert!(c.bypass_all_inserts(Pane::Rack));
    assert_eq!((bypass_of(&c, "fx1"), bypass_of(&c, "fx2")), (1.0, 1.0));
    assert!(c.undo());
    assert_eq!(
        (bypass_of(&c, "fx1"), bypass_of(&c, "fx2")),
        (0.0, 0.0),
        "one undo step"
    );
    assert!(c.redo());
    // With one playing again, the command bypasses the rest; with all bypassed it restores.
    c.apply_command(Command::SetParam {
        key: key("fx1"),
        param: "bypass".into(),
        value: 0.0,
        merge: None,
    })
    .unwrap();
    assert!(c.bypass_all_inserts(Pane::Rack));
    assert_eq!((bypass_of(&c, "fx1"), bypass_of(&c, "fx2")), (1.0, 1.0));
    assert!(c.bypass_all_inserts(Pane::Rack));
    assert_eq!((bypass_of(&c, "fx1"), bypass_of(&c, "fx2")), (0.0, 0.0));
}

#[test]
fn bypass_all_inserts_needs_a_strip_with_inserts() {
    let (mut c, _) = with_two_inserts();
    c.view_mut().set_selection(&[], None);
    assert!(!c.bypass_all_inserts(Pane::Rack));
    assert!(c.last_rejection().unwrap().contains("Select"));
    // `b` has no insert effects.
    c.view_mut().set_selection(&[key("b")], Some(&key("b")));
    assert!(!c.strip_menu(Pane::Rack).can_bypass);
    assert!(!c.bypass_all_inserts(Pane::Rack));
    assert!(c.last_rejection().unwrap().contains("no insert effects"));
    // With both selected only the chain of `a` is touched.
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("a")));
    assert!(c.bypass_all_inserts(Pane::Rack));
    assert_eq!(bypass_of(&c, "fx1"), 1.0);
}
