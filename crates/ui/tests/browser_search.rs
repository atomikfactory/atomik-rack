//! The device browser's search field is an editable text field: Ctrl/Cmd+A selects the whole
//! query, copy, cut and paste work on the text and not on the rack, typing over a selection
//! replaces it, Backspace deletes it, and a click places the caret.

mod support;

use atomik_model::RackDoc;
use atomik_ui::kurbo::Point;
use atomik_ui::{Key, Modifiers, PointerButton, RackView, UiEvent};
use support::song::song_doc;
use support::*;

fn setup() -> (RackDoc, RackView) {
    let doc = song_doc();
    let reg = registry();
    let mut v = plain_view(&doc, &reg);
    v.set_browser_visible(true);
    (doc, v)
}

fn type_text(v: &mut RackView, doc: &RackDoc, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { Key::Char(c) };
        let _ = v.handle(
            &UiEvent::Key {
                key,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            doc,
            &registry(),
        );
    }
}

fn command(v: &mut RackView, doc: &RackDoc, id: &str) {
    let _ = v.command(id, None, doc, &registry());
}

#[test]
fn select_all_then_typing_replaces_the_query() {
    let (doc, mut v) = setup();
    type_text(&mut v, &doc, "bass");
    assert_eq!(v.browser_query(), "bass");
    command(&mut v, &doc, "edit.select_all");
    assert_eq!(v.browser_query(), "bass");
    type_text(&mut v, &doc, "x");
    assert_eq!(v.browser_query(), "x");
}

#[test]
fn select_all_then_backspace_empties_the_query() {
    let (doc, mut v) = setup();
    type_text(&mut v, &doc, "bass");
    command(&mut v, &doc, "edit.select_all");
    let _ = v.handle(
        &UiEvent::Key {
            key: Key::Backspace,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        &doc,
        &registry(),
    );
    assert_eq!(v.browser_query(), "");
}

#[test]
fn cut_and_paste_work_on_the_query() {
    let (doc, mut v) = setup();
    type_text(&mut v, &doc, "bass");
    command(&mut v, &doc, "edit.select_all");
    command(&mut v, &doc, "edit.cut");
    assert_eq!(v.browser_query(), "");
    command(&mut v, &doc, "edit.paste");
    assert_eq!(v.browser_query(), "bass");
}

#[test]
fn a_click_in_the_search_field_places_the_caret() {
    let (doc, mut v) = setup();
    type_text(&mut v, &doc, "bass");
    let reg = registry();
    let at = Point::new(14.0, 49.0);
    let _ = v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        &doc,
        &reg,
    );
    type_text(&mut v, &doc, "x");
    // Nothing was painted, so the click maps with the default glyph width: it lands inside
    // the text, not at its end.
    assert!(v.browser_query().contains('x'));
    assert_ne!(
        v.browser_query(),
        "bassx",
        "the caret was placed by the click"
    );
}
