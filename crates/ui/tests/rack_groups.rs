//! The rack and device groups: Select All in Device Group takes everything of the selected
//! device's track, and the single column view stacks the columns of a per-track rack without
//! touching the document.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, Document, RackSort, WorkflowCommand,
    empty_doc,
};
use atomik_ui::RackView;
use atomik_ui::keymap::CommandOutcome;
use support::*;

fn sorted_doc(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    for k in ["s1", "s2"] {
        d.apply(
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type("atomik.poly-synth".into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
            reg,
        )
        .unwrap();
    }
    d.apply(
        Command::Workflow(WorkflowCommand::CreatePlayer {
            type_id: "atomik.arpeggiator".into(),
            target: CreateTarget::Selected(key("s1")),
            options: CreateOptions::default(),
        }),
        reg,
    )
    .unwrap();
    d.apply(Command::SetRackSort(RackSort::PerTrack), reg)
        .unwrap();
    d
}

fn selected(v: &RackView) -> Vec<String> {
    v.selection().into_iter().map(|k| k.0).collect()
}

#[test]
fn select_all_in_device_group_takes_the_whole_track_and_nothing_else() {
    let reg = registry();
    let d = sorted_doc(&reg);
    let mut v = view(d.doc(), &reg);
    v.set_selection(&[key("s1")], Some(&key("s1")));
    let out = v.command("rack.select_group", None, d.doc(), &reg);
    assert!(out.is_taken());
    let group = d.doc().device_group(&reg, &key("s1")).unwrap();
    let mut want: Vec<String> = group.members.iter().map(|k| k.0.clone()).collect();
    want.sort();
    let mut got = selected(&v);
    got.sort();
    assert_eq!(got, want);
    assert!(!got.contains(&"s2".to_owned()));
    assert!(got.len() >= 3, "instrument, player and channel: {got:?}");
}

#[test]
fn select_group_declines_without_a_selection_and_for_a_loose_device() {
    let reg = registry();
    let d = sorted_doc(&reg);
    let mut v = view(d.doc(), &reg);
    assert_eq!(
        v.command("rack.select_group", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
    let master = d.doc().console.master.clone().unwrap();
    v.set_selection(std::slice::from_ref(&master), Some(&master));
    assert_eq!(
        v.command("rack.select_group", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn the_single_column_view_stacks_the_columns_and_leaves_the_document_alone() {
    let reg = registry();
    let d = sorted_doc(&reg);
    assert!(d.doc().columns >= 3);
    let mut v = view(d.doc(), &reg);
    let (a, b) = (
        v.plate_center(&key("s1")).unwrap(),
        v.plate_center(&key("s2")).unwrap(),
    );
    assert!(
        (a.x - b.x).abs() > 100.0,
        "different columns sit side by side"
    );
    v.set_single_column(true);
    assert!(v.single_column());
    v.sync(d.doc(), &reg);
    let (a, b) = (
        v.plate_center(&key("s1")).unwrap(),
        v.plate_center(&key("s2")).unwrap(),
    );
    assert!((a.x - b.x).abs() < 1.0, "one column: {a:?} {b:?}");
    assert!(d.doc().columns >= 3, "the document keeps its columns");
    v.set_single_column(false);
    v.sync(d.doc(), &reg);
    let (a, b) = (
        v.plate_center(&key("s1")).unwrap(),
        v.plate_center(&key("s2")).unwrap(),
    );
    assert!((a.x - b.x).abs() > 100.0);
}
