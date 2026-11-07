//! Add Transition through the shell: the submenu of the Track menu, the registry ids and their
//! arguments, the Add Transition rows of the marker, range and ruler menus, and a menu command
//! making its transition at the selected marker in one undo step.

use super::*;
use crate::commands::TransitionChoice;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Marker, Note,
    SongCommand, WorkflowCommand,
};
use atomik_ui::ContextTarget;
use atomik_ui::MarkSel;
use atomik_ui::context_menu::Spec;

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

/// A shell with a synth `lead` that plays a note on every beat for four bars and markers at
/// bars 2 and 3.
fn with_a_song(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 4 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                (0..16)
                    .map(|i| Note::new(i * PPQ, PPQ / 2, 60, 0.8))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        },
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::SetMarkers(
        [(1, BAR), (2, 2 * BAR)]
            .into_iter()
            .map(|(id, tick)| Marker {
                id,
                tick,
                name: format!("S{id}"),
                color: None,
            })
            .collect(),
    )))
    .unwrap();
}

fn track_items(s: &Shell<NullBackend>) -> Vec<(AppCommand, String, bool)> {
    s.menu_model()
        .into_iter()
        .find(|m| m.title == "Track")
        .unwrap()
        .items()
        .map(|i| (i.command, i.label.to_string(), i.enabled))
        .collect()
}

#[test]
fn the_track_menu_has_the_add_transition_submenu_with_a_row_per_choice() {
    let s = shell();
    let items = track_items(&s);
    for choice in TransitionChoice::ALL {
        let row = items
            .iter()
            .find(|i| i.0 == AppCommand::AddTransition(choice))
            .unwrap_or_else(|| panic!("{choice:?}"));
        assert_eq!(row.1, choice.label());
        assert!(row.2, "{choice:?} is enabled");
    }
}

#[test]
fn the_registry_ids_and_their_arguments_round_trip() {
    let registry = atomik_ui::keymap::Registry::bundled();
    for choice in TransitionChoice::ALL {
        let command = AppCommand::AddTransition(choice);
        let (id, arg) = command.command_id().unwrap();
        assert_eq!(id, "sequencer.add_transition");
        assert_eq!(AppCommand::from_command(id, arg), Some(command));
        assert!(registry.validate(id, arg), "{arg:?}");
    }
    assert_eq!(
        AppCommand::from_command("sequencer.add_transition", Some("sideways")),
        None
    );
    assert_eq!(
        AppCommand::from_command("sequencer.add_transition", None),
        None
    );
    // Every choice has its own argument and its own label.
    let mut args: Vec<&str> = TransitionChoice::ALL.iter().map(|c| c.arg()).collect();
    args.sort_unstable();
    args.dedup();
    assert_eq!(args.len(), TransitionChoice::ALL.len());
}

#[test]
fn the_marker_range_and_ruler_menus_hold_the_add_transition_submenu() {
    let registry = atomik_ui::keymap::Registry::bundled();
    for target in [
        ContextTarget::Marker,
        ContextTarget::MarkerRange,
        ContextTarget::RulerMeter,
    ] {
        let specs = context_menus::specs(target, &context_menus::MenuEnable::default());
        let rows = specs
            .iter()
            .find_map(|s| match s {
                Spec::Submenu { label, items, .. } if *label == "Add Transition" => Some(items),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{target:?} has no Add Transition"));
        let args: Vec<&str> = rows
            .iter()
            .filter_map(|s| match s {
                Spec::Item {
                    command: "sequencer.add_transition",
                    arg,
                    ..
                } => *arg,
                _ => None,
            })
            .collect();
        let want: Vec<&str> = TransitionChoice::ALL.iter().map(|c| c.arg()).collect();
        assert_eq!(args, want, "{target:?}");
        for arg in args {
            assert!(registry.validate("sequencer.add_transition", Some(arg)));
        }
    }
}

#[test]
fn a_menu_command_adds_the_transition_at_the_selected_marker_in_one_undo_step() {
    let mut s = shell();
    with_a_song(&mut s);
    s.set_focus(Pane::Arrangement);
    s.core_mut()
        .arrangement_mut()
        .set_selected_marker(Some(MarkSel::Marker(2)));
    let depth = s.core().document().undo_depth();
    s.execute(AppCommand::AddTransition(TransitionChoice::Silence));
    assert_eq!(s.core().document().undo_depth(), depth + 1);
    let master = s.core().doc().console.master.clone().unwrap();
    let track = s.core().doc().song.track(&master).unwrap();
    let lane = &track.lanes[track.automation_lane("fader").unwrap()];
    assert_eq!(
        (lane.clips[0].start, lane.clips[0].length),
        (2 * BAR, PPQ),
        "at the selected marker"
    );
    // The registry path (a key or a menu item) does the same at the other marker.
    s.core_mut()
        .arrangement_mut()
        .set_selected_marker(Some(MarkSel::Marker(1)));
    assert!(
        s.run_command("sequencer.add_transition", Some("filter-1"))
            .is_some()
    );
    let chain = atomik_model::insert_chain(s.core().doc(), s.core().registry(), &master);
    assert_eq!(chain.len(), 1, "the filter is an insert of the master");
    s.core_mut().undo();
    s.core_mut().undo();
    assert!(
        s.core()
            .doc()
            .song
            .track(&master)
            .is_none_or(|t| t.lanes.is_empty())
    );
}
