//! The webcam controller in the app core, driven by a fake tracker: the camera target follows the
//! last press, a pinch and a twist turn it through the same gesture a mapped controller makes,
//! the camera starts and stops, and the consent dialog never fetches before Download.

use super::*;
use crate::app_core::tests::rack_only_core;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::{SYNTH as PLUGIN_SYNTH, add_plugin, plugin_core, post, settle};
use crate::sink::RecordingSink;
use crate::vision_link::FakeVision;
use atomik_engine::TransportState;
use atomik_model::automation_rec::WriteMode;
use atomik_model::{AutomationPoint, ClipContent, SongCommand};
use atomik_plugin_host::{ParamPhase, PluginMessage};
use atomik_ui::{CameraDialogChoice, UiAction};
use atomik_vision::Handedness;
use std::time::Duration;

const SYNTH: &str = "atomik.poly-synth";
const CUTOFF: &str = "filter.cutoff";
const BAR: i64 = 4 * atomik_core::PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Two synths `a` and `b`, a fake camera with the models ready and the camera on.
fn core() -> (AppCore<RecordingSink>, FakeVision) {
    let mut c = rack_only_core();
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    c.sink_mut().take();
    let fake = FakeVision::new(ModelState::Ready);
    c.set_vision(Arc::new(fake.clone()));
    c.camera_on();
    assert!(c.camera_is_on());
    (c, fake)
}

fn touch<S: EngineSink>(c: &mut AppCore<S>, device: &str, param: &str) {
    c.handle_action(UiAction::ParamTouched {
        device: key(device),
        param: param.into(),
    });
}

fn hand_state(depth: f32, pinch: bool) -> HandState {
    HandState {
        id: HandId(0),
        present: true,
        handedness: Handedness::Right,
        pinch,
        pinch_strength: if pinch { 1.0 } else { 0.0 },
        twist_angle: 0.0,
        palm_center: (0.5, 0.5),
        palm_height: 0.2,
        hand_size: depth,
        depth_proxy: depth,
    }
}

/// A frame report with one hand of size `depth`.
fn frame(depth: f32) -> TrackerEvent {
    TrackerEvent::Frame(FrameReport {
        t: 0.0,
        hands: vec![hand_state(depth, false)],
        fps: 30.0,
        infer_ms: 20.0,
    })
}

fn pinch_start() -> TrackerEvent {
    pinch_start_of(Handedness::Right)
}

fn pinch_start_of(handedness: Handedness) -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::PinchStart {
        hand: HandId(0),
        handedness,
    })
}

fn twist(degrees: f32) -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::Twist {
        hand: HandId(0),
        delta_radians: degrees.to_radians(),
    })
}

fn pinch_end() -> TrackerEvent {
    TrackerEvent::Gesture(GestureEvent::PinchEnd { hand: HandId(0) })
}

fn feed(c: &mut AppCore<RecordingSink>, fake: &FakeVision, events: Vec<TrackerEvent>) {
    fake.push(events);
    c.camera_frame();
}

fn desc(
    device: &str,
    param: &str,
    c: &AppCore<impl EngineSink>,
) -> &'static atomik_core::ParamDescriptor {
    let d = c.doc().resolve(&key(device)).unwrap();
    c.registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
}

fn value(c: &AppCore<impl EngineSink>, device: &str, p: &str) -> f32 {
    c.doc().param_value(&key(device), p, c.registry()).unwrap()
}

/// The value of `p` as a fraction of its range.
fn norm(c: &AppCore<impl EngineSink>, device: &str, p: &str) -> f32 {
    desc(device, p, c).to_normalized(value(c, device, p))
}

/// Sets `p` to the fraction `n` of its range.
fn set_norm(c: &mut AppCore<RecordingSink>, device: &str, p: &str, n: f32) {
    let plain = desc(device, p, c).from_normalized(n);
    c.apply_command(Command::SetParam {
        key: key(device),
        param: p.into(),
        value: plain,
        merge: None,
    })
    .unwrap();
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// ---- The target -------------------------------------------------------------------------------

#[test]
fn a_press_on_a_knob_sets_the_target_and_another_press_replaces_it() {
    let (mut c, _fake) = core();
    assert_eq!(c.camera_target(), None);
    touch(&mut c, "a", CUTOFF);
    assert_eq!(c.camera_target(), Some(&(key("a"), CUTOFF.to_owned())));
    touch(&mut c, "b", "filter.resonance");
    assert_eq!(
        c.camera_target(),
        Some(&(key("b"), "filter.resonance".to_owned()))
    );
}

#[test]
fn the_target_is_marked_in_the_views_while_the_camera_is_on_and_never_saved() {
    let (mut c, _fake) = core();
    touch(&mut c, "a", CUTOFF);
    assert_eq!(
        c.midi_marks().camera,
        Some((key("a"), CUTOFF.to_owned())),
        "the ring"
    );
    let saved = atomik_model::save_project(c.doc());
    assert!(!saved.contains("camera"), "view state only");
    c.camera_off(None);
    assert_eq!(c.midi_marks().camera, None, "no ring with the camera off");
    assert!(c.camera_target().is_some(), "the target is kept");
}

#[test]
fn deleting_the_target_device_clears_it() {
    let (mut c, _fake) = core();
    touch(&mut c, "b", CUTOFF);
    assert!(c.camera_target().is_some());
    c.apply_command(Command::RemoveDevice { key: key("b") })
        .unwrap();
    assert_eq!(c.camera_target(), None);
}

// ---- Turning it -------------------------------------------------------------------------------

#[test]
fn a_135_degree_twist_with_a_270_degree_turn_moves_the_target_by_half_its_range() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.25);
    touch(&mut c, "a", CUTOFF);
    // A far hand: full scale.
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), twist(135.0)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.75),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // Twisting back turns it back.
    feed(&mut c, &fake, vec![twist(-135.0), pinch_end()]);
    assert!(near(norm(&c, "a", CUTOFF), 0.25));
}

#[test]
fn the_full_turn_setting_scales_the_twist_and_the_range_is_never_passed() {
    let (mut c, fake) = core();
    let mut prefs = c.camera_prefs().clone();
    prefs.full_turn = 90;
    c.set_camera_prefs(prefs);
    set_norm(&mut c, "a", CUTOFF, 0.5);
    touch(&mut c, "a", CUTOFF);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), twist(45.0)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 1.0),
        "half a quarter turn is half a range"
    );
    feed(&mut c, &fake, vec![twist(300.0)]);
    assert!(near(norm(&c, "a", CUTOFF), 1.0), "clamped at the top");
    // No wind-up: turning back moves it at once.
    feed(&mut c, &fake, vec![twist(-9.0)]);
    assert!(norm(&c, "a", CUTOFF) < 1.0 - 0.05);
}

#[test]
fn one_pinch_is_one_undo_step() {
    let (mut c, fake) = core();
    set_norm(&mut c, "a", CUTOFF, 0.2);
    let before = value(&c, "a", CUTOFF);
    touch(&mut c, "a", CUTOFF);
    feed(
        &mut c,
        &fake,
        vec![
            frame(0.1),
            pinch_start(),
            twist(10.0),
            twist(10.0),
            twist(10.0),
        ],
    );
    feed(&mut c, &fake, vec![twist(10.0), pinch_end()]);
    let after = value(&c, "a", CUTOFF);
    assert!(after > before);
    // The next pinch is a step of its own.
    feed(&mut c, &fake, vec![pinch_start(), twist(20.0), pinch_end()]);
    assert!(value(&c, "a", CUTOFF) > after);
    assert!(c.undo());
    assert_eq!(
        value(&c, "a", CUTOFF),
        after,
        "the second pinch is one step"
    );
    assert!(c.undo());
    assert_eq!(value(&c, "a", CUTOFF), before, "so is the first");
}

#[test]
fn fine_control_scales_the_move_by_the_distance_of_the_hand() {
    let (mut c, fake) = core();
    touch(&mut c, "a", CUTOFF);
    // A near hand (big): a quarter of the move.
    set_norm(&mut c, "a", CUTOFF, 0.25);
    feed(
        &mut c,
        &fake,
        vec![frame(0.4), pinch_start(), twist(135.0), pinch_end()],
    );
    assert!(
        near(norm(&c, "a", CUTOFF), 0.25 + 0.125),
        "{}",
        norm(&c, "a", CUTOFF)
    );
    // A far hand (small): the whole move.
    set_norm(&mut c, "a", CUTOFF, 0.25);
    feed(
        &mut c,
        &fake,
        vec![frame(0.05), pinch_start(), twist(135.0), pinch_end()],
    );
    assert!(near(norm(&c, "a", CUTOFF), 0.75));
    // With the setting off the near hand is as coarse as the far one.
    let mut prefs = c.camera_prefs().clone();
    prefs.fine = false;
    c.set_camera_prefs(prefs);
    set_norm(&mut c, "a", CUTOFF, 0.25);
    feed(
        &mut c,
        &fake,
        vec![frame(0.4), pinch_start(), twist(135.0), pinch_end()],
    );
    assert!(near(norm(&c, "a", CUTOFF), 0.75));
}

#[test]
fn a_hand_whose_size_is_not_known_yet_gets_its_scale_from_the_next_frame() {
    let (mut c, fake) = core();
    touch(&mut c, "a", CUTOFF);
    set_norm(&mut c, "a", CUTOFF, 0.25);
    // The gesture comes before any report of the hand: the twist waits for its size.
    feed(&mut c, &fake, vec![pinch_start(), twist(135.0)]);
    assert!(near(norm(&c, "a", CUTOFF), 0.25), "waiting");
    feed(&mut c, &fake, vec![frame(0.4)]);
    assert!(
        near(norm(&c, "a", CUTOFF), 0.375),
        "{}",
        norm(&c, "a", CUTOFF)
    );
}

#[test]
fn a_stepped_param_steps_cleanly() {
    let (mut c, fake) = core();
    // A stepped param of the synth with several steps.
    let d = c.doc().resolve(&key("a")).unwrap().type_id.clone();
    let stepped = c
        .registry()
        .get(&d)
        .unwrap()
        .descriptor
        .params
        .iter()
        .find(|p| p.discrete && p.max - p.min >= 3.0 && (p.default - p.min).abs() < 1e-6)
        .copied()
        .expect("a stepped param");
    let steps = stepped.max - stepped.min;
    let step_deg = 270.0 / steps;
    touch(&mut c, "a", stepped.key);
    let start = value(&c, "a", stepped.key);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start()]);
    // A little less than one step width of twist: nothing moves.
    feed(&mut c, &fake, vec![twist(step_deg * 0.9)]);
    assert_eq!(value(&c, "a", stepped.key), start);
    // Trembling at the boundary does not flicker.
    feed(
        &mut c,
        &fake,
        vec![twist(step_deg * 0.2), twist(-step_deg * 0.05)],
    );
    assert_eq!(value(&c, "a", stepped.key), start + 1.0);
    feed(&mut c, &fake, vec![twist(-step_deg * 0.1)]);
    assert_eq!(
        value(&c, "a", stepped.key),
        start + 1.0,
        "{} still one",
        value(&c, "a", stepped.key)
    );
    // Two more steps.
    feed(&mut c, &fake, vec![twist(step_deg * 1.1), pinch_end()]);
    assert_eq!(value(&c, "a", stepped.key), (start + 2.0).min(stepped.max));
}

#[test]
fn a_pinch_with_no_target_says_to_click_a_knob_first_and_changes_nothing() {
    let (mut c, fake) = core();
    let a = value(&c, "a", CUTOFF);
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), twist(90.0)]);
    assert_eq!(
        c.camera_preview_state().message.as_deref(),
        Some(NO_TARGET_MESSAGE)
    );
    assert_eq!(c.camera_preview_state().lamp, CameraLamp::Pinch);
    assert_eq!(value(&c, "a", CUTOFF), a);
    feed(&mut c, &fake, vec![pinch_end()]);
    // Pressing a knob clears the message.
    touch(&mut c, "a", CUTOFF);
    assert_eq!(c.camera_preview_state().message, None);
}

#[test]
fn the_hand_choice_turns_the_other_hand_away() {
    let (mut c, fake) = core();
    let mut prefs = c.camera_prefs().clone();
    prefs.hand = crate::keymap_store::HandChoice::Left;
    c.set_camera_prefs(prefs);
    touch(&mut c, "a", CUTOFF);
    let before = value(&c, "a", CUTOFF);
    feed(
        &mut c,
        &fake,
        vec![
            frame(0.1),
            pinch_start_of(Handedness::Right),
            twist(90.0),
            pinch_end(),
        ],
    );
    assert_eq!(value(&c, "a", CUTOFF), before, "the right hand is ignored");
    feed(
        &mut c,
        &fake,
        vec![pinch_start_of(Handedness::Left), twist(90.0), pinch_end()],
    );
    assert_ne!(value(&c, "a", CUTOFF), before);
}

#[test]
fn the_state_of_the_hand_and_the_target_while_pinched_show_in_the_preview() {
    let (mut c, fake) = core();
    touch(&mut c, "a", CUTOFF);
    assert_eq!(c.camera_preview_state().lamp, CameraLamp::NoHand);
    feed(&mut c, &fake, vec![frame(0.1)]);
    assert_eq!(c.camera_preview_state().lamp, CameraLamp::Hand);
    assert_eq!(c.camera_preview_state().target, None, "only while pinched");
    feed(&mut c, &fake, vec![pinch_start()]);
    let s = c.camera_preview_state();
    assert_eq!(s.lamp, CameraLamp::Pinch);
    assert!(
        s.target.as_deref().is_some_and(|t| t.contains("Cutoff")),
        "{s:?}"
    );
    assert!(s.value.is_some());
    assert_eq!((s.fps, s.infer_ms), (30.0, 20.0));
    feed(&mut c, &fake, vec![pinch_end()]);
    assert_eq!(c.camera_preview_state().lamp, CameraLamp::Hand);
}

// ---- Automation -------------------------------------------------------------------------------

/// A clip written to a lane: start, length and its points.
type Written = (i64, i64, Vec<(i64, f32)>);

fn at(c: &mut AppCore<RecordingSink>, tick: i64) {
    c.sink_mut().state = TransportState {
        playing: true,
        tick,
        engine_frame: 1000 + tick as u64,
        loop_end: BAR,
        ..TransportState::default()
    };
}

#[test]
fn touch_automation_writes_only_while_pinched() {
    let (mut c, fake) = core();
    for cmd in [
        Command::Song(SongCommand::AddTrack { device: key("a") }),
        Command::Song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: CUTOFF.into(),
        }),
        Command::Song(SongCommand::SetLaneWriteMode {
            device: key("a"),
            param: CUTOFF.into(),
            mode: WriteMode::Touch,
        }),
    ] {
        c.apply_command(cmd).unwrap();
    }
    c.handle_action(UiAction::ToggleLaneArm {
        device: key("a"),
        param: CUTOFF.into(),
    });
    set_norm(&mut c, "a", CUTOFF, 0.3);
    touch(&mut c, "a", CUTOFF);
    at(&mut c, 0);
    c.transport_action(atomik_ui::TransportAction::ToggleRecord);
    // Open hand: nothing is written.
    at(&mut c, BAR / 8);
    feed(&mut c, &fake, vec![frame(0.1)]);
    // The fingers come together a quarter of the way in, and the hand twists.
    at(&mut c, BAR / 4);
    feed(&mut c, &fake, vec![pinch_start(), twist(30.0)]);
    at(&mut c, BAR / 2);
    feed(&mut c, &fake, vec![twist(30.0)]);
    // They open at three quarters; the touch ends there.
    at(&mut c, 3 * BAR / 4);
    feed(&mut c, &fake, vec![pinch_end()]);
    at(&mut c, BAR);
    c.transport_action(atomik_ui::TransportAction::Stop);
    let clips: Vec<Written> = c.doc().song.track(&key("a")).unwrap().lanes[1]
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
        .collect();
    assert_eq!(clips.len(), 1, "{clips:?}");
    assert_eq!(clips[0].0, BAR / 4, "from the pinch");
    assert_eq!(clips[0].0 + clips[0].1, 3 * BAR / 4, "to the release");
    assert!(clips[0].2.len() >= 2, "{:?}", clips[0].2);
}

// ---- A plugin -----------------------------------------------------------------------------------

fn plugin_edit(core: &mut AppCore<OfflineSink>, id: u32, phase: ParamPhase, value: f64) {
    post(
        core,
        &key("lead"),
        PluginMessage::ParamEdit { id, value, phase },
    );
    core.frame();
}

#[test]
fn a_touch_in_a_plugin_window_sets_the_target_and_a_plugin_param_turns_in_one_gesture() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, PLUGIN_SYNTH, "lead");
    core.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("lead"),
    }))
    .unwrap();
    settle(&mut core);
    let fake = FakeVision::new(ModelState::Ready);
    core.set_vision(Arc::new(fake.clone()));
    core.camera_on();
    assert_eq!(core.camera_target(), None);
    // The editor sends a gesture begin: that is a click, with no Learn mode armed.
    plugin_edit(&mut core, 1, ParamPhase::Begin, f64::NAN);
    plugin_edit(&mut core, 1, ParamPhase::End, f64::NAN);
    assert_eq!(core.camera_target(), Some(&(key("lead"), "p1".to_owned())));
    // A plugin that sends no gesture begin: its first change after the gap is the click.
    plugin_edit(&mut core, 2, ParamPhase::Change, 0.5);
    assert_eq!(core.camera_target(), Some(&(key("lead"), "p2".to_owned())));
    // The pinch turns the plugin's param: begin, changes and one end, one undo step.
    let d = core.doc().resolve(&key("lead")).unwrap().type_id.clone();
    let p = *core
        .registry()
        .get(&d)
        .unwrap()
        .descriptor
        .param_by_key("p2")
        .unwrap();
    let start = core
        .doc()
        .param_value(&key("lead"), "p2", core.registry())
        .unwrap();
    let depth = core.document.undo_depth();
    fake.push([
        frame(0.1),
        pinch_start(),
        twist(30.0),
        twist(30.0),
        twist(30.0),
        pinch_end(),
    ]);
    core.camera_frame();
    let end = core
        .doc()
        .param_value(&key("lead"), "p2", core.registry())
        .unwrap();
    assert_ne!(start, end, "{p:?}");
    assert_eq!(core.document.undo_depth(), depth + 1, "one step");
}

// ---- The switch ---------------------------------------------------------------------------------

fn wait_for(
    c: &mut AppCore<RecordingSink>,
    what: &str,
    done: impl Fn(&AppCore<RecordingSink>) -> bool,
) {
    // Up to five seconds, counted in the sleeps (the tests read no clock).
    for _ in 0..2500 {
        if done(c) {
            return;
        }
        c.camera_frame();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(done(c), "timed out waiting for {what}");
}

#[test]
fn turning_the_camera_on_with_missing_models_asks_first_and_fetches_nothing() {
    let mut c = rack_only_core();
    let fake = FakeVision::new(ModelState::Missing);
    c.set_vision(Arc::new(fake.clone()));
    c.camera_on();
    assert!(!c.camera_is_on());
    let Some(CameraDialogState::Consent { source, bytes }) = c.camera_dialog().cloned() else {
        panic!("the consent dialog should be open: {:?}", c.camera_dialog());
    };
    assert_eq!(source, "storage.googleapis.com");
    assert!(bytes > 0);
    c.camera_frame();
    assert_eq!(fake.fetches(), 0, "nothing is fetched before Download");
    assert_eq!(fake.started(), 0);
    // Cancel closes it and still fetches nothing.
    c.camera_dialog_choice(CameraDialogChoice::Cancel);
    assert!(c.camera_dialog().is_none());
    assert_eq!(fake.fetches(), 0);
}

#[test]
fn download_fetches_on_a_worker_then_turns_the_camera_on() {
    let mut c = rack_only_core();
    let fake = FakeVision::new(ModelState::Missing);
    c.set_vision(Arc::new(fake.clone()));
    c.camera_toggle();
    c.camera_dialog_choice(CameraDialogChoice::Download);
    assert!(matches!(
        c.camera_dialog(),
        Some(CameraDialogState::Downloading { .. })
    ));
    wait_for(&mut c, "the camera", |c| c.camera_is_on());
    assert_eq!(fake.fetches(), 2);
    assert!(c.camera_dialog().is_none());
    assert_eq!(fake.started(), 1);
}

#[test]
fn a_failed_download_says_why_and_retry_tries_again() {
    let mut c = rack_only_core();
    let fake = FakeVision::new(ModelState::Missing);
    fake.fail_download(Some("The server answered 500"));
    c.set_vision(Arc::new(fake.clone()));
    c.camera_on();
    c.camera_dialog_choice(CameraDialogChoice::Download);
    wait_for(&mut c, "the failure", |c| {
        matches!(c.camera_dialog(), Some(CameraDialogState::Failed { .. }))
    });
    assert!(!c.camera_is_on());
    let Some(CameraDialogState::Failed { message }) = c.camera_dialog() else {
        unreachable!()
    };
    assert_eq!(message, "The server answered 500");
    fake.fail_download(None);
    c.camera_dialog_choice(CameraDialogChoice::Retry);
    wait_for(&mut c, "the camera", |c| c.camera_is_on());
}

#[test]
fn turning_it_off_stops_the_tracker_and_so_does_a_camera_error() {
    let (mut c, fake) = core();
    assert_eq!((fake.started(), fake.stopped()), (1, 0));
    c.camera_toggle();
    assert!(!c.camera_is_on());
    assert_eq!(fake.stopped(), 1, "the camera is released");
    // On again, then an error: the tracker has stopped and the user is told.
    c.camera_toggle();
    assert!(c.camera_is_on());
    feed(
        &mut c,
        &fake,
        vec![TrackerEvent::Failed("the camera was unplugged".into())],
    );
    assert!(!c.camera_is_on());
    assert_eq!(fake.stopped(), 2);
    assert!(
        c.camera_preview_state()
            .message
            .is_some_and(|m| m.contains("unplugged"))
    );
}

#[test]
fn a_pinch_open_when_the_camera_goes_off_ends_its_gesture() {
    let (mut c, fake) = core();
    touch(&mut c, "a", CUTOFF);
    set_norm(&mut c, "a", CUTOFF, 0.2);
    let depth = c.document.undo_depth();
    feed(&mut c, &fake, vec![frame(0.1), pinch_start(), twist(30.0)]);
    c.camera_off(None);
    assert_eq!(c.document.undo_depth(), depth + 1);
    // The next move is not merged into the pinch.
    set_norm(&mut c, "a", CUTOFF, 0.9);
    assert_eq!(c.document.undo_depth(), depth + 2);
}

#[test]
fn the_camera_is_off_by_default_and_the_settings_choose_the_camera_and_the_thresholds() {
    let mut c = rack_only_core();
    let fake = FakeVision::new(ModelState::Ready);
    c.set_vision(Arc::new(fake.clone()));
    assert!(!c.camera_is_on());
    assert_eq!(fake.started(), 0);
    let mut prefs = c.camera_prefs().clone();
    prefs.camera = Some("fake-0".into());
    prefs.sensitivity = 120;
    c.set_camera_prefs(prefs);
    c.camera_on();
    assert_eq!(fake.stats.camera.lock().unwrap().as_deref(), Some("fake-0"));
    assert_eq!(fake.stats.sensitivity.load(Ordering::SeqCst), 1200);
    // A new sensitivity starts the tracker again with it.
    let mut prefs = c.camera_prefs().clone();
    prefs.sensitivity = 80;
    c.set_camera_prefs(prefs);
    assert!(c.camera_is_on());
    assert_eq!(fake.started(), 2);
    assert_eq!(fake.stopped(), 1);
}

#[test]
fn deleting_the_models_turns_the_camera_off() {
    let (mut c, fake) = core();
    c.camera_delete_models();
    assert!(!c.camera_is_on());
    assert_eq!(fake.stats.deleted.load(Ordering::SeqCst), 1);
    assert_eq!(c.camera_models(), ModelState::Missing);
    // With the models gone the next start asks again.
    c.camera_on();
    assert!(matches!(
        c.camera_dialog(),
        Some(CameraDialogState::Consent { .. })
    ));
}

#[test]
fn the_fine_scale_runs_from_a_quarter_to_the_whole_range() {
    assert_eq!(fine_scale(true, 0.0), 1.0);
    assert_eq!(fine_scale(true, 0.12), 1.0);
    assert_eq!(fine_scale(true, 0.30), 0.25);
    assert_eq!(fine_scale(true, 0.9), 0.25);
    let mid = fine_scale(true, 0.21);
    assert!(mid > 0.25 && mid < 1.0);
    assert_eq!(fine_scale(false, 0.9), 1.0);
    assert_eq!(fine_scale(true, f32::NAN), 1.0);
}

#[test]
fn a_core_without_a_camera_cannot_turn_one_on() {
    let mut c = rack_only_core();
    c.camera_toggle();
    assert!(!c.camera_is_on());
    assert!(c.camera_list().is_empty());
}
