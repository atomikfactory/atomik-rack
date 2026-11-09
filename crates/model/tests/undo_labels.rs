//! Undo step names: `Command::label`, `Document::undo_label` and `Document::redo_label`.

mod common;

use atomik_model::{
    Command, CreateOptions, CreateSource, DEFAULT_LABEL, Document, SongCommand, WorkflowCommand,
    empty_doc,
};
use common::song::*;
use common::workflow::*;
use common::*;

#[test]
fn the_most_common_commands_have_their_own_names() {
    let names: Vec<(Command, &str)> = vec![
        (add(0, SYNTH, Some("a")), "Add Device"),
        (
            Command::MoveDevice {
                key: key("a"),
                to: 0,
            },
            "Move Device",
        ),
        (
            Command::Rename {
                key: key("a"),
                name: "x".into(),
            },
            "Rename",
        ),
        (Command::RemoveDevice { key: key("a") }, "Delete Device"),
        (connect("a:out_l", "b:in_l"), "Connect Cable"),
        (set_param("a", "p", 0.5), "Change Parameter"),
        (Command::SetTempo(120.0), "Change Tempo"),
        (add_track("a"), "Add Track"),
        (add_clip("a", 0, notes_clip(0, 960, Vec::new())), "Add Clip"),
        (
            Command::Song(SongCommand::RemoveClip {
                id: atomik_model::song::ClipId(1),
            }),
            "Delete Clip",
        ),
        (
            wf(WorkflowCommand::MoveDevices {
                keys: keys_of(&["a"]),
                to: 0,
                route: true,
            }),
            "Move Device",
        ),
        (
            wf(WorkflowCommand::RemoveDevices {
                keys: keys_of(&["a"]),
            }),
            "Delete Devices",
        ),
        (
            wf(WorkflowCommand::DuplicateDevices {
                keys: keys_of(&["a"]),
                route: true,
            }),
            "Duplicate Devices",
        ),
        (
            wf(WorkflowCommand::CreateInstrument {
                index: 0,
                source: CreateSource::Type(SYNTH.into()),
                key: None,
                options: CreateOptions::default(),
            }),
            "Create Instrument",
        ),
        (
            wf(WorkflowCommand::ReplaceDevice {
                target: key("a"),
                source: CreateSource::Type(SYNTH.into()),
            }),
            "Replace Device",
        ),
        (
            wf(WorkflowCommand::SplitSub {
                device: key("a"),
                freq: 120.0,
            }),
            "Split Sub",
        ),
        (
            wf(WorkflowCommand::AddReverbSend {
                track: key("a"),
                options: atomik_model::reverb_send::ReverbSendOptions::default(),
            }),
            "Add Reverb Send",
        ),
        (
            wf(WorkflowCommand::AddRumble {
                kick: key("a"),
                start: 0,
                length: 1,
                audio: atomik_model::AudioClipContent::new("a".repeat(64), 0),
                name: None,
                options: atomik_model::rumble::RumbleTrackOptions::default(),
            }),
            "Add Rumble",
        ),
        (
            wf(WorkflowCommand::AddTransition {
                at: BAR,
                edit: atomik_model::transition::TransitionEdit::Silence,
            }),
            "Add Silence Transition",
        ),
        (
            wf(WorkflowCommand::AddTransition {
                at: BAR,
                edit: atomik_model::transition::TransitionEdit::Fill { track: key("a") },
            }),
            "Add Fill Transition",
        ),
        (
            wf(WorkflowCommand::AddTransition {
                at: BAR,
                edit: atomik_model::transition::TransitionEdit::FilterDown {
                    target: key("a"),
                    bars: 1,
                },
            }),
            "Add Filter Down Transition",
        ),
        (
            wf(WorkflowCommand::SetFoldedMany {
                keys: keys_of(&["a"]),
                folded: true,
            }),
            "Fold Devices",
        ),
        (
            wf(WorkflowCommand::JoinClips { ids: Vec::new() }),
            "Join Clips",
        ),
        (
            wf(WorkflowCommand::SplitClips {
                ids: Vec::new(),
                at: 0,
            }),
            "Split Clips",
        ),
        (
            Command::Batch(vec![Command::SetTempo(100.0), add(0, SYNTH, None)]),
            "Change Tempo",
        ),
        (Command::AddRackColumn, "Add Rack Column"),
    ];
    for (command, name) in names {
        assert_eq!(command.label(), name, "{command:?}");
    }
    assert_eq!(DEFAULT_LABEL, "Edit");
}

#[test]
fn undo_and_redo_name_the_step_they_would_run() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    assert_eq!(d.undo_label(), None);
    assert_eq!(d.redo_label(), None);
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, DELAY, Some("b")),
            Command::MoveDevice {
                key: key("b"),
                to: 0,
            },
        ],
    );
    assert_eq!(d.undo_label(), Some("Move Device"));
    d.undo().unwrap();
    assert_eq!(d.undo_label(), Some("Add Device"));
    assert_eq!(d.redo_label(), Some("Move Device"));
    d.redo().unwrap();
    assert_eq!(d.undo_label(), Some("Move Device"));
    assert_eq!(d.redo_label(), None);
    // A new command drops the redo names with the redo steps.
    d.undo().unwrap();
    run(&mut d, &r, [set_param("a", "osc1.wave", 1.0)]);
    assert_eq!(d.redo_label(), None);
    assert_eq!(d.undo_label(), Some("Change Parameter"));
}

#[test]
fn merged_drags_and_joined_steps_keep_the_first_name() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("a"))]);
    let drag = |value: f32| Command::SetParam {
        key: key("a"),
        param: "osc1.wave".into(),
        value,
        merge: Some(7),
    };
    run(&mut d, &r, [drag(1.0), drag(2.0), drag(3.0)]);
    assert_eq!(d.undo_depth(), 2, "the drag is one step");
    assert_eq!(d.undo_label(), Some("Change Parameter"));
    d.apply_joined(Command::SetTempo(90.0), &r).unwrap();
    assert_eq!(d.undo_depth(), 2);
    assert_eq!(
        d.undo_label(),
        Some("Change Parameter"),
        "the first name stays"
    );
    d.undo().unwrap();
    assert_eq!(d.undo_label(), Some("Add Device"));
    assert_eq!(d.redo_label(), Some("Change Parameter"));
}

#[test]
fn a_trial_is_named_after_its_first_step_and_a_cancel_keeps_the_redo_names() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("a")), add(1, DELAY, Some("b"))],
    );
    d.undo().unwrap();
    d.apply_trial(Command::SetTempo(100.0), &r).unwrap();
    d.apply_trial(add(1, DELAY, Some("c")), &r).unwrap();
    d.cancel_trial().unwrap();
    assert_eq!(d.redo_label(), Some("Add Device"));
    d.apply_trial(Command::SetTempo(101.0), &r).unwrap();
    d.apply_trial(add(1, DELAY, Some("c")), &r).unwrap();
    d.commit_trial();
    assert_eq!(d.undo_label(), Some("Change Tempo"));
    assert_eq!(d.redo_label(), None);
}

#[test]
fn the_history_lists_every_step_oldest_first_and_tracks_the_position() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    assert_eq!(d.history().count(), 0);
    assert_eq!((d.history_position(), d.history_len()), (0, 0));
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("a")),
            add(1, DELAY, Some("b")),
            Command::MoveDevice {
                key: key("b"),
                to: 0,
            },
        ],
    );
    let names = |d: &Document| d.history().map(|s| s.label).collect::<Vec<_>>();
    assert_eq!(names(&d), ["Add Device", "Add Device", "Move Device"]);
    assert_eq!((d.history_position(), d.history_len()), (3, 3));
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(
        names(&d),
        ["Add Device", "Add Device", "Move Device"],
        "an undone step stays in the list, after the position"
    );
    assert_eq!((d.history_position(), d.history_len()), (1, 3));
    d.redo().unwrap();
    assert_eq!((d.history_position(), d.history_len()), (2, 3));
    // A new edit drops what could have been redone.
    run(&mut d, &r, [Command::SetTempo(90.0)]);
    assert_eq!(names(&d), ["Add Device", "Add Device", "Change Tempo"]);
    assert_eq!((d.history_position(), d.history_len()), (3, 3));
}

#[test]
fn a_step_keeps_its_stamp_through_undo_and_redo_and_stamps_rise() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("a")), add(1, DELAY, Some("b"))],
    );
    let before: Vec<u64> = d.history().map(|s| s.stamp).collect();
    assert!(before[0] < before[1]);
    d.undo().unwrap();
    d.undo().unwrap();
    d.redo().unwrap();
    d.redo().unwrap();
    let after: Vec<u64> = d.history().map(|s| s.stamp).collect();
    assert_eq!(before, after);
    run(&mut d, &r, [Command::SetTempo(100.0)]);
    assert!(d.history().last().unwrap().stamp > before[1]);
}

#[test]
fn a_merged_drag_is_one_history_row() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("a"))]);
    let drag = |value: f32| Command::SetParam {
        key: key("a"),
        param: "osc1.wave".into(),
        value,
        merge: Some(7),
    };
    run(&mut d, &r, [drag(1.0), drag(2.0), drag(3.0)]);
    assert_eq!(d.history_len(), 2);
}
