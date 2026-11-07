//! The mix check panel: it lists the findings with their track, number and reason, a row click
//! chooses the finding (and a finding with a time locates it), the fix and tool buttons emit their
//! actions, a fixed finding cannot be fixed twice, the list scrolls, and every control has a tooltip.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, Key, MixCheckAction, MixCheckData, MixCheckRow, MixCheckView, Modifiers, Theme,
    UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn row(
    tag: &str,
    track: &str,
    fix: Option<&str>,
    tool: Option<&str>,
    at: Option<f64>,
) -> MixCheckRow {
    MixCheckRow {
        tag: tag.into(),
        track: track.into(),
        measured: format!("{tag} number for {track}"),
        why: format!("{tag} reason"),
        at,
        fix: fix.map(Into::into),
        tool: tool.map(Into::into),
        done: false,
    }
}

fn data() -> MixCheckData {
    MixCheckData {
        strips: 6,
        rows: vec![
            row("BASS ROOT", "Bass", None, None, None),
            row(
                "LOW MIDS",
                "Keys 41%, Pad 22%",
                Some("CUT 250 Hz"),
                Some("MASKING MAP"),
                None,
            ),
            row("PEAKS", "Kick, Vox", None, None, Some(83.4)),
        ],
    }
}

fn view() -> MixCheckView {
    let mut v = MixCheckView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(Some(data()));
    v
}

fn click(v: &mut MixCheckView, p: Point) -> Vec<MixCheckAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::MixCheck(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &MixCheckView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_panel_lists_the_findings_with_track_number_and_reason() {
    let t = texts(&view());
    assert!(t.iter().any(|s| s == "MIX CHECK"));
    assert!(
        t.iter()
            .any(|s| s.contains("BASS ROOT") && s.contains("Bass")),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "BASS ROOT number for Bass"));
    assert!(t.iter().any(|s| s == "LOW MIDS reason"));
    assert!(t.iter().any(|s| s == "CUT 250 Hz"));
    assert!(t.iter().any(|s| s == "MASKING MAP"));
    // A finding with a time shows it.
    assert!(t.iter().any(|s| s.contains("at 1:23.4")), "{t:?}");
}

#[test]
fn an_empty_panel_says_what_to_do_and_a_clean_check_says_so() {
    let mut v = MixCheckView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "No check yet"));
    assert!(t.iter().any(|s| s.starts_with("Check renders")));
    v.set_data(Some(MixCheckData {
        strips: 4,
        rows: Vec::new(),
    }));
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "No findings in 4 strips"), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("Nothing to report")));
}

#[test]
fn the_buttons_emit_their_actions() {
    let mut v = view();
    let run = v.run_rect().center();
    assert_eq!(click(&mut v, run), vec![MixCheckAction::Run]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![MixCheckAction::Close]);
    let fix = v.fix_rect(1).center();
    assert_eq!(click(&mut v, fix), vec![MixCheckAction::Fix(1)]);
    assert_eq!(v.chosen(), 1);
    let tool = v.tool_rect(1).center();
    assert_eq!(click(&mut v, tool), vec![MixCheckAction::Open(1)]);
    // A row with no fix and no tool has no button there; a plain click only chooses it.
    let none = v.fix_rect(0).center();
    assert!(click(&mut v, none).is_empty());
    assert_eq!(v.chosen(), 0);
    // A row with a time locates the playhead.
    let peak = v.row_rect(2).center();
    assert_eq!(click(&mut v, peak), vec![MixCheckAction::Locate(2)]);
}

#[test]
fn a_fix_that_was_applied_says_done_and_does_not_emit_again() {
    let mut v = view();
    let mut d = data();
    d.rows[1].done = true;
    v.set_data(Some(d));
    assert!(texts(&v).iter().any(|s| s == "DONE"));
    let fix = v.fix_rect(1).center();
    assert!(click(&mut v, fix).is_empty());
    assert!(v.help_at(fix).is_none_or(|h| !h.text.contains("Apply")));
}

#[test]
fn while_a_check_runs_the_button_cancels_and_shows_the_progress() {
    let mut v = view();
    v.set_progress(Some(0.42));
    assert!(v.busy());
    assert!(texts(&v).iter().any(|s| s == "CANCEL 42%"));
    let button = v.run_rect().center();
    assert_eq!(click(&mut v, button), vec![MixCheckAction::Cancel]);
    v.set_progress(None);
    assert!(texts(&v).iter().any(|s| s == "CHECK"));
}

#[test]
fn a_long_list_scrolls_with_the_wheel_and_the_arrow_keys() {
    let mut v = MixCheckView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    let rows: Vec<MixCheckRow> = (0..10)
        .map(|n| row("LEVEL", &format!("Track {n}"), Some("TRIM"), None, None))
        .collect();
    v.set_data(Some(MixCheckData { strips: 10, rows }));
    assert_eq!((v.chosen(), v.top()), (0, 0));
    let inside = v.list_rect().center();
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -1.0),
        pos: inside,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.top(), 1);
    // Arrow keys move the choice and keep it in view.
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
    // The wheel does not run past the end.
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -3.0),
        pos: inside,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.top(), 4);
    // A click on a row after scrolling chooses that finding.
    let slot = v.row_rect(0).center();
    click(&mut v, slot);
    assert_eq!(v.chosen(), 4);
}

#[test]
fn every_control_has_a_short_tooltip() {
    let v = view();
    for p in [
        v.close_rect().center(),
        v.run_rect().center(),
        v.fix_rect(1).center(),
        v.tool_rect(1).center(),
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
    assert!(v.tool_rect(5).y1 <= p.y1);
}
