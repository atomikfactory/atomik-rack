//! The reverb send wizard panel: its labels and defaults, the steppers and switches, the decay
//! that starts from the suggestion, ADD only with a track, CLOSE, the wheel and a drag.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, Modifiers, ReverbSendAction, ReverbSendRow, ReverbSendValues, ReverbSendView,
    Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 700.0);

fn view() -> ReverbSendView {
    let mut v = ReverbSendView::new(Theme::dark(), ReverbSendValues::default());
    v.set_bounds(BOUNDS);
    v.set_target("Lead".into(), 1.0);
    v
}

fn actions(out: Vec<UiAction>) -> Vec<ReverbSendAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::ReverbSend(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn click(v: &mut ReverbSendView, p: Point) -> Vec<ReverbSendAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    actions(out)
}

fn click_at(
    v: &mut ReverbSendView,
    rect: impl Fn(&ReverbSendView) -> Rect,
) -> Vec<ReverbSendAction> {
    let p = centre(rect(v));
    click(v, p)
}

fn centre(r: Rect) -> Point {
    Point::new((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
}

fn texts(v: &ReverbSendView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn step(v: &mut ReverbSendView, row: ReverbSendRow, plus: bool) {
    let r = if plus {
        v.plus_rect(row)
    } else {
        v.minus_rect(row)
    };
    let p = centre(r);
    assert!(click(v, p).is_empty());
}

#[test]
fn the_panel_shows_its_labels_the_track_and_the_sensible_defaults() {
    let v = view();
    let t = texts(&v);
    for want in [
        "ADD REVERB SEND",
        "Track: Lead",
        "De-esser (vocal)",
        "Duck from track",
        "Send",
        "Decay",
        "High-pass",
        "Low-pass",
        "Send level",
        "ADD",
        "CLOSE",
        "auto: 1.0 s",
        "300 Hz",
        "8000 Hz",
        "-12 dB",
        "post-fader",
    ] {
        assert!(t.iter().any(|x| x.contains(want)), "{want} in {t:?}");
    }
    let d = ReverbSendValues::default();
    assert_eq!(v.values(), &d);
    assert!(!d.vocal && d.duck && !d.pre_fader && d.decay.is_none());
}

#[test]
fn the_switches_and_the_lists_step_and_stay_in_range() {
    let mut v = view();
    step(&mut v, ReverbSendRow::Vocal, true);
    step(&mut v, ReverbSendRow::Duck, false);
    step(&mut v, ReverbSendRow::Position, true);
    assert_eq!(
        (v.values().vocal, v.values().duck, v.values().pre_fader),
        (true, false, true)
    );
    // A click on the value of a switch flips it.
    assert!(click_at(&mut v, |v| v.value_rect(ReverbSendRow::Vocal)).is_empty());
    assert!(!v.values().vocal);

    step(&mut v, ReverbSendRow::HighPass, true);
    assert_eq!(v.values().high_pass, 350);
    step(&mut v, ReverbSendRow::LowPass, false);
    assert_eq!(v.values().low_pass, 7_000);
    for _ in 0..40 {
        step(&mut v, ReverbSendRow::Level, true);
    }
    assert_eq!(v.values().send_db, 0);
    for _ in 0..40 {
        step(&mut v, ReverbSendRow::Level, false);
    }
    assert_eq!(v.values().send_db, -30);
    for _ in 0..30 {
        step(&mut v, ReverbSendRow::HighPass, false);
    }
    assert_eq!(v.values().high_pass, 100);
}

#[test]
fn the_decay_starts_from_the_suggestion_and_the_lowest_step_goes_back_to_auto() {
    let mut v = view();
    step(&mut v, ReverbSendRow::Decay, true);
    assert_eq!(
        v.values().decay,
        Some(1.0),
        "the entry nearest to the suggestion"
    );
    step(&mut v, ReverbSendRow::Decay, true);
    assert_eq!(v.values().decay, Some(1.2));
    for _ in 0..10 {
        step(&mut v, ReverbSendRow::Decay, false);
    }
    assert_eq!(v.values().decay, None, "back to auto");
    assert!(texts(&v).iter().any(|t| t.contains("auto: 1.0 s")));

    // A different suggestion shows in the auto text and is where the first step lands.
    v.set_target("Lead".into(), 2.4);
    assert!(texts(&v).iter().any(|t| t.contains("auto: 2.4 s")));
    step(&mut v, ReverbSendRow::Decay, true);
    assert_eq!(v.values().decay, Some(2.5));
}

#[test]
fn add_needs_a_track_and_close_always_works() {
    let mut v = view();
    assert_eq!(click_at(&mut v, |v| v.add_rect()), [ReverbSendAction::Add]);
    assert_eq!(
        click_at(&mut v, |v| v.close_rect()),
        [ReverbSendAction::Close]
    );
    v.set_target(String::new(), 2.0);
    assert!(click_at(&mut v, |v| v.add_rect()).is_empty());
    assert!(texts(&v).iter().any(|t| t.contains("no track selected")));
    assert_eq!(
        click_at(&mut v, |v| v.close_rect()),
        [ReverbSendAction::Close]
    );
}

#[test]
fn the_wheel_and_a_drag_step_a_value() {
    let mut v = view();
    let at = centre(v.value_rect(ReverbSendRow::LowPass));
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, 1.0),
        pos: at,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.values().low_pass, 9_000);
    // Dragging up steps up, one step for every few pixels.
    let start = centre(v.value_rect(ReverbSendRow::HighPass));
    v.handle(&down(start));
    assert!(v.is_dragging());
    v.handle(&mv(Point::new(start.x, start.y - 60.0)));
    v.handle(&up(Point::new(start.x, start.y - 60.0)));
    assert!(!v.is_dragging());
    assert!(v.values().high_pass > 300);
}

#[test]
fn every_button_and_value_has_a_tooltip() {
    let v = view();
    for row in ReverbSendRow::ALL {
        for r in [v.minus_rect(row), v.value_rect(row), v.plus_rect(row)] {
            assert!(v.help_at(centre(r)).is_some(), "{row:?}");
        }
    }
    assert!(v.help_at(centre(v.add_rect())).is_some());
    assert!(v.help_at(centre(v.close_rect())).is_some());
    assert!(v.help_at(Point::new(1.0, 1.0)).is_none());
}

#[test]
fn the_panel_fits_a_small_window_and_paints_inside_its_bounds() {
    let mut v = view();
    v.set_bounds(Rect::new(0.0, 0.0, 400.0, 420.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 400.0 && p.y1 <= 420.0, "{p:?}");
    for row in ReverbSendRow::ALL {
        let plus = v.plus_rect(row);
        assert!(plus.x1 <= p.x1, "{row:?} {plus:?} {p:?}");
    }
}
