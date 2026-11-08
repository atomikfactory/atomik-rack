//! The "Will the bass read?" panel: it lists the master and the tracks, a click chooses a strip,
//! the buttons emit their actions, the meter and the verdict show what was found, a stale reading
//! says so, the list scrolls, and every control has a tooltip.

mod support;

use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    BassLevel, BassReadAction, BassReadData, BassReadView, BassReadingRow, DisplayList, Geom, Key,
    Modifiers, Op, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 960.0, 700.0);

fn reading(share: f32, level: BassLevel, stale: bool) -> BassReadingRow {
    BassReadingRow {
        source: "Bass".into(),
        share,
        verdict: "Yes: the harmonics carry the bass".into(),
        advice: "Nothing to do.".into(),
        detail: "Low notes around 55 Hz, 12 windows".into(),
        level,
        stale,
    }
}

fn data(count: usize) -> BassReadData {
    let mut sources = vec!["Master".to_owned()];
    sources.extend((1..count).map(|n| format!("Track {n}")));
    BassReadData {
        sources,
        chosen: 0,
        running: None,
        reading: None,
    }
}

fn view(count: usize) -> BassReadView {
    let mut v = BassReadView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(data(count));
    v
}

fn click(v: &mut BassReadView, p: Point) -> Vec<BassReadAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::BassRead(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &BassReadView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn key_press(v: &mut BassReadView, key: Key) -> Vec<BassReadAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::BassRead(a) => Some(a),
        _ => None,
    })
    .collect()
}

#[test]
fn the_panel_lists_the_master_and_the_tracks() {
    let t = texts(&view(3));
    assert!(t.iter().any(|s| s == "WILL THE BASS READ?"));
    assert!(t.iter().any(|s| s == "Master"));
    assert!(t.iter().any(|s| s == "Track 2"));
    assert!(t.iter().any(|s| s == "MEASURE"));
    assert!(t.iter().any(|s| s == "Not measured yet"));
}

#[test]
fn the_buttons_and_rows_emit_their_actions() {
    let mut v = view(4);
    let measure = v.measure_rect().center();
    assert_eq!(click(&mut v, measure), vec![BassReadAction::Measure]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![BassReadAction::Close]);
    let row = v.row_rect(2).center();
    assert_eq!(click(&mut v, row), vec![BassReadAction::Choose(2)]);
    // The arrows move the choice, Enter measures.
    let mut chosen = data(4);
    chosen.chosen = 1;
    v.set_data(chosen);
    assert_eq!(
        key_press(&mut v, Key::Down),
        vec![BassReadAction::Choose(2)]
    );
    assert_eq!(key_press(&mut v, Key::Up), vec![BassReadAction::Choose(0)]);
    assert_eq!(key_press(&mut v, Key::Enter), vec![BassReadAction::Measure]);
}

#[test]
fn the_choice_cannot_leave_the_list() {
    let mut v = view(3);
    assert!(
        key_press(&mut v, Key::Up).is_empty(),
        "the first strip is chosen"
    );
    let mut last = data(3);
    last.chosen = 2;
    v.set_data(last);
    assert!(key_press(&mut v, Key::Down).is_empty());
}

#[test]
fn while_it_measures_the_button_stops_and_the_meter_shows_the_progress() {
    let mut v = view(3);
    let mut running = data(3);
    running.running = Some(0.4);
    v.set_data(running);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "STOP"), "{t:?}");
    assert!(t.iter().any(|s| s == "40 %"), "{t:?}");
    let button = v.measure_rect().center();
    assert_eq!(click(&mut v, button), vec![BassReadAction::Stop]);
    // Enter does not start a second one.
    assert!(key_press(&mut v, Key::Enter).is_empty());
    let help = v.help_at(button).expect("a tooltip");
    assert!(help.text.contains("Stop"));
}

#[test]
fn a_reading_shows_its_share_verdict_advice_and_numbers() {
    let mut v = view(3);
    let mut d = data(3);
    d.reading = Some(reading(0.16, BassLevel::Reads, false));
    v.set_data(d);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "16 %"), "{t:?}");
    assert!(
        t.iter()
            .any(|s| s == "Bass: Yes: the harmonics carry the bass"),
        "{t:?}"
    );
    assert!(t.iter().any(|s| s == "Nothing to do."));
    assert!(t.iter().any(|s| s.starts_with("Low notes around 55 Hz")));
    assert!(t.iter().any(|s| s == "thin") && t.iter().any(|s| s == "reads"));
}

#[test]
fn the_meter_is_longer_for_a_larger_share_and_a_stale_reading_says_so() {
    let mut v = view(3);
    let mut d = data(3);
    d.reading = Some(reading(0.02, BassLevel::Partly, false));
    v.set_data(d.clone());
    let short = filled_width(&v);
    d.reading = Some(reading(0.2, BassLevel::Reads, false));
    v.set_data(d.clone());
    let long = filled_width(&v);
    assert!(long > 3.0 * short, "{short} {long}");
    d.reading = Some(reading(0.2, BassLevel::Reads, true));
    v.set_data(d);
    assert!(texts(&v).iter().any(|s| s.starts_with("The song changed")));
}

/// The widest filled rectangle inside the meter.
fn filled_width(v: &BassReadView) -> f64 {
    let m = v.meter_rect();
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.ops()
        .iter()
        .filter_map(|op| match op {
            Op::Fill {
                shape: Geom::Rect(r),
                ..
            } if r.y0 >= m.y0 - 0.5 && r.y1 <= m.y1 + 0.5 && r.x0 >= m.x0 - 0.5 => Some(r.width()),
            _ => None,
        })
        .fold(0.0, f64::max)
        .min(m.width())
}

#[test]
fn a_long_list_scrolls_with_the_wheel() {
    let mut v = view(12);
    assert_eq!(v.top(), 0);
    let inside = v.list_rect().center();
    v.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -1.0),
        pos: inside,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.top(), 1);
    let slot = v.row_rect(0).center();
    assert_eq!(click(&mut v, slot), vec![BassReadAction::Choose(1)]);
    assert!(
        texts(&v).iter().any(|s| s == "2-8 of 12"),
        "{:?}",
        texts(&v)
    );
    // A chosen strip below the window brings itself into view.
    let mut d = data(12);
    d.chosen = 11;
    v.set_data(d);
    assert_eq!(v.top(), 5);
}

#[test]
fn every_control_has_a_short_tooltip() {
    let v = view(3);
    for p in [
        v.close_rect().center(),
        v.measure_rect().center(),
        v.row_rect(0).center(),
        v.meter_rect().center(),
    ] {
        let help = v.help_at(p).expect("a tooltip");
        assert!(help.text.len() <= 80, "{}", help.text);
    }
}

#[test]
fn the_panel_stays_inside_a_small_window() {
    let mut v = view(8);
    v.set_bounds(Rect::new(0.0, 0.0, 600.0, 520.0));
    let p = v.panel();
    assert!(p.x0 >= 0.0 && p.x1 <= 600.0 && p.y0 >= 0.0 && p.y1 <= 520.0);
    assert!(v.list_rect().x1 <= p.x1);
    assert!(v.meter_rect().y1 <= p.y1);
    assert!(v.measure_rect().x0 >= p.x0);
}
