//! A send/return loop cabled by hand through the master (`fx_send{k}` into an effect, the
//! effect into `return{k}`) is the same as `SetReturn`: the console lists the effect on slot
//! `k`. Completing the loop registers it in the same undo step, cutting it frees the slot, and
//! a project saved with the loop cabled and the slot empty gets the slot on load.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, Document, WorkflowCommand, load_project,
    new_project_doc,
};
use common::workflow::*;
use common::*;

const REVERB: &str = "atomik.reverb";

fn reg() -> Registry {
    console_registry()
}

/// A starter project with a loose reverb `verb` (no routing).
fn with_loose_reverb(r: &Registry) -> Document {
    let mut d = Document::new(new_project_doc());
    d.apply(
        wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Type(REVERB.into()),
            target: CreateTarget::Selected(key("master")),
            key: Some(key("verb")),
            options: CreateOptions {
                route: false,
                ..CreateOptions::default()
            },
        }),
        r,
    )
    .unwrap();
    d
}

fn send_cables(k: usize) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for s in ["l", "r"] {
        v.push((format!("master:fx_send{k}_{s}"), format!("verb:in_{s}")));
        v.push((format!("verb:out_{s}"), format!("master:return{k}_{s}")));
    }
    v
}

fn wire_all(d: &mut Document, r: &Registry, k: usize) {
    for (a, b) in send_cables(k) {
        d.apply(connect(&a, &b), r).unwrap();
    }
}

#[test]
fn the_last_cable_of_the_loop_registers_the_return() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    let cables = send_cables(1);
    for (a, b) in &cables[..3] {
        d.apply(connect(a, b), &r).unwrap();
        assert!(d.doc().console.returns[0].is_none(), "loop still open");
    }
    one_step(&mut d, &r, connect(&cables[3].0, &cables[3].1));
    assert_eq!(d.doc().console.returns[0], Some(key("verb")));
}

#[test]
fn a_loop_on_the_second_send_takes_the_second_slot() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    wire_all(&mut d, &r, 2);
    assert_eq!(d.doc().console.returns[1], Some(key("verb")));
    assert!(d.doc().console.returns[0].is_none());
}

#[test]
fn undo_reverts_the_cable_and_the_slot_together() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    let cables = send_cables(1);
    for (a, b) in &cables[..3] {
        d.apply(connect(a, b), &r).unwrap();
    }
    let open = d.doc().cables.len();
    d.apply(connect(&cables[3].0, &cables[3].1), &r).unwrap();
    assert!(d.doc().console.returns[0].is_some());
    d.undo().unwrap();
    assert!(d.doc().console.returns[0].is_none());
    assert_eq!(d.doc().cables.len(), open);
    d.redo().unwrap();
    assert_eq!(d.doc().console.returns[0], Some(key("verb")));
}

#[test]
fn cutting_the_loop_frees_the_slot() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    wire_all(&mut d, &r, 1);
    assert!(d.doc().console.returns[0].is_some());
    one_step(
        &mut d,
        &r,
        Command::Disconnect {
            to: sock("master:return1_l"),
        },
    );
    assert!(d.doc().console.returns[0].is_none());
}

#[test]
fn a_registered_loop_is_not_registered_twice() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    wire_all(&mut d, &r, 1);
    let before = text(&d);
    // An unrelated edit leaves the slot alone.
    d.apply(add(usize::MAX, DELAY, Some("other")), &r).unwrap();
    assert!(text(&d).contains("\"verb\""));
    assert_ne!(before, text(&d));
    assert_eq!(d.doc().console.returns[0], Some(key("verb")));
}

#[test]
fn loading_a_project_with_the_loop_cabled_infers_the_slot() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    wire_all(&mut d, &r, 1);
    let saved = text(&d);
    assert!(saved.contains("\"returns\": [\"verb\", null"), "{saved}");
    let blank = saved.replace("\"returns\": [\"verb\", null", "\"returns\": [null, null");
    assert_ne!(saved, blank);
    let loaded = load_project(&blank, &r).unwrap();
    assert_eq!(loaded.console.returns[0], Some(key("verb")));
    assert_eq!(loaded.console.returns.iter().flatten().count(), 1);
    // Deterministic: the same file, the same result.
    assert_eq!(
        load_project(&blank, &r).unwrap().console.returns,
        loaded.console.returns
    );
}

#[test]
fn loading_does_not_register_an_open_loop() {
    let r = reg();
    let mut d = with_loose_reverb(&r);
    let cables = send_cables(1);
    for (a, b) in &cables[..3] {
        d.apply(connect(a, b), &r).unwrap();
    }
    let loaded = load_project(&text(&d), &r).unwrap();
    assert!(loaded.console.returns.iter().all(Option::is_none));
}
