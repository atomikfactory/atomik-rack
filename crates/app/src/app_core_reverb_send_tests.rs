//! The reverb send wizard in the app core: one undo step with its name, the reverb selected, the
//! status line, the menu gate and the refusals.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::reverb_send::ReverbSendOptions;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// A project with a synth track `lead` that plays a note on every beat for four bars.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    let notes: Vec<Note> = (0..16)
        .map(|i| Note::new(i * PPQ, PPQ / 2, 60, 0.8))
        .collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 16 * PPQ,
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
            content: ClipContent::Notes(notes.into()),
        },
    }))
    .unwrap();
    c
}

#[test]
fn the_wizard_is_one_named_undo_step_that_selects_the_reverb_and_says_what_it_did() {
    let mut c = core();
    c.view_mut().set_selection(&[], None);
    assert!(!c.reverb_send_menu(), "no track selected");
    assert!(!c.add_reverb_send(ReverbSendOptions::default()));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Select a track"))
    );

    c.view_mut()
        .set_selection(&[key("lead")], Some(&key("lead")));
    assert!(c.reverb_send_menu());
    let before = c.doc().clone();
    let undo_before = c.document().undo_label();
    assert!(c.add_reverb_send(ReverbSendOptions::default()));
    assert_eq!(c.document().undo_label(), Some("Add Reverb Send"));
    let reverb = c.doc().console.returns[0]
        .as_ref()
        .and(atomik_model::reverb_send::reverb_of_return(c.doc(), 0))
        .expect("a reverb on the first return");
    assert_eq!(c.view().selection(), vec![reverb]);
    let said = c.last_rejection().unwrap();
    assert!(said.starts_with("Reverb send: "), "{said}");
    assert!(
        said.contains("return 1") && said.contains("post-fader"),
        "{said}"
    );
    assert!(
        said.contains("decay 1.0 s") && said.contains("ducked"),
        "{said}"
    );

    c.undo();
    assert_eq!(c.doc().devices, before.devices);
    assert_eq!(c.doc().cables, before.cables);
    assert_eq!(c.doc().console, before.console);
    assert_eq!(c.document().undo_label(), undo_before);
}

#[test]
fn the_selected_track_of_the_arrangement_is_enough_and_info_suggests_the_decay() {
    let mut c = core();
    c.view_mut().set_selection(&[], None);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    assert!(c.reverb_send_menu());
    let (name, decay) = c.reverb_send_info(false).expect("a track");
    assert!(!name.is_empty());
    assert!((decay - 1.0).abs() < 1e-3, "{decay}");
    c.view_mut().set_selection(&[], None);
    c.arrangement_mut().set_selected_track(None);
    assert!(c.reverb_send_info(false).is_none());
}

#[test]
fn a_full_console_turns_the_menu_off_and_says_why() {
    let mut c = core();
    c.view_mut()
        .set_selection(&[key("lead")], Some(&key("lead")));
    for _ in 0..8 {
        assert!(c.add_reverb_send(ReverbSendOptions::default()));
        c.view_mut()
            .set_selection(&[key("lead")], Some(&key("lead")));
    }
    assert!(!c.reverb_send_menu());
    assert!(!c.add_reverb_send(ReverbSendOptions::default()));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("send and return slots"))
    );
}
