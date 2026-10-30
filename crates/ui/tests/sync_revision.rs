//! `sync_at` and `set_doc_revision` of the sequencer views: a document known by its revision
//! is read once, and a new revision is read again.

mod support;

use atomik_model::{ClipContent, ClipId, Command, Document, Note, SongCommand};
use support::song::*;
use support::*;

fn edited(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(doc.clone());
    d.apply(
        Command::Song(SongCommand::SetClipContent {
            id: ClipId(1),
            content: ClipContent::Notes(vec![Note::new(0, 480, 60, 0.5)]),
            merge: None,
        }),
        reg,
    )
    .unwrap();
    d
}

#[test]
fn the_roll_reads_the_open_clip_again_only_when_the_revision_changed() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let before = v.notes().len();
    assert!(before > 1);
    let new = edited(&doc, &reg);
    // The same revision stands for the same document: nothing is read.
    v.sync_at(new.doc(), &reg, 7);
    assert_eq!(v.notes().len(), 1, "the first sight of a revision reads it");
    v.sync_at(&doc, &reg, 7);
    assert_eq!(v.notes().len(), 1, "an equal revision is not read again");
    v.sync_at(&doc, &reg, 8);
    assert_eq!(v.notes().len(), before, "a new revision is");
}

#[test]
fn a_plain_sync_always_reads_and_resets_the_revision() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let new = edited(&doc, &reg);
    v.sync_at(&doc, &reg, 1);
    v.sync(new.doc(), &reg);
    assert_eq!(v.notes().len(), 1);
    v.sync_at(&doc, &reg, 1);
    assert!(v.notes().len() > 1, "a plain sync forgot the revision");
}

#[test]
fn the_host_revision_gates_the_view_own_sync_in_handle_and_paint() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let new = edited(&doc, &reg);
    let mut list = atomik_ui::DisplayList::new();
    // Without a revision every call reads the document.
    v.paint(&mut list, new.doc(), &reg);
    assert_eq!(v.notes().len(), 1);
    v.paint(&mut list, &doc, &reg);
    assert!(v.notes().len() > 1);
    // With one, a paint of the same revision does not.
    v.set_doc_revision(Some(3));
    v.paint(&mut list, new.doc(), &reg);
    assert_eq!(v.notes().len(), 1);
    v.paint(&mut list, &doc, &reg);
    assert_eq!(v.notes().len(), 1);
    v.set_doc_revision(Some(4));
    v.paint(&mut list, &doc, &reg);
    assert!(v.notes().len() > 1);
}
