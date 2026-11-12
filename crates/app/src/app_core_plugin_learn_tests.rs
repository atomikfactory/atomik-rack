//! Learn in the plugin window: a touch on a control of the plugin's own editor (a gesture begin,
//! or the first change of a plugin that sends none) shows that param's automation lane or arms
//! it for MIDI learn; the edit still goes through; a control that is not a param says so; the
//! mode is dropped by closing the window, by Esc and by the global MIDI Learn mode.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_core::ChannelMessage;
use atomik_model::{LaneKind, MidiTarget, SongCommand};
use atomik_plugin_host::{NativeParent, ParamPhase, PluginMessage};
use atomik_ui::{PluginLearnMode, PluginLearnRequest, PluginLearnStatus};

/// The test synth `lead` on a track of its own.
fn learn_core() -> AppCore<OfflineSink> {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    apply(
        &mut core,
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
    );
    settle(&mut core);
    core
}

fn edit(core: &mut AppCore<OfflineSink>, id: u32, phase: ParamPhase, value: f64) {
    post(
        core,
        &key("lead"),
        PluginMessage::ParamEdit { id, value, phase },
    );
    core.frame();
}

/// A whole gesture on `id` in the plugin's window, ending at `value`.
fn gesture(core: &mut AppCore<OfflineSink>, id: u32, value: f64) {
    edit(core, id, ParamPhase::Begin, f64::NAN);
    edit(core, id, ParamPhase::Change, value);
    edit(core, id, ParamPhase::End, f64::NAN);
}

fn name_of(core: &AppCore<OfflineSink>, id: u32) -> String {
    let d = core.doc().resolve(&key("lead")).unwrap();
    core.registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .param_by_key(&format!("p{id}"))
        .unwrap()
        .name
        .to_owned()
}

fn lanes(core: &AppCore<OfflineSink>) -> Vec<String> {
    core.doc()
        .song
        .track(&key("lead"))
        .unwrap()
        .lanes
        .iter()
        .filter_map(|l| match &l.kind {
            LaneKind::Automation(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn status(core: &AppCore<OfflineSink>) -> PluginLearnStatus {
    core.plugin_learn_state(&key("lead")).status
}

fn fake_parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xA11C_E000)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xA11C_E000)
    } else {
        NativeParent::X11(0x00A1_1CE0)
    }
}

/// Adds the test gain effect `echo`, which has an editor.
fn add_echo(core: &mut AppCore<OfflineSink>) {
    add_plugin(core, GAIN, "echo");
    settle(core);
}

/// Opens the embedded editor of `device` the way the platform layer does.
fn open_window(core: &mut AppCore<OfflineSink>, device: &DeviceKey) {
    core.handle_action(UiAction::OpenPluginGui {
        device: device.clone(),
    });
    let _ = core.take_plugin_window_requests();
    core.plugin_window_created(
        device,
        PluginWindowInfo {
            parent: fake_parent(),
            scale: 1.0,
        },
    );
    let _ = core.take_plugin_window_requests();
    assert!(core.plugin_gui_open(device));
}

#[test]
fn learn_automation_shows_the_lane_of_the_touched_param_and_stays_armed() {
    let mut core = learn_core();
    let dev = key("lead");
    assert_eq!(core.plugin_learn_mode(&dev), None);
    assert_eq!(status(&core), PluginLearnStatus::Waiting);
    core.handle_action(UiAction::PluginLearn {
        device: dev.clone(),
        request: PluginLearnRequest::Toggle(PluginLearnMode::Automation),
    });
    assert_eq!(
        core.plugin_learn_mode(&dev),
        Some(PluginLearnMode::Automation)
    );
    gesture(&mut core, 2, 0.4);
    assert_eq!(lanes(&core), vec!["p2".to_string()], "the lane was added");
    assert_eq!(
        core.doc().resolve(&dev).unwrap().params.get("p2").copied(),
        Some(0.4),
        "the edit itself went through"
    );
    assert_eq!(
        status(&core),
        PluginLearnStatus::LaneShown(name_of(&core, 2))
    );
    assert_eq!(
        core.plugin_learn_mode(&dev),
        Some(PluginLearnMode::Automation),
        "still armed for the next touch"
    );
    // A second touch, on another control, shows that one's lane.
    gesture(&mut core, 1, 1.0);
    assert_eq!(lanes(&core), vec!["p2".to_string(), "p1".to_string()]);
    assert_eq!(
        status(&core),
        PluginLearnStatus::LaneShown(name_of(&core, 1))
    );
    // Touching the first again adds nothing.
    gesture(&mut core, 2, 0.5);
    assert_eq!(lanes(&core).len(), 2);
    // The button again disarms; a touch then does nothing more.
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Automation));
    assert_eq!(core.plugin_learn_mode(&dev), None);
    edit(&mut core, 9_999, ParamPhase::Begin, f64::NAN);
    assert_eq!(lanes(&core).len(), 2);
    assert_eq!(
        status(&core),
        PluginLearnStatus::LaneShown(name_of(&core, 2)),
        "the strip keeps its last message"
    );
}

#[test]
fn learn_midi_arms_the_touched_param_and_the_next_control_maps_it_in_one_undo_step() {
    let mut core = learn_core();
    let dev = key("lead");
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Midi));
    gesture(&mut core, 2, 0.4);
    let target = MidiTarget::Param {
        device: dev.clone(),
        param: "p2".into(),
    };
    assert_eq!(core.midi_armed(), Some(&target));
    assert_eq!(
        status(&core),
        PluginLearnStatus::MoveHardware(name_of(&core, 2))
    );
    assert_eq!(
        core.doc().resolve(&dev).unwrap().params.get("p2").copied(),
        Some(0.4),
        "the edit went through too"
    );
    assert!(lanes(&core).is_empty(), "MIDI learn shows no lane");
    // CC 74 decides at once (no 14 bit pair to wait for).
    core.inject_midi("Pad", ChannelMessage::cc(0, 74, 64));
    core.midi_control_frame(0.0);
    let maps = &core.doc().midi_maps;
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0].target, target);
    assert!(core.midi_armed().is_none());
    match status(&core) {
        PluginLearnStatus::Mapped { param, source } => {
            assert_eq!(param, name_of(&core, 2));
            assert!(source.starts_with("CC 74 ch 1"), "{source}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        core.plugin_learn_mode(&dev),
        Some(PluginLearnMode::Midi),
        "armed for the next touch"
    );
    // The mapping is one undo step of its own.
    assert!(core.undo());
    assert!(core.doc().midi_maps.is_empty());
    assert_eq!(
        core.doc().resolve(&dev).unwrap().params.get("p2").copied(),
        Some(0.4),
        "the gesture is a step of its own"
    );
    // The next touch arms the next param.
    gesture(&mut core, 1, 1.0);
    assert_eq!(
        core.midi_armed(),
        Some(&MidiTarget::Param {
            device: dev.clone(),
            param: "p1".into(),
        })
    );
}

#[test]
fn a_plugin_that_sends_no_gesture_begin_is_caught_by_its_first_change() {
    let mut core = learn_core();
    let dev = key("lead");
    let p2 = MidiTarget::Param {
        device: dev.clone(),
        param: "p2".into(),
    };
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Automation));
    core.plugin_tick(10.0);
    edit(&mut core, 2, ParamPhase::Change, 0.3);
    assert_eq!(lanes(&core), vec!["p2".to_string()], "the first change");
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Midi));
    core.plugin_tick(10.1);
    edit(&mut core, 2, ParamPhase::Change, 0.32);
    assert_eq!(core.midi_armed(), Some(&p2), "a new mode starts afresh");
    core.midi_cancel_arm();
    // The rest of the same move is not a new touch.
    core.plugin_tick(10.3);
    edit(&mut core, 2, ParamPhase::Change, 0.35);
    core.plugin_tick(10.6);
    edit(&mut core, 2, ParamPhase::Change, 0.36);
    assert!(core.midi_armed().is_none(), "seen less than 0.5 s ago");
    // Still for half a second: a new touch.
    core.plugin_tick(11.2);
    edit(&mut core, 2, ParamPhase::Change, 0.4);
    assert_eq!(core.midi_armed(), Some(&p2));
}

#[test]
fn a_control_that_is_not_a_param_says_it_is_not_automatable() {
    let mut core = learn_core();
    assert!(core.toggle_plugin_learn(&key("lead"), PluginLearnMode::Automation));
    edit(&mut core, 9_999, ParamPhase::Begin, f64::NAN);
    assert_eq!(status(&core), PluginLearnStatus::NotAutomatable);
    assert!(lanes(&core).is_empty());
    let text = core.plugin_learn_state(&key("lead")).status_text();
    assert_eq!(text, "This control is not automatable");
}

#[test]
fn closing_the_window_esc_and_the_global_midi_learn_disarm() {
    let mut core = learn_core();
    let dev = key("lead");
    // Closing the window.
    add_echo(&mut core);
    let echo = key("echo");
    open_window(&mut core, &echo);
    assert!(core.toggle_plugin_learn(&echo, PluginLearnMode::Automation));
    core.plugin_window_closed(&echo);
    assert_eq!(core.plugin_learn_mode(&echo), None);
    // Esc (the shell calls this for Esc in the main window).
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Midi));
    gesture(&mut core, 2, 0.4);
    assert!(core.midi_armed().is_some());
    assert!(core.disarm_all_plugin_learn());
    assert_eq!(core.plugin_learn_mode(&dev), None);
    assert!(
        core.midi_armed().is_none(),
        "the target LEARN MIDI armed is dropped too"
    );
    assert!(!core.disarm_all_plugin_learn(), "nothing left to disarm");
    // The global MIDI Learn mode and the window's modes take turns.
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Automation));
    core.set_midi_learn(true);
    assert_eq!(core.plugin_learn_mode(&dev), None);
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Midi));
    assert!(!core.midi_learn_mode(), "arming a window's mode ends it");
    // At most one mode: arming the other one replaces it.
    assert!(core.toggle_plugin_learn(&dev, PluginLearnMode::Automation));
    assert_eq!(
        core.plugin_learn_mode(&dev),
        Some(PluginLearnMode::Automation)
    );
    core.handle_action(UiAction::PluginLearn {
        device: dev.clone(),
        request: PluginLearnRequest::Off,
    });
    assert_eq!(core.plugin_learn_mode(&dev), None);
}

#[test]
fn the_commands_work_on_the_selected_plugin_and_open_its_window() {
    let mut core = learn_core();
    core.plugin_learn_command(PluginLearnMode::Automation);
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("Select a plugin device"))
    );
    core.view_mut()
        .set_selection(&[key("lead")], Some(&key("lead")));
    core.plugin_learn_command(PluginLearnMode::Automation);
    assert_eq!(
        core.plugin_learn_mode(&key("lead")),
        Some(PluginLearnMode::Automation)
    );
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![],
        "a plugin without an editor learns from the parameter pane"
    );
    // The command again disarms.
    core.plugin_learn_command(PluginLearnMode::Automation);
    assert_eq!(core.plugin_learn_mode(&key("lead")), None);
    // A plugin with an editor: its window opens to be touched.
    add_echo(&mut core);
    let echo = key("echo");
    core.view_mut()
        .set_selection(std::slice::from_ref(&echo), Some(&echo));
    core.plugin_learn_command(PluginLearnMode::Midi);
    assert_eq!(core.plugin_learn_mode(&echo), Some(PluginLearnMode::Midi));
    assert!(
        matches!(
            core.take_plugin_window_requests().as_slice(),
            [PluginWindowRequest::Open { .. }]
        ),
        "the editor opens to be touched"
    );
    // The window the user brought forward last wins over the selection.
    core.plugin_window_created(
        &echo,
        PluginWindowInfo {
            parent: fake_parent(),
            scale: 1.0,
        },
    );
    core.plugin_window_focused(&echo);
    core.view_mut()
        .set_selection(&[key("lead")], Some(&key("lead")));
    assert_eq!(core.plugin_learn_target(), Some(echo.clone()));
    core.plugin_window_closed(&echo);
    assert_eq!(core.plugin_learn_target(), Some(key("lead")));
}

#[test]
fn a_row_of_the_parameter_pane_is_a_touch_too() {
    let mut core = learn_core();
    assert!(core.toggle_plugin_learn(&key("lead"), PluginLearnMode::Automation));
    core.handle_action(UiAction::ParamTouched {
        device: key("lead"),
        param: "p2".into(),
    });
    assert_eq!(lanes(&core), vec!["p2".to_string()]);
}
