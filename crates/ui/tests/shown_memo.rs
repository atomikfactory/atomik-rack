//! The rack builds the document with the shown automation values once per document revision and
//! set of shown values, not once per paint, and shows exactly what a build per paint showed.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, RackDoc, ShownValues};
use atomik_ui::{DisplayList, RackView};
use support::song::*;
use support::*;

const CUTOFF: &str = "filter.cutoff";

fn shown(device: &str, param: &str, value: f32) -> ShownValues {
    let mut s = ShownValues::new();
    s.insert((key(device), param.to_string()), value);
    s
}

/// One host paint: the revision is declared for the call only, as the app does.
fn paint(v: &mut RackView, doc: &RackDoc, revision: u64, reg: &Registry) -> DisplayList {
    let mut dl = DisplayList::new();
    v.set_doc_revision(Some(revision));
    v.paint(&mut dl, doc, reg);
    v.set_doc_revision(None);
    dl
}

fn set_param(d: &mut Document, device: &str, param: &str, value: f32, reg: &Registry) {
    d.apply(
        Command::SetParam {
            key: key(device),
            param: param.to_string(),
            value,
            merge: None,
        },
        reg,
    )
    .unwrap();
}

fn setup() -> (Document, Registry, RackView) {
    let (doc, reg) = (song_doc(), registry());
    let v = view(&doc, &reg);
    (Document::new(doc), reg, v)
}

#[test]
fn paints_of_an_unchanged_document_and_shown_set_build_the_overlay_once() {
    let (d, reg, mut v) = setup();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    for _ in 0..20 {
        let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    }
    assert_eq!(v.shown_doc_builds(), 1);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
    // The same values again change nothing and build nothing.
    assert!(!v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), 1);
}

#[test]
fn a_call_without_a_declared_revision_builds_every_time() {
    let (d, reg, mut v) = setup();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    for _ in 0..3 {
        v.paint(&mut DisplayList::new(), d.doc(), &reg);
    }
    assert_eq!(
        v.shown_doc_builds(),
        0,
        "nothing is kept without a revision"
    );
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
}

#[test]
fn a_changed_shown_value_shows_on_the_next_paint() {
    let (d, reg, mut v) = setup();
    for (n, value) in [1234.0, 2000.0, 3000.0].into_iter().enumerate() {
        assert!(v.set_shown_values(&shown("bass", CUTOFF, value)));
        let _ = paint(&mut v, d.doc(), d.revision(), &reg);
        assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(value));
        assert_eq!(v.shown_doc_builds(), n as u64 + 1);
    }
}

#[test]
fn an_edit_of_the_document_shows_on_the_next_paint_and_so_does_its_undo() {
    let (mut d, reg, mut v) = setup();
    let stored_lead = v.shown_value(&key("lead"), CUTOFF).unwrap();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), 1);
    // An edit of a param that is not shown: the overlay is built again, from the new document.
    set_param(&mut d, "lead", CUTOFF, 777.0, &reg);
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), 2);
    assert_eq!(v.shown_value(&key("lead"), CUTOFF), Some(777.0));
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
    // Undo: the stored value is back, and the shown one is still laid over its param.
    d.undo().unwrap();
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), 3);
    assert_eq!(v.shown_value(&key("lead"), CUTOFF), Some(stored_lead));
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
    // An edit of the shown param itself: the lane still wins on screen.
    set_param(&mut d, "bass", CUTOFF, 555.0, &reg);
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
}

#[test]
fn a_lane_that_loses_its_last_point_gives_the_stored_value_back() {
    let (d, reg, mut v) = setup();
    let stored = v.shown_value(&key("bass"), CUTOFF).unwrap();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
    assert!(v.set_shown_values(&ShownValues::new()));
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(stored));
    // Nothing shown: nothing is built, and a later show builds again.
    let builds = v.shown_doc_builds();
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), builds);
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    let _ = paint(&mut v, d.doc(), d.revision(), &reg);
    assert_eq!(v.shown_doc_builds(), builds + 1);
}

#[test]
fn a_kept_overlay_paints_what_a_fresh_one_paints() {
    let (d, reg, mut kept) = setup();
    let mut fresh = view(d.doc(), &reg);
    for value in [1234.0, 1234.0, 2000.0] {
        let s = shown("bass", CUTOFF, value);
        kept.set_shown_values(&s);
        fresh.set_shown_values(&s);
        let a = paint(&mut kept, d.doc(), d.revision(), &reg);
        let mut b = DisplayList::new();
        fresh.paint(&mut b, d.doc(), &reg);
        assert_eq!(a.to_json(), b.to_json());
    }
}
