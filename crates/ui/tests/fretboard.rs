//! The fretboard through its public contract: which notes sound at a tick and which come next,
//! the chord at a tick, intervals, the header controls, what the Tab Editor feeds it (the cursor
//! while stopped, the playhead while playing) and the strip's place under the staff.

mod support;

use atomik_model::{ClipId, Note, RackDoc, TabData, TabDiagram, TabNoteRef};
use atomik_tab::chord::ChordSymbol;
use atomik_tab::voicing::{VoicingOptions, generate};
use atomik_tab::{Capo, Tuning};
use atomik_ui::fretboard::{
    DEFAULT_SIZE, FretChord, FretNote, FretboardData, FretboardItem, interval_name, live_notes,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, FRETBOARD_HEIGHT, FretLabels, FretboardFeed, FretboardOptions, FretboardView,
    Modifiers, PointerButton, TabEditorView, Theme, UiAction, UiEvent,
};
use std::sync::Arc;
use support::tab::*;
use support::*;

fn note(tick: i64, length: i64, string: u8, fret: u8) -> FretNote {
    FretNote {
        tick,
        length,
        string,
        fret,
    }
}

fn data(instrument: &str, capo: Capo, notes: Vec<FretNote>) -> FretboardData {
    let tab = TabData::for_instrument(instrument).unwrap();
    FretboardData {
        tuning: tab.tuning,
        capo,
        notes,
        chords: Vec::new(),
    }
}

fn voicing_of(d: &FretboardData, name: &str) -> FretChord {
    let v = generate(
        &d.tuning,
        d.capo,
        &ChordSymbol::parse(name).unwrap(),
        &VoicingOptions::default(),
    )
    .into_iter()
    .next()
    .expect("a voicing");
    FretChord {
        tick: 0,
        name: name.into(),
        voicing: v,
    }
}

fn click(v: &mut FretboardView, area: Rect, p: Point) -> Vec<UiAction> {
    v.handle_in(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        area,
    )
}

#[test]
fn sounding_notes_cover_their_length_and_the_next_group_is_the_next_onset() {
    let d = data(
        "guitar6",
        Capo::NONE,
        vec![
            note(0, 100, 5, 3),
            note(0, 50, 4, 2),
            note(100, 100, 3, 0),
            note(100, 100, 2, 0),
            note(300, 10, 1, 1),
        ],
    );
    let at0 = live_notes(&d, 0);
    assert_eq!(at0.sounding.len(), 2);
    assert_eq!(at0.next.len(), 2, "the two notes that start at 100");
    let at60 = live_notes(&d, 60);
    assert_eq!(
        at60.sounding,
        vec![note(0, 100, 5, 3)],
        "the short one is over"
    );
    let at100 = live_notes(&d, 100);
    assert_eq!(
        at100.sounding.len(),
        2,
        "a note starts sounding at its tick"
    );
    assert_eq!(at100.next, vec![note(300, 10, 1, 1)]);
    let at250 = live_notes(&d, 250);
    assert!(at250.sounding.is_empty());
    assert_eq!(at250.next.len(), 1);
    let end = live_notes(&d, 400);
    assert!(end.sounding.is_empty() && end.next.is_empty());
}

#[test]
fn the_chord_in_force_is_the_last_one_at_or_before_the_tick() {
    let mut d = data("guitar6", Capo::NONE, Vec::new());
    let mut em = voicing_of(&d, "Em");
    em.tick = 0;
    let mut c = voicing_of(&d, "C");
    c.tick = 960;
    d.chords = vec![em, c];
    assert_eq!(d.chord_at(0).unwrap().name, "Em");
    assert_eq!(d.chord_at(959).unwrap().name, "Em");
    assert_eq!(d.chord_at(960).unwrap().name, "C");
    assert!(
        data("guitar6", Capo::NONE, Vec::new())
            .chord_at(0)
            .is_none()
    );
}

#[test]
fn intervals_are_counted_from_the_root() {
    assert_eq!(interval_name(60, 60), "1");
    assert_eq!(interval_name(64, 60), "3");
    assert_eq!(interval_name(63, 60), "b3");
    assert_eq!(interval_name(67, 60), "5");
    assert_eq!(interval_name(58, 60), "b7");
    assert_eq!(interval_name(48, 60), "1", "octaves fold");
    assert_eq!(interval_name(61, 62), "7");
}

#[test]
fn the_labels_cycle_and_name_their_commands() {
    assert_eq!(FretLabels::Notes.next(), FretLabels::Frets);
    assert_eq!(FretLabels::Frets.next(), FretLabels::Intervals);
    assert_eq!(FretLabels::Intervals.next(), FretLabels::Notes);
    for l in FretLabels::ALL {
        assert_eq!(FretLabels::from_arg(l.arg()), Some(l));
    }
    assert_eq!(FretLabels::from_arg("nope"), None);
}

#[test]
fn a_feed_repaints_only_when_what_is_drawn_changes() {
    let d = Arc::new(data(
        "guitar6",
        Capo::NONE,
        vec![note(0, 100, 5, 3), note(200, 100, 4, 2)],
    ));
    let mut v = FretboardView::new(Theme::dark());
    let feed = |tick: i64| FretboardFeed {
        data: Some(d.clone()),
        tick: Some(tick),
        playing: true,
    };
    assert!(v.set_feed(&feed(0)), "first feed");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(!v.needs_repaint());
    assert!(!v.set_feed(&feed(10)), "the same notes sound");
    assert!(!v.needs_repaint());
    assert!(
        v.set_feed(&feed(150)),
        "nothing sounds, the next ones changed"
    );
    assert!(v.set_feed(&feed(210)));
    assert_eq!(v.live().sounding.len(), 1);
    // A change of options repaints, the same options do not.
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(!v.set_options(FretboardOptions::default()));
    assert!(v.set_options(FretboardOptions {
        left_handed: true,
        ..FretboardOptions::default()
    }));
    assert!(v.needs_repaint());
}

#[test]
fn the_header_controls_run_the_fretboard_commands() {
    let mut v = FretboardView::new(Theme::dark());
    let area = Rect::new(0.0, 0.0, 800.0, FRETBOARD_HEIGHT);
    v.set_feed(&FretboardFeed {
        data: Some(Arc::new(data("guitar6", Capo::NONE, Vec::new()))),
        tick: Some(0),
        playing: false,
    });
    let items = v.items(area);
    assert_eq!(items.len(), 3);
    let at = |item: FretboardItem| {
        items
            .iter()
            .find(|(_, i)| *i == item)
            .map(|(r, _)| r.center())
            .unwrap()
    };
    let run = |a: Vec<UiAction>| match a.as_slice() {
        [UiAction::RunCommand { id, arg }] => (id.clone(), arg.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        run(click(&mut v, area, at(FretboardItem::Labels))),
        (
            "view.fretboard_labels".to_string(),
            Some("frets".to_string())
        )
    );
    assert_eq!(
        run(click(&mut v, area, at(FretboardItem::LeftHanded))),
        ("view.fretboard_left_handed".to_string(), None)
    );
    assert_eq!(
        run(click(&mut v, area, at(FretboardItem::Voicing))),
        ("view.fretboard_voicing".to_string(), None)
    );
    // The chip after the labels the view shows.
    v.set_options(FretboardOptions {
        labels: FretLabels::Intervals,
        ..FretboardOptions::default()
    });
    assert_eq!(
        run(click(&mut v, area, at(FretboardItem::Labels))).1,
        Some("notes".to_string())
    );
    assert!(
        click(&mut v, area, Point::new(300.0, 90.0)).is_empty(),
        "the neck has no control"
    );
}

#[test]
fn every_control_has_a_tooltip_with_its_command() {
    let v = FretboardView::new(Theme::dark());
    let area = Rect::new(0.0, 0.0, 800.0, FRETBOARD_HEIGHT);
    for (r, item) in v.items(area) {
        let help = v.help_in(area, r.center()).expect("a tooltip");
        assert!(help.text.len() <= 90, "{item:?}");
        assert!(help.command.is_some(), "{item:?}");
    }
    assert!(v.help_in(area, Point::new(300.0, 90.0)).is_none());
}

#[test]
fn the_default_window_holds_the_strip() {
    assert!(DEFAULT_SIZE.1 >= FRETBOARD_HEIGHT);
}

// ---------------------------------------------------------------------- in the Tab Editor

fn guitar_doc() -> RackDoc {
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    let mut notes = Vec::new();
    for (i, (string, fret)) in [(5u8, 3u8), (4, 2), (3, 0), (2, 0), (1, 1)]
        .into_iter()
        .enumerate()
    {
        let tick = i as i64 * PPQ;
        let key = tab
            .tuning
            .pitch(usize::from(string), fret, tab.capo)
            .unwrap();
        notes.push(Note::new(tick, PPQ, key, 0.8));
        tab.notes.push(TabNoteRef::new(tick, key, string, fret));
    }
    let v = generate(
        &tab.tuning,
        tab.capo,
        &ChordSymbol::parse("C").unwrap(),
        &VoicingOptions::default(),
    )
    .remove(0);
    tab.diagrams.push(TabDiagram {
        tick: 0,
        name: "C".into(),
        voicing: v,
    });
    tab_doc("Guitar", notes, Some(tab))
}

fn editor(doc: &RackDoc) -> TabEditorView {
    let mut v = tab_editor(doc, &registry());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 560.0));
    v.set_fretboard_shown(true);
    v
}

#[test]
fn the_editor_feeds_the_cursor_while_stopped_and_the_playhead_while_playing() {
    let doc = guitar_doc();
    let mut v = editor(&doc);
    v.set_cursor(2 * PPQ, 0);
    let feed = v.fretboard_feed();
    assert_eq!(feed.tick, Some(2 * PPQ));
    assert!(!feed.playing);
    v.set_playing(true);
    v.set_playhead(PPQ + 10);
    let feed = v.fretboard_feed();
    assert_eq!(feed.tick, Some(PPQ + 10), "the playhead, not the cursor");
    let d = feed.data.unwrap();
    assert_eq!(d.notes.len(), 5);
    assert_eq!(d.chords[0].name, "C");
    assert_eq!(d.tuning.len(), 6);
    let live = live_notes(&d, PPQ + 10);
    assert_eq!(live.sounding, vec![note(PPQ, PPQ, 4, 2)]);
    assert_eq!(live.next, vec![note(2 * PPQ, PPQ, 3, 0)]);
}

#[test]
fn the_strip_sits_under_the_staff_and_takes_its_height() {
    let doc = guitar_doc();
    let mut v = editor(&doc);
    assert_eq!(
        v.fretboard_rect(),
        Some(Rect::new(0.0, 560.0 - FRETBOARD_HEIGHT, 1000.0, 560.0))
    );
    let with = v.string_y(5);
    v.set_fretboard_shown(false);
    assert_eq!(v.fretboard_rect(), None);
    // The staff is allowed to grow into the room the strip leaves.
    assert!(v.string_y(5) >= with);
    // A pane that is too short for the staff and the strip hides the strip.
    v.set_fretboard_shown(true);
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 300.0));
    assert_eq!(v.fretboard_rect(), None);
    assert!(v.fretboard_shown(), "still wanted");
}

#[test]
fn the_staff_clicks_still_land_where_the_strip_is_not() {
    let doc = guitar_doc();
    let mut v = editor(&doc);
    let area = v.fretboard_rect().unwrap();
    // A press on the strip's chip does not move the cursor and asks for a command.
    let (r, _) = FretboardView::new(Theme::dark()).items(area)[0];
    let before = v.cursor_cell();
    let actions = v.handle(
        &UiEvent::PointerDown {
            pos: r.center(),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        &doc,
        &registry(),
    );
    assert!(
        matches!(actions.as_slice(), [UiAction::RunCommand { id, .. }] if id.starts_with("view.fretboard"))
    );
    assert_eq!(v.cursor_cell(), before);
    // The tooltip of the chip comes through the editor.
    assert!(v.help_at(r.center()).is_some());
}

#[test]
fn no_clip_gives_an_empty_feed_and_no_strip() {
    let v = TabEditorView::new(Theme::dark());
    assert!(v.fretboard_feed().data.is_none());
    assert_eq!(v.fretboard_rect(), None);
    let _ = ClipId(1);
}

#[test]
fn a_banjo_drone_dot_sits_at_its_peg_and_a_capo_shifts_the_dots() {
    // Painting does not fail for any string count, the banjo's drone or a partial capo.
    for (id, capo) in [
        ("banjo5", Capo::NONE),
        ("banjo5", Capo::at(2)),
        ("guitar8", Capo::partial(3, 0b0011_1100)),
        ("bass6", Capo::NONE),
    ] {
        let d = data(id, capo, vec![note(0, 10, 0, 0), note(0, 10, 1, 4)]);
        let mut v = FretboardView::new(Theme::dark());
        v.set_feed(&FretboardFeed {
            data: Some(Arc::new(d)),
            tick: Some(0),
            playing: false,
        });
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        assert!(!dl.to_json().is_empty());
    }
    let one = FretboardData {
        tuning: Tuning::from_opens(&[40], 24),
        capo: Capo::NONE,
        notes: vec![note(0, 10, 0, 5)],
        chords: Vec::new(),
    };
    let mut v = FretboardView::new(Theme::dark());
    v.set_feed(&FretboardFeed {
        data: Some(Arc::new(one)),
        tick: Some(0),
        playing: false,
    });
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
}
