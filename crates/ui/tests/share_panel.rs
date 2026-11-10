//! The share panel: it shows the texts and choices it was given, typing edits the focused text up
//! to its limit, Tab moves between the texts, the check boxes toggle and say so, SHARE and Enter
//! emit the share action, CLOSE closes, the size line follows what the app tells it, and the text
//! and geometry match the golden.

mod support;

use atomik_model::{MAX_INFO_NOTES_CHARS, MAX_INFO_TITLE_CHARS};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, Modifiers, ShareAction, ShareBox, ShareField, ShareValues, ShareView, Theme,
    UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn view(values: ShareValues) -> ShareView {
    let mut v = ShareView::new(Theme::dark(), values);
    v.set_bounds(BOUNDS);
    v
}

fn actions(out: Vec<UiAction>) -> Vec<ShareAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Share(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn press(v: &mut ShareView, key: Key) -> Vec<ShareAction> {
    actions(v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }))
}

fn type_text(v: &mut ShareView, text: &str) {
    for c in text.chars() {
        press(v, Key::Char(c));
    }
}

fn click(v: &mut ShareView, p: Point) -> Vec<ShareAction> {
    let mut out = actions(v.handle(&down(p)));
    out.extend(actions(v.handle(&up(p))));
    out
}

fn texts(v: &ShareView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn filled() -> ShareValues {
    ShareValues {
        title: "Example Song".into(),
        author: "Example Author".into(),
        note: "one\ntwo".into(),
        preview: true,
        stems: false,
        plugins: true,
    }
}

#[test]
fn the_panel_shows_its_labels_and_what_it_was_given() {
    let mut v = view(filled());
    v.set_size("This share will be about 12.3 MB".into(), false);
    let t = texts(&v);
    for want in [
        "SHARE PROJECT",
        "Title",
        "Author",
        "Note",
        "Include a preview",
        "Include stems",
        "Include plugin list",
        "SHARE",
        "CLOSE",
        "Example Song",
        "Example Author",
        "one",
        "two",
        "This share will be about 12.3 MB",
    ] {
        assert!(t.iter().any(|x| x.contains(want)), "{want} in {t:?}");
    }
    assert_eq!(v.values(), &filled());
    assert_eq!(v.focus(), ShareField::Title);
    let fresh = view(ShareValues::default());
    assert!(fresh.values().preview && !fresh.values().stems && fresh.values().plugins);
}

#[test]
fn typing_edits_the_focused_text_and_tab_moves_between_the_texts() {
    let mut v = view(ShareValues::default());
    type_text(&mut v, "My song");
    press(&mut v, Key::Backspace);
    assert_eq!(v.values().title, "My son");
    press(&mut v, Key::Tab);
    assert_eq!(v.focus(), ShareField::Author);
    type_text(&mut v, "Me");
    press(&mut v, Key::Tab);
    assert_eq!(v.focus(), ShareField::Note);
    assert!(
        press(&mut v, Key::Enter).is_empty(),
        "a new line in the note"
    );
    type_text(&mut v, "hi");
    assert_eq!(v.values().note, "\nhi");
    press(&mut v, Key::Tab);
    assert_eq!(v.focus(), ShareField::Title, "wraps");
    let mut seen = vec![v.focus()];
    for _ in 0..2 {
        press(&mut v, Key::Tab);
        seen.push(v.focus());
    }
    assert_eq!(
        seen,
        [ShareField::Title, ShareField::Author, ShareField::Note]
    );
}

#[test]
fn a_text_stops_at_its_limit() {
    let mut v = view(ShareValues::default());
    type_text(&mut v, &"x".repeat(MAX_INFO_TITLE_CHARS + 10));
    assert_eq!(v.values().title.chars().count(), MAX_INFO_TITLE_CHARS);
    assert_eq!(ShareField::Note.limit(), MAX_INFO_NOTES_CHARS);
    press(&mut v, Key::Char('\u{7}'));
    assert_eq!(v.values().title.chars().count(), MAX_INFO_TITLE_CHARS);
}

#[test]
fn the_check_boxes_toggle_and_ask_for_the_size_again() {
    let mut v = view(ShareValues::default());
    for (which, was) in [
        (ShareBox::Preview, true),
        (ShareBox::Stems, false),
        (ShareBox::Plugins, true),
    ] {
        let at = v.box_row(which).center();
        assert_eq!(click(&mut v, at), [ShareAction::Changed], "{which:?}");
        let now = match which {
            ShareBox::Preview => v.values().preview,
            ShareBox::Stems => v.values().stems,
            ShareBox::Plugins => v.values().plugins,
        };
        assert_eq!(now, !was, "{which:?}");
    }
    // The texts and the boxes do not overlap.
    for pair in ShareBox::ALL.windows(2) {
        assert!(v.box_row(pair[0]).y1 <= v.box_row(pair[1]).y0);
    }
    assert!(v.field_rect(ShareField::Note).y1 <= v.box_row(ShareBox::Preview).y0);
    assert!(v.box_row(ShareBox::Plugins).y1 <= v.share_rect().y0);
}

#[test]
fn share_enter_and_close_emit_their_actions() {
    let mut v = view(filled());
    let share = v.share_rect().center();
    assert_eq!(click(&mut v, share), [ShareAction::Share]);
    assert_eq!(press(&mut v, Key::Enter), [ShareAction::Share]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), [ShareAction::Close]);
}

#[test]
fn the_size_line_and_the_preview_hint_follow_the_app() {
    let json = |v: &ShareView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl.to_json()
    };
    let mut v = view(filled());
    assert!(v.needs_repaint());
    v.set_size("This share will be about 820.0 MB".into(), false);
    assert_eq!(v.size_text(), "This share will be about 820.0 MB");
    let plain = json(&v);
    assert!(plain.contains("This share will be about 820.0 MB"));
    // The same line over the limit is drawn in another colour (the theme's warning).
    v.set_size("This share will be about 820.0 MB".into(), true);
    let warned = json(&v);
    assert_ne!(plain, warned, "a warning looks different");
    assert_eq!(
        plain.lines().count(),
        warned.lines().count(),
        "only the colour changes"
    );
    v.set_preview_hint("WAV of the whole song (FFmpeg is not set up)".into());
    assert!(texts(&v).iter().any(|t| t.contains("FFmpeg is not set up")));
}

#[test]
fn tooltips_name_what_the_controls_do_and_stay_short() {
    let v = view(filled());
    for p in [
        v.close_rect().center(),
        v.share_rect().center(),
        v.box_row(ShareBox::Preview).center(),
        v.box_row(ShareBox::Stems).center(),
        v.box_row(ShareBox::Plugins).center(),
    ] {
        let help = v.help_at(p).expect("a tooltip");
        assert!(help.text.chars().count() <= 90, "{}", help.text);
    }
    assert!(
        v.help_at(v.field_rect(ShareField::Title).center())
            .is_none()
    );
}

#[test]
fn the_panel_text_and_geometry_match_the_golden() {
    let mut out = String::new();
    for (name, values, size, warn) in [
        (
            "default",
            ShareValues::default(),
            "This share will be about 3.2 MB",
            false,
        ),
        (
            "filled",
            filled(),
            "This share will be about 820.0 MB",
            true,
        ),
    ] {
        let mut v = view(values);
        v.set_size(size.into(), warn);
        out.push_str(&format!("== {name}\n"));
        let p = v.panel();
        out.push_str(&format!(
            "panel {:.1} {:.1} {:.1} {:.1}\n",
            p.x0, p.y0, p.x1, p.y1
        ));
        out.push_str(&texts(&v).join("\n"));
        out.push('\n');
        for field in ShareField::ALL {
            let r = v.field_rect(field);
            out.push_str(&format!(
                "{field:?}: {:.1} {:.1} {:.1} {:.1}\n",
                r.x0, r.y0, r.x1, r.y1
            ));
        }
        for which in ShareBox::ALL {
            let r = v.box_row(which);
            out.push_str(&format!(
                "{which:?}: {:.1} {:.1} {:.1} {:.1}\n",
                r.x0, r.y0, r.x1, r.y1
            ));
        }
        for (label, r) in [("SHARE", v.share_rect()), ("CLOSE", v.close_rect())] {
            out.push_str(&format!(
                "{label}: {:.1} {:.1} {:.1} {:.1}\n",
                r.x0, r.y0, r.x1, r.y1
            ));
        }
    }
    starter::check_golden("share_panel.txt", &out);
}
