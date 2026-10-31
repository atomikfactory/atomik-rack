//! The controls of an automated param: the mark that says a lane plays it, and the value they
//! show while the song plays or is positioned (set by the app with `set_shown_values`, never
//! written to the document). Regenerate the golden with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test automation_shown`.

mod support;

use atomik_model::{RackDoc, ShownValues};
use atomik_ui::{DisplayList, Flip, RackView};
use std::path::PathBuf;
use support::console::*;
use support::song::*;
use support::*;

const CUTOFF: &str = "filter.cutoff";

fn shown(device: &str, param: &str, value: f32) -> ShownValues {
    let mut s = ShownValues::new();
    s.insert((key(device), param.to_string()), value);
    s
}

fn rack() -> (RackDoc, atomik_device_sdk::Registry, RackView) {
    let (doc, reg) = (song_doc(), registry());
    let v = view(&doc, &reg);
    (doc, reg, v)
}

fn paint(v: &mut RackView, doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl
}

#[test]
fn a_control_follows_the_value_it_is_given_and_the_document_stays() {
    let (doc, reg, mut v) = rack();
    let before = doc.clone();
    let stored = v.shown_value(&key("bass"), CUTOFF).unwrap();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    v.sync(&doc, &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(1234.0));
    assert_eq!(doc, before, "the document is not touched");
    // Another lane value: the next frame of the song.
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 2000.0)));
    v.sync(&doc, &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(2000.0));
    // Nothing shown any more (the lane stopped playing): the document's value is back.
    assert!(v.set_shown_values(&ShownValues::new()));
    v.sync(&doc, &reg);
    assert_eq!(v.shown_value(&key("bass"), CUTOFF), Some(stored));
}

#[test]
fn only_the_panel_whose_displayed_value_changed_is_re_recorded() {
    let (doc, reg, mut v) = rack();
    let _ = paint(&mut v, &doc, &reg);
    assert!(!v.needs_repaint());
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    assert!(v.needs_repaint());
    let revs = v.panel_revisions();
    let _ = paint(&mut v, &doc, &reg);
    let now = v.panel_revisions();
    for ((k, a), (_, b)) in revs.iter().zip(&now) {
        assert_eq!(a != b, *k == key("bass"), "{k:?}");
    }
    // The same values again: nothing to do, nothing re-recorded.
    assert!(!v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    let _ = paint(&mut v, &doc, &reg);
    assert_eq!(v.panel_revisions(), now);
    assert!(!v.needs_repaint());
    // A device the rack does not have is ignored.
    let mut more = shown("bass", CUTOFF, 1234.0);
    more.insert((key("nope"), CUTOFF.to_string()), 1.0);
    assert!(v.set_shown_values(&more));
    v.sync(&doc, &reg);
    assert_eq!(v.panel_revisions(), now);
}

#[test]
fn a_param_with_a_lane_is_marked_and_the_others_are_not() {
    let (doc, reg, mut v) = rack();
    v.sync(&doc, &reg);
    assert!(v.is_automated(&key("bass"), CUTOFF));
    assert!(!v.is_automated(&key("bass"), "osc2.wave"));
    assert!(!v.is_automated(&key("keys"), CUTOFF));
    // The mark is there with the transport stopped and nothing shown.
    let plain = {
        let mut d = doc.clone();
        d.song_mut().tracks[0].lanes.pop();
        let mut u = view(&d, &reg);
        u.sync(&d, &reg);
        u
    };
    assert!(!plain.is_automated(&key("bass"), CUTOFF));
}

#[test]
fn the_mark_changes_the_look_of_the_panel_only() {
    let (doc, reg, mut v) = rack();
    let marked = paint(&mut v, &doc, &reg).to_json();
    let mut bare = doc.clone();
    bare.song_mut().tracks[0].lanes.pop();
    let mut u = view(&bare, &reg);
    let unmarked = paint(&mut u, &bare, &reg).to_json();
    assert_ne!(marked, unmarked);
}

#[test]
fn the_hover_tooltip_of_an_automated_control_says_so_and_shows_the_value() {
    let (doc, reg, mut v) = rack();
    assert!(v.set_shown_values(&shown("bass", CUTOFF, 1234.0)));
    v.sync(&doc, &reg);
    let _ = paint(&mut v, &doc, &reg);
    let at = v
        .control_center(&key("bass"), CUTOFF)
        .expect("the cutoff control");
    let _ = v.handle(&mv(at), &doc, &reg);
    let tip = v.tooltip().expect("a tooltip over the control");
    assert!(tip.contains("(automated)"), "{tip}");
    assert!(tip.contains("1.2") || tip.contains("1234"), "{tip}");
}

// ---------------------------------------------------------------- golden

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with ATOMIK_UPDATE_GOLDEN=1"))
        .replace("\r\n", "\n");
    assert!(expected == actual, "{name} differs from its golden");
}

#[test]
fn an_automated_panel_front_matches_golden() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Front);
    v.set_shown_values(&shown("bass", CUTOFF, 1234.0));
    golden(
        "automated_panel_front_1x.json",
        &paint(&mut v, &doc, &reg).to_json(),
    );
}

// ---------------------------------------------------------------- console

#[test]
fn a_console_strip_follows_its_shown_value_and_marks_the_lane() {
    let (mut doc, reg) = (console_doc(), console_registry());
    // A channel strip has an automation track of its own: the lane alone.
    let mut t = song_doc().song_mut().tracks.remove(0);
    t.device = key("ch-bass");
    t.lanes = vec![atomik_model::Lane::automation("fader")];
    doc.song_mut().tracks = vec![t];
    let mut v = console_view(&doc, &reg);
    assert!(v.is_automated(&key("ch-bass"), "fader"));
    assert!(!v.is_automated(&key("ch-lead"), "fader"));
    let stored = v.shown_value(&key("ch-bass"), "fader").unwrap();
    assert!(v.set_shown_values(&shown("ch-bass", "fader", 0.31)));
    v.sync(&doc, &reg);
    assert!((v.shown_value(&key("ch-bass"), "fader").unwrap() - 0.31).abs() < 1e-6);
    assert!(!v.set_shown_values(&shown("ch-bass", "fader", 0.31)));
    assert!(v.set_shown_values(&ShownValues::new()));
    v.sync(&doc, &reg);
    assert_eq!(v.shown_value(&key("ch-bass"), "fader"), Some(stored));
}

#[test]
fn the_master_section_follows_its_shown_value_and_marks_the_lane() {
    let (mut doc, reg) = (console_doc(), console_registry());
    let paint_master = |doc: &RackDoc| {
        let mut v = console_view(doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, doc, &reg);
        (v, dl.to_json())
    };
    let (plain, without) = paint_master(&doc);
    assert!(!plain.is_automated(&key("master"), "return1.level"));
    let mut t = song_doc().song_mut().tracks.remove(0);
    t.device = key("master");
    t.lanes = vec![atomik_model::Lane::automation("return1.level")];
    doc.song_mut().tracks = vec![t];
    let (mut v, with) = paint_master(&doc);
    assert!(v.is_automated(&key("master"), "return1.level"));
    assert_ne!(
        without, with,
        "the master control carries the automation mark"
    );
    // The control follows the value it is given, and the document stays.
    let stored = v.shown_value(&key("master"), "return1.level").unwrap();
    assert!(v.set_shown_values(&shown("master", "return1.level", -20.0)));
    v.sync(&doc, &reg);
    assert!((v.shown_value(&key("master"), "return1.level").unwrap() + 20.0).abs() < 1e-3);
    assert!(v.set_shown_values(&ShownValues::new()));
    v.sync(&doc, &reg);
    assert_eq!(v.shown_value(&key("master"), "return1.level"), Some(stored));
}
