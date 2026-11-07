//! The Sidechain Tool's curve editor: the published meter layout, the curve on the panel, and the
//! gestures (move a point, bend a segment, add and remove points, merge ids, Esc, LEARN).

mod support;

use atomik_dsp::duck_curve::{COUNT_KEY, DEFAULT_POINTS, MAX_POINTS, POINT_KEYS, value_at};
use atomik_model::{Command, Document};
use atomik_ui::fx::{self, FxKind};
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, Flip, Modifiers, PointerButton, RackView, UiAction};
use support::fx::*;
use support::*;

fn rig() -> (Document, RackView) {
    let (doc, mut v) = rack(FxKind::Duck);
    let document = Document::new(doc);
    v.sync(document.doc(), &registry());
    (document, v)
}

fn apply(document: &mut Document, v: &mut RackView, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                document
                    .apply(c.clone(), &registry())
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    v.sync(document.doc(), &registry());
}

fn param(document: &Document, p: &str) -> f32 {
    document
        .doc()
        .param_value(&key("fx"), p, &registry())
        .unwrap()
}

fn drag(document: &mut Document, v: &mut RackView, from: Point, path: &[Point]) -> Vec<UiAction> {
    let reg = registry();
    let mut all = Vec::new();
    let a = v.handle(&down(from), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    for p in path {
        let a = v.handle(&mv(*p), document.doc(), &reg);
        apply(document, v, &a);
        all.extend(a);
    }
    let end = path.last().copied().unwrap_or(from);
    let a = v.handle(&up(end), document.doc(), &reg);
    apply(document, v, &a);
    all.extend(a);
    all
}

fn click(document: &mut Document, v: &mut RackView, at: Point, clicks: u8) -> Vec<UiAction> {
    let reg = registry();
    let a = v.handle(
        &down_with(at, PointerButton::Primary, clicks),
        document.doc(),
        &reg,
    );
    apply(document, v, &a);
    let b = v.handle(&up(at), document.doc(), &reg);
    apply(document, v, &b);
    a.into_iter().chain(b).collect()
}

fn count(document: &Document) -> usize {
    param(document, COUNT_KEY) as usize
}

fn stored(document: &Document) -> Vec<(f32, f32, f32)> {
    (0..count(document))
        .map(|n| {
            (
                param(document, POINT_KEYS[n][0]),
                param(document, POINT_KEYS[n][1]),
                param(document, POINT_KEYS[n][2]),
            )
        })
        .collect()
}

#[test]
fn the_published_layout_is_the_one_the_panel_reads() {
    use atomik_devices::duck as dev;
    assert_eq!(fx::DUCK_METERS, dev::METERS);
    assert_eq!(dev::meter::DIP, 0);
    assert_eq!(dev::meter::POSITION, 1);
    assert_eq!(FxKind::Duck.meter_count(), dev::METERS);
    assert_eq!(FxKind::of("atomik.duck"), Some(FxKind::Duck));
    assert!(FxKind::EFFECTS.contains(&FxKind::Duck));
}

#[test]
fn a_device_without_points_shows_the_built_in_curve_and_a_drawn_one_shows_its_points() {
    let (mut document, mut v) = rig();
    assert_eq!(v.duck_points(&key("fx")), DEFAULT_POINTS.to_vec());
    let before = {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, document.doc(), &registry());
        dl.to_json()
    };
    for (p, val) in [
        (COUNT_KEY, 2.0),
        (POINT_KEYS[0][1], 0.5),
        (POINT_KEYS[1][0], 1.0),
        (POINT_KEYS[1][1], 0.5),
    ] {
        document
            .apply(
                Command::SetParam {
                    key: key("fx"),
                    param: p.into(),
                    value: val,
                    merge: None,
                },
                &registry(),
            )
            .unwrap();
    }
    v.sync(document.doc(), &registry());
    let points = v.duck_points(&key("fx"));
    assert_eq!(points.len(), 2);
    assert_eq!((points[0].y, points[1].x, points[1].y), (0.5, 1.0, 0.5));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, document.doc(), &registry());
    assert_ne!(dl.to_json(), before, "the plot follows the points");
}

#[test]
fn the_first_drag_writes_the_curve_and_the_drag_is_one_undo_step() {
    let (mut document, mut v) = rig();
    let k = key("fx");
    assert_eq!(count(&document), 0);
    let h = v.duck_point_center(&k, 1).unwrap();
    let path = [
        Point::new(h.x + 6.0, h.y - 4.0),
        Point::new(h.x + 14.0, h.y - 9.0),
        Point::new(h.x + 20.0, h.y - 12.0),
    ];
    let a = drag(&mut document, &mut v, h, &path);
    assert_eq!(count(&document), 3, "the curve was written out");
    let pts = stored(&document);
    assert!(pts[1].0 > 0.04, "the point moved right: {pts:?}");
    assert!(pts[1].1 < 1.0, "and up in gain (less dip): {pts:?}");
    // The moves send the same two params with one merge id.
    let moves: Vec<_> = commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            Command::Batch(b) => Some(b),
            _ => None,
        })
        .filter(|b| b.len() == 2)
        .collect();
    assert!(moves.len() >= 3, "{moves:?}");
    let merges: Vec<_> = moves
        .iter()
        .flatten()
        .map(|c| match c {
            Command::SetParam { param, merge, .. } => (param.as_str(), *merge),
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(
        merges
            .iter()
            .all(|(p, m)| { (*p == POINT_KEYS[1][0] || *p == POINT_KEYS[1][1]) && m.is_some() })
    );
    let first = merges[0].1;
    assert!(merges.iter().all(|(_, m)| *m == first), "one merge id");
    // The seed is one step and the drag another: two undos put the default back.
    document.undo().unwrap();
    assert_eq!(count(&document), 3);
    assert!((stored(&document)[1].0 - 0.04).abs() < 1e-6);
    document.undo().unwrap();
    assert_eq!(count(&document), 0);
}

#[test]
fn the_first_and_last_point_move_only_up_and_down() {
    let (mut document, mut v) = rig();
    let k = key("fx");
    let h = v.duck_point_center(&k, 0).unwrap();
    drag(
        &mut document,
        &mut v,
        h,
        &[Point::new(h.x + 80.0, h.y + 20.0)],
    );
    assert_eq!(stored(&document)[0].0, 0.0);
    assert!(stored(&document)[0].1 > 0.0);
    let n = count(&document);
    let h = v.duck_point_center(&k, n - 1).unwrap();
    drag(
        &mut document,
        &mut v,
        h,
        &[Point::new(h.x - 80.0, h.y - 20.0)],
    );
    assert_eq!(stored(&document)[n - 1].0, 1.0);
    // An inner point cannot pass its neighbour.
    let h = v.duck_point_center(&k, 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x + 4000.0, h.y)]);
    let pts = stored(&document);
    assert!(pts[1].0 < pts[2].0, "{pts:?}");
}

#[test]
fn a_click_on_empty_plot_adds_a_point_and_a_double_click_removes_it() {
    let (mut document, mut v) = rig();
    let k = key("fx");
    // Well away from the built-in curve: near the bottom, a third of the way.
    let at = v.duck_curve_at(&k, 0.3, 0.9).unwrap();
    click(&mut document, &mut v, at, 1);
    assert_eq!(count(&document), 4);
    let pts = stored(&document);
    assert!(pts.windows(2).all(|w| w[0].0 < w[1].0), "sorted: {pts:?}");
    assert!(
        (pts[2].0 - 0.3).abs() < 0.02 && (pts[2].1 - 0.9).abs() < 0.05,
        "{pts:?}"
    );
    // A second click on the point itself with two clicks removes it.
    let p = v.duck_point_center(&k, 2).unwrap();
    click(&mut document, &mut v, p, 2);
    assert_eq!(count(&document), 3);
    // The end points cannot be removed.
    let p = v.duck_point_center(&k, 0).unwrap();
    click(&mut document, &mut v, p, 2);
    assert_eq!(count(&document), 3);
    // At most eight points.
    for n in 0..12 {
        let at = v
            .duck_curve_at(&k, 0.1 + 0.07 * f64::from(n), 0.95)
            .unwrap();
        click(&mut document, &mut v, at, 1);
    }
    assert_eq!(count(&document), MAX_POINTS);
}

#[test]
fn dragging_the_line_bends_the_segment_to_follow_the_pointer() {
    let (mut document, mut v) = rig();
    let k = key("fx");
    // The way back of the built-in curve, half way along it, pulled down towards more dip.
    let x = 0.5;
    let y = f64::from(value_at(&DEFAULT_POINTS, x));
    let on = v.duck_curve_at(&k, x, y).unwrap();
    let a = drag(
        &mut document,
        &mut v,
        on,
        &[Point::new(on.x, on.y + 8.0), Point::new(on.x, on.y + 16.0)],
    );
    let pts = stored(&document);
    assert_eq!(pts.len(), 3);
    assert!(
        pts[1].2 > -24.0,
        "pulled down: the bend rose from -24 ({})",
        pts[1].2
    );
    // Only the bend param moved, with a merge id.
    let sets: Vec<_> = commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { param, merge, .. } if param == POINT_KEYS[1][2] => Some(merge),
            _ => None,
        })
        .collect();
    assert!(
        sets.len() >= 2 && sets.iter().all(Option::is_some),
        "{sets:?}"
    );
}

#[test]
fn escape_puts_the_point_back() {
    let (mut document, mut v) = rig();
    let reg = registry();
    let k = key("fx");
    // Write the curve first so there is a stored value to come back to.
    let h = v.duck_point_center(&k, 1).unwrap();
    drag(&mut document, &mut v, h, &[Point::new(h.x + 1.0, h.y)]);
    let h = v.duck_point_center(&k, 1).unwrap();
    let before = stored(&document)[1];
    let a = v.handle(&down(h), document.doc(), &reg);
    apply(&mut document, &mut v, &a);
    for i in 1..5 {
        let a = v.handle(
            &mv(Point::new(
                h.x + 6.0 * f64::from(i),
                h.y + 5.0 * f64::from(i),
            )),
            document.doc(),
            &reg,
        );
        apply(&mut document, &mut v, &a);
    }
    assert_ne!(stored(&document)[1], before);
    let a = v.handle(
        &support::song::press(atomik_ui::Key::Escape, Modifiers::NONE),
        document.doc(),
        &reg,
    );
    apply(&mut document, &mut v, &a);
    let after = stored(&document)[1];
    assert!((after.0 - before.0).abs() < 1e-6 && (after.1 - before.1).abs() < 1e-6);
    assert!(a.contains(&UiAction::EndMerge));
}

#[test]
fn the_learn_button_asks_the_app() {
    let (mut document, mut v) = rig();
    let at = v.duck_learn_center(&key("fx")).unwrap();
    let a = click(&mut document, &mut v, at, 1);
    assert!(a.contains(&UiAction::LearnDuck(key("fx"))), "{a:?}");
    // The back of the panel has neither the button nor points.
    v.set_flip(Flip::Back);
    assert!(v.duck_learn_center(&key("fx")).is_none());
    assert!(v.duck_point_center(&key("fx"), 1).is_none());
}

#[test]
fn every_control_of_the_panel_explains_itself_in_one_short_line() {
    let (_, v) = rig();
    let desc = registry().get("atomik.duck").unwrap().descriptor;
    for p in desc.params {
        if p.key == "bypass" || atomik_model::console::is_hidden_param(p.key) {
            continue;
        }
        let text = atomik_ui::duck_panel::duck_help(p.key)
            .unwrap_or_else(|| panic!("{} has no help", p.key));
        assert!(text.len() <= 100, "{}: {} characters", p.key, text.len());
        let at = v
            .control_center(&key("fx"), p.key)
            .unwrap_or_else(|| panic!("{} is not on the panel", p.key));
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{} has no tooltip", p.key));
        assert_eq!(help.text, text, "{}", p.key);
    }
}

#[test]
fn random_presses_and_drags_only_emit_commands_the_model_accepts() {
    let (mut document, mut v) = rig();
    let k = key("fx");
    let mut seed = 0x2545_f491_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..60 {
        let from = v
            .duck_curve_at(&k, next(), next())
            .expect("the front is showing");
        let to = Point::new(
            from.x + (next() - 0.5) * 120.0,
            from.y + (next() - 0.5) * 60.0,
        );
        match (next() * 3.0) as u8 {
            0 => {
                click(&mut document, &mut v, from, 1);
            }
            1 => {
                click(&mut document, &mut v, from, 2);
            }
            _ => {
                drag(&mut document, &mut v, from, &[to]);
            }
        }
        let pts = stored(&document);
        assert!(
            pts.len() <= MAX_POINTS && (pts.is_empty() || pts.len() >= 2),
            "{pts:?}"
        );
        assert!(pts.windows(2).all(|w| w[0].0 <= w[1].0), "{pts:?}");
    }
}
