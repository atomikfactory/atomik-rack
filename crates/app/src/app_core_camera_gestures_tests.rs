//! The camera's gestures in the app core, driven by a fake tracker: the gesture command table
//! (nothing happens by default, a gesture runs its command once per hold, the cooldown, never
//! while pinching), the project's own table and mode, the two-hand pad (two knobs in one undo
//! step, the controllers 24 to 26 in MIDI mode) and the gestures as notes MIDI Learn can map.

use super::*;
use crate::camera_gestures::{GestureBinding, suggested};
use crate::keymap_store::{CameraMode, HandChoice};
use atomik_model::{
    CAMERA_PORT, CameraControl, CameraGesture, CameraHand, CameraOverride, GestureHand, GestureRow,
    MapOptions, MidiMapCommand, MidiSource, MidiTarget, load_project, save_project,
};
use atomik_vision::gesture::{Pose, SwipeDirection};

const RESONANCE: &str = "filter.resonance";

fn r_hand(id: u32, handedness: Handedness, pinch: bool) -> HandState {
    HandState {
        id: HandId(id),
        handedness,
        ..hand_state(0.2, pinch)
    }
}

fn frame_at(t: f64, hands: Vec<HandState>, pad: Option<Pad>) -> TrackerEvent {
    TrackerEvent::Frame(FrameReport {
        t,
        hands,
        pad,
        fps: 30.0,
        infer_ms: 20.0,
    })
}

fn open_frame(t: f64) -> TrackerEvent {
    frame_at(t, vec![r_hand(0, Handedness::Right, false)], None)
}

fn pose_start(pose: Pose) -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::PoseStart {
        hand: HandId(0),
        handedness: Handedness::Right,
        pose,
    })
}

fn pose_end(pose: Pose) -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::PoseEnd {
        hand: HandId(0),
        pose,
    })
}

fn palm_start() -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::OpenPalmStart {
        hand: HandId(0),
        handedness: Handedness::Right,
    })
}

fn swipe(direction: SwipeDirection) -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::Swipe {
        hand: HandId(0),
        handedness: Handedness::Right,
        direction,
    })
}

fn table(c: &mut AppCore<RecordingSink>, rows: &[(CameraGesture, HandChoice, &str)]) {
    let mut prefs = c.camera_prefs().clone();
    prefs.gestures = rows
        .iter()
        .map(|(gesture, hand, command)| GestureBinding {
            gesture: *gesture,
            hand: *hand,
            command: (*command).to_owned(),
        })
        .collect();
    c.set_camera_prefs(prefs);
}

fn mode_of(c: &mut AppCore<RecordingSink>, mode: CameraMode) {
    let mut prefs = c.camera_prefs().clone();
    prefs.mode = mode;
    c.set_camera_prefs(prefs);
}

fn run(
    c: &mut AppCore<RecordingSink>,
    fake: &FakeVision,
    events: Vec<TrackerEvent>,
) -> Vec<String> {
    feed(c, fake, events);
    c.take_camera_commands()
}

// ---- The table --------------------------------------------------------------------------------

#[test]
fn by_default_no_gesture_runs_anything() {
    let (mut c, fake) = core();
    assert!(c.camera_prefs().gestures.is_empty());
    assert!(c.camera_gesture_table().is_empty());
    let out = run(
        &mut c,
        &fake,
        vec![
            open_frame(1.0),
            pose_start(Pose::Fist),
            pose_start(Pose::ThumbsUp),
            palm_start(),
            swipe(SwipeDirection::Left),
            open_frame(5.0),
        ],
    );
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn a_gesture_runs_its_command_once_per_hold() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[(CameraGesture::Fist, HandChoice::Either, "transport.stop")],
    );
    let out = run(&mut c, &fake, vec![open_frame(1.0), pose_start(Pose::Fist)]);
    assert_eq!(out, ["transport.stop"]);
    // Held for seconds: nothing more.
    let more: Vec<TrackerEvent> = (0..60)
        .map(|i| open_frame(1.0 + f64::from(i) * 0.05))
        .collect();
    assert!(run(&mut c, &fake, more).is_empty());
    // The hand opens and makes the fist again: a new hold.
    let out = run(
        &mut c,
        &fake,
        vec![
            pose_end(Pose::Fist),
            open_frame(5.0),
            pose_start(Pose::Fist),
        ],
    );
    assert_eq!(out, ["transport.stop"]);
}

#[test]
fn the_open_palm_runs_its_command_after_a_second_and_not_before() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[(
            CameraGesture::OpenPalm,
            HandChoice::Either,
            "transport.play",
        )],
    );
    // The recogniser says so after 300 ms; the command needs 700 ms more.
    let out = run(&mut c, &fake, vec![open_frame(0.3), palm_start()]);
    assert!(out.is_empty());
    assert!(run(&mut c, &fake, vec![open_frame(0.7)]).is_empty());
    assert_eq!(
        run(&mut c, &fake, vec![open_frame(1.0)]),
        ["transport.play"]
    );
    assert!(run(&mut c, &fake, vec![open_frame(1.5), open_frame(3.0)]).is_empty());
}

#[test]
fn the_suggested_table_is_stop_play_and_record() {
    let (mut c, fake) = core();
    let mut prefs = c.camera_prefs().clone();
    prefs.gestures = suggested();
    c.set_camera_prefs(prefs);
    assert_eq!(
        run(&mut c, &fake, vec![open_frame(1.0), pose_start(Pose::Fist)]),
        ["transport.stop"]
    );
    assert_eq!(
        run(
            &mut c,
            &fake,
            vec![
                pose_end(Pose::Fist),
                open_frame(2.0),
                pose_start(Pose::ThumbsUp)
            ]
        ),
        ["transport.record"]
    );
}

#[test]
fn a_command_is_followed_by_a_pause_of_half_a_second() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[
            (CameraGesture::SwipeLeft, HandChoice::Either, "edit.undo"),
            (CameraGesture::SwipeRight, HandChoice::Either, "edit.redo"),
        ],
    );
    let out = run(
        &mut c,
        &fake,
        vec![open_frame(1.0), swipe(SwipeDirection::Left)],
    );
    assert_eq!(out, ["edit.undo"]);
    let out = run(
        &mut c,
        &fake,
        vec![open_frame(1.3), swipe(SwipeDirection::Right)],
    );
    assert!(out.is_empty(), "inside the pause: {out:?}");
    let out = run(
        &mut c,
        &fake,
        vec![open_frame(1.6), swipe(SwipeDirection::Right)],
    );
    assert_eq!(out, ["edit.redo"]);
}

#[test]
fn nothing_runs_while_a_hand_is_pinching() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[
            (CameraGesture::SwipeLeft, HandChoice::Either, "edit.undo"),
            (CameraGesture::Fist, HandChoice::Either, "transport.stop"),
        ],
    );
    // The right hand pinches; the left hand makes a fist and the right one swipes.
    let pinching = vec![
        r_hand(0, Handedness::Right, true),
        r_hand(1, Handedness::Left, false),
    ];
    let out = run(
        &mut c,
        &fake,
        vec![
            frame_at(1.0, pinching, None),
            pose_start(Pose::Fist),
            swipe(SwipeDirection::Left),
        ],
    );
    assert!(out.is_empty(), "{out:?}");
    // With the pinch gone, a swipe runs.
    let out = run(
        &mut c,
        &fake,
        vec![open_frame(3.0), swipe(SwipeDirection::Left)],
    );
    assert_eq!(out, ["edit.undo"]);
}

#[test]
fn a_row_for_one_hand_leaves_the_other_hand_alone() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[(CameraGesture::Fist, HandChoice::Left, "transport.stop")],
    );
    let out = run(&mut c, &fake, vec![open_frame(1.0), pose_start(Pose::Fist)]);
    assert!(out.is_empty(), "the right hand made it");
    let left = TrackerEvent::Gesture(GestureEvent::PoseStart {
        hand: HandId(1),
        handedness: Handedness::Left,
        pose: Pose::Fist,
    });
    assert_eq!(
        run(&mut c, &fake, vec![open_frame(3.0), left]),
        ["transport.stop"]
    );
}

#[test]
fn turning_the_camera_off_forgets_the_holds() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[(CameraGesture::Fist, HandChoice::Either, "transport.stop")],
    );
    fake.push(vec![open_frame(1.0), pose_start(Pose::Fist)]);
    c.camera_off(None);
    assert!(
        c.take_camera_commands().is_empty(),
        "off at once: nothing is ready to run"
    );
}

// ---- The project's own settings ---------------------------------------------------------------

fn own() -> CameraOverride {
    CameraOverride {
        mode: "midi".into(),
        gestures: vec![GestureRow {
            gesture: CameraGesture::Fist,
            hand: GestureHand::Either,
            command: "edit.undo".into(),
        }],
    }
}

#[test]
fn the_projects_table_and_mode_win_over_the_settings() {
    let (mut c, fake) = core();
    table(
        &mut c,
        &[(CameraGesture::Fist, HandChoice::Either, "transport.stop")],
    );
    assert_eq!(c.camera_mode(), CameraMode::Knob);
    assert!(c.set_camera_override(Some(own())));
    assert_eq!(c.camera_mode(), CameraMode::Midi);
    assert_eq!(c.camera_gesture_table().len(), 1);
    let out = run(&mut c, &fake, vec![open_frame(1.0), pose_start(Pose::Fist)]);
    assert_eq!(
        out,
        ["edit.undo"],
        "the project's command, not the settings'"
    );
    // The settings themselves are untouched.
    assert_eq!(c.camera_prefs().mode, CameraMode::Knob);
    assert_eq!(c.camera_prefs().gestures[0].command, "transport.stop");
    // Clearing it gives the settings back.
    assert!(c.set_camera_override(None));
    assert_eq!(c.camera_mode(), CameraMode::Knob);
    let out = run(
        &mut c,
        &fake,
        vec![
            pose_end(Pose::Fist),
            open_frame(3.0),
            pose_start(Pose::Fist),
        ],
    );
    assert_eq!(out, ["transport.stop"]);
}

#[test]
fn the_projects_settings_are_one_undo_step_and_save_and_load_with_the_project() {
    let (mut c, _fake) = core();
    let before = save_project(c.doc());
    assert!(c.set_camera_override(Some(own())));
    assert!(
        c.set_camera_override(Some(own())),
        "the same again is nothing"
    );
    let saved = save_project(c.doc());
    assert!(saved.contains("\"camera\": {"), "{saved}");
    let loaded = load_project(&saved, c.registry()).expect("the project loads");
    assert_eq!(loaded.camera, Some(own()));
    assert!(c.undo(), "one undo step");
    assert_eq!(c.camera_override(), None);
    assert_eq!(save_project(c.doc()), before);
    assert!(c.redo());
    assert_eq!(c.camera_override(), Some(&own()));
}

#[test]
fn a_project_without_settings_of_its_own_uses_the_users() {
    let (mut c, _fake) = core();
    mode_of(&mut c, CameraMode::Both);
    assert!(c.camera_override().is_none());
    assert_eq!(c.camera_mode(), CameraMode::Both);
}

// ---- The two-hand pad -------------------------------------------------------------------------

fn pad(x: f32, y: f32) -> Option<Pad> {
    Some(Pad { x, y, spread: 0.5 })
}

fn both_pinched(t: f64, p: Option<Pad>) -> TrackerEvent {
    frame_at(
        t,
        vec![
            r_hand(0, Handedness::Right, true),
            r_hand(1, Handedness::Left, true),
        ],
        p,
    )
}

fn pinch_both() -> Vec<TrackerEvent> {
    vec![
        pinch_start(),
        TrackerEvent::Gesture(GestureEvent::PinchStart {
            hand: HandId(1),
            handedness: Handedness::Left,
        }),
        TrackerEvent::Gesture(GestureEvent::PadStart),
    ]
}

fn pad_end() -> Vec<TrackerEvent> {
    vec![
        TrackerEvent::Gesture(GestureEvent::PinchEnd { hand: HandId(0) }),
        TrackerEvent::Gesture(GestureEvent::PinchEnd { hand: HandId(1) }),
        TrackerEvent::Gesture(GestureEvent::PadEnd),
    ]
}

#[test]
fn a_two_hand_pinch_moves_the_last_two_knobs_in_one_undo_step() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.25);
    set_norm(&mut c, "b", RESONANCE, 0.25);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "b", RESONANCE);
    // X is the older knob (the cutoff) and Y the newer (the resonance).
    let mut events = vec![open_frame(0.0)];
    events.extend(pinch_both());
    events.push(both_pinched(0.1, pad(0.5, 0.5)));
    feed(&mut c, &fake, events);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.25),
        "the origin: nothing moved yet"
    );
    // The hands go 0.1 to the right of the mirrored preview (a smaller x) and 0.1 up.
    feed(&mut c, &fake, vec![both_pinched(0.2, pad(0.4, 0.4))]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.375),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    assert!(
        near(norm(&c, "b", RESONANCE), 0.375),
        "{}",
        norm(&c, "b", RESONANCE)
    );
    // Further, then back to the left and down.
    feed(&mut c, &fake, vec![both_pinched(0.3, pad(0.2, 0.7))]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.625),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    assert!(
        near(norm(&c, "b", RESONANCE), 0.0),
        "clamped: {}",
        norm(&c, "b", RESONANCE)
    );
    feed(&mut c, &fake, pad_end());
    // The whole gesture is one undo step.
    assert!(c.undo());
    assert!(
        near(norm(&c, "a", CUTOFF), 0.25),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    assert!(
        near(norm(&c, "b", RESONANCE), 0.25),
        "{}",
        norm(&c, "b", RESONANCE)
    );
    // A pad after it is a step of its own.
    assert!(c.redo());
    assert!(near(norm(&c, "a", CUTOFF), 0.625));
}

#[test]
fn a_second_pad_is_a_second_undo_step() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.25);
    set_norm(&mut c, "b", RESONANCE, 0.25);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "b", RESONANCE);
    for round in 0..2 {
        let t = f64::from(round) * 10.0;
        let mut events = pinch_both();
        events.push(both_pinched(t, pad(0.5, 0.5)));
        events.push(both_pinched(t + 0.1, pad(0.4, 0.5)));
        events.extend(pad_end());
        feed(&mut c, &fake, events);
    }
    assert!(
        near(norm(&c, "a", CUTOFF), 0.5),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    assert!(c.undo());
    assert!(near(norm(&c, "a", CUTOFF), 0.375), "one pad undone");
    assert!(c.undo());
    assert!(near(norm(&c, "a", CUTOFF), 0.25), "the other undone");
}

#[test]
fn a_pad_with_one_clicked_knob_says_so_and_moves_nothing() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.25);
    touch(&mut c, "a", CUTOFF);
    let mut events = pinch_both();
    events.push(both_pinched(0.1, pad(0.5, 0.5)));
    events.push(both_pinched(0.2, pad(0.3, 0.3)));
    feed(&mut c, &fake, events);
    assert!(near(norm(&c, "a", CUTOFF), 0.25));
    assert_eq!(
        c.camera_preview_state().message.as_deref(),
        Some(NO_PAD_TARGETS_MESSAGE)
    );
    feed(&mut c, &fake, pad_end());
    assert!(near(norm(&c, "a", CUTOFF), 0.25));
}

#[test]
fn the_pad_takes_over_from_the_first_hand_that_pinched() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.25);
    set_norm(&mut c, "b", RESONANCE, 0.25);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "b", RESONANCE);
    // One hand pinches and twists the newer knob.
    // (A hand of 0.2 frame heights turns it at two thirds of the full scale.)
    feed(
        &mut c,
        &fake,
        vec![open_frame(0.0), pinch_start(), twist(81.0)],
    );
    assert!(
        near(norm(&c, "b", RESONANCE), 0.45),
        "{}",
        norm(&c, "b", RESONANCE)
    );
    // The second hand pinches: the single pinch is one undo step, the pad another.
    feed(
        &mut c,
        &fake,
        vec![
            TrackerEvent::Gesture(GestureEvent::PinchStart {
                hand: HandId(1),
                handedness: Handedness::Left,
            }),
            TrackerEvent::Gesture(GestureEvent::PadStart),
            both_pinched(0.1, pad(0.5, 0.5)),
            both_pinched(0.2, pad(0.4, 0.5)),
        ],
    );
    assert!(near(norm(&c, "a", CUTOFF), 0.375));
    feed(&mut c, &fake, pad_end());
    assert!(c.undo());
    assert!(near(norm(&c, "a", CUTOFF), 0.25));
    assert!(
        near(norm(&c, "b", RESONANCE), 0.45),
        "the twist is still there"
    );
    assert!(c.undo());
    assert!(near(norm(&c, "b", RESONANCE), 0.25));
}

#[test]
fn the_preview_shows_the_pad_as_the_mirrored_crosshair() {
    let (mut c, fake) = core();
    assert!(c.camera_preview_state().pad.is_none());
    feed(&mut c, &fake, vec![both_pinched(0.1, pad(0.3, 0.6))]);
    let (x, y, spread) = c.camera_preview_state().pad.expect("a crosshair");
    assert!(
        near(x, 0.7) && near(y, 0.6) && near(spread, 0.5),
        "{x} {y} {spread}"
    );
    feed(&mut c, &fake, vec![open_frame(0.2)]);
    assert!(c.camera_preview_state().pad.is_none());
}

fn no_takeover(o: &mut MapOptions) {
    o.pickup = false;
}

fn map_pad(c: &mut AppCore<RecordingSink>, control: CameraControl, target: MidiTarget) {
    let source = MidiSource::camera(CameraHand::Both, control);
    c.apply_command(Command::MidiMap(MidiMapCommand::Learn { source, target }))
        .unwrap();
    let id = c.doc().midi_maps.iter().map(|m| m.id).max().unwrap();
    let mut m = c.doc().midi_map(id).unwrap().clone();
    no_takeover(&mut m.options);
    c.apply_command(Command::MidiMap(MidiMapCommand::Set(m)))
        .unwrap();
}

fn param_target(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

#[test]
fn in_midi_mode_the_pad_sends_controllers_24_to_26_on_channel_3() {
    let (mut c, fake) = core();
    mode_of(&mut c, CameraMode::Midi);
    map_pad(&mut c, CameraControl::PadX, param_target("a", CUTOFF));
    map_pad(&mut c, CameraControl::PadY, param_target("b", CUTOFF));
    map_pad(
        &mut c,
        CameraControl::PadSpread,
        param_target("b", RESONANCE),
    );
    let source = &c.doc().midi_maps[0].source;
    assert_eq!(source.port.as_deref(), Some(CAMERA_PORT));
    assert_eq!(source.channel, Some(2), "channel 3");
    feed(&mut c, &fake, vec![both_pinched(0.1, pad(0.5, 0.5))]);
    // x 0.5 and y 0.5 are the middle of the band (CC 64 of 127); the spread of 0.5 frame heights
    // is (0.5 - 0.15) / 1.05 of its range.
    assert!(
        (norm(&c, "a", CUTOFF) - 0.5).abs() < 0.01,
        "{}",
        norm(&c, "a", CUTOFF)
    );
    assert!(
        (norm(&c, "b", CUTOFF) - 0.5).abs() < 0.01,
        "{}",
        norm(&c, "b", CUTOFF)
    );
    assert!(
        (norm(&c, "b", RESONANCE) - 0.333).abs() < 0.01,
        "{}",
        norm(&c, "b", RESONANCE)
    );
    // The hands move up and apart: height and spread rise, the controllers hold when they let go.
    feed(
        &mut c,
        &fake,
        vec![both_pinched(
            0.3,
            Some(Pad {
                x: 0.5,
                y: 0.1,
                spread: 1.2,
            }),
        )],
    );
    assert!(near(norm(&c, "b", CUTOFF), 1.0), "the top of the band");
    assert!(near(norm(&c, "b", RESONANCE), 1.0));
    feed(&mut c, &fake, vec![open_frame(0.5)]);
    assert!(near(norm(&c, "b", CUTOFF), 1.0), "held, not reset to 0");
}

#[test]
fn in_both_mode_the_pad_turns_the_knobs_and_sends_only_its_spread() {
    let (mut c, fake) = core();
    mode_of(&mut c, CameraMode::Both);
    set_norm(&mut c, "a", CUTOFF, 0.25);
    set_norm(&mut c, "b", RESONANCE, 0.25);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "b", RESONANCE);
    map_pad(&mut c, CameraControl::PadX, param_target("b", CUTOFF));
    map_pad(
        &mut c,
        CameraControl::PadSpread,
        param_target("a", RESONANCE),
    );
    let (x_before, spread_before) = (norm(&c, "b", CUTOFF), norm(&c, "a", RESONANCE));
    let mut events = pinch_both();
    events.push(both_pinched(0.1, pad(0.5, 0.5)));
    events.push(both_pinched(0.2, pad(0.4, 0.5)));
    feed(&mut c, &fake, events);
    assert!(near(norm(&c, "a", CUTOFF), 0.375), "the pad turns the knob");
    assert!(
        near(norm(&c, "b", CUTOFF), x_before),
        "the position is not sent as well: one gesture never drives two targets"
    );
    assert!(
        !near(norm(&c, "a", RESONANCE), spread_before),
        "the spread is sent"
    );
}

// ---- The gestures as notes --------------------------------------------------------------------

fn learned(c: &AppCore<RecordingSink>) -> Vec<MidiSource> {
    c.doc().midi_maps.iter().map(|m| m.source.clone()).collect()
}

#[test]
fn each_gesture_note_can_be_learned_and_then_follows_the_gesture() {
    for (event, control, hand, note) in [
        (
            pose_start(Pose::Fist),
            CameraControl::Fist,
            CameraHand::Right,
            62,
        ),
        (
            pose_start(Pose::ThumbsUp),
            CameraControl::ThumbsUp,
            CameraHand::Right,
            63,
        ),
        (
            pose_start(Pose::Point),
            CameraControl::Point,
            CameraHand::Right,
            64,
        ),
        (
            pose_start(Pose::Peace),
            CameraControl::Peace,
            CameraHand::Right,
            65,
        ),
        (
            swipe(SwipeDirection::Left),
            CameraControl::SwipeLeft,
            CameraHand::Right,
            66,
        ),
        (
            swipe(SwipeDirection::Right),
            CameraControl::SwipeRight,
            CameraHand::Right,
            67,
        ),
    ] {
        let (mut c, fake) = core();
        assert!(c.midi_arm(param_target("a", CUTOFF)));
        // No frame first: the controllers of a frame would be learned before the note.
        feed(&mut c, &fake, vec![event]);
        assert_eq!(
            learned(&c),
            [MidiSource::camera(hand, control)],
            "note {note}"
        );
        assert_eq!(
            c.doc().midi_maps[0].source.camera_control(),
            Some((hand, control))
        );
        assert_eq!(control.kind(), atomik_model::SourceKind::Note(note));
    }
}

#[test]
fn a_learned_gesture_note_runs_its_mapping_as_a_button() {
    let (mut c, fake) = core();
    mode_of(&mut c, CameraMode::Midi);
    set_norm(&mut c, "a", CUTOFF, 0.0);
    let source = MidiSource::camera(CameraHand::Right, CameraControl::Peace);
    c.apply_command(Command::MidiMap(MidiMapCommand::Learn {
        source,
        target: param_target("a", CUTOFF),
    }))
    .unwrap();
    let before = norm(&c, "a", CUTOFF);
    feed(
        &mut c,
        &fake,
        vec![open_frame(1.0), pose_start(Pose::Peace)],
    );
    assert!(
        norm(&c, "a", CUTOFF) > before,
        "the note on toggled the mapped knob"
    );
    // The note goes off with the pose: no hanging note.
    feed(&mut c, &fake, vec![pose_end(Pose::Peace), open_frame(2.0)]);
}
