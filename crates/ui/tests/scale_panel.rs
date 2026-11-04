//! The Scale Tempo panel: typing a percentage announces a preview, APPLY and Enter apply it only
//! while it is usable, CLOSE closes, and the text takes a comma, a percent sign and the clipboard.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, Modifiers, ScaleAction, ScaleView, Theme, UiAction, UiEvent, parse_percent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn view(percent: f64) -> ScaleView {
    let mut v = ScaleView::new(Theme::dark(), percent);
    v.set_bounds(BOUNDS);
    v.set_target("3 clips".into(), true);
    v
}

fn scale(actions: Vec<UiAction>) -> Vec<ScaleAction> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            UiAction::ScaleTempo(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn press(v: &mut ScaleView, key: Key) -> Vec<ScaleAction> {
    scale(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn type_text(v: &mut ScaleView, text: &str) -> Vec<ScaleAction> {
    text.chars().flat_map(|c| press(v, Key::Char(c))).collect()
}

fn click(v: &mut ScaleView, p: Point) -> Vec<ScaleAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    scale(out)
}

fn texts(v: &ScaleView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_shows_its_labels_and_the_percentage_it_was_given() {
    let v = view(150.0);
    let t = texts(&v);
    for want in [
        "SCALE TEMPO",
        "Applies to: 3 clips",
        "Tempo",
        "150",
        "APPLY",
        "CLOSE",
    ] {
        assert!(t.iter().any(|x| x.contains(want)), "{want} in {t:?}");
    }
    assert_eq!(v.percent(), Some(150.0));
}

#[test]
fn the_text_starts_selected_so_typing_replaces_it_and_each_usable_value_previews() {
    let mut v = view(150.0);
    assert_eq!(
        v.open_preview(),
        vec![UiAction::ScaleTempo(ScaleAction::Preview(Some(150.0)))]
    );
    // Typing "8" replaces the selection: 8 is below the range, so the preview goes away.
    let a = type_text(&mut v, "8");
    assert_eq!(a, vec![ScaleAction::Preview(None)]);
    // "87" is usable, "87." is the same number (no new preview), "87.5" is a new one.
    let a = type_text(&mut v, "7");
    assert_eq!(a, vec![ScaleAction::Preview(Some(87.0))]);
    assert!(type_text(&mut v, ".").is_empty());
    let a = type_text(&mut v, "5");
    assert_eq!(a, vec![ScaleAction::Preview(Some(87.5))]);
    assert_eq!(v.text(), "87.5");
    // Backspacing: "87." and "87" are 87 (one preview), "8" and "" are nothing (one preview).
    let mut a = Vec::new();
    for _ in 0..4 {
        a.extend(press(&mut v, Key::Backspace));
    }
    assert_eq!(
        a,
        vec![ScaleAction::Preview(Some(87.0)), ScaleAction::Preview(None)]
    );
    assert_eq!(v.percent(), None);
}

#[test]
fn a_hundred_or_a_number_out_of_range_is_not_usable() {
    for bad in ["100", "9", "1001", "abc", ""] {
        assert_eq!(parse_percent(bad), None, "{bad}");
    }
    let mut v = view(100.0);
    assert!(
        v.open_preview().is_empty(),
        "100 changes nothing: no preview"
    );
    assert!(press(&mut v, Key::Enter).is_empty());
    let apply = v.apply_rect().center();
    assert!(
        click(&mut v, apply).is_empty(),
        "APPLY waits for a usable value"
    );
}

#[test]
fn apply_and_enter_apply_close_closes_and_a_panel_without_a_target_applies_nothing() {
    let mut v = view(200.0);
    assert_eq!(press(&mut v, Key::Enter), vec![ScaleAction::Apply]);
    let apply = v.apply_rect().center();
    assert_eq!(click(&mut v, apply), vec![ScaleAction::Apply]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![ScaleAction::Close]);
    v.set_target(String::new(), false);
    assert!(press(&mut v, Key::Enter).is_empty());
    assert!(v.open_preview().is_empty(), "nothing to preview");
    assert!(
        texts(&v)
            .iter()
            .any(|t| t.contains("Select the clips to scale"))
    );
}

#[test]
fn a_comma_and_a_percent_sign_are_understood() {
    let mut v = view(150.0);
    type_text(&mut v, "62,5 %");
    assert_eq!(v.percent(), Some(62.5));
}
