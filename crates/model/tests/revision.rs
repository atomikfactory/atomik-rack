//! `Document::revision`: it differs after every change of the document and after nothing else.

mod common;

use atomik_model::{Document, empty_doc};
use common::*;

#[test]
fn every_change_takes_a_new_revision() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    let r0 = d.revision();
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    let r1 = d.revision();
    assert_ne!(r1, r0, "apply");
    d.apply(set_param("synth", "filter.cutoff", 500.0), &reg)
        .unwrap();
    let r2 = d.revision();
    assert_ne!(r2, r1, "apply of a second command");
    assert!(d.undo(&reg).is_some());
    let r3 = d.revision();
    assert_ne!(r3, r2, "undo");
    assert!(d.redo(&reg).is_some());
    assert_ne!(d.revision(), r3, "redo");
}

#[test]
fn nothing_to_undo_or_redo_and_a_rejected_command_keep_the_revision() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    let r = d.revision();
    assert!(d.undo(&reg).is_none());
    assert!(d.redo(&reg).is_none());
    assert!(d.apply(set_param("nobody", "gain", 0.5), &reg).is_err());
    d.end_merge();
    assert_eq!(d.revision(), r);
    assert_eq!(d.revision(), r, "reading changes nothing");
}

#[test]
fn a_new_document_never_reuses_a_revision() {
    let a = Document::new(empty_doc());
    let b = Document::new(empty_doc());
    assert_ne!(a.revision(), b.revision());
}

#[test]
fn a_clone_diverges_from_the_original() {
    let reg = registry();
    let mut a = Document::new(empty_doc());
    let mut b = a.clone();
    assert_eq!(a.revision(), b.revision());
    a.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    b.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    assert_ne!(a.revision(), b.revision());
}
