//! The camera as a virtual MIDI controller, driven by a fake tracker: MIDI Learn maps a camera
//! control, the mapping follows the hand, saves and reloads, hardware never drives it, the twist
//! is an endless encoder, the pinch note is a button, the modes, hand loss, the rate limit and
//! which notes reach an instrument.

use super::*;
use crate::keymap_store::CameraMode;
use crate::sink::SinkCall;
use atomik_core::{ChannelMessage, NoteKind};
use atomik_model::{
    CAMERA_PORT, CameraControl, CameraHand, MapMode, MapOptions, MidiMapCommand, MidiSource,
    MidiTarget, RelativeKind, load_project, save_project,
};

fn param(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

fn mode(c: &mut AppCore<RecordingSink>, mode: CameraMode) {
    let mut prefs = c.camera_prefs().clone();
    prefs.mode = mode;
    c.set_camera_prefs(prefs);
}

/// The right hand with its palm at (`x`, `y`) and `size` frame heights big.
fn hand_at(x: f32, y: f32, size: f32) -> HandState {
    HandState {
        palm_center: (x, y),
        ..hand_state(size, false)
    }
}

/// A frame report at time `t`.
fn report(t: f64, hands: Vec<HandState>) -> TrackerEvent {
    TrackerEvent::Frame(FrameReport {
        t,
        hands,
        fps: 30.0,
        infer_ms: 20.0,
    })
}

/// The hand at height `y` (x and size fixed) in a frame at `t`.
fn at_height(t: f64, y: f32) -> TrackerEvent {
    report(t, vec![hand_at(0.5, y, 0.2)])
}

fn palm_start() -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::OpenPalmStart {
        hand: HandId(0),
        handedness: Handedness::Right,
    })
}

fn palm_end() -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::OpenPalmEnd { hand: HandId(0) })
}

/// Maps `control` of the right hand to `target` as MIDI Learn would, and returns the id.
fn map_camera(
    c: &mut AppCore<RecordingSink>,
    control: CameraControl,
    target: MidiTarget,
    edit: impl FnOnce(&mut MapOptions),
) -> u32 {
    map_source(
        c,
        MidiSource::camera(CameraHand::Right, control),
        target,
        edit,
    )
}

/// Maps `source` to `target` as MIDI Learn would, then edits its options.
fn map_source(
    c: &mut AppCore<RecordingSink>,
    source: MidiSource,
    target: MidiTarget,
    edit: impl FnOnce(&mut MapOptions),
) -> u32 {
    c.apply_command(Command::MidiMap(MidiMapCommand::Learn { source, target }))
        .unwrap();
    let id = c.doc().midi_maps.iter().map(|m| m.id).max().unwrap();
    let mut m = c.doc().midi_map(id).unwrap().clone();
    edit(&mut m.options);
    if m != *c.doc().midi_map(id).unwrap() {
        c.apply_command(Command::MidiMap(MidiMapCommand::Set(m)))
            .unwrap();
    }
    id
}

fn no_takeover(o: &mut MapOptions) {
    o.pickup = false;
}

fn played(c: &mut AppCore<RecordingSink>) -> Vec<NoteKind> {
    c.sink_mut()
        .take()
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::PlayNote { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

// ---- MIDI Learn -------------------------------------------------------------------------------

#[test]
fn midi_learn_maps_the_height_of_the_hand_and_the_mapping_then_follows_it() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    assert!(c.midi_arm(param("a", CUTOFF)));
    // The first frame sends height, left/right and closeness: the first one wins.
    feed(&mut c, &fake, vec![at_height(0.0, 0.5)]);
    assert_eq!(c.doc().midi_maps.len(), 1, "{:?}", c.doc().midi_maps);
    let m = &c.doc().midi_maps[0];
    assert_eq!(
        m.source,
        MidiSource::camera(CameraHand::Right, CameraControl::Height),
        "CC 21 on channel 1 from the Camera"
    );
    assert_eq!(m.source.port.as_deref(), Some(CAMERA_PORT));
    assert_eq!(m.options.mode, MapMode::Absolute);
    assert!(c.midi_armed().is_none());
    assert!(c.undo(), "the learn is one undo step");
    assert!(c.doc().midi_maps.is_empty());
    assert!(c.redo());

    let id = c.doc().midi_maps[0].id;
    let mut m = c.doc().midi_map(id).unwrap().clone();
    m.options.pickup = false;
    c.apply_command(Command::MidiMap(MidiMapCommand::Set(m)))
        .unwrap();
    // Top of the band is the top of the range, and the bottom is the bottom.
    feed(&mut c, &fake, vec![at_height(0.1, 0.1)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 1.0),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    feed(&mut c, &fake, vec![at_height(0.2, 0.9)]);
    assert!(near(norm(&c, "a", CUTOFF), 0.0));
    feed(&mut c, &fake, vec![at_height(0.3, 0.3)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 95.0 / 127.0),
        "{}",
        norm(&c, "a", CUTOFF)
    );
}

#[test]
fn learning_works_while_the_camera_is_in_the_knob_mode_and_the_preview_says_so() {
    let (mut c, fake) = core();
    assert_eq!(c.camera_prefs().mode, CameraMode::Knob, "the default");
    assert!(
        c.camera_preview_state().controls.is_none(),
        "nothing to show in the knob mode"
    );
    c.midi_arm(param("a", CUTOFF));
    let state = c.camera_preview_state();
    let controls = state.controls.expect("the controls show while learning");
    assert!(controls.learning, "and it says it sends for Learn");
    feed(&mut c, &fake, vec![at_height(0.0, 0.4)]);
    assert_eq!(
        c.doc().midi_maps[0].source,
        MidiSource::camera(CameraHand::Right, CameraControl::Height)
    );
    // The learn is over: the camera sends nothing again in the knob mode.
    assert!(c.camera_preview_state().controls.is_none());
    let received = c.midi_received();
    feed(&mut c, &fake, vec![at_height(0.1, 0.8)]);
    assert_eq!(c.midi_received(), received);
}

#[test]
fn while_learning_a_pinch_turns_no_knob() {
    let (mut c, fake) = core();
    touch(&mut c, "a", CUTOFF);
    let before = norm(&c, "a", CUTOFF);
    c.midi_arm(param("b", CUTOFF));
    feed(&mut c, &fake, vec![pinch_start(), twist(90.0)]);
    assert!(
        near(norm(&c, "a", CUTOFF), before),
        "the clicked knob stays"
    );
    // The pinch note was the first control: it is what was learned.
    assert_eq!(
        c.doc().midi_maps[0].source,
        MidiSource::camera(CameraHand::Right, CameraControl::Pinch)
    );
    assert_eq!(c.doc().midi_maps[0].options.mode, MapMode::Toggle);
}

#[test]
fn the_twist_is_learned_as_an_endless_encoder() {
    let (mut c, _fake) = core();
    c.midi_arm(param("a", CUTOFF));
    c.camera_message(ChannelMessage::cc(0, 20, 3));
    c.midi_control_frame(1.0);
    let m = &c.doc().midi_maps[0];
    assert_eq!(
        m.source,
        MidiSource::camera(CameraHand::Right, CameraControl::Twist)
    );
    assert_eq!(
        m.options.mode,
        MapMode::Relative(RelativeKind::TwosComplement)
    );
}

// ---- Who hears it -----------------------------------------------------------------------------

#[test]
fn a_camera_mapping_saves_and_reloads_and_a_hardware_port_does_not_drive_it() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    map_camera(
        &mut c,
        CameraControl::Height,
        param("a", CUTOFF),
        no_takeover,
    );
    let text = save_project(c.doc());
    let saved = load_project(&text, c.registry()).unwrap();
    assert_eq!(saved.midi_maps, c.doc().midi_maps);
    assert_eq!(saved.midi_maps[0].source.port.as_deref(), Some(CAMERA_PORT));

    // A hardware controller sending CC 21 on channel 1 does nothing to it.
    set_norm(&mut c, "a", CUTOFF, 0.5);
    c.inject_midi("Pad", ChannelMessage::cc(0, 21, 127));
    c.midi_control_frame(1.0);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.5),
        "hardware does not drive it"
    );
    // The camera does.
    feed(&mut c, &fake, vec![at_height(2.0, 0.1)]);
    assert!(near(norm(&c, "a", CUTOFF), 1.0));
}

#[test]
fn a_mapping_on_any_port_never_hears_the_camera() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let hardware = MidiSource::any_port(Some(0), atomik_model::SourceKind::Cc(21));
    map_source(&mut c, hardware, param("a", CUTOFF), no_takeover);
    set_norm(&mut c, "a", CUTOFF, 0.5);
    feed(&mut c, &fake, vec![at_height(0.0, 0.1)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.5),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // A hardware port still does.
    c.inject_midi("Pad", ChannelMessage::cc(0, 21, 127));
    c.midi_control_frame(1.0);
    assert!(norm(&c, "a", CUTOFF) > 0.9);
}

#[test]
fn the_status_line_names_the_camera() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    feed(&mut c, &fake, vec![at_height(0.0, 0.5)]);
    let text = c.midi_last_message().unwrap();
    assert!(text.starts_with("Camera: CC "), "{text}");
}

// ---- The controls -----------------------------------------------------------------------------

/// The degrees of one detent of the encoder at the default 270 degree turn.
const DETENT: f32 = 270.0 / 64.0;

/// Events between two frames at `t` and `t` + 20 ms (so a message held back by the rate goes out).
fn turn(c: &mut AppCore<RecordingSink>, fake: &FakeVision, t: f64, events: Vec<TrackerEvent>) {
    let mut all = vec![at_height(t, 0.5)];
    all.extend(events);
    all.push(at_height(t + 0.02, 0.5));
    feed(c, fake, all);
}

#[test]
fn the_twist_turns_a_relative_mapping_like_an_endless_encoder() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let id = map_camera(
        &mut c,
        CameraControl::Twist,
        param("a", CUTOFF),
        no_takeover,
    );
    assert_eq!(
        c.doc().midi_map(id).unwrap().options.mode,
        MapMode::Relative(RelativeKind::TwosComplement)
    );
    set_norm(&mut c, "a", CUTOFF, 0.25);
    // 135 degrees of a 270 degree turn is half a range.
    turn(
        &mut c,
        &fake,
        0.0,
        vec![pinch_start(), twist(32.0 * DETENT)],
    );
    assert!(
        near(norm(&c, "a", CUTOFF), 0.75),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // A whole turn the other way clamps at the bottom, and turning on moves it at once: there
    // is no wind-up at the end of the range.
    turn(&mut c, &fake, 0.1, vec![twist(-64.0 * DETENT)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.0),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    turn(&mut c, &fake, 0.2, vec![twist(16.0 * DETENT)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.25),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // A ratchet: let go, turn the wrist back, pinch again and keep turning.
    turn(
        &mut c,
        &fake,
        0.3,
        vec![pinch_end(), pinch_start(), twist(16.0 * DETENT)],
    );
    assert!(
        near(norm(&c, "a", CUTOFF), 0.5),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // A turn smaller than a detent is kept, never lost.
    for i in 0..4 {
        turn(
            &mut c,
            &fake,
            0.4 + f64::from(i) * 0.1,
            vec![twist(DETENT / 4.0)],
        );
    }
    assert!(
        near(norm(&c, "a", CUTOFF), 0.5 + 1.0 / 64.0),
        "{}",
        norm(&c, "a", CUTOFF)
    );
}

#[test]
fn the_pinch_note_is_a_button_a_mapping_can_toggle() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    map_camera(&mut c, CameraControl::Pinch, param("b", "legato"), |_| {});
    assert_eq!(value(&c, "b", "legato"), 0.0);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    assert_eq!(value(&c, "b", "legato"), 1.0, "on at the pinch");
    feed(&mut c, &fake, vec![pinch_end()]);
    assert_eq!(value(&c, "b", "legato"), 1.0, "the release does not flip");
    feed(&mut c, &fake, vec![pinch_start()]);
    assert_eq!(value(&c, "b", "legato"), 0.0);
}

#[test]
fn a_momentary_pinch_lets_go_with_the_fingers_and_when_the_camera_goes_off() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    map_camera(&mut c, CameraControl::Pinch, param("b", "legato"), |o| {
        o.mode = MapMode::Momentary;
    });
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    assert_eq!(value(&c, "b", "legato"), 1.0);
    feed(&mut c, &fake, vec![pinch_end()]);
    assert_eq!(value(&c, "b", "legato"), 0.0);
    // The camera stopping with the fingers together must not leave it stuck.
    feed(&mut c, &fake, vec![pinch_start()]);
    assert_eq!(value(&c, "b", "legato"), 1.0);
    c.camera_off(None);
    assert_eq!(value(&c, "b", "legato"), 0.0, "released");
}

#[test]
fn the_open_palm_is_note_61() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let id = map_camera(&mut c, CameraControl::OpenPalm, param("b", "legato"), |o| {
        o.mode = MapMode::Momentary;
    });
    assert_eq!(
        c.doc().midi_map(id).unwrap().source.kind,
        atomik_model::SourceKind::Note(61)
    );
    feed(&mut c, &fake, vec![frame(0.1), palm_start()]);
    assert_eq!(value(&c, "b", "legato"), 1.0);
    feed(&mut c, &fake, vec![palm_end()]);
    assert_eq!(value(&c, "b", "legato"), 0.0);
}

#[test]
fn the_left_hand_sends_on_channel_2() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let left = MidiSource::camera(CameraHand::Left, CameraControl::Height);
    map_source(&mut c, left, param("a", CUTOFF), no_takeover);
    let mut right = hand_at(0.5, 0.1, 0.2);
    right.id = HandId(0);
    // The right hand at the top does not drive a left hand mapping.
    set_norm(&mut c, "a", CUTOFF, 0.5);
    feed(&mut c, &fake, vec![report(0.0, vec![right])]);
    assert!(near(norm(&c, "a", CUTOFF), 0.5));
    let mut l = hand_at(0.5, 0.1, 0.2);
    l.id = HandId(1);
    l.handedness = Handedness::Left;
    feed(&mut c, &fake, vec![report(0.1, vec![right, l])]);
    assert!(norm(&c, "a", CUTOFF) > 0.9, "{}", norm(&c, "a", CUTOFF));
}

// ---- The modes --------------------------------------------------------------------------------

#[test]
fn in_both_mode_the_twist_turns_the_clicked_knob_and_cc_20_is_not_sent() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Both);
    // A mapping on the twist would move b if the controller were sent.
    map_camera(
        &mut c,
        CameraControl::Twist,
        param("b", CUTOFF),
        no_takeover,
    );
    let b = norm(&c, "b", CUTOFF);
    set_norm(&mut c, "a", CUTOFF, 0.25);
    touch(&mut c, "a", CUTOFF);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), twist(135.0)]);
    assert!(near(norm(&c, "a", CUTOFF), 0.75), "the clicked knob turns");
    assert!(near(norm(&c, "b", CUTOFF), b), "the encoder is not sent");
    // The first frame sent three controllers and the pinch a note: nothing else.
    assert_eq!(c.midi_received(), 4);
    // The other controls do send in this mode.
    map_camera(
        &mut c,
        CameraControl::Height,
        param("b", "filter.resonance"),
        no_takeover,
    );
    feed(&mut c, &fake, vec![at_height(0.2, 0.1)]);
    assert!(norm(&c, "b", "filter.resonance") > 0.9);
}

#[test]
fn in_the_midi_mode_a_pinch_turns_no_knob_and_asks_for_none() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    touch(&mut c, "a", CUTOFF);
    let before = norm(&c, "a", CUTOFF);
    let pinched = report(0.0, vec![hand_state(0.1, true)]);
    feed(&mut c, &fake, vec![pinched, pinch_start(), twist(135.0)]);
    assert!(near(norm(&c, "a", CUTOFF), before));
    assert!(
        c.camera_preview_state().message.is_none(),
        "no Click a knob first"
    );
    assert_eq!(c.camera_preview_state().lamp, atomik_ui::CameraLamp::Pinch);
    // The twist went out as a controller, and the preview shows its bar.
    let controls = c.camera_preview_state().controls.unwrap();
    assert_eq!(controls.hand, "Right hand, channel 1");
    assert_eq!(controls.bars.len(), 6);
    assert!(controls.bars[0].centred && controls.bars[0].value > 0.0);
    assert_eq!(controls.bars[4].value, 1.0, "pinched");
    assert!(!controls.learning);
}

#[test]
fn leaving_the_midi_mode_lets_go_of_held_notes() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    map_camera(&mut c, CameraControl::Pinch, param("b", "legato"), |o| {
        o.mode = MapMode::Momentary;
    });
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    assert_eq!(value(&c, "b", "legato"), 1.0);
    mode(&mut c, CameraMode::Knob);
    assert_eq!(value(&c, "b", "legato"), 0.0);
}

// ---- The hand and the rate --------------------------------------------------------------------

#[test]
fn a_lost_hand_holds_the_values() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    map_camera(
        &mut c,
        CameraControl::Height,
        param("a", CUTOFF),
        no_takeover,
    );
    feed(&mut c, &fake, vec![at_height(0.0, 0.1)]);
    let held = norm(&c, "a", CUTOFF);
    assert!(near(held, 1.0));
    feed(&mut c, &fake, vec![report(0.1, vec![])]);
    let mut gone = hand_at(0.5, 0.9, 0.2);
    gone.present = false;
    feed(&mut c, &fake, vec![report(0.2, vec![gone])]);
    assert!(near(norm(&c, "a", CUTOFF), held), "no jump to 0");
    // The same place again is no news either; a new place moves it.
    feed(&mut c, &fake, vec![at_height(0.3, 0.1)]);
    assert!(near(norm(&c, "a", CUTOFF), held));
    feed(&mut c, &fake, vec![at_height(0.4, 0.5)]);
    assert!(norm(&c, "a", CUTOFF) < 0.6);
}

#[test]
fn a_control_sends_at_most_60_times_a_second() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let first = c.midi_received();
    // One second of a fast camera: 240 frames, the height changing in every one.
    for i in 0..240 {
        let y = 0.9 - 0.8 * (i as f32 / 240.0);
        feed(&mut c, &fake, vec![at_height(f64::from(i) / 240.0, y)]);
    }
    let sent = c.midi_received() - first;
    // Height at most 60, and the two controllers that never move send once.
    assert!((55..=64).contains(&sent), "{sent} messages in a second");
}

// ---- Notes and tracks -------------------------------------------------------------------------

#[test]
fn a_track_listening_to_all_inputs_does_not_get_the_camera_notes() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    assert!(!c.camera_prefs().play_notes, "off by default");
    assert!(c.notes_target().is_some(), "an instrument is selected");
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), palm_start()]);
    feed(&mut c, &fake, vec![pinch_end(), palm_end()]);
    assert!(
        played(&mut c).is_empty(),
        "the camera is a controller, not an instrument"
    );
}

#[test]
fn with_the_camera_chosen_as_an_input_its_notes_play_the_instrument_and_end_with_the_gesture() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let mut prefs = c.camera_prefs().clone();
    prefs.play_notes = true;
    c.set_camera_prefs(prefs);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    let on = played(&mut c);
    assert!(
        matches!(on.as_slice(), [NoteKind::On { key: 60, .. }]),
        "{on:?}"
    );
    feed(&mut c, &fake, vec![pinch_end()]);
    assert_eq!(played(&mut c), vec![NoteKind::Off { key: 60 }]);
    // A note a consuming mapping took is not played.
    map_camera(
        &mut c,
        CameraControl::OpenPalm,
        param("b", "legato"),
        |_| {},
    );
    feed(&mut c, &fake, vec![palm_start()]);
    assert!(played(&mut c).is_empty(), "consumed by the mapping");
}

#[test]
fn camera_notes_that_were_playing_end_when_the_camera_goes_off() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    let mut prefs = c.camera_prefs().clone();
    prefs.play_notes = true;
    c.set_camera_prefs(prefs);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    played(&mut c);
    c.camera_off(None);
    assert_eq!(played(&mut c), vec![NoteKind::Off { key: 60 }]);
}

// ---- The project ------------------------------------------------------------------------------

#[test]
fn the_camera_adds_nothing_to_a_project_that_does_not_map_it() {
    let (mut c, fake) = core();
    mode(&mut c, CameraMode::Midi);
    feed(
        &mut c,
        &fake,
        vec![at_height(0.0, 0.5), pinch_start(), twist(30.0)],
    );
    let text = save_project(c.doc());
    assert!(!text.contains(CAMERA_PORT));
    assert!(c.doc().midi_maps.is_empty());
}
