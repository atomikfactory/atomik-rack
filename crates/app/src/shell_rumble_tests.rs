//! Rumble Maker and Reverse Reverb Print through the shell: the two submenus of the Track menu,
//! the registry ids and their arguments, and a menu command making its result in one undo step.

use super::*;
use crate::commands::{RumbleChoice, SwellLength};
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};

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

/// A shell with a drum machine `kick` (its first pad a synthesised kick) that plays the kick on
/// every beat of a bar, starting after one bar so a swell has room before it.
fn with_a_kick(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.drum-machine".into()),
        key: Some(key("kick")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::SetParam {
        key: key("kick"),
        param: "pad1.module".into(),
        value: 1.0,
        merge: None,
    })
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("kick"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 4 * PPQ,
            length: 4 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                (0..4)
                    .map(|i| Note::new(i * PPQ, PPQ / 4, 36, 0.9))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        },
    }))
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
fn the_track_menu_has_both_submenus_with_a_row_per_choice() {
    let s = shell();
    let items = track_items(&s);
    for choice in RumbleChoice::ALL {
        let row = items
            .iter()
            .find(|i| i.0 == AppCommand::RumbleMaker(choice))
            .unwrap_or_else(|| panic!("{choice:?}"));
        assert_eq!(row.1, choice.label());
    }
    for length in SwellLength::ALL {
        let row = items
            .iter()
            .find(|i| i.0 == AppCommand::ReverseReverbPrint(length))
            .unwrap_or_else(|| panic!("{length:?}"));
        assert_eq!(row.1, length.label());
    }
}

#[test]
fn the_registry_ids_and_their_arguments_round_trip() {
    for choice in RumbleChoice::ALL {
        let command = AppCommand::RumbleMaker(choice);
        let (id, arg) = command.command_id().unwrap();
        assert_eq!(id, "sequencer.rumble_maker");
        assert_eq!(AppCommand::from_command(id, arg), Some(command));
    }
    for length in SwellLength::ALL {
        let command = AppCommand::ReverseReverbPrint(length);
        let (id, arg) = command.command_id().unwrap();
        assert_eq!(id, "sequencer.reverse_reverb_print");
        assert_eq!(AppCommand::from_command(id, arg), Some(command));
    }
    assert_eq!(
        AppCommand::from_command("sequencer.rumble_maker", Some("sideways")),
        None
    );
    assert_eq!(
        AppCommand::from_command("sequencer.reverse_reverb_print", Some("3")),
        None
    );
    assert!((SwellLength::Half.seconds() - 0.5).abs() < f32::EPSILON);
    assert!((SwellLength::Eight.seconds() - 8.0).abs() < f32::EPSILON);
}

#[test]
fn the_menu_commands_make_a_rumble_and_a_swell_each_in_one_undo_step() {
    let mut s = shell();
    with_a_kick(&mut s);
    let tracks = s.core().doc().song.tracks.len();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("kick")));
    s.execute(AppCommand::RumbleMaker(RumbleChoice::Tail));
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), tracks + 1);
    s.core_mut().undo();
    assert_eq!(s.core().doc().song.tracks.len(), tracks);

    let clip = s.core().doc().song.track(&key("kick")).unwrap().lanes[0].clips[0].id;
    s.core_mut().arrangement_mut().set_selected_clips(&[clip]);
    s.execute(AppCommand::ReverseReverbPrint(SwellLength::One));
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), tracks + 1);
    s.core_mut().undo();
    assert_eq!(s.core().doc().song.tracks.len(), tracks);
}
