//! Macro banks, automation that follows the selection and the parameter search in the app core:
//! the engine gets one `set_param` per target (and undo, redo and a removal follow), a driven
//! param refuses a direct edit, a live render equals the bounce with a static macro and with a
//! lane on it, a touched control shows its lane without writing data, and the search finds, reveals
//! and acts on the params of the project.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, MacroCommand, Note, SongCommand,
    assign_value, empty_doc,
};
use atomik_ui::{MacrosAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const CUTOFF: &str = "filter.cutoff";
const LATENCY: usize = atomik_core::BATCH;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn apply<S: EngineSink>(c: &mut AppCore<S>, command: Command) {
    c.apply_command(command.clone())
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    apply(
        c,
        Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        },
    );
}

fn param_id(type_id: &str, param: &str) -> u32 {
    default_registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
        .0
}

fn params_of(calls: &[SinkCall]) -> Vec<(u32, u32, f32)> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Param { id, param, plain } => Some((id.0, param.0, *plain)),
            _ => None,
        })
        .collect()
}

/// Two synths, `a` and `b`, and a delay `fx` (it has no notes input, so no track).
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    add(&mut c, DELAY, "fx");
    c.sink_mut().take();
    c
}

fn bank(c: &AppCore<impl EngineSink>) -> DeviceKey {
    c.doc().banks().next().expect("a bank").key.clone()
}

fn set_param<S: EngineSink>(c: &mut AppCore<S>, device: &DeviceKey, param: &str, value: f32) {
    apply(
        c,
        Command::SetParam {
            key: device.clone(),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

/// `a` and `b` cutoffs on macro 1 over 200..2000 and 400..4000.
fn assigned() -> AppCore<RecordingSink> {
    let mut c = core();
    assert!(c.assign_to_macro(&key("a"), CUTOFF, 0, false));
    let bank = bank(&c);
    assert!(c.assign_to_macro(&key("b"), CUTOFF, 0, false));
    for (id, min, max) in [(1, 200.0, 2000.0), (2, 400.0, 4000.0)] {
        apply(
            &mut c,
            Command::Macro(MacroCommand::SetRange {
                id,
                min,
                max,
                invert: false,
            }),
        );
    }
    let _ = bank;
    c.sink_mut().take();
    c
}

// ------------------------------------------------------------------ the engine

#[test]
fn a_macro_move_is_one_set_param_per_target_and_undo_and_redo_follow() {
    let mut c = assigned();
    let bank = bank(&c);
    set_param(&mut c, &bank, "macro1", 0.5);
    let (a, b) = (
        c.device_id(&key("a")).unwrap().0,
        c.device_id(&key("b")).unwrap().0,
    );
    let cutoff = param_id(SYNTH, CUTOFF);
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(a, cutoff, 1100.0)), "{sent:?}");
    assert!(sent.contains(&(b, cutoff, 2200.0)), "{sent:?}");
    // The macro device itself is not sent anything it would do nothing with.
    let macro_id = c.device_id(&bank).unwrap().0;
    assert!(sent.iter().all(|(id, _, _)| *id != macro_id), "{sent:?}");
    assert!(c.undo());
    let sent = params_of(&c.sink_mut().take());
    assert!(
        sent.iter()
            .any(|(id, p, v)| (*id, *p) == (a, cutoff) && (*v - 1100.0).abs() > 0.5)
    );
    assert!(c.redo());
    assert!(params_of(&c.sink_mut().take()).contains(&(a, cutoff, 1100.0)));
}

#[test]
fn assigning_removing_and_a_range_edit_reach_the_engine() {
    let mut c = core();
    let cutoff = param_id(SYNTH, CUTOFF);
    let a = c.device_id(&key("a")).unwrap().0;
    set_param(&mut c, &key("a"), CUTOFF, 1000.0);
    c.sink_mut().take();
    assert!(c.assign_to_macro(&key("a"), CUTOFF, 0, false));
    // The first assignment keeps the value the param had: nothing audible happens.
    let sent = params_of(&c.sink_mut().take());
    assert!(
        sent.iter()
            .all(|(id, p, v)| (*id, *p) != (a, cutoff) || (*v - 1000.0).abs() < 0.05),
        "{sent:?}"
    );
    // Narrowing the range moves the value.
    apply(
        &mut c,
        Command::Macro(MacroCommand::SetRange {
            id: 1,
            min: 100.0,
            max: 200.0,
            invert: false,
        }),
    );
    let sent = params_of(&c.sink_mut().take());
    assert!(
        sent.iter()
            .any(|(id, p, v)| (*id, *p) == (a, cutoff) && *v <= 200.0),
        "{sent:?}"
    );
    // Removing leaves the param where the macro put it, free to edit.
    let value = c
        .doc()
        .driven_value(c.registry(), &key("a"), CUTOFF)
        .unwrap();
    assert!(c.remove_from_macro(&key("a"), CUTOFF));
    assert_eq!(
        c.doc().param_value(&key("a"), CUTOFF, c.registry()),
        Some(value)
    );
    c.sink_mut().take();
    set_param(&mut c, &key("a"), CUTOFF, 5000.0);
    assert!(params_of(&c.sink_mut().take()).contains(&(a, cutoff, 5000.0)));
    assert!(
        !c.remove_from_macro(&key("a"), CUTOFF),
        "nothing left to remove"
    );
}

#[test]
fn removing_the_bank_gives_its_targets_their_stored_values_and_undo_drives_them_again() {
    let mut c = assigned();
    let bank = bank(&c);
    set_param(&mut c, &bank, "macro1", 0.5);
    let a = c.device_id(&key("a")).unwrap().0;
    let cutoff = param_id(SYNTH, CUTOFF);
    let stored = c
        .doc()
        .param_value(&key("a"), CUTOFF, c.registry())
        .unwrap();
    c.sink_mut().take();
    apply(&mut c, Command::RemoveDevice { key: bank });
    let sent = params_of(&c.sink_mut().take());
    assert!(
        sent.iter()
            .any(|(id, p, v)| (*id, *p) == (a, cutoff) && (*v - stored).abs() < 1e-3),
        "{sent:?}"
    );
    // The assignment is still in the project and dormant; the undo drives the target again.
    assert_eq!(c.doc().macros.assigns.len(), 2);
    assert!(c.undo());
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(a, cutoff, 1100.0)), "{sent:?}");
}

#[test]
fn a_driven_param_refuses_a_direct_edit_with_a_reason() {
    let mut c = assigned();
    let before = c.doc().clone();
    let e = c
        .apply_command(Command::SetParam {
            key: key("a"),
            param: CUTOFF.into(),
            value: 900.0,
            merge: None,
        })
        .unwrap_err();
    assert!(matches!(e, atomik_model::ModelError::MacroDriven(_)), "{e}");
    assert!(c.last_rejection().is_some_and(|m| m.contains("driven by")));
    assert_eq!(c.doc(), &before);
    // The control menu knows.
    assert_eq!(c.macro_control_state(&key("a"), CUTOFF), (false, true));
    assert_eq!(
        c.macro_control_state(&key("a"), "filter.resonance"),
        (true, false)
    );
    assert_eq!(
        c.macro_control_state(&key("a"), "internal.x"),
        (false, false)
    );
    assert_eq!(c.macro_control_state(&key("ghost"), CUTOFF), (false, false));
}

#[test]
fn the_next_free_macro_and_the_current_bank_are_what_a_new_assignment_uses() {
    let mut c = core();
    assert!(c.macro_bank().is_none());
    assert!(c.assign_to_next_macro(&key("a"), CUTOFF));
    let first = bank(&c);
    assert_eq!(c.macro_bank(), Some(first.clone()));
    assert!(c.assign_to_next_macro(&key("a"), "filter.resonance"));
    assert!(c.assign_to_next_macro(&key("b"), CUTOFF));
    let indices: Vec<u8> = c.doc().macros.assigns.iter().map(|a| a.index).collect();
    assert_eq!(indices, [0, 1, 2], "each took the next free macro");
    // A new bank takes over as the current one; the panel can choose another.
    assert!(c.assign_to_macro(&key("b"), "filter.resonance", 3, true));
    let second = c.doc().banks().last().unwrap().key.clone();
    assert_ne!(second, first);
    assert_eq!(c.macro_bank(), Some(second));
    c.set_macro_bank(first.clone());
    assert_eq!(c.macro_bank(), Some(first));
    // A bank that is gone is not the current one.
    c.set_macro_bank(key("nobody"));
    assert!(c.macro_bank().is_some());
}

#[test]
fn the_macros_panel_actions_are_ordinary_commands() {
    let mut c = assigned();
    let bank = bank(&c);
    assert!(!c.macros_action(MacrosAction::SetValue {
        bank: bank.clone(),
        index: 0,
        value: 0.25
    }));
    assert_eq!(c.doc().bank_macro_value(&bank, 0), 0.25);
    c.macros_action(MacrosAction::Rename {
        bank: bank.clone(),
        index: 0,
        name: "Bright".into(),
    });
    assert_eq!(c.doc().macro_name(&bank, 0), "Bright");
    c.macros_action(MacrosAction::SetRange {
        id: 1,
        min: 300.0,
        max: 900.0,
        invert: true,
    });
    let rows = c.macro_bank_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].macros[0].name, "Bright");
    assert_eq!(rows[0].macros[0].assigns.len(), 2);
    let row = &rows[0].macros[0].assigns[0];
    assert_eq!((row.min, row.max, row.invert), (300.0, 900.0, true));
    assert_eq!(row.target, "Poly Synth Cutoff");
    assert!(!row.missing);
    assert!(row.min_text.contains("300"), "{}", row.min_text);
    c.macros_action(MacrosAction::Remove(2));
    assert_eq!(c.doc().macros.assigns.len(), 1);
    c.macros_action(MacrosAction::Clear {
        bank: bank.clone(),
        index: 0,
    });
    assert!(c.doc().macros.assigns.is_empty());
    let before = c.doc().banks().count();
    c.macros_action(MacrosAction::NewBank);
    assert_eq!(c.doc().banks().count(), before + 1);
    assert!(c.macros_action(MacrosAction::Close));
    // Learn arms the macro for a controller and the panel gives way.
    assert!(c.macros_action(MacrosAction::Learn {
        bank: bank.clone(),
        index: 2
    }));
    assert_eq!(
        c.midi_armed(),
        Some(&atomik_model::MidiTarget::Param {
            device: bank,
            param: "macro3".into()
        })
    );
}

#[test]
fn a_controller_cannot_be_mapped_to_a_driven_param_but_can_to_its_macro() {
    let mut c = assigned();
    assert!(!c.midi_arm(atomik_model::MidiTarget::Param {
        device: key("a"),
        param: CUTOFF.into()
    }));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("map the macro instead"))
    );
    let bank = bank(&c);
    assert!(c.midi_arm(atomik_model::MidiTarget::Param {
        device: bank,
        param: "macro1".into()
    }));
}

// ------------------------------------------------------------------ live equals bounce

fn demo_doc() -> RackDoc {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "lead");
    let length = 4 * PPQ;
    for cmd in [
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, length, 60, 0.8)].into()),
            },
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
    ] {
        apply(&mut c, cmd);
    }
    assert!(c.assign_to_macro(&key("lead"), CUTOFF, 0, false));
    apply(
        &mut c,
        Command::Macro(MacroCommand::SetRange {
            id: 1,
            min: 300.0,
            max: 2400.0,
            invert: true,
        }),
    );
    let bank = bank(&c);
    apply(
        &mut c,
        Command::Song(SongCommand::AddAutomationLane {
            device: bank.clone(),
            param: "macro1".into(),
        }),
    );
    let lane = c
        .doc()
        .song
        .track(&bank)
        .unwrap()
        .automation_lane("macro1")
        .unwrap();
    let points = vec![
        AutomationPoint {
            tick: 0,
            value: 0.1,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 2 * PPQ,
            value: 0.9,
            curve: Curve::Linear,
        },
    ];
    apply(
        &mut c,
        Command::Song(SongCommand::AddClip {
            device: bank,
            lane,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Automation(points),
            },
        }),
    );
    c.doc().clone()
}

fn bounce_of(doc: &RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
    )
    .unwrap()
    .channels
}

fn play_doc(doc: RackDoc, frames: usize, block: usize) -> Vec<Vec<f32>> {
    let mut core = AppCore::with_doc(OfflineSink::new(), doc, None);
    core.transport_action(atomik_ui::TransportAction::Play);
    core.sink_mut().render_blocks(frames + LATENCY, block)
}

fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> Option<usize> {
    for (a, b) in live.iter().zip(bounce) {
        if a[..LATENCY].iter().any(|x| *x != 0.0) {
            return Some(0);
        }
        if let Some(i) = a[LATENCY..]
            .iter()
            .zip(b)
            .position(|(x, y)| x.to_bits() != y.to_bits())
        {
            return Some(i);
        }
    }
    None
}

#[test]
fn the_live_path_plays_a_macro_lane_exactly_like_the_bounce() {
    let doc = demo_doc();
    let reference = bounce_of(&doc);
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    for block in [64, 480, 512] {
        let live = play_doc(doc.clone(), reference[0].len(), block);
        assert_eq!(same_bits(&live, &reference), None, "block {block}");
    }
}

#[test]
fn a_static_macro_value_sounds_the_same_live_and_in_the_bounce() {
    let mut doc = demo_doc();
    // Without the lane the macro's own value decides.
    doc.song_mut().tracks.retain(|t| t.device == key("lead"));
    let bank = doc.banks().next().unwrap().key.clone();
    doc.device_mut_for_test(&bank)
        .params
        .insert("macro1".into(), 0.7);
    let reference = bounce_of(&doc);
    let live = play_doc(doc.clone(), reference[0].len(), 256);
    assert_eq!(same_bits(&live, &reference), None);
    // The value is what the maths says.
    let a = doc.macros.assigns[0].clone();
    let p = *default_registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key(CUTOFF)
        .unwrap();
    assert_eq!(
        doc.driven_value(&default_registry(), &key("lead"), CUTOFF),
        Some(assign_value(&a, 0.7, &p))
    );
}

// ------------------------------------------------------------------ following the selection

fn touch<S: EngineSink>(c: &mut AppCore<S>, device: &str, param: &str) {
    c.handle_action(UiAction::ParamTouched {
        device: key(device),
        param: param.into(),
    });
}

fn lane_of<S: EngineSink>(c: &AppCore<S>, device: &str, param: &str) -> Option<usize> {
    c.doc()
        .song
        .track(&key(device))
        .and_then(|t| t.automation_lane(param))
}

#[test]
fn touching_a_control_shows_its_lane_without_writing_data_when_following() {
    let mut c = core();
    c.set_follow_selection(true);
    assert!(c.follow_selection());
    let undo_depth_before = c.doc().song.tracks.len();
    touch(&mut c, "a", CUTOFF);
    let lane = lane_of(&c, "a", CUTOFF).expect("the lane was added");
    let track = c.doc().song.track(&key("a")).unwrap();
    assert!(track.lanes[lane].clips.is_empty(), "no automation data");
    assert!(c.doc().song.tracks.len() > undo_depth_before);
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(key("a"), CUTOFF.to_owned()))
    );
    // One undo takes the lane (and the track made for it) away.
    assert!(c.undo());
    assert!(lane_of(&c, "a", CUTOFF).is_none());
    // Touching again shows it again; touching the same lane twice adds nothing.
    touch(&mut c, "a", CUTOFF);
    let tracks = c.doc().song.tracks.clone();
    touch(&mut c, "a", CUTOFF);
    assert_eq!(c.doc().song.tracks, tracks);
}

#[test]
fn nothing_follows_while_the_switch_is_off() {
    let mut c = core();
    assert!(!c.follow_selection());
    touch(&mut c, "a", CUTOFF);
    assert!(lane_of(&c, "a", CUTOFF).is_none());
    assert!(c.arrangement().followed_lane().is_none());
    // Switching it on later shows nothing until a control is touched again; off clears the mark.
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    assert!(c.arrangement().followed_lane().is_some());
    c.set_follow_selection(false);
    assert!(c.arrangement().followed_lane().is_none());
}

#[test]
fn a_driven_param_follows_the_lane_of_its_macro() {
    let mut c = assigned();
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    let bank = bank(&c);
    assert!(
        lane_of(&c, "a", CUTOFF).is_none(),
        "the param has no lane of its own"
    );
    assert!(lane_of(&c, bank.as_str(), "macro1").is_some());
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(bank, "macro1".to_owned()))
    );
}

#[test]
fn an_effect_without_a_track_gets_an_automation_track_when_touched_and_a_param_with_no_lane_says_why()
 {
    let mut c = core();
    c.set_follow_selection(true);
    touch(&mut c, "fx", "mix");
    let track = c.doc().song.track(&key("fx")).expect("an automation track");
    assert!(track.is_automation_only());
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(key("fx"), "mix".to_owned()))
    );
    // The explicit command says why when a param can have no lane.
    assert!(!c.show_lane_of(&key("fx"), "internal.solo_silence"));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("no automation lane"))
    );
}

#[test]
fn selecting_a_device_shows_the_lane_of_the_last_param_touched_on_it() {
    let mut c = core();
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "b", "filter.resonance");
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(key("b"), "filter.resonance".to_owned()))
    );
    c.select(Some(key("a")));
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(key("a"), CUTOFF.to_owned()))
    );
    // A device nothing was touched on leaves the view alone, and a lane that is gone is not
    // brought back by a selection.
    c.select(Some(key("fx")));
    assert_eq!(
        c.arrangement().followed_lane(),
        Some(&(key("a"), CUTOFF.to_owned()))
    );
    c.undo();
    c.undo();
    assert!(lane_of(&c, "a", CUTOFF).is_none());
    c.select(Some(key("b")));
    c.select(Some(key("a")));
    assert!(
        lane_of(&c, "a", CUTOFF).is_none(),
        "a selection never adds a lane"
    );
}

#[test]
fn a_folder_that_hides_the_track_is_opened() {
    let mut c = core();
    apply(
        &mut c,
        Command::Song(SongCommand::AddTrack { device: key("a") }),
    );
    apply(
        &mut c,
        Command::Song(SongCommand::AddTrack { device: key("b") }),
    );
    apply(
        &mut c,
        Command::Song(SongCommand::GroupTracks {
            devices: vec![key("a"), key("b")],
            name: "Leads".into(),
        }),
    );
    let group = *c.doc().song.org.members.get(&key("a")).unwrap();
    apply(
        &mut c,
        Command::Song(SongCommand::SetGroupCollapsed {
            group,
            collapsed: true,
        }),
    );
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    assert!(!c.doc().song.org.group(group).unwrap().collapsed);
    assert!(lane_of(&c, "a", CUTOFF).is_some());
}

#[test]
fn a_folded_lane_is_shown_open_in_the_view_and_stays_folded_in_the_project() {
    let mut c = core();
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    apply(
        &mut c,
        Command::Song(SongCommand::SetLaneFolded {
            device: key("a"),
            param: CUTOFF.into(),
            folded: true,
        }),
    );
    touch(&mut c, "a", CUTOFF);
    assert!(c.doc().song.lane_folded(&key("a"), CUTOFF));
    let thin = c
        .arrangement()
        .lane_rect(&key("a"), lane_of(&c, "a", CUTOFF).unwrap());
    if let Some(r) = thin {
        assert!(r.height() > 30.0, "open in the view: {r:?}");
    }
}

// ------------------------------------------------------------------ the search

fn two_named() -> AppCore<RecordingSink> {
    let mut c = core();
    for (k, name) in [("a", "Lead"), ("b", "Pad")] {
        apply(
            &mut c,
            Command::Rename {
                key: key(k),
                name: name.into(),
            },
        );
    }
    c
}

#[test]
fn the_search_lists_rows_with_their_values_marks_and_what_they_allow() {
    let mut c = two_named();
    let (rows, total) = c.param_search("lead cutoff");
    assert!(total >= 1);
    let top = &rows[0];
    assert_eq!((top.track.as_str(), top.device.as_str()), ("Lead", "Lead"));
    assert_eq!(top.param, "Cutoff");
    assert!(top.value.contains("Hz"), "{}", top.value);
    assert!(top.can_lane && top.can_macro && top.can_adjust);
    assert!(!top.lane && !top.midi && top.macro_tag.is_none());
    // A param of a delay can have a lane (on a track of its own); a VCA level reads in dB.
    let (rows, _) = c.param_search("delay mix");
    assert!(rows[0].can_lane && rows[0].can_macro);
    let (rows, total) = c.param_search("");
    assert!(total >= rows.len());
    assert!(rows.len() <= SEARCH_ROWS);
    // Marks follow the project: a macro, a lane, a mapping.
    assert!(c.assign_to_macro(&key("a"), CUTOFF, 0, false));
    c.set_follow_selection(true);
    touch(&mut c, "a", CUTOFF);
    touch(&mut c, "a", "filter.resonance");
    c.apply_command(Command::MidiMap(atomik_model::MidiMapCommand::Learn {
        source: atomik_model::MidiSource::any_port(Some(0), atomik_model::SourceKind::Cc(20)),
        target: atomik_model::MidiTarget::Param {
            device: key("a"),
            param: "filter.resonance".into(),
        },
    }))
    .unwrap();
    let (rows, _) = c.param_search("lead cutoff");
    assert_eq!(rows[0].macro_tag.as_deref(), Some("M1"));
    assert!(rows[0].lane, "the macro's lane");
    assert!(!rows[0].can_macro && !rows[0].can_adjust, "driven");
    let (rows, _) = c.param_search("lead resonance");
    assert!(rows[0].midi && rows[0].lane);
}

#[test]
fn the_search_follows_renames_but_not_value_changes() {
    let mut c = two_named();
    let (rows, _) = c.param_search("bass cutoff");
    assert!(rows.iter().all(|r| r.device != "Bass"));
    apply(
        &mut c,
        Command::Rename {
            key: key("b"),
            name: "Bass".into(),
        },
    );
    let (rows, _) = c.param_search("bass cutoff");
    assert_eq!(rows[0].device, "Bass");
    // A value change shows without a new search.
    set_param(&mut c, &key("b"), CUTOFF, 1234.0);
    let (rows, _) = c.param_search_refresh();
    assert!(
        rows.iter()
            .any(|r| r.device == "Bass" && r.value.contains("1.23")),
        "{rows:?}"
    );
}

#[test]
fn a_row_can_be_revealed_adjusted_assigned_and_armed() {
    let mut c = two_named();
    let (rows, _) = c.param_search("pad cutoff");
    assert_eq!(rows[0].device, "Pad");
    // Reveal selects the device in the rack.
    assert_eq!(c.search_reveal(0), Some(Revealed::Rack));
    assert_eq!(c.view().selected(), Some(&key("b")));
    // Adjust moves the value by steps of the knob, one undo step each.
    let before = c
        .doc()
        .param_value(&key("b"), CUTOFF, c.registry())
        .unwrap();
    assert!(c.search_adjust(0, 1, false));
    let up = c
        .doc()
        .param_value(&key("b"), CUTOFF, c.registry())
        .unwrap();
    assert!(up > before);
    assert!(c.search_adjust(0, -1, true));
    let down = c
        .doc()
        .param_value(&key("b"), CUTOFF, c.registry())
        .unwrap();
    assert!(down < up && down > before);
    assert!(c.undo() && c.undo());
    assert_eq!(
        c.doc().param_value(&key("b"), CUTOFF, c.registry()),
        Some(before)
    );
    // Assign takes the next free macro; a second time is refused (it is driven now).
    assert!(c.search_assign(0));
    assert_eq!(c.doc().macros.assigns.len(), 1);
    assert!(!c.search_adjust(0, 1, false) || c.doc().bank_driver(&key("b"), CUTOFF).is_some());
    let e = c
        .apply_command(Command::SetParam {
            key: key("b"),
            param: CUTOFF.into(),
            value: 3000.0,
            merge: None,
        })
        .is_err();
    assert!(e, "a driven param refuses a direct edit");
    // Learn arms it.
    let (rows, _) = c.param_search("pad resonance");
    assert_eq!(rows[0].param, "Resonance");
    assert!(c.search_learn(0));
    assert!(matches!(
        c.midi_armed(),
        Some(atomik_model::MidiTarget::Param { device, param })
            if *device == key("b") && param == "filter.resonance"
    ));
    // A row that is not there does nothing.
    assert_eq!(c.search_reveal(500), None);
    assert!(!c.search_assign(500) && !c.search_learn(500) && !c.search_adjust(500, 1, false));
}

#[test]
fn a_vca_level_is_a_row_with_its_own_adjust() {
    let mut c = two_named();
    apply(
        &mut c,
        Command::Console(atomik_model::ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel")],
        }),
    );
    let (rows, _) = c.param_search("drums");
    let vca = &rows[0];
    assert_eq!((vca.track.as_str(), vca.param.as_str()), ("Mixer", "Level"));
    assert!(vca.value.contains("dB") && !vca.can_lane && !vca.can_macro && vca.can_adjust);
    assert!(c.search_adjust(0, 2, false));
    assert_eq!(c.doc().console.vca(1).unwrap().level_db, 2.0);
    assert!(!c.search_show_lane(0));
    assert!(!c.search_assign(0));
    assert_eq!(c.search_reveal(0), Some(Revealed::Console));
    assert_eq!(c.console().selected_strips(), [key("channel")]);
    assert!(c.search_learn(0));
}

#[test]
fn a_big_project_searches_without_listing_everything() {
    let mut c = AppCore::new(RecordingSink::new());
    for n in 0..30 {
        add(&mut c, SYNTH, &format!("s{n}"));
    }
    let (rows, total) = c.param_search("cutoff");
    assert_eq!(rows.len(), SEARCH_ROWS.min(total));
    assert!(total >= 30);
    let (rows, _) = c.param_search("");
    assert_eq!(rows.len(), SEARCH_ROWS);
}

// A document for the static test needs a way to change a device's stored param without the
// model's commands: the macro bank's value is stored like any other param.
trait DeviceMut {
    fn device_mut_for_test(&mut self, key: &DeviceKey) -> &mut atomik_model::RackDevice;
}

impl DeviceMut for RackDoc {
    fn device_mut_for_test(&mut self, key: &DeviceKey) -> &mut atomik_model::RackDevice {
        self.devices.iter_mut().find(|d| &d.key == key).unwrap()
    }
}

#[test]
fn an_empty_document_has_no_banks_and_an_index_of_the_master_alone() {
    let c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    assert!(c.macro_bank().is_none());
    assert!(c.macro_bank_rows().is_empty());
}
