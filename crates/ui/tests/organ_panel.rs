//! The organ's rack panel: the nine drawbars as sliders (layout, colours, the pull-down gesture,
//! reset, the right-click menu), the other controls beside them, the tooltips and a display-list
//! golden. Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test organ_panel`.

mod support;

use atomik_core::Taper;
use atomik_device_sdk::ControlKind;
use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::organ_panel::{self as op, ORGAN_TYPE, organ_layout};
use atomik_ui::{DisplayList, PointerButton, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BARS: [&str; 9] = [
    "drawbar.16",
    "drawbar.5_1_3",
    "drawbar.8",
    "drawbar.4",
    "drawbar.2_2_3",
    "drawbar.2",
    "drawbar.1_3_5",
    "drawbar.1_1_3",
    "drawbar.1",
];

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn organ_doc(params: &[(&str, f32)]) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("organ", ORGAN_TYPE, "Organ");
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc.devices = vec![d];
    doc.cables.clear();
    doc
}

fn setup(params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let doc = organ_doc(params);
    let v = view(&doc, &registry());
    (doc, v)
}

fn json_of(v: &mut RackView, doc: &RackDoc) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

fn spec() -> atomik_ui::autolayout::PanelSpec {
    organ_layout(registry().get(ORGAN_TYPE).unwrap().descriptor)
}

fn set_params(actions: &[UiAction]) -> Vec<(String, f32)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { param, value, .. } => Some((param, value)),
            _ => None,
        })
        .collect()
}

fn pointer(v: &mut RackView, doc: &RackDoc, ev: UiEvent) -> Vec<UiAction> {
    v.handle(&ev, doc, &registry())
}

#[test]
fn the_nine_drawbars_are_faders_in_footage_order_and_nothing_overlaps() {
    let reg = registry();
    let desc = reg.get(ORGAN_TYPE).unwrap().descriptor;
    let s = spec();
    let bars: Vec<_> = s
        .front
        .iter()
        .filter(|pc| op::is_drawbar_key(desc.params[pc.param].key))
        .collect();
    assert_eq!(bars.len(), 9);
    for (n, pc) in bars.iter().enumerate() {
        assert_eq!(desc.params[pc.param].key, BARS[n]);
        assert_eq!(pc.kind, ControlKind::Fader);
        assert_eq!(op::drawbar_index(BARS[n]), n);
    }
    // Every param has exactly one control, and no two cells overlap.
    assert_eq!(s.front.len(), desc.params.len());
    for (a, pa) in s.front.iter().enumerate() {
        for pb in &s.front[a + 1..] {
            assert!(
                pa.rect.intersect(pb.rect).area() <= 0.0,
                "{:?} overlaps {:?}",
                pa.rect,
                pb.rect
            );
        }
        let panel = Rect::new(0.0, 0.0, 760.0, s.height());
        assert!(panel.contains_rect(pa.rect), "{:?}", pa.rect);
    }
    // A track long enough to show nine positions.
    let (_, top, bottom) = op::drawbar_travel(bars[0].rect);
    assert!(bottom - top >= 60.0, "{}", bottom - top);
}

#[test]
fn the_drawbar_colours_follow_the_footages() {
    let brown = op::bar_color(0);
    assert_eq!(op::bar_color(1), brown);
    let white = op::bar_color(2);
    for n in [3, 5, 8] {
        assert_eq!(op::bar_color(n), white);
    }
    let black = op::bar_color(4);
    for n in [6, 7] {
        assert_eq!(op::bar_color(n), black);
    }
    assert!(brown != white && white != black && brown != black);
}

#[test]
fn the_other_controls_keep_their_widgets() {
    let reg = registry();
    let desc = reg.get(ORGAN_TYPE).unwrap().descriptor;
    for pc in spec().front {
        let p = &desc.params[pc.param];
        if !op::is_drawbar_key(p.key) {
            assert_ne!(pc.kind, ControlKind::Fader, "{}", p.key);
            assert!(!matches!(p.taper, Taper::Decibel) || p.key != "drawbar");
        }
    }
}

#[test]
fn pulling_a_drawbar_down_raises_it_and_the_handle_follows_the_pointer() {
    let (doc, mut v) = setup(&[]);
    let start = v.control_center(&key("organ"), "drawbar.4").unwrap();
    let reg = registry();
    let desc = reg.get(ORGAN_TYPE).unwrap().descriptor;
    let rect = spec()
        .front
        .iter()
        .find(|pc| desc.params[pc.param].key == "drawbar.4")
        .unwrap()
        .rect;
    let (_, top, bottom) = op::drawbar_travel(rect);
    let track = bottom - top;
    let _ = pointer(&mut v, &doc, down(start));
    // Down half the track from position 0: position 4.
    let a = pointer(&mut v, &doc, mv(Point::new(start.x, start.y + track / 2.0)));
    let moved = set_params(&a);
    assert_eq!(moved.len(), 1, "{a:?}");
    assert_eq!(moved[0].0, "drawbar.4");
    assert!((moved[0].1 - 4.0).abs() < 0.01, "{}", moved[0].1);
    // Back up the whole way: pushed in, 0.
    let a = pointer(&mut v, &doc, mv(Point::new(start.x, start.y - track)));
    assert_eq!(set_params(&a), vec![("drawbar.4".to_string(), 0.0)]);
    let _ = pointer(&mut v, &doc, up(start));
}

#[test]
fn a_drawbar_drag_stops_at_the_ends_and_steps_whole_positions() {
    let (doc, mut v) = setup(&[("drawbar.16", 8.0)]);
    let start = v.control_center(&key("organ"), "drawbar.16").unwrap();
    let _ = pointer(&mut v, &doc, down(start));
    let a = pointer(&mut v, &doc, mv(Point::new(start.x, start.y + 500.0)));
    assert!(set_params(&a).is_empty(), "already at 8: {a:?}");
    let a = pointer(&mut v, &doc, mv(Point::new(start.x, start.y - 20.0)));
    let moved = set_params(&a);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].1.fract(), 0.0, "{}", moved[0].1);
    assert!(moved[0].1 < 8.0);
    let _ = pointer(&mut v, &doc, up(start));
}

#[test]
fn a_double_click_resets_a_drawbar_and_a_right_click_opens_its_menu() {
    let (doc, mut v) = setup(&[("drawbar.8", 3.0), ("drawbar.4", 6.0)]);
    let at = v.control_center(&key("organ"), "drawbar.4").unwrap();
    let a = pointer(&mut v, &doc, down_with(at, PointerButton::Primary, 2));
    assert_eq!(set_params(&a), vec![("drawbar.4".to_string(), 0.0)]);
    let _ = pointer(&mut v, &doc, up(at));
    let a = pointer(&mut v, &doc, down_with(at, PointerButton::Secondary, 1));
    assert!(
        a.iter().any(|x| matches!(
            x,
            UiAction::ControlMenu { param, .. } if param == "drawbar.4"
        )),
        "{a:?}"
    );
}

#[test]
fn the_new_controls_have_labels_and_tooltips() {
    for k in ["tone.leak", "tone.fold", "wheel.target"] {
        assert!(op::organ_label(k).is_some(), "{k}");
        assert!(op::organ_help(k).is_some(), "{k}");
    }
    for k in BARS {
        assert!(op::organ_help(k).is_some(), "{k}");
    }
    assert!(op::organ_help("nonsense").is_none());
}

#[test]
fn the_panel_shows_the_position_numbers_and_footage_names() {
    let (doc, mut v) = setup(&[("drawbar.4", 5.0)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let texts = dl.texts();
    for name in [
        "16'", "5 1/3'", "8'", "4'", "2 2/3'", "2'", "1 3/5'", "1 1/3'", "1'",
    ] {
        assert!(texts.contains(&name), "{name} in {texts:?}");
    }
    assert!(texts.contains(&"5"), "the position of the 4' drawbar");
    assert!(texts.contains(&"DRAWBARS"));
}

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup(&[
        ("drawbar.4", 5.0),
        ("drawbar.2_2_3", 3.0),
        ("tone.leak", 0.4),
    ]);
    check("organ_panel_front_1x.json", &json_of(&mut v, &doc));
}
