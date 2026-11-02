//! MIDI learn and mappings in the app core: the learn flow (armed target, learn mode, the
//! right-click path), the maths through real params, soft takeover, buttons, consumed and
//! passed-through messages, automation touch recording from a controller, one undo step per
//! gesture, persistence, transport, the selected-strip target, a vanished target and presets.

use super::*;
use crate::app_core::tests::rack_only_core;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_engine::TransportState;
use atomik_model::automation_rec::WriteMode;
use atomik_model::{
    AutomationPoint, ClipContent, MapOptions, RelativeKind, SongCommand, SourceKind, save_project,
};
use atomik_ui::UiAction;
use std::sync::atomic::{AtomicU64, Ordering};

const SYNTH: &str = "atomik.poly-synth";
const CUTOFF: &str = "filter.cutoff";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Two synths `a` and `b` with their channels `channel` and `channel-2`.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    c.sink_mut().take();
    c
}

fn param(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

fn cc_source(channel: u8, n: u8) -> MidiSource {
    MidiSource::any_port(Some(channel), SourceKind::Cc(n))
}

fn map(c: &mut AppCore<RecordingSink>, source: MidiSource, target: MidiTarget) -> u32 {
    c.apply_command(Command::MidiMap(MidiMapCommand::Learn { source, target }))
        .unwrap();
    c.doc().midi_maps.iter().map(|m| m.id).max().unwrap()
}

fn options(c: &mut AppCore<RecordingSink>, id: u32, edit: impl FnOnce(&mut MapOptions)) {
    let mut m = c.doc().midi_map(id).unwrap().clone();
    edit(&mut m.options);
    c.apply_command(Command::MidiMap(MidiMapCommand::Set(m)))
        .unwrap();
}

/// Sends `msg` from the port `Pad` and runs a frame at `t`.
fn send(c: &mut AppCore<RecordingSink>, t: f64, msg: ChannelMessage) {
    c.inject_midi("Pad", msg);
    c.midi_control_frame(t);
}

fn cc(n: u8, v: u8) -> ChannelMessage {
    ChannelMessage::cc(0, n, v)
}

fn value(c: &AppCore<impl EngineSink>, device: &str, p: &str) -> f32 {
    c.doc().param_value(&key(device), p, c.registry()).unwrap()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ---- Learning ---------------------------------------------------------------------------------

#[test]
fn an_armed_target_takes_the_first_control_moved() {
    let mut c = core();
    assert!(c.midi_arm(param("channel", "fader")));
    assert_eq!(c.midi_armed(), Some(&param("channel", "fader")));
    assert!(
        c.control_hub().is_learning(),
        "everything is consumed while learning"
    );
    // CC 74 is above 63: no pair to wait for, so it decides at once.
    send(&mut c, 0.0, ChannelMessage::cc(2, 74, 64));
    let m = &c.doc().midi_maps[0];
    assert_eq!(m.source, cc_source(2, 74));
    assert_eq!(m.target, param("channel", "fader"));
    assert!(c.midi_armed().is_none());
    assert!(!c.control_hub().is_learning());
    assert!(c.last_rejection().unwrap().starts_with("Mapped CC 74 ch 3"));
    // The learn is one undo step, and the move that taught it did not move the param.
    assert!(near(value(&c, "channel", "fader"), 0.75));
    assert!(c.undo());
    assert!(c.doc().midi_maps.is_empty());
}

#[test]
fn a_lone_low_controller_is_decided_after_the_pair_window() {
    let mut c = core();
    c.midi_arm(param("channel", "pan"));
    send(&mut c, 0.0, cc(7, 64));
    assert!(c.doc().midi_maps.is_empty(), "it waits for a possible LSB");
    c.midi_control_frame(0.05);
    assert!(c.doc().midi_maps.is_empty());
    c.midi_control_frame(0.2);
    assert_eq!(c.doc().midi_maps[0].source, cc_source(0, 7));
}

#[test]
fn a_fourteen_bit_pair_is_learnt_as_one_source() {
    let mut c = core();
    c.midi_arm(param("channel", "fader"));
    send(&mut c, 0.0, cc(1, 50));
    send(&mut c, 0.01, cc(33, 9));
    assert_eq!(
        c.doc().midi_maps[0].source,
        MidiSource::any_port(Some(0), SourceKind::Cc14(1))
    );
    // And it drives the target with 14 bit resolution.
    options(&mut c, 1, |o| o.pickup = false);
    send(&mut c, 1.0, cc(1, 64));
    send(&mut c, 1.0, cc(33, 0));
    assert!(near(value(&c, "channel", "fader"), 8192.0 / 16_383.0));
}

#[test]
fn a_note_is_learnt_as_a_toggle_button() {
    let mut c = core();
    c.midi_arm(param("channel", "mute"));
    send(&mut c, 0.0, ChannelMessage::note_on(9, 36, 100));
    let m = &c.doc().midi_maps[0];
    assert_eq!(
        m.source,
        MidiSource::any_port(Some(9), SourceKind::Note(36))
    );
    assert_eq!(m.options.mode, MapMode::Toggle);
}

#[test]
fn learn_mode_arms_the_control_whose_value_the_user_changes() {
    let mut c = core();
    c.set_midi_learn(true);
    assert!(c.midi_learn_mode());
    assert!(
        c.control_hub().is_learning(),
        "learn mode consumes messages"
    );
    // The view's change is swallowed: it arms the control and does not move it.
    let before = value(&c, "a", CUTOFF);
    c.handle_action(UiAction::Command(Command::SetParam {
        key: key("a"),
        param: CUTOFF.into(),
        value: 500.0,
        merge: Some(7),
    }));
    assert!(near(value(&c, "a", CUTOFF), before));
    assert_eq!(c.midi_armed(), Some(&param("a", CUTOFF)));
    // The rest of the drag re-arms the same target and changes nothing.
    c.handle_action(UiAction::Command(Command::SetParam {
        key: key("a"),
        param: CUTOFF.into(),
        value: 900.0,
        merge: Some(7),
    }));
    assert!(near(value(&c, "a", CUTOFF), before));
    // Messages with nothing armed are ignored; once armed the first control is mapped.
    send(&mut c, 0.0, ChannelMessage::cc(0, 80, 10));
    assert_eq!(c.doc().midi_maps.len(), 1);
    assert!(
        c.midi_learn_mode(),
        "the mode stays on for the next control"
    );
    // The VCA fader, the tempo and the transport buttons arm too.
    c.handle_action(UiAction::Command(Command::SetTempo(140.0)));
    assert_eq!(c.midi_armed(), Some(&MidiTarget::Tempo));
    assert!((c.doc().tempo_bpm - 120.0).abs() < 1e-9);
    c.handle_action(UiAction::Transport(atomik_ui::TransportAction::Play));
    assert_eq!(
        c.midi_armed(),
        Some(&MidiTarget::Transport(TransportTarget::Play))
    );
    assert!(c.sink().calls.is_empty(), "the button did not play");
    c.set_midi_learn(false);
    assert!(c.midi_armed().is_none());
    assert!(!c.control_hub().is_learning());
    // Out of learn mode a change is a change again.
    c.handle_action(UiAction::Command(Command::SetTempo(140.0)));
    assert!((c.doc().tempo_bpm - 140.0).abs() < 1e-9);
}

#[test]
fn learning_a_target_again_moves_it_to_the_new_control() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 7), param("channel", "fader"));
    c.midi_arm(param("channel", "fader"));
    send(&mut c, 0.0, ChannelMessage::cc(0, 90, 1));
    assert_eq!(c.doc().midi_maps.len(), 1);
    assert_eq!(c.doc().midi_map(id).unwrap().source, cc_source(0, 90));
}

#[test]
fn a_target_that_cannot_be_mapped_is_not_armed() {
    let mut c = core();
    assert!(!c.midi_arm(param("nope", "fader")));
    assert!(c.midi_armed().is_none());
    assert!(!c.control_hub().is_learning());
}

#[test]
fn forget_removes_the_mappings_of_a_target() {
    let mut c = core();
    map(&mut c, cc_source(0, 7), param("channel", "fader"));
    assert!(c.is_midi_mapped(&key("channel"), "fader"));
    assert!(!c.is_midi_mapped(&key("channel"), "pan"));
    assert!(c.midi_forget(param("channel", "fader")));
    assert!(!c.is_midi_mapped(&key("channel"), "fader"));
    assert!(
        !c.midi_forget(param("channel", "fader")),
        "nothing left to forget"
    );
    assert!(c.undo());
    assert!(c.is_midi_mapped(&key("channel"), "fader"));
}

// ---- Applying ---------------------------------------------------------------------------------

#[test]
fn an_absolute_control_sets_the_param_through_its_range_and_inversion() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 7), param("channel", "fader"));
    options(&mut c, id, |o| o.pickup = false);
    send(&mut c, 0.0, cc(7, 127));
    assert!(near(value(&c, "channel", "fader"), 1.0));
    send(&mut c, 0.0, cc(7, 0));
    assert!(near(value(&c, "channel", "fader"), 0.0));
    options(&mut c, id, |o| {
        o.min = 0.25;
        o.max = 0.75;
    });
    send(&mut c, 0.0, cc(7, 127));
    assert!(near(value(&c, "channel", "fader"), 0.75));
    options(&mut c, id, |o| o.invert = true);
    send(&mut c, 0.0, cc(7, 127));
    assert!(near(value(&c, "channel", "fader"), 0.25));
    // The engine got ordinary param changes.
    assert!(
        c.sink()
            .calls
            .iter()
            .any(|call| matches!(call, SinkCall::Param { .. }))
    );
}

#[test]
fn a_param_with_a_curve_is_moved_as_a_fraction_of_its_range() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 74), param("a", CUTOFF));
    options(&mut c, id, |o| o.pickup = false);
    let desc = c
        .registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key(CUTOFF)
        .unwrap();
    send(&mut c, 0.0, cc(74, 127));
    assert!(near(value(&c, "a", CUTOFF), desc.max));
    send(&mut c, 0.0, cc(74, 0));
    assert!(near(value(&c, "a", CUTOFF), desc.min));
    send(&mut c, 0.0, cc(74, 64));
    let expected = desc.from_normalized(64.0 / 127.0);
    assert!(
        near(value(&c, "a", CUTOFF), expected),
        "{}",
        value(&c, "a", CUTOFF)
    );
}

#[test]
fn soft_takeover_holds_the_control_back_until_it_reaches_the_param() {
    let mut c = core();
    // The fader is at 0.75 (unity). The hardware starts at the bottom.
    map(&mut c, cc_source(0, 7), param("channel", "fader"));
    send(&mut c, 0.0, cc(7, 0));
    send(&mut c, 0.0, cc(7, 40));
    assert!(near(value(&c, "channel", "fader"), 0.75), "no jump");
    send(&mut c, 0.0, cc(7, 95));
    assert!(near(value(&c, "channel", "fader"), 95.0 / 127.0));
    // A mouse drag moves the fader elsewhere: the control must catch it again.
    c.apply_command(Command::SetParam {
        key: key("channel"),
        param: "fader".into(),
        value: 0.2,
        merge: None,
    })
    .unwrap();
    send(&mut c, 0.0, cc(7, 96));
    assert!(
        near(value(&c, "channel", "fader"), 0.2),
        "still where the mouse left it"
    );
    send(&mut c, 0.0, cc(7, 25));
    assert!(near(value(&c, "channel", "fader"), 25.0 / 127.0));
}

#[test]
fn a_relative_encoder_turns_the_param_by_detents() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 16), param("channel", "pan"));
    options(&mut c, id, |o| {
        o.mode = MapMode::Relative(RelativeKind::TwosComplement);
        o.sensitivity = 0.05;
    });
    // Pan is -1..1: centre is 0.5 of the range; two detents clockwise add 0.1 of the range.
    send(&mut c, 0.0, cc(16, 2));
    assert!(near(value(&c, "channel", "pan"), 0.2));
    send(&mut c, 0.0, cc(16, 126));
    assert!(near(value(&c, "channel", "pan"), 0.0));
    send(&mut c, 0.0, cc(16, 127));
    assert!(near(value(&c, "channel", "pan"), -0.1));
}

#[test]
fn a_toggle_button_flips_a_switch_and_a_momentary_one_holds_it() {
    let mut c = core();
    let id = map(
        &mut c,
        MidiSource::any_port(Some(0), SourceKind::Note(36)),
        param("channel", "mute"),
    );
    let on = ChannelMessage::note_on(0, 36, 100);
    let off = ChannelMessage::note_off(0, 36);
    assert!(near(value(&c, "channel", "mute"), 0.0));
    send(&mut c, 0.0, on);
    assert!(near(value(&c, "channel", "mute"), 1.0));
    send(&mut c, 0.0, off);
    assert!(
        near(value(&c, "channel", "mute"), 1.0),
        "a release does not flip"
    );
    send(&mut c, 0.0, on);
    assert!(near(value(&c, "channel", "mute"), 0.0));
    send(&mut c, 0.0, off);
    options(&mut c, id, |o| o.mode = MapMode::Momentary);
    send(&mut c, 0.0, on);
    assert!(near(value(&c, "channel", "mute"), 1.0));
    send(&mut c, 0.0, off);
    assert!(
        near(value(&c, "channel", "mute"), 0.0),
        "released with the key"
    );
}

#[test]
fn the_vca_level_and_the_tempo_are_targets() {
    let mut c = core();
    c.apply_command(Command::Console(atomik_model::ConsoleCommand::CreateVca {
        name: "Drums".into(),
        members: vec![key("channel")],
    }))
    .unwrap();
    let id = map(&mut c, cc_source(0, 1), MidiTarget::Vca { id: 1 });
    options(&mut c, id, |o| o.pickup = false);
    send(&mut c, 0.0, cc(1, 127));
    assert!(near(c.doc().console.vca(1).unwrap().level_db, 12.0));
    send(&mut c, 0.0, cc(1, 0));
    assert!(near(c.doc().console.vca(1).unwrap().level_db, -60.0));
    let id = map(&mut c, cc_source(0, 2), MidiTarget::Tempo);
    options(&mut c, id, |o| o.pickup = false);
    send(&mut c, 0.0, cc(2, 127));
    assert!((c.doc().tempo_bpm - 999.0).abs() < 1e-9);
    send(&mut c, 0.0, cc(2, 0));
    assert!((c.doc().tempo_bpm - 20.0).abs() < 1e-9);
    assert!(c.sink().calls.contains(&SinkCall::Tempo(999.0)));
}

#[test]
fn a_transport_target_presses_the_transport() {
    let mut c = core();
    map(
        &mut c,
        MidiSource::any_port(Some(0), SourceKind::Note(50)),
        MidiTarget::Transport(TransportTarget::Play),
    );
    map(
        &mut c,
        cc_source(0, 115),
        MidiTarget::Transport(TransportTarget::Stop),
    );
    send(&mut c, 0.0, ChannelMessage::note_on(0, 50, 90));
    assert!(c.is_playing());
    send(&mut c, 0.0, ChannelMessage::note_off(0, 50));
    send(&mut c, 0.0, cc(115, 127));
    assert!(!c.is_playing());
    // Loop is an undoable song edit like the button.
    map(
        &mut c,
        cc_source(0, 116),
        MidiTarget::Transport(TransportTarget::Loop),
    );
    let before = c.doc().song.loop_enabled;
    send(&mut c, 0.0, cc(116, 127));
    assert_ne!(c.doc().song.loop_enabled, before);
}

#[test]
fn the_selected_strip_target_follows_the_selection() {
    let mut c = core();
    let id = map(
        &mut c,
        cc_source(0, 7),
        MidiTarget::SelectedStrip {
            param: "fader".into(),
        },
    );
    options(&mut c, id, |o| o.pickup = false);
    c.console_mut().set_selected_strips(&[key("channel-2")]);
    send(&mut c, 0.0, cc(7, 127));
    assert!(near(value(&c, "channel-2", "fader"), 1.0));
    assert!(
        near(value(&c, "channel", "fader"), 0.75),
        "the other strip is untouched"
    );
    c.console_mut().set_selected_strips(&[key("channel")]);
    send(&mut c, 1.0, cc(7, 0));
    assert!(near(value(&c, "channel", "fader"), 0.0));
    assert!(near(value(&c, "channel-2", "fader"), 1.0));
}

#[test]
fn a_mapping_to_a_vanished_target_does_nothing() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 74), param("a", CUTOFF));
    options(&mut c, id, |o| o.pickup = false);
    c.apply_command(Command::RemoveDevice { key: key("a") })
        .unwrap();
    let calls = c.sink().calls.len();
    send(&mut c, 0.0, cc(74, 100));
    assert_eq!(c.sink().calls.len(), calls);
    assert_eq!(
        c.doc().midi_maps.len(),
        1,
        "the mapping stays, listed as missing"
    );
    assert!(c.undo());
    send(&mut c, 1.0, cc(74, 100));
    assert!(value(&c, "a", CUTOFF) > 0.0);
}

#[test]
fn a_message_no_mapping_listens_to_does_nothing_and_a_source_may_drive_two_targets() {
    let mut c = core();
    let a = map(&mut c, cc_source(0, 7), param("channel", "fader"));
    let b = map(&mut c, cc_source(0, 7), param("channel-2", "fader"));
    for id in [a, b] {
        options(&mut c, id, |o| o.pickup = false);
    }
    send(&mut c, 0.0, cc(8, 100));
    assert!(near(value(&c, "channel", "fader"), 0.75));
    send(&mut c, 0.0, cc(7, 127));
    assert!(near(value(&c, "channel", "fader"), 1.0));
    assert!(near(value(&c, "channel-2", "fader"), 1.0));
    // Another channel or port is another control.
    send(&mut c, 0.0, ChannelMessage::cc(1, 7, 0));
    assert!(near(value(&c, "channel", "fader"), 1.0));
}

// ---- Undo merging -----------------------------------------------------------------------------

#[test]
fn a_sweep_is_one_undo_step_and_a_pause_starts_the_next() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 74), param("a", CUTOFF));
    options(&mut c, id, |o| o.pickup = false);
    let start = value(&c, "a", CUTOFF);
    for (i, v) in [10u8, 30, 60, 90].into_iter().enumerate() {
        send(&mut c, i as f64 * 0.05, cc(74, v));
    }
    let after_first = value(&c, "a", CUTOFF);
    // Half a second of silence lets go; the next move is a new gesture.
    c.midi_control_frame(2.0);
    for (i, v) in [100u8, 120].into_iter().enumerate() {
        send(&mut c, 2.1 + i as f64 * 0.05, cc(74, v));
    }
    assert!(value(&c, "a", CUTOFF) > after_first);
    assert!(c.undo());
    assert!(
        near(value(&c, "a", CUTOFF), after_first),
        "the second sweep is one step"
    );
    assert!(c.undo());
    assert!(near(value(&c, "a", CUTOFF), start), "so is the first");
    assert!(c.redo());
    assert!(near(value(&c, "a", CUTOFF), after_first));
}

// ---- Automation -------------------------------------------------------------------------------

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

fn automation_core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for cmd in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        },
        song(SongCommand::AddTrack { device: key("a") }),
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: CUTOFF.into(),
        }),
    ] {
        c.apply_command(cmd).unwrap();
    }
    c.sink_mut().take();
    c
}

fn at(c: &mut AppCore<RecordingSink>, tick: i64) {
    c.sink_mut().state = TransportState {
        playing: true,
        tick,
        engine_frame: 1000 + tick as u64,
        loop_end: BAR,
        ..TransportState::default()
    };
}

type Written = (i64, i64, Vec<(i64, f32)>);

fn clips(c: &AppCore<impl EngineSink>) -> Vec<Written> {
    c.doc().song.track(&key("a")).unwrap().lanes[1]
        .clips
        .iter()
        .map(|cl| {
            let ClipContent::Automation(p) = &cl.content else {
                panic!("not automation");
            };
            (
                cl.start,
                cl.length,
                p.iter()
                    .map(|p: &AutomationPoint| (p.tick, p.value))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn a_mapped_move_is_a_touch_that_lets_go_when_the_control_is_still() {
    let mut c = automation_core();
    let id = map(&mut c, cc_source(0, 74), param("a", CUTOFF));
    options(&mut c, id, |o| o.pickup = false);
    c.handle_action(UiAction::ToggleLaneArm {
        device: key("a"),
        param: CUTOFF.into(),
    });
    c.apply_command(song(SongCommand::SetLaneWriteMode {
        device: key("a"),
        param: CUTOFF.into(),
        mode: WriteMode::Touch,
    }))
    .unwrap();
    at(&mut c, 0);
    c.transport_action(atomik_ui::TransportAction::ToggleRecord);
    // The controller moves from a quarter of the bar to half of it...
    at(&mut c, BAR / 4);
    send(&mut c, 1.0, cc(74, 20));
    at(&mut c, BAR / 2);
    send(&mut c, 1.2, cc(74, 80));
    // ...and rests; half a second later the touch ends at the position of that frame.
    at(&mut c, 3 * BAR / 4);
    c.midi_control_frame(1.9);
    at(&mut c, BAR);
    c.transport_action(atomik_ui::TransportAction::Stop);
    let written = clips(&c);
    assert_eq!(written.len(), 1, "{written:?}");
    let (start, length, points) = &written[0];
    assert_eq!(*start, BAR / 4, "Touch writes from the first move");
    assert_eq!(
        *start + *length,
        3 * BAR / 4,
        "and lets go with the control"
    );
    assert!(points.len() >= 2, "{points:?}");
    // The take is one undo step: the lane is as it was.
    assert!(c.undo());
    assert!(clips(&c).is_empty());
}

// ---- Consumed and passed through ---------------------------------------------------------------

#[test]
fn the_consume_list_follows_the_mappings() {
    let mut c = core();
    let note = ChannelMessage::note_on(0, 36, 100);
    assert!(!c.control_hub().consumes("Pad", &note));
    let id = map(
        &mut c,
        MidiSource::any_port(Some(0), SourceKind::Note(36)),
        param("channel", "mute"),
    );
    assert!(c.control_hub().consumes("Pad", &note), "mapped: consumed");
    assert!(
        !c.control_hub()
            .consumes("Pad", &ChannelMessage::note_on(0, 37, 100)),
        "another key still plays"
    );
    options(&mut c, id, |o| o.consume = false);
    assert!(
        !c.control_hub().consumes("Pad", &note),
        "pass-through by choice"
    );
    options(&mut c, id, |o| o.consume = true);
    assert!(c.control_hub().consumes("Pad", &note));
    assert!(c.undo());
    assert!(!c.control_hub().consumes("Pad", &note));
    assert!(c.undo());
    assert!(c.undo());
    assert!(
        !c.control_hub().consumes("Pad", &note),
        "no mappings, nothing consumed"
    );
}

#[test]
fn a_mapping_restricted_to_a_port_consumes_only_that_ports_messages() {
    let mut c = core();
    map(
        &mut c,
        MidiSource {
            port: Some("Pad".into()),
            channel: None,
            kind: SourceKind::Cc(1),
            surface: None,
        },
        param("channel", "pan"),
    );
    assert!(c.control_hub().consumes("Pad", &cc(1, 5)));
    assert!(!c.control_hub().consumes("Other", &cc(1, 5)));
    // A message from the wrong port does not drive the target either.
    c.inject_midi("Other", cc(1, 127));
    c.midi_control_frame(0.0);
    assert!(near(value(&c, "channel", "pan"), 0.0));
}

// ---- Persistence ------------------------------------------------------------------------------

#[test]
fn mappings_survive_a_save_and_load_and_work_again() {
    let mut c = core();
    let id = map(&mut c, cc_source(0, 7), param("channel", "fader"));
    options(&mut c, id, |o| o.pickup = false);
    let text = save_project(c.doc());
    let mut d = core();
    d.open_str(&text, None).unwrap();
    assert_eq!(d.doc().midi_maps, c.doc().midi_maps);
    assert!(
        d.control_hub().consumes("Pad", &cc(7, 1)),
        "the consume list is rebuilt"
    );
    send(&mut d, 0.0, cc(7, 127));
    assert!(near(value(&d, "channel", "fader"), 1.0));
    // Loading again from a file without mappings clears the table and the consume list.
    let empty = save_project(core().doc());
    d.open_str(&empty, None).unwrap();
    assert!(d.doc().midi_maps.is_empty());
    assert!(!d.control_hub().consumes("Pad", &cc(7, 1)));
}

// ---- Presets ----------------------------------------------------------------------------------

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-midimap-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_preset_exported_from_one_project_applies_to_another_by_role() {
    let tmp = Tmp::new();
    let path = tmp.0.join("Desk.json");
    let mut c = core();
    map(&mut c, cc_source(0, 74), param("b", CUTOFF));
    map(&mut c, cc_source(0, 7), param("channel-2", "fader"));
    map(
        &mut c,
        cc_source(0, 115),
        MidiTarget::Transport(TransportTarget::Stop),
    );
    assert!(c.export_midi_preset(&path, &[]));
    // Another project with differently named synths: `b` was the second poly synth.
    let mut d = AppCore::new(RecordingSink::new());
    for k in ["x", "y"] {
        d.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    assert!(d.import_midi_preset(&path));
    assert_eq!(d.doc().midi_maps.len(), 3);
    assert_eq!(
        d.doc().midi_maps[0].target,
        param("y", CUTOFF),
        "the second poly synth"
    );
    assert_eq!(d.doc().midi_maps[1].target, param("channel-2", "fader"));
    // One undo step for the whole preset.
    assert!(d.undo());
    assert!(d.doc().midi_maps.is_empty());
    // A missing file or a file that is not a preset says so.
    assert!(!d.import_midi_preset(&tmp.0.join("missing.json")));
    std::fs::write(tmp.0.join("bad.json"), "{}").unwrap();
    assert!(!d.import_midi_preset(&tmp.0.join("bad.json")));
    // An empty export says there is nothing.
    assert!(!AppCore::new(RecordingSink::new()).export_midi_preset(&tmp.0.join("n.json"), &[]));
}

#[test]
fn a_preset_that_finds_nothing_in_this_project_adds_nothing() {
    let tmp = Tmp::new();
    let path = tmp.0.join("p.json");
    let mut c = core();
    map(&mut c, cc_source(0, 74), param("b", CUTOFF));
    assert!(c.export_midi_preset(&path, &[]));
    let mut empty = AppCore::new(RecordingSink::new());
    assert!(!empty.import_midi_preset(&path));
    assert!(empty.doc().midi_maps.is_empty());
}

#[test]
fn clear_removes_every_mapping_in_one_step() {
    let mut c = core();
    map(&mut c, cc_source(0, 1), param("channel", "fader"));
    map(&mut c, cc_source(0, 2), param("channel", "pan"));
    assert!(c.clear_midi_maps());
    assert!(c.doc().midi_maps.is_empty());
    assert!(c.undo());
    assert_eq!(c.doc().midi_maps.len(), 2);
    assert!(!AppCore::new(RecordingSink::new()).clear_midi_maps());
}

#[test]
fn the_last_message_and_the_count_are_kept_for_the_status_line() {
    let mut c = core();
    assert_eq!(c.midi_last_message(), None);
    send(&mut c, 0.0, ChannelMessage::cc(2, 7, 64));
    assert_eq!(
        c.midi_last_message().as_deref(),
        Some("Pad: CC 7 ch 3 = 64")
    );
    send(&mut c, 0.0, ChannelMessage::note_on(9, 60, 100));
    assert_eq!(
        c.midi_last_message().as_deref(),
        Some("Pad: Note C3 ch 10 on 100")
    );
    assert_eq!(c.midi_received(), 2);
    assert_eq!(
        describe_message(&ChannelMessage::pitch_bend(0, 8192)),
        "Bend ch 1 = 8192"
    );
    assert_eq!(
        describe_message(&ChannelMessage::note_off(0, 60)),
        "Note C3 ch 1 off"
    );
}
