//! The repetition panel: it lists the ideas with their track, what repeats and where, a row click
//! chooses the idea and locates it, a suggestion that is an edit emits its action once, advice
//! shows its text, the list scrolls, and every control has a tooltip.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, Key, Modifiers, RepetitionAction, RepetitionChoice, RepetitionData, RepetitionRow,
    RepetitionView, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 960.0, 700.0);

fn choice(label: &str, can_apply: bool) -> RepetitionChoice {
    RepetitionChoice {
        label: label.into(),
        text: format!("{label} text"),
        can_apply,
        done: false,
    }
}

fn row(track: &str, bars: &str, apply: [bool; 3]) -> RepetitionRow {
    RepetitionRow {
        track: track.into(),
        what: "a 1-bar pattern".into(),
        repeats: 4,
        bars: bars.into(),
        choices: vec![
            choice("Drop a hit", apply[0]),
            choice("Filter the last bar", apply[1]),
            choice("Cut a beat", apply[2]),
        ],
    }
}

fn data() -> RepetitionData {
    RepetitionData {
        tracks: 5,
        rows: vec![
            row("Drums", "bars 5-8", [true, true, true]),
            row("Bass", "bars 9-12", [false, true, false]),
            row("Hats", "bars 1-4", [true, false, true]),
        ],
    }
}

fn view() -> RepetitionView {
    let mut v = RepetitionView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(Some(data()));
    v
}

fn click(v: &mut RepetitionView, p: Point) -> Vec<RepetitionAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Repetition(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &RepetitionView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_lists_the_ideas_with_track_what_and_where() {
    let t = texts(&view());
    assert!(t.iter().any(|s| s == "FIND REPETITION"));
    assert!(
        t.iter().any(|s| s.contains("Drums")
            && s.contains("a 1-bar pattern")
            && s.contains("4 times")
            && s.contains("bars 5-8")),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "DROP A HIT"));
    assert!(t.iter().any(|s| s == "CUT A BEAT"));
    // The line under an idea is its first suggestion.
    assert!(t.iter().any(|s| s == "Drop a hit text"));
}

#[test]
fn an_empty_panel_says_what_to_do_and_a_clean_song_says_so() {
    let mut v = RepetitionView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "No search yet"));
    assert!(t.iter().any(|s| s.starts_with("Find lists")));
    v.set_data(Some(RepetitionData {
        tracks: 4,
        rows: Vec::new(),
    }));
    let t = texts(&v);
    assert!(
        t.iter().any(|s| s == "No repeated ideas in 4 tracks"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s.starts_with("Nothing repeats")));
}

#[test]
fn the_buttons_emit_their_actions() {
    let mut v = view();
    let find = v.find_rect().center();
    assert_eq!(click(&mut v, find), vec![RepetitionAction::Find]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![RepetitionAction::Close]);
    // A suggestion that is an edit emits its place.
    let drop = v.choice_rect(0, 0).center();
    assert_eq!(click(&mut v, drop), vec![RepetitionAction::Apply(0, 0)]);
    let filter = v.choice_rect(1, 1).center();
    assert_eq!(click(&mut v, filter), vec![RepetitionAction::Apply(1, 1)]);
    assert_eq!(v.chosen(), 1);
    // A click on the row away from the buttons chooses it and locates it.
    let away = v.row_rect(2).center();
    assert_eq!(click(&mut v, away), vec![RepetitionAction::Locate(2)]);
    assert_eq!(v.chosen(), 2);
}

#[test]
fn advice_shows_its_text_instead_of_emitting() {
    let mut v = view();
    // Idea 1 (Bass): "Drop a hit" is advice only.
    let advice = v.choice_rect(1, 0).center();
    assert!(click(&mut v, advice).is_empty());
    assert_eq!(v.note(), "Drop a hit text");
    assert!(texts(&v).iter().any(|s| s == "Drop a hit text"));
}

#[test]
fn a_variation_that_was_made_says_done_and_does_not_emit_again() {
    let mut v = view();
    let mut d = data();
    d.rows[0].choices[0].done = true;
    v.set_data(Some(d));
    assert!(texts(&v).iter().any(|s| s == "DONE"));
    let button = v.choice_rect(0, 0).center();
    assert!(click(&mut v, button).is_empty());
    let help = v.help_at(button).expect("a tooltip");
    assert_eq!(help.text, "Already made");
}

#[test]
fn a_long_list_scrolls_with_the_wheel_and_the_arrow_keys() {
    let mut v = RepetitionView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let rows: Vec<RepetitionRow> = (0..10)
        .map(|n| row(&format!("Track {n}"), "bars 1-4", [true, true, true]))
        .collect();
    v.set_data(Some(RepetitionData { tracks: 10, rows }));
    assert_eq!((v.chosen(), v.top()), (0, 0));
    let inside = v.list_rect().center();
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -1.0),
        pos: inside,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.top(), 1);
    for _ in 0..9 {
        v.handle(&UiEvent::Key {
            key: Key::Down,
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
    assert_eq!(v.chosen(), 9);
    assert_eq!(v.top(), 4);
    assert!(
        texts(&v).iter().any(|s| s == "5-10 of 10"),
        "{:?}",
        texts(&v)
    );
    // After scrolling a click chooses the idea in that slot.
    let slot = v.row_rect(0).center();
    click(&mut v, slot);
    assert_eq!(v.chosen(), 4);
}

#[test]
fn every_control_has_a_short_tooltip() {
    let v = view();
    for p in [
        v.close_rect().center(),
        v.find_rect().center(),
        v.choice_rect(0, 0).center(),
        v.choice_rect(1, 0).center(),
        v.row_rect(0).center(),
    ] {
        let help = v.help_at(p).expect("a tooltip");
        assert!(help.text.len() <= 80, "{}", help.text);
    }
}

#[test]
fn the_panel_stays_inside_a_small_window() {
    let mut v = view();
    v.set_bounds(Rect::new(0.0, 0.0, 700.0, 520.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 700.0 && p.y0 >= 0.0 && p.y1 <= 520.0);
    assert!(v.list_rect().x1 <= p.x1);
    assert!(v.choice_rect(5, 2).y1 <= p.y1);
    assert!(v.choice_rect(0, 0).x0 >= v.list_rect().x0);
}
