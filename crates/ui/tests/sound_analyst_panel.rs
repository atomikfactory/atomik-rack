//! The Sound Analyst panel: it shows the three curves, the numbers and the plan of the proposed
//! patch, the buttons emit their actions only when they can act, the match shows a distance and a
//! score per curve, a stale match says so, and every control has a short tooltip.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    Curve, DisplayList, Distance, Key, MatchRow, Modifiers, Op, SoundAnalystAction,
    SoundAnalystData, SoundAnalystView, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 960.0, 700.0);

fn curve(values: &[f32], made: &[f32]) -> Curve {
    Curve {
        target: values.to_vec(),
        target_hop_s: 0.005,
        made: made.to_vec(),
        made_hop_s: 0.005,
    }
}

fn ramp(n: usize, from: f32, to: f32) -> Vec<f32> {
    (0..n)
        .map(|i| from + (to - from) * i as f32 / (n - 1) as f32)
        .collect()
}

fn distance(value: f32, unit: &'static str, score: f32) -> Distance {
    Distance { value, unit, score }
}

fn analysed(pitched: bool) -> SoundAnalystData {
    SoundAnalystData {
        file: Some("pluck.wav, 1.2 s".into()),
        running: None,
        numbers: vec![
            "Envelope: attack 2 ms, then it dies away in 300 ms".into(),
            "Brightness: 1560 Hz at the start, 290 Hz at the end".into(),
        ],
        plan: vec![
            "Source: a saw on the poly synth (all harmonics)".into(),
            "Filter: 330 Hz".into(),
        ],
        level: curve(&ramp(100, 0.0, -40.0), &[]),
        brightness: curve(&ramp(100, 1500.0, 300.0), &[]),
        pitch: pitched.then(|| curve(&vec![220.0; 100], &[])),
        matched: None,
        can_make: true,
        can_match: false,
        stale: false,
    }
}

fn view(data: SoundAnalystData) -> SoundAnalystView {
    let mut v = SoundAnalystView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_data(data);
    v
}

fn click(v: &mut SoundAnalystView, p: Point) -> Vec<SoundAnalystAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::SoundAnalyst(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn key_press(v: &mut SoundAnalystView, key: Key) -> Vec<SoundAnalystAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::SoundAnalyst(a) => Some(a),
        _ => None,
    })
    .collect()
}

fn texts(v: &SoundAnalystView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

/// How many line strokes the panel draws inside the curve boxes.
fn curve_strokes(v: &SoundAnalystView) -> usize {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let boxes: Vec<Rect> = (0..3).map(|i| v.curve_rect(i)).collect();
    dl.ops()
        .iter()
        .filter(|op| match op {
            Op::Stroke {
                shape: atomik_ui::Geom::Line(l),
                ..
            } => boxes
                .iter()
                .any(|b| l.p0.x > b.x0 + 0.5 && l.p1.x <= b.x1 + 0.5 && l.p0.y >= b.y0 - 0.5),
            _ => false,
        })
        .count()
}

#[test]
fn an_empty_panel_asks_for_a_file() {
    let t = texts(&view(SoundAnalystData::default()));
    assert!(t.iter().any(|s| s == "SOUND ANALYST"), "{t:?}");
    assert!(
        t.iter().any(|s| s.starts_with("Drop an audio file")),
        "{t:?}"
    );
    for name in [
        "LEVEL",
        "BRIGHTNESS",
        "PITCH",
        "OPEN FILE",
        "MAKE PATCH",
        "MATCH",
        "CLOSE",
    ] {
        assert!(t.iter().any(|s| s == name), "{name} in {t:?}");
    }
    assert!(t.iter().any(|s| s == "nothing analysed yet"));
}

#[test]
fn an_analysed_sound_shows_its_name_numbers_plan_and_curves() {
    let v = view(analysed(true));
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "pluck.wav, 1.2 s"), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("Envelope: attack 2 ms")));
    assert!(t.iter().any(|s| s.starts_with("Source: a saw")));
    assert!(curve_strokes(&v) > 100, "{}", curve_strokes(&v));
}

#[test]
fn an_unpitched_sound_has_no_pitch_curve() {
    let t = texts(&view(analysed(false)));
    assert!(t.iter().any(|s| s == "unpitched"), "{t:?}");
}

#[test]
fn the_buttons_emit_their_actions_only_when_they_can_act() {
    let mut v = view(analysed(true));
    let open = v.open_rect().center();
    assert_eq!(click(&mut v, open), vec![SoundAnalystAction::OpenFile]);
    let close = v.close_rect().center();
    assert_eq!(click(&mut v, close), vec![SoundAnalystAction::Close]);
    let make = v.make_rect().center();
    assert_eq!(click(&mut v, make), vec![SoundAnalystAction::MakePatch]);
    // Nothing was made yet, so there is nothing to match.
    let matched = v.match_rect().center();
    assert!(click(&mut v, matched).is_empty());
    let mut made = analysed(true);
    made.can_match = true;
    v.set_data(made);
    assert_eq!(click(&mut v, matched), vec![SoundAnalystAction::Match]);
    // With nothing proposed, Make Patch does nothing and Enter neither.
    v.set_data(SoundAnalystData::default());
    assert!(click(&mut v, make).is_empty());
    assert!(key_press(&mut v, Key::Enter).is_empty());
}

#[test]
fn enter_makes_the_patch_and_while_it_runs_the_open_button_stops() {
    let mut v = view(analysed(true));
    assert_eq!(
        key_press(&mut v, Key::Enter),
        vec![SoundAnalystAction::MakePatch]
    );
    let mut running = analysed(true);
    running.running = Some(0.5);
    v.set_data(running);
    assert!(texts(&v).iter().any(|s| s == "STOP"));
    let button = v.open_rect().center();
    assert_eq!(click(&mut v, button), vec![SoundAnalystAction::Stop]);
    assert!(
        key_press(&mut v, Key::Enter).is_empty(),
        "no patch while it runs"
    );
    assert!(v.help_at(button).unwrap().text.contains("Stop"));
}

#[test]
fn a_match_shows_a_distance_and_a_score_per_curve_and_a_second_line() {
    let mut d = analysed(true);
    d.level.made = ramp(100, 0.0, -30.0);
    d.brightness.made = ramp(100, 1200.0, 350.0);
    d.pitch = Some(curve(&vec![220.0; 100], &vec![222.0; 100]));
    d.matched = Some(MatchRow {
        of: "Proposed patch".into(),
        level: distance(3.1, "dB", 0.66),
        brightness: distance(0.4, "oct", 0.65),
        pitch: Some(distance(0.1, "st", 0.83)),
        overall: 0.71,
    });
    let without = curve_strokes(&view(analysed(true)));
    let v = view(d.clone());
    let t = texts(&v);
    for s in ["3.10 dB", "66 %", "0.40 oct", "65 %", "0.10 st", "83 %"] {
        assert!(t.iter().any(|x| x == s), "{s} in {t:?}");
    }
    assert!(t.iter().any(|s| s == "Proposed patch: 71 % match"), "{t:?}");
    assert!(
        curve_strokes(&v) > without + 100,
        "the patch's curves are drawn too"
    );
    // A match of a patch that has changed since says so.
    d.stale = true;
    let t = texts(&view(d));
    assert!(
        t.iter().any(|s| s.starts_with("The patch changed")),
        "{t:?}"
    );
}

#[test]
fn an_unpitched_match_has_no_pitch_score() {
    let mut d = analysed(false);
    d.matched = Some(MatchRow {
        of: "Proposed patch".into(),
        level: distance(2.0, "dB", 0.75),
        brightness: distance(0.1, "oct", 0.88),
        pitch: None,
        overall: 0.8,
    });
    let t = texts(&view(d));
    assert!(t.iter().any(|s| s == "75 %"));
    assert!(!t.iter().any(|s| s.ends_with(" st")), "{t:?}");
}

#[test]
fn a_curve_that_is_longer_than_the_other_shares_the_time_axis() {
    // The same target with a render that lasts twice as long: the target ends halfway.
    let mut d = analysed(true);
    d.level.made = ramp(200, 0.0, -40.0);
    let v = view(d);
    let r = v.curve_rect(0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let widest = dl
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::Stroke {
                shape: atomik_ui::Geom::Line(l),
                ..
            } if l.p0.y >= r.y0 - 0.5 && l.p1.y <= r.y1 + 0.5 => Some(l.p1.x),
            _ => None,
        })
        .fold(0.0f64, f64::max);
    assert!(
        widest > r.x0 + 0.9 * r.width(),
        "the render reaches the right edge"
    );
}

#[test]
fn every_control_has_a_short_tooltip() {
    let v = view(analysed(true));
    let mut points = vec![
        v.close_rect().center(),
        v.match_rect().center(),
        v.make_rect().center(),
        v.open_rect().center(),
    ];
    points.extend((0..3).map(|i| v.curve_rect(i).center()));
    for p in points {
        let help = v.help_at(p).expect("a tooltip");
        assert!(help.text.len() <= 90, "{}", help.text);
    }
}

#[test]
fn the_panel_and_its_boxes_stay_inside_the_window() {
    let mut v = view(analysed(true));
    for (w, h) in [(960.0, 700.0), (800.0, 640.0)] {
        v.set_bounds(Rect::new(0.0, 0.0, w, h));
        let p = v.panel();
        assert!(
            p.x0 >= 0.0 && p.x1 <= w && p.y0 >= 0.0 && p.y1 <= h,
            "{p:?}"
        );
        for i in 0..3 {
            let r = v.curve_rect(i);
            assert!(r.y1 <= p.y1 && r.x0 >= p.x0 && r.x1 <= p.x1, "{r:?}");
        }
        assert!(v.open_rect().x0 >= p.x0, "the buttons fit the header");
    }
}
