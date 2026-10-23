//! The relink panel: the list of missing audio files paints, LOCATE and SEARCH FOLDER emit the
//! right actions, CLOSE closes, the list scrolls and an empty list says nothing is missing.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, Modifiers, RelinkAction, RelinkEntry, RelinkView, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 600.0);

fn entry(n: u8, name: &str) -> RelinkEntry {
    RelinkEntry {
        hash: format!("{n:02x}").repeat(32),
        name: name.to_owned(),
    }
}

fn view(entries: Vec<RelinkEntry>) -> RelinkView {
    let mut v = RelinkView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_missing(entries);
    v
}

fn two() -> RelinkView {
    view(vec![entry(1, "kick.wav"), entry(2, "snare.wav")])
}

fn click(v: &mut RelinkView, p: Point) -> Vec<RelinkAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Relink(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn texts(v: &RelinkView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

#[test]
fn the_panel_lists_every_missing_file_with_its_buttons() {
    let v = two();
    let t = texts(&v);
    for want in [
        "RELINK MISSING AUDIO",
        "2 audio files missing",
        "SEARCH FOLDER...",
        "CLOSE",
        "kick.wav",
        "snare.wav",
        "01010101",
        "02020202",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
    assert_eq!(t.iter().filter(|x| *x == "LOCATE...").count(), 2);
}

#[test]
fn a_single_missing_file_reads_in_the_singular() {
    let v = view(vec![entry(1, "kick.wav")]);
    assert!(has(&texts(&v), "1 audio file missing"));
}

#[test]
fn locate_emits_the_asset_of_its_row() {
    let mut v = two();
    for (i, e) in v.entries().to_vec().iter().enumerate() {
        let r = v.locate_rect(i).expect("row in view");
        assert_eq!(
            click(&mut v, r.center()),
            [RelinkAction::Locate {
                hash: e.hash.clone()
            }]
        );
    }
}

#[test]
fn search_folder_and_close_emit_their_actions() {
    let mut v = two();
    assert_eq!(
        {
            let at = v.search_rect().center();
            click(&mut v, at)
        },
        [RelinkAction::SearchFolder]
    );
    assert_eq!(
        {
            let at = v.close_rect().center();
            click(&mut v, at)
        },
        [RelinkAction::Close]
    );
    // A click elsewhere in the panel, or outside it, does nothing.
    let p = v.panel();
    assert!(click(&mut v, Point::new(p.x0 + 3.0, p.y0 + 60.0)).is_empty());
    assert!(click(&mut v, Point::new(2.0, 2.0)).is_empty());
}

#[test]
fn a_secondary_click_does_nothing() {
    let mut v = two();
    let at = v.locate_rect(0).unwrap().center();
    let actions = v.handle(&down_with(at, atomik_ui::PointerButton::Secondary, 1));
    assert!(actions.is_empty());
}

#[test]
fn an_empty_list_says_nothing_is_missing_and_only_closes() {
    let mut v = view(Vec::new());
    let t = texts(&v);
    assert!(has(&t, "Nothing is missing."));
    assert!(!has(&t, "SEARCH FOLDER..."));
    assert!(!has(&t, "LOCATE..."));
    assert_eq!(
        {
            let at = v.close_rect().center();
            click(&mut v, at)
        },
        [RelinkAction::Close]
    );
    assert!(
        {
            let at = v.search_rect().center();
            click(&mut v, at)
        }
        .is_empty()
    );
}

#[test]
fn a_long_list_scrolls_and_only_paints_the_rows_in_view() {
    let entries: Vec<RelinkEntry> = (0..40)
        .map(|n| entry(n, &format!("take-{n}.wav")))
        .collect();
    let mut v = view(entries);
    let first = texts(&v);
    assert!(has(&first, "take-0.wav"));
    assert!(
        !has(&first, "take-39.wav"),
        "the last row is below the panel"
    );
    assert!(v.panel().y1 <= BOUNDS.y1);
    assert_eq!(v.scroll(), 0.0);
    // Scroll to the end: the wheel moves by rows and stops at the last one.
    for _ in 0..200 {
        v.handle(&UiEvent::Wheel {
            pos: Point::new(450.0, 300.0),
            delta: Vec2::new(0.0, -1.0),
            modifiers: Modifiers::NONE,
        });
    }
    assert!(v.scroll() > 0.0);
    let last = texts(&v);
    assert!(has(&last, "take-39.wav"), "{last:?}");
    assert!(!has(&last, "take-0.wav"));
    // The visible row's button still names its own asset.
    let i = (0..40).rev().find(|i| v.locate_rect(*i).is_some()).unwrap();
    assert_eq!(i, 39);
    let hit = {
        let at = v.locate_rect(i).unwrap().center();
        click(&mut v, at)
    };
    assert_eq!(
        hit,
        [RelinkAction::Locate {
            hash: format!("{:02x}", 39).repeat(32)
        }]
    );
    // Scrolling back up.
    for _ in 0..200 {
        v.handle(&UiEvent::Wheel {
            pos: Point::new(450.0, 300.0),
            delta: Vec2::new(0.0, 1.0),
            modifiers: Modifiers::NONE,
        });
    }
    assert_eq!(v.scroll(), 0.0);
}

#[test]
fn the_list_follows_what_the_app_feeds_it() {
    let mut v = two();
    v.set_missing(vec![entry(2, "snare.wav")]);
    let t = texts(&v);
    assert!(!has(&t, "kick.wav"));
    assert!(has(&t, "1 audio file missing"));
    assert!(v.locate_rect(1).is_none());
}

#[test]
fn the_panel_stays_inside_a_small_window() {
    let mut v = two();
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 200.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 300.0 && p.y1 <= 200.0, "{p:?}");
}
