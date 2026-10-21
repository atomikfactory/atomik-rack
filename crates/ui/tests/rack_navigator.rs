//! The rack navigator: the strip at the right edge of the rack, the overview of every column,
//! the viewport rectangle, press / drag / wheel, the strip never starting a device drag, the
//! setting, and the display-list goldens `rack_navigator_*`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_navigator`.

mod support;

use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_model::RackDoc;
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{
    COLUMN_GAP, DisplayList, Flip, Modifiers, NAVIGATOR_WIDTH, PointerButton, RackView, UiAction,
    UiEvent,
};
use support::container::{check, layered, paint_rack, scoped_view};
use support::*;

const WINDOW: Size = Size::new(1000.0, 420.0);

/// Synth in column 0, delay in column 1, mixer in column 2.
fn three_columns() -> (RackDoc, Registry) {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.columns = 3;
    doc.devices[1].column = 1;
    doc.devices[2].column = 2;
    (doc, reg)
}

fn nav_view(doc: &RackDoc, reg: &Registry, window: Size) -> RackView {
    let mut v = RackView::new(atomik_ui::Theme::dark());
    v.set_show_console_devices(false);
    v.set_navigator_visible(true);
    let _ = v.handle(&UiEvent::Resize(window), doc, reg);
    v.sync(doc, reg);
    v
}

fn paint(v: &mut RackView, doc: &RackDoc, reg: &Registry) -> DisplayList {
    let mut l = DisplayList::new();
    v.paint(&mut l, doc, reg);
    l
}

fn wheel(pos: Point, delta: Vec2, shift: bool) -> UiEvent {
    UiEvent::Wheel {
        delta,
        pos,
        modifiers: Modifiers {
            shift,
            ..Modifiers::NONE
        },
    }
}

/// The left edge of the rack area's content at the far end of the scroll: where the view's
/// right edge (the strip) cuts the rack.
fn right_edge(v: &RackView) -> f64 {
    v.view_to_rack(Point::new(
        v.navigator_rect().map_or(WINDOW.width, |r| r.x0),
        0.0,
    ))
    .x
}

// ---------------------------------------------------------------- the setting and the area

#[test]
fn the_navigator_is_off_in_a_new_view_and_changes_nothing_while_off() {
    let (doc, reg) = three_columns();
    let mut v = RackView::new(atomik_ui::Theme::dark());
    assert!(!v.navigator_visible());
    assert!(v.navigator_rect().is_none());
    let _ = v.handle(&UiEvent::Resize(WINDOW), &doc, &reg);
    v.sync(&doc, &reg);
    let before = paint(&mut v, &doc, &reg).to_json();
    v.set_navigator_visible(true);
    v.set_navigator_visible(false);
    assert_eq!(paint(&mut v, &doc, &reg).to_json(), before);
}

#[test]
fn the_strip_takes_the_right_edge_and_the_rack_area_gives_it_up() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, WINDOW);
    let strip = v.navigator_rect().expect("on");
    assert!((strip.x0 - (WINDOW.width - NAVIGATOR_WIDTH)).abs() < 1e-9);
    assert!((strip.x1 - WINDOW.width).abs() < 1e-9 && (strip.y1 - WINDOW.height).abs() < 1e-9);
    // At the end of the scroll the rack ends at the strip, not under it.
    v.scroll_by(Vec2::new(1e9, 0.0));
    let end = v.content_width();
    assert!(
        (right_edge(&v) - (end + 24.0)).abs() < 1e-6,
        "{} vs {end}",
        right_edge(&v)
    );
    // Without it the whole window is the rack area.
    v.set_navigator_visible(false);
    v.scroll_by(Vec2::new(1e9, 0.0));
    let wide = v.view_to_rack(Point::new(WINDOW.width, 0.0)).x;
    assert!((wide - (end + 24.0)).abs() < 1e-6);
}

#[test]
fn a_press_on_the_strip_never_reaches_the_devices_under_it() {
    // A rack narrow enough that the strip would cover a device's right edge.
    let (doc, reg) = (reference_doc(), registry());
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 1200.0));
    let strip = v.navigator_rect().unwrap();
    let p = Point::new(strip.center().x, strip.y0 + 5.0);
    let a = v.handle(&down(p), &doc, &reg);
    let _ = v.handle(&up(p), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
    assert!(v.selected().is_none());
}

#[test]
fn a_window_too_narrow_for_the_strip_hides_it() {
    let (doc, reg) = (reference_doc(), registry());
    let v = nav_view(&doc, &reg, Size::new(300.0, 400.0));
    assert!(v.navigator_visible());
    assert!(v.navigator_rect().is_none());
}

// ---------------------------------------------------------------- the overview

#[test]
fn every_column_fits_the_strip_with_one_scale() {
    let (doc, reg) = three_columns();
    let v = nav_view(&doc, &reg, WINDOW);
    let strip = v.navigator_rect().unwrap();
    let origin = v.navigator_point(Point::ZERO).unwrap();
    let far = v
        .navigator_point(Point::new(v.content_width(), v.content_height()))
        .unwrap();
    assert!(
        origin.x >= strip.x0 && far.x <= strip.x1,
        "{origin:?} {far:?}"
    );
    assert!(origin.y >= strip.y0 && far.y <= strip.y1);
    // Uniform scale: a column step is the same in x as the heights are in y.
    let step = v
        .navigator_point(Point::new(f64::from(RACK_WIDTH) + COLUMN_GAP, 0.0))
        .unwrap();
    let down = v.navigator_point(Point::new(0.0, 100.0)).unwrap();
    let sx = (step.x - origin.x) / (f64::from(RACK_WIDTH) + COLUMN_GAP);
    let sy = (down.y - origin.y) / 100.0;
    assert!((sx - sy).abs() < 1e-9);
    assert!(sx > 0.0);
}

#[test]
fn the_viewport_follows_the_scroll_and_the_zoom() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, WINDOW);
    let a = v.navigator_viewport().unwrap();
    v.scroll_by(Vec2::new(300.0, 0.0));
    let b = v.navigator_viewport().unwrap();
    assert!(b.x0 > a.x0 && (b.width() - a.width()).abs() < 1e-6 || b.x1 >= a.x1);
    // Zoomed in the viewport covers less of the rack.
    let before = b.width();
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 3.0),
            pos: Point::new(300.0, 200.0),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
        },
        &doc,
        &reg,
    );
    assert!(v.navigator_viewport().unwrap().width() < before);
}

#[test]
fn a_rack_that_fits_has_a_viewport_as_big_as_the_rack() {
    let (doc, reg) = (reference_doc(), registry());
    let v = nav_view(&doc, &reg, Size::new(1400.0, 1400.0));
    let vp = v.navigator_viewport().unwrap();
    let a = v.navigator_point(Point::ZERO).unwrap();
    let b = v
        .navigator_point(Point::new(v.content_width(), v.content_height()))
        .unwrap();
    assert!((vp.x0 - a.x).abs() < 1e-6 && (vp.x1 - b.x).abs() < 1e-6);
    assert!((vp.y0 - a.y).abs() < 1e-6 && (vp.y1 - b.y).abs() < 1e-6);
}

// ---------------------------------------------------------------- press, drag, wheel

#[test]
fn a_press_outside_the_viewport_centres_it_on_the_point() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 1200.0));
    // Content is 3 columns wide; the viewport shows about one and a bit of it.
    let target = Point::new(1500.0, 40.0);
    let at = v.navigator_point(target).unwrap();
    let vp = v.navigator_viewport().unwrap();
    assert!(
        !vp.contains(at),
        "the press must start outside the viewport"
    );
    let a = v.handle(&down(at), &doc, &reg);
    assert!(a.is_empty(), "the navigator emits no action: {a:?}");
    let vp = v.navigator_viewport().unwrap();
    assert!((vp.center().x - at.x).abs() < 1e-6, "{vp:?} vs {at:?}");
    let _ = v.handle(&up(at), &doc, &reg);
}

#[test]
fn a_drag_inside_the_viewport_keeps_the_grab_point_and_follows() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 1200.0));
    // Scrolled in, so no margin of the viewport is cut off by the rack's edge.
    v.scroll_by(Vec2::new(300.0, 0.0));
    let vp = v.navigator_viewport().unwrap();
    let grab = Point::new(vp.x0 + 4.0, vp.y0 + 2.0);
    let _ = v.handle(&down(grab), &doc, &reg);
    let still = v.navigator_viewport().unwrap();
    assert!((still.x0 - vp.x0).abs() < 1e-6, "no jump on a press inside");
    let to = Point::new(grab.x + 25.0, grab.y);
    let a = v.handle(&mv(to), &doc, &reg);
    assert!(a.is_empty());
    let moved = v.navigator_viewport().unwrap();
    assert!((moved.x0 - (vp.x0 + 25.0)).abs() < 1e-6, "{moved:?}");
    // Leaving the strip while the button is down still drags; the end of the rack clamps.
    let _ = v.handle(&mv(Point::new(grab.x + 5000.0, grab.y)), &doc, &reg);
    let end = v.navigator_viewport().unwrap();
    assert!(
        end.x1
            >= v.navigator_point(Point::new(v.content_width(), 0.0))
                .unwrap()
                .x
                - 1e-6
    );
    let _ = v.handle(&up(to), &doc, &reg);
    // After the release the pointer no longer scrolls.
    let kept = v.navigator_viewport().unwrap();
    let _ = v.handle(&mv(grab), &doc, &reg);
    assert_eq!(v.navigator_viewport().unwrap(), kept);
}

#[test]
fn the_wheel_over_the_strip_scrolls_the_rack_like_over_the_rack() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 200.0));
    let on = v.navigator_rect().unwrap().center();
    let y0 = v.navigator_viewport().unwrap().y0;
    let _ = v.handle(&wheel(on, Vec2::new(0.0, -1.0), false), &doc, &reg);
    assert!(
        v.navigator_viewport().unwrap().y0 > y0,
        "wheel down scrolls down"
    );
    let x0 = v.navigator_viewport().unwrap().x0;
    let _ = v.handle(&wheel(on, Vec2::new(0.0, -1.0), true), &doc, &reg);
    assert!(
        v.navigator_viewport().unwrap().x0 > x0,
        "Shift scrolls sideways"
    );
}

#[test]
fn the_strip_never_starts_a_device_drag_or_a_drop() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 1200.0));
    // A device dragged out of the rack and released over the strip is dropped.
    let from = v.plate_center(&key("synth")).unwrap();
    let on = v.navigator_rect().unwrap().center();
    let mut out = v.handle(&down(from), &doc, &reg);
    out.extend(v.handle(&mv(on), &doc, &reg));
    out.extend(v.handle(&up(on), &doc, &reg));
    assert!(commands(&out).is_empty(), "{out:?}");
    // A browser row released over it creates nothing.
    v.set_browser_visible(true);
    let i = v
        .browser_rows(&reg)
        .iter()
        .position(|t| *t == "atomik.stereo-delay")
        .unwrap();
    let row = v.browser_row_center(i).unwrap();
    let mut out = v.handle(&down(row), &doc, &reg);
    out.extend(v.handle(&mv(Point::new(500.0, 300.0)), &doc, &reg));
    out.extend(v.handle(&mv(on), &doc, &reg));
    out.extend(v.handle(&up(on), &doc, &reg));
    assert!(commands(&out).is_empty(), "{out:?}");
    // A press on the strip with the right button does nothing either.
    let a = v.handle(&down_with(on, PointerButton::Secondary, 1), &doc, &reg);
    assert!(a.is_empty());
    assert!(!a.iter().any(|x| matches!(x, UiAction::Command(_))));
}

// ---------------------------------------------------------------- cost

#[test]
fn painting_the_strip_does_not_repaint_a_panel() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, WINDOW);
    let with = paint(&mut v, &doc, &reg);
    v.set_navigator_visible(false);
    let without = paint(&mut v, &doc, &reg);
    // The strip is a handful of rectangles on top of the same panels.
    assert!(with.len() > without.len());
    assert!(
        with.len() - without.len() < 40,
        "{} extra ops",
        with.len() - without.len()
    );
}

// ---------------------------------------------------------------- the container view

#[test]
fn inside_a_container_the_strip_shows_its_single_column() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_navigator_visible(true);
    let _ = v.handle(
        &UiEvent::Resize(Size::new(1000.0, 900.0)),
        d.doc(),
        &registry(),
    );
    assert_eq!(v.scope().len(), 1);
    let strip = v.navigator_rect().unwrap();
    // The bar of the breadcrumb sits above the strip.
    assert!(strip.y0 > 0.0);
    let a = v.navigator_point(Point::ZERO).unwrap();
    let b = v
        .navigator_point(Point::new(f64::from(RACK_WIDTH), v.content_height()))
        .unwrap();
    // One column: as wide as the tall one allows, centred in the strip.
    assert!(b.x - a.x > 0.0 && a.x >= strip.x0 && b.x <= strip.x1);
    let mid = (a.x + b.x) / 2.0;
    assert!((mid - strip.center().x).abs() < 1e-6);
}

// ---------------------------------------------------------------- goldens

#[test]
fn golden_three_columns_scrolled_with_a_selection() {
    let (doc, reg) = three_columns();
    let mut v = nav_view(&doc, &reg, WINDOW);
    v.set_selected(Some(key("delay")));
    v.scroll_by(Vec2::new(400.0, 0.0));
    check(
        "rack_navigator_columns_1x.json",
        &paint(&mut v, &doc, &reg).to_json(),
    );
}

#[test]
fn golden_one_column_back_of_the_rack() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = nav_view(&doc, &reg, Size::new(1000.0, 500.0));
    v.set_flip(Flip::Back);
    check(
        "rack_navigator_one_column_1x.json",
        &paint(&mut v, &doc, &reg).to_json(),
    );
}

#[test]
fn golden_inside_a_container() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_navigator_visible(true);
    let _ = v.handle(
        &UiEvent::Resize(Size::new(1000.0, 700.0)),
        d.doc(),
        &registry(),
    );
    check(
        "rack_navigator_container_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}
