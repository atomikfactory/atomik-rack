//! Reference tracks: `SetReferences` as one undo step, the optional `"references"` field of the
//! project format (written only when non-empty, byte-identical save, load and save) and its limits.

mod common;

use atomik_model::{
    Command, Document, MAX_REFERENCES, ModelError, Reference, SyncOp, load_project,
    new_project_doc, save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

fn refs() -> Vec<Reference> {
    vec![
        Reference::new("first.wav"),
        Reference {
            path: "other/second.flac".into(),
            cue_ms: Some(42_000),
        },
    ]
}

#[test]
fn a_new_project_has_none_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().references.is_empty());
    assert!(!text(&d).contains("references"));
}

#[test]
fn setting_the_list_is_one_undo_step() {
    let (mut d, r) = doc();
    let ops = one_step(&mut d, &r, Command::SetReferences(refs()));
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.doc().references, refs());
    assert_eq!(d.undo_label(), Some("Edit Reference Tracks"));
    d.undo().unwrap();
    assert!(d.doc().references.is_empty());
    d.redo().unwrap();
    assert_eq!(d.doc().references, refs());
    rejected(&mut d, &r, Command::SetReferences(refs()));
}

#[test]
fn the_limits_are_enforced() {
    let (mut d, r) = doc();
    for bad in [
        vec![Reference::new("")],
        vec![Reference::new("a\nb.wav")],
        vec![Reference::new("a.wav"), Reference::new("a.wav")],
        (0..=MAX_REFERENCES)
            .map(|i| Reference::new(format!("{i}.wav")))
            .collect(),
    ] {
        let e = rejected(&mut d, &r, Command::SetReferences(bad));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    }
}

#[test]
fn the_save_is_canonical_and_loads_back_byte_for_byte() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetReferences(refs()));
    let saved = text(&d);
    assert!(saved.contains("\"references\": ["), "{saved}");
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.references, refs());
    assert_eq!(save_project(&back), saved, "save, load, save");
}

#[test]
fn the_loader_names_the_path_of_a_bad_entry() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetReferences(refs()));
    let saved = text(&d);
    let bad = saved.replacen("\"cue_ms\": 42000", "\"cue_ms\": \"soon\"", 1);
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(e.contains("references[1].cue_ms"), "{e}");
    let empty = saved.replacen("\"first.wav\"", "\"\"", 1);
    let e = load_project(&empty, &r).unwrap_err().to_string();
    assert!(e.contains("references[0]"), "{e}");
}

#[test]
fn a_copy_of_the_document_keeps_the_list() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetReferences(refs()));
    let copy = d.doc().clone();
    assert_eq!(copy, *d.doc());
}
