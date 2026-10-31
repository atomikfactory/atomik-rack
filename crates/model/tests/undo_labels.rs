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
    d.undo(&r).unwrap();
    assert_eq!(d.undo_label(), Some("Add Device"));
    assert_eq!(d.redo_label(), Some("Move Device"));
    d.redo(&r).unwrap();
    assert_eq!(d.undo_label(), Some("Move Device"));
    assert_eq!(d.redo_label(), None);
    // A new command drops the redo names with the redo steps.
    d.undo(&r).unwrap();
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
    d.undo(&r).unwrap();
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
    d.undo(&r).unwrap();
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
