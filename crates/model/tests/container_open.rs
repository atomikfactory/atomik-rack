//! A container's unfolded flag (`ContainerCommand::SetOpen`, the optional `"open"` field of
//! `"inner"`): undo, redo, the project text, the combine that leaves its container unfolded and
//! the engine never seeing the flag.

mod common;

use atomik_model::{
    Command, ContainerCommand, Document, SyncOp, empty_doc, load_project, save_project,
};
use common::*;

fn set_open(container: &str, open: bool) -> Command {
    Command::Container(ContainerCommand::SetOpen {
        container: key(container),
        open,
    })
}

fn is_open(d: &Document, container: &str) -> bool {
    d.doc().inner(&key(container)).expect("a container").open
}

fn nested(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, CONTAINER, Some("layer")),
            add_inner("layer", 0, DELAY, "echo"),
            add_inner("layer", 1, CONTAINER, "fx"),
            add_inner("layer/fx", 0, DELAY, "tail"),
        ],
    );
    d
}

#[test]
fn a_new_container_is_folded() {
    let r = registry();
    let d = nested(&r);
    assert!(!is_open(&d, "layer"));
    assert!(!is_open(&d, "layer/fx"));
}

#[test]
fn unfolding_a_container_is_one_undo_step_that_only_changes_the_view() {
    let r = registry();
    let mut d = nested(&r);
    let expanded = d.doc().expand(&r);
    let ops = d.apply(set_open("layer", true), &r).unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert!(is_open(&d, "layer"));
    assert!(
        !is_open(&d, "layer/fx"),
        "the inner container has its own flag"
    );
    assert_eq!(
        d.doc().expand(&r),
        expanded,
        "the engine never sees the flag"
    );
    d.apply(set_open("layer/fx", true), &r).unwrap();
    assert!(is_open(&d, "layer/fx"));
    d.undo(&r).unwrap();
    assert!(!is_open(&d, "layer/fx"));
    assert!(is_open(&d, "layer"));
    d.undo(&r).unwrap();
    assert!(!is_open(&d, "layer"));
    d.redo(&r).unwrap();
    assert!(is_open(&d, "layer"));
}

#[test]
fn only_a_container_has_an_inside_to_unfold() {
    let r = registry();
    let mut d = nested(&r);
    let before = d.doc().clone();
    assert!(d.apply(set_open("layer/echo", true), &r).is_err());
    assert!(d.apply(set_open("nothing", true), &r).is_err());
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_flag_is_saved_only_when_set_and_loads_back() {
    let r = registry();
    let mut d = nested(&r);
    let folded = save_project(d.doc());
    assert!(
        !folded.contains("\"open\""),
        "a folded container writes no flag"
    );
    d.apply(set_open("layer", true), &r).unwrap();
    d.apply(set_open("layer/fx", true), &r).unwrap();
    let text = save_project(d.doc());
    assert_eq!(text.matches("\"open\": true").count(), 2);
    let back = load_project(&text, &r).unwrap();
    assert!(back.inner(&key("layer")).unwrap().open);
    assert!(back.inner(&key("layer/fx")).unwrap().open);
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text, "the text is canonical");
}

#[test]
fn the_flag_must_be_a_boolean() {
    let r = registry();
    let mut d = nested(&r);
    d.apply(set_open("layer", true), &r).unwrap();
    let text = save_project(d.doc());
    let bad = text.replace("\"open\": true", "\"open\": 1");
    let e = load_project(&bad, &r).expect_err("a number is not a flag");
    assert!(e.to_string().contains("open"), "{e}");
}

#[test]
fn combining_leaves_the_new_container_unfolded() {
    let r = registry();
    let mut d = chain(&r);
    d.apply(
        Command::Container(ContainerCommand::Combine {
            keys: vec![key("synth"), key("delay")],
            name: String::new(),
        }),
        &r,
    )
    .unwrap();
    assert!(is_open(&d, "container"));
    d.apply(set_open("container", false), &r).unwrap();
    d.apply(
        Command::Container(ContainerCommand::Uncombine {
            container: key("container"),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(keys(&d), ["synth", "delay", "mixer"]);
}
