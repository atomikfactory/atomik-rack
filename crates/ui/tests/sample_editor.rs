//! SampleEditorView behaviour through its public contract:
//! hit testing of zones, edges and markers, every drag kind (valid lists, one merge id per
//! gesture), the numeric fields, selectors, delete, audition, drop mapping, open/close and
//! note labels (C4 = 60). Every emitted zone list is applied to a real model `Document`, so
//! the model's own `SetZones` validation judges it.

mod support;

use atomik_device_sdk::{LoopMode, Registry, TriggerMode};
use atomik_model::{Command, Document, RackDoc, ZoneDoc};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::zone_edit::{Field, Marker, note_name};
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton, SampleEditorView, UiAction, UiEvent};
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use support::sampler::*;
use support::song::{alt, down_mods, press, run};
use support::*;

fn setup(zones: Vec<ZoneDoc>) -> (Document, Registry, SampleEditorView) {
    let (doc, reg) = (sampler_doc(zones), registry());
    let v = editor(&doc, &reg);
    (Document::new(doc), reg, v)
}

/// Press at `points[0]`, move through the rest, release at the last point.
fn gesture(
    v: &mut SampleEditorView,
    d: &Document,
    reg: &Registry,
    points: &[Point],
) -> Vec<UiAction> {
    let mut events = vec![down(points[0])];
    events.extend(points[1..].iter().map(|p| mv(*p)));
    events.push(up(*points.last().unwrap()));
    run(|e| v.handle(e, d.doc(), reg), &events)
}

/// A straight drag from `p` by `d`, in three moves.
fn drag_by(p: Point, d: Vec2) -> Vec<Point> {
    vec![p, p + d * 0.3, p + d * 0.6, p + d]
}

/// Re-syncs the view with the document (a harmless pointer move far outside).
fn resync(v: &mut SampleEditorView, d: &Document, reg: &Registry) {
    let _ = v.handle(&mv(Point::new(-50.0, -50.0)), d.doc(), reg);
}

fn texts(v: &mut SampleEditorView, d: &Document, reg: &Registry) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn last_list(a: &[UiAction]) -> Vec<ZoneDoc> {
    zone_lists(a).pop().expect("a zone list").0
}

fn select(v: &mut SampleEditorView, d: &Document, reg: &Registry, i: usize) {
    let p = v.list_row_center(i).unwrap();
    let _ = v.handle(&down(p), d.doc(), reg);
    let _ = v.handle(&up(p), d.doc(), reg);
    assert_eq!(v.selected_zone(), Some(i));
}

fn ppk(v: &SampleEditorView) -> f64 {
    v.key_x(1) - v.key_x(0)
}

#[test]
fn opens_with_the_first_zone_selected_and_a_consistent_axis() {
    let (_, _, v) = setup(layered_zones());
    assert_eq!(v.selected_zone(), Some(0));
    assert_eq!(v.zones().len(), 4);
    assert!(v.is_present());
    for k in [0u8, 1, 59, 60, 127] {
        assert_eq!(v.key_at(v.key_center_x(k)), k);
    }
    for vel in [1u8, 2, 64, 126, 127] {
        assert_eq!(v.velocity_at(v.velocity_y(vel)), vel);
    }
    assert!(v.velocity_y(127) < v.velocity_y(1), "loud is up");
    let r = v.zone_rect(2).unwrap();
    assert!((r.x0 - v.key_x(60)).abs() < 1e-9 && (r.x1 - v.key_x(96)).abs() < 1e-9);
    assert!(v.zone_rect(4).is_none());
}

#[test]
fn an_empty_sampler_opens_with_nothing_selected() {
    let (d, reg, mut v) = setup(Vec::new());
    assert_eq!(v.selected_zone(), None);
    let t = texts(&mut v, &d, &reg);
    for s in [
        "SAMPLE EDITOR",
        "Drop sample files here",
        "No zones",
        "No zone selected",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
}

#[test]
fn note_labels_use_c4_equals_60() {
    assert_eq!(note_name(60), "C4");
    assert_eq!(note_name(0), "C-1");
    assert_eq!(note_name(127), "G9");
    assert_eq!(note_name(61), "C#4");
    let (d, reg, mut v) = setup(layered_zones());
    let t = texts(&mut v, &d, &reg);
    // List rows, keyboard octave labels and the root field of the selected zone (root 48).
    for s in [
        "C3-B4  v1-63  root C4",
        "C-1-B3  v1-127  root C3",
        "C4",
        "C3",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
}

#[test]
fn clicking_a_zone_selects_it_and_auditions_at_the_pointer_height() {
    let (d, reg, mut v) = setup(layered_zones());
    // Keys 60..=71 below velocity 64: only zone 1 covers them.
    let p = Point::new(v.key_center_x(65), v.velocity_y(30));
    let a = v.handle(&down(p), d.doc(), &reg);
    assert_eq!(v.selected_zone(), Some(1));
    assert_eq!(auditions(&a), vec![(65, 30, true)]);
    assert!(zone_lists(&a).is_empty(), "a click edits nothing");
    let a = v.handle(&up(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(65, 30, false)]);
    assert!(!a.contains(&UiAction::EndMerge));
}

#[test]
fn clicking_empty_map_auditions_and_keeps_the_selection() {
    let (d, reg, mut v) = setup(layered_zones());
    v.set_selected_zone(Some(2));
    // Keys 72..=95 below velocity 64 are empty.
    let p = Point::new(v.key_center_x(80), v.velocity_y(10));
    let a = v.handle(&down(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(80, 10, true)]);
    assert_eq!(v.selected_zone(), Some(2));
    let a = v.handle(&up(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(80, 10, false)]);
}

#[test]
fn the_keyboard_strip_auditions_and_glides() {
    let (d, reg, mut v) = setup(layered_zones());
    let a = v.handle(&down(v.keyboard_point(60)), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(60, 100, true)]);
    let a = v.handle(&mv(v.keyboard_point(63)), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(60, 100, false), (63, 100, true)]);
    let a = v.handle(
        &mv(v.keyboard_point(63) + Vec2::new(1.0, 0.0)),
        d.doc(),
        &reg,
    );
    assert!(a.is_empty(), "same key, nothing to send");
    let a = v.handle(&up(v.keyboard_point(63)), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(63, 100, false)]);
}

#[test]
fn the_zone_list_selects_rows() {
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 2);
    select(&mut v, &d, &reg, 3);
    assert!(v.list_row_center(4).is_none());
    let a = v.handle(&down(v.list_row_center(1).unwrap()), d.doc(), &reg);
    assert!(a.is_empty(), "selecting does not audition or edit");
}

#[test]
fn dragging_edges_changes_the_ranges() {
    let (mut d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let k = ppk(&v);
    let ppv = v.velocity_y(1) - v.velocity_y(2);
    // Left edge: three keys down.
    let r = v.zone_rect(1).unwrap();
    let start = Point::new(r.x0 + 1.0, r.center().y);
    let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(-3.0 * k, 0.0)));
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (45, 71));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Right edge: two keys up.
    let r = v.zone_rect(1).unwrap();
    let start = Point::new(r.x1 - 1.0, r.center().y);
    let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(2.0 * k, 0.0)));
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (45, 73));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Top edge: velocity ceiling 63 -> 100.
    let r = v.zone_rect(1).unwrap();
    let start = Point::new(r.center().x, r.y0 + 1.0);
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &drag_by(start, Vec2::new(0.0, -37.0 * ppv)),
    );
    let z = &last_list(&a)[1];
    assert_eq!((z.vel_lo, z.vel_hi), (1, 100));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Bottom edge: velocity floor 1 -> 20.
    let r = v.zone_rect(1).unwrap();
    let start = Point::new(r.center().x, r.y1 - 1.0);
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &drag_by(start, Vec2::new(0.0, -19.0 * ppv)),
    );
    let z = &last_list(&a)[1];
    assert_eq!((z.vel_lo, z.vel_hi), (20, 100));
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn a_corner_drag_moves_two_edges() {
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let r = v.zone_rect(1).unwrap();
    let k = ppk(&v);
    let ppv = v.velocity_y(1) - v.velocity_y(2);
    let start = Point::new(r.x1 - 1.0, r.y0 + 1.0);
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &drag_by(start, Vec2::new(4.0 * k, -10.0 * ppv)),
    );
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi, z.vel_lo, z.vel_hi), (48, 75, 1, 73));
}

#[test]
fn edges_never_cross() {
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let r = v.zone_rect(1).unwrap();
    let k = ppk(&v);
    let start = Point::new(r.x0 + 1.0, r.center().y);
    let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(500.0 * k, 0.0)));
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (71, 71));
}

#[test]
fn dragging_the_body_moves_the_key_range_keeping_its_width() {
    let (mut d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let r = v.zone_rect(1).unwrap();
    let start = r.center();
    let k = ppk(&v);
    let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(10.0 * k, 0.0)));
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (58, 81));
    assert_eq!((z.vel_lo, z.vel_hi), (1, 63), "velocity range untouched");
    let au = auditions(&a);
    assert_eq!(au.len(), 2, "pressed note on and off");
    assert!(au[0].2 && !au[1].2);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Far right and far left: stops at the ends of the keyboard.
    let r = v.zone_rect(1).unwrap();
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &drag_by(r.center(), Vec2::new(900.0, 0.0)),
    );
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (104, 127));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let r = v.zone_rect(1).unwrap();
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &drag_by(r.center(), Vec2::new(-1900.0, 0.0)),
    );
    let z = &last_list(&a)[1];
    assert_eq!((z.key_lo, z.key_hi), (0, 23));
}

#[test]
fn a_body_press_without_movement_edits_nothing() {
    let (d, reg, mut v) = setup(layered_zones());
    let p = v.zone_rect(3).unwrap().center();
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert!(zone_lists(&a).is_empty() && !a.contains(&UiAction::EndMerge));
    assert_eq!(v.selected_zone(), Some(3));
}

#[test]
fn a_drag_uses_one_merge_id_and_ends_with_one_end_merge() {
    let (mut d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let k = ppk(&v);
    let mut seen = BTreeSet::new();
    for dx in [4.0, -9.0, 2.0] {
        let start = v.zone_rect(1).unwrap().center();
        let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(dx * k, 0.0)));
        let lists = zone_lists(&a);
        assert!(lists.len() >= 2, "several moves, several lists");
        let merge = lists[0].1;
        assert!(merge.is_some());
        assert!(lists.iter().all(|(_, m)| *m == merge), "one id per gesture");
        assert_eq!(a.iter().filter(|x| **x == UiAction::EndMerge).count(), 1);
        assert!(seen.insert(merge), "a new id for every gesture");
        apply_zones(&mut d, &reg, &a);
        resync(&mut v, &d, &reg);
    }
    assert!(d.can_undo());
}

#[test]
fn escape_cancels_a_drag_restoring_the_start_values() {
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let p = v.zone_rect(1).unwrap().center();
    let mut a = v.handle(&down(p), d.doc(), &reg);
    a.extend(v.handle(&mv(p + Vec2::new(5.0 * ppk(&v), 0.0)), d.doc(), &reg));
    assert!(v.is_dragging());
    let moved = zone_lists(&a);
    let b = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert!(!v.is_dragging());
    let lists = zone_lists(&b);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].0, d.doc().devices[0].zones);
    assert_eq!(lists[0].1, moved[0].1, "same merge group");
    assert!(b.contains(&UiAction::EndMerge));
}

#[test]
fn markers_drag_with_the_ordering_rules() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    let w = v.waveform_rect();
    // Start marker 50 px right.
    let p = v.marker_point(Marker::Start).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(50.0, 0.0)));
    let z = &last_list(&a)[0];
    assert!(
        (z.start as f64 - v.x_to_frame(p.x + 50.0)).abs() <= 1.0,
        "{}",
        z.start
    );
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // End marker far right: clamps at the end of the asset.
    let p = v.marker_point(Marker::End).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(w.width(), 0.0)));
    assert_eq!(last_list(&a)[0].end, FR_A);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Loop end far left: stays right of the loop start.
    let p = v.marker_point(Marker::LoopEnd).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(-w.width(), 0.0)));
    let z = &last_list(&a)[0];
    assert_eq!(z.loop_end, z.loop_start + 1);
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn start_end_and_loop_stay_ordered_and_the_crossfade_fits() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    let w = v.waveform_rect();
    // Start dragged past the end stops one frame before it, taking the loop along.
    let p = v.marker_point(Marker::Start).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(w.width(), 0.0)));
    let z = &last_list(&a)[0];
    assert_eq!(z.start, z.end - 1);
    assert!(z.start <= z.loop_start && z.loop_start < z.loop_end && z.loop_end <= z.end);
    assert!(z.crossfade <= z.loop_start - z.start);
    apply_zones(&mut d, &reg, &a);
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    // Loop start dragged near the start: the crossfade shrinks to fit before it.
    let p = v.marker_point(Marker::LoopStart).unwrap();
    let target = v.marker_point(Marker::Start).unwrap().x + 12.0;
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(target - p.x, 0.0)));
    let z = &last_list(&a)[0];
    assert!(
        z.crossfade < 4_800 && z.crossfade <= z.loop_start - z.start,
        "{z:?}"
    );
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn the_crossfade_edge_and_alt_drag_set_the_crossfade() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    let p = v.marker_point(Marker::Crossfade).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(-10.0, 0.0)));
    let z = &last_list(&a)[0];
    assert!(z.crossfade > 4_800, "grew to the left: {}", z.crossfade);
    assert_eq!(z.loop_start, 24_000);
    apply_zones(&mut d, &reg, &a);
    // With crossfade 0 there is no handle; Alt+drag on the loop start grows it.
    let mut zero = looped_zone();
    zero.crossfade = 0;
    let (mut d, reg, mut v) = setup(vec![zero]);
    let p = v.marker_point(Marker::LoopStart).unwrap();
    let pts = drag_by(p, Vec2::new(-20.0, 0.0));
    let mut events = vec![down_mods(pts[0], alt(), 1)];
    events.extend(pts[1..].iter().map(|q| mv(*q)));
    events.push(up(*pts.last().unwrap()));
    let a = run(|e| v.handle(e, d.doc(), &reg), &events);
    let z = &last_list(&a)[0];
    assert!(z.crossfade > 0 && z.loop_start == 24_000, "{z:?}");
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn loop_markers_are_not_grabbable_while_the_loop_is_off() {
    let mut z = looped_zone();
    z.loop_mode = LoopMode::Off;
    let (d, reg, mut v) = setup(vec![z]);
    let p = v.marker_point(Marker::LoopStart).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(30.0, 0.0)));
    assert!(zone_lists(&a).is_empty());
}

#[test]
fn loop_and_trigger_selectors_emit_full_lists() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    for mode in [LoopMode::PingPong, LoopMode::Off, LoopMode::Forward] {
        let p = v.loop_button_center(mode);
        let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
        let lists = zone_lists(&a);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].1, None, "a click is its own undo step");
        assert_eq!(lists[0].0[0].loop_mode, mode);
        apply_zones(&mut d, &reg, &a);
        resync(&mut v, &d, &reg);
    }
    let p = v.loop_button_center(LoopMode::Forward);
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert!(a.is_empty(), "already forward");
    let p = v.trigger_button_center(TriggerMode::OneShot);
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert_eq!(last_list(&a)[0].trigger, TriggerMode::OneShot);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert!(a.is_empty(), "already one-shot");
}

#[test]
fn switching_the_loop_on_for_a_fresh_zone_is_valid() {
    let (mut d, reg, mut v) = setup(vec![ZoneDoc::new(S_A, FR_A)]);
    let p = v.loop_button_center(LoopMode::PingPong);
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert_eq!(last_list(&a)[0].loop_mode, LoopMode::PingPong);
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn fields_drag_vertically_reset_on_double_click_and_step_with_the_wheel() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    let p = v.field_center(Field::Tune);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -20.0)));
    assert!((last_list(&a)[0].tune_cents - 10.0).abs() < 1e-3);
    let merge = zone_lists(&a)[0].1;
    assert!(merge.is_some() && zone_lists(&a).iter().all(|(_, m)| *m == merge));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Shift = fine (a tenth).
    let events = [
        down(p),
        mv_with(p + Vec2::new(0.0, -20.0), Modifiers::SHIFT),
        up(p),
    ];
    let a = run(|e| v.handle(e, d.doc(), &reg), &events);
    assert!((last_list(&a)[0].tune_cents - 11.0).abs() < 1e-3);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Double-click resets to the ZoneDoc::new default.
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), d.doc(), &reg);
    assert_eq!(last_list(&a)[0].tune_cents, 0.0);
    assert_eq!(zone_lists(&a)[0].1, None);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Root key drag, clamped to 127, then reset to C4.
    let p = v.field_center(Field::Root);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -4000.0)));
    assert_eq!(last_list(&a)[0].root_key, 127);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), d.doc(), &reg);
    assert_eq!(last_list(&a)[0].root_key, 60);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    // Wheel steps the gain by half a dB.
    let wheel = UiEvent::Wheel {
        delta: Vec2::new(0.0, 1.0),
        pos: v.field_center(Field::Gain),
        modifiers: Modifiers::NONE,
    };
    let a = v.handle(&wheel, d.doc(), &reg);
    assert!((last_list(&a)[0].gain_db - 0.5).abs() < 1e-6);
}

#[test]
fn key_and_velocity_fields_never_cross_and_the_crossfade_is_limited() {
    let (mut d, reg, mut v) = setup(vec![looped_zone()]);
    let p = v.field_center(Field::KeyLo);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -4000.0)));
    let z = &last_list(&a)[0];
    assert_eq!((z.key_lo, z.key_hi), (84, 84));
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let p = v.field_center(Field::VelHi);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, 4000.0)));
    assert_eq!(last_list(&a)[0].vel_hi, 1);
    apply_zones(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let p = v.field_center(Field::Crossfade);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -4000.0)));
    let z = &last_list(&a)[0];
    assert_eq!(
        z.crossfade,
        z.loop_start - z.start,
        "limited by the audio before the loop"
    );
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn delete_removes_the_selected_zone() {
    let (mut d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    let a = delete(&mut v, &d, &reg);
    let lists = zone_lists(&a);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].1, None);
    let expected: Vec<ZoneDoc> = layered_zones()
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, z)| z)
        .collect();
    assert_eq!(lists[0].0, expected);
    assert_eq!(v.selected_zone(), Some(1), "the next zone takes over");
    apply_zones(&mut d, &reg, &a);
    // Backspace until the list is empty: no selection left and nothing more to delete.
    for _ in 0..3 {
        resync(&mut v, &d, &reg);
        let a = delete(&mut v, &d, &reg);
        apply_zones(&mut d, &reg, &a);
    }
    resync(&mut v, &d, &reg);
    assert!(d.doc().devices[0].zones.is_empty());
    assert_eq!(v.selected_zone(), None);
    let out = v.command("edit.delete", None, d.doc(), &reg);
    assert_eq!(out, CommandOutcome::Declined, "nothing selected: declined");
}

/// Runs `edit.delete` on the view (the registry command the shell offers for Delete).
fn delete(v: &mut SampleEditorView, d: &Document, reg: &Registry) -> Vec<UiAction> {
    match v.command("edit.delete", None, d.doc(), reg) {
        CommandOutcome::Handled(a) => a,
        other => panic!("edit.delete was {other:?}"),
    }
}

#[test]
fn the_view_no_longer_reads_delete_or_undo_keys_itself() {
    // Delete is a registry command (rebindable); the raw keys do nothing in the view.
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    for key in [Key::Delete, Key::Backspace] {
        let a = v.handle(&press(key, Modifiers::NONE), d.doc(), &reg);
        assert!(a.is_empty(), "{a:?}");
    }
    assert_eq!(v.selected_zone(), Some(1));
    // Undo and redo are the global `edit.undo` / `edit.redo`, not the view's keys.
    let shift_ctrl = Modifiers {
        shift: true,
        ..Modifiers::CTRL
    };
    for (c, m) in [
        ('z', Modifiers::CTRL),
        ('y', Modifiers::CTRL),
        ('z', shift_ctrl),
    ] {
        assert!(v.handle(&press(Key::Char(c), m), d.doc(), &reg).is_empty());
    }
}

#[test]
fn only_edit_delete_is_the_views_command_and_it_declines_mid_gesture() {
    let (d, reg, mut v) = setup(layered_zones());
    select(&mut v, &d, &reg, 1);
    for id in [
        "edit.copy",
        "edit.select_all",
        "edit.duplicate",
        "view.zoom_in",
        "sequencer.follow",
        "editor.quantize",
        "transport.play_stop",
    ] {
        assert_eq!(
            v.command(id, None, d.doc(), &reg),
            CommandOutcome::Declined,
            "{id}"
        );
    }
    // A drag in progress: the command waits (Esc cancels the drag first).
    let p = v.zone_rect(1).unwrap().center();
    let _ = v.handle(&down(p), d.doc(), &reg);
    let _ = v.handle(&mv(p + Vec2::new(5.0 * ppk(&v), 0.0)), d.doc(), &reg);
    assert!(v.is_dragging());
    assert_eq!(
        v.command("edit.delete", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
    let _ = v.handle(&up(p), d.doc(), &reg);
}

#[test]
fn drops_map_to_the_key_under_the_pointer_on_the_map_and_to_none_elsewhere() {
    let (d, reg, mut v) = setup(layered_zones());
    let paths = vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")];
    let import = |at_key| UiAction::ImportSamples {
        paths: paths.clone(),
        device: key("sampler"),
        at_key,
    };
    let hover = |pos| UiEvent::FileHover {
        pos,
        paths: paths.clone(),
    };
    let drop_at = |pos| UiEvent::FileDrop {
        pos,
        paths: paths.clone(),
    };
    // Over the map.
    let on_map = Point::new(v.key_center_x(70), v.velocity_y(100));
    assert!(v.handle(&hover(on_map), d.doc(), &reg).is_empty());
    assert_eq!(v.drop_hover(), Some(Some(70)));
    let a = v.handle(&drop_at(on_map), d.doc(), &reg);
    assert_eq!(a, vec![import(Some(70))]);
    assert_eq!(v.drop_hover(), None);
    // Over the keyboard strip: also a key.
    let a = v.handle(&drop_at(v.keyboard_point(33)), d.doc(), &reg);
    assert_eq!(a, vec![import(Some(33))]);
    // Over the waveform and the zone list: no key.
    for pos in [v.waveform_rect().center(), v.list_rect().center()] {
        let _ = v.handle(&hover(pos), d.doc(), &reg);
        assert_eq!(v.drop_hover(), Some(None));
        let a = v.handle(&drop_at(pos), d.doc(), &reg);
        assert_eq!(a, vec![import(None)]);
    }
    // Outside the view, or no files: nothing.
    let outside = Point::new(-5.0, 100.0);
    let _ = v.handle(&hover(outside), d.doc(), &reg);
    assert_eq!(v.drop_hover(), None);
    assert!(v.handle(&drop_at(outside), d.doc(), &reg).is_empty());
    let none = UiEvent::FileDrop {
        pos: on_map,
        paths: Vec::new(),
    };
    assert!(v.handle(&none, d.doc(), &reg).is_empty());
    // Cancel clears the indicator.
    let _ = v.handle(&hover(on_map), d.doc(), &reg);
    assert!(v.drop_hover().is_some());
    let _ = v.handle(&UiEvent::FileHoverCancel, d.doc(), &reg);
    assert_eq!(v.drop_hover(), None);
}

#[test]
fn the_drop_indicator_names_the_target() {
    let (d, reg, mut v) = setup(layered_zones());
    let on_map = Point::new(v.key_center_x(61), v.velocity_y(100));
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on_map,
            paths: vec![PathBuf::from("a.wav")],
        },
        d.doc(),
        &reg,
    );
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|t| t == "Drop 1 file at C#4"), "{t:?}");
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: v.waveform_rect().center(),
            paths: vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")],
        },
        d.doc(),
        &reg,
    );
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|t| t == "Drop 2 files to add zones"), "{t:?}");
}

#[test]
fn close_button_and_escape_close_the_editor() {
    let (d, reg, mut v) = setup(layered_zones());
    let p = v.close_button_center();
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert_eq!(a, vec![UiAction::CloseSampleEditor]);
    // First Esc deselects, the next closes.
    let esc = press(Key::Escape, Modifiers::NONE);
    assert!(v.handle(&esc, d.doc(), &reg).is_empty());
    assert_eq!(v.selected_zone(), None);
    assert_eq!(
        v.handle(&esc, d.doc(), &reg),
        vec![UiAction::CloseSampleEditor]
    );
}

#[test]
fn the_load_button_asks_the_host_for_the_file_dialog() {
    let (d, reg, mut v) = setup(layered_zones());
    let p = v.load_button_center();
    let a = run(|e| v.handle(e, d.doc(), &reg), &[down(p), up(p)]);
    assert_eq!(
        a,
        vec![UiAction::LoadSamplesDialog {
            device: key("sampler")
        }]
    );
    assert!(texts(&mut v, &d, &reg).iter().any(|t| t == "LOAD..."));
    assert_ne!(p, v.close_button_center());
}

#[test]
fn a_deleted_sampler_is_no_longer_present() {
    let (d, reg, mut v) = setup(layered_zones());
    assert!(v.is_present());
    let mut doc: RackDoc = d.doc().clone();
    doc.devices.retain(|x| x.key != key("sampler"));
    v.sync(&doc, &reg);
    assert!(!v.is_present());
    assert!(v.zones().is_empty());
    assert!(
        v.handle(&down(Point::new(100.0, 100.0)), &doc, &reg)
            .is_empty()
    );
    // A device that does not take samples is not present either.
    let mut v = SampleEditorView::new(atomik_ui::Theme::dark());
    v.open(key("synth"));
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
}

#[test]
fn a_missing_asset_stays_listed_and_has_no_waveform_or_markers() {
    let (mut d, reg, mut v) = setup(layered_zones());
    v.set_selected_zone(Some(3));
    assert!(v.marker_point(Marker::Start).is_none());
    let t = texts(&mut v, &d, &reg);
    assert!(
        t.iter()
            .any(|x| x == "Missing asset: the sample file is not available"),
        "{t:?}"
    );
    assert!(
        t.iter().any(|x| x == "lost.wav"),
        "the zone is still listed"
    );
    // Its ranges still drag: key edits need no audio.
    let k = ppk(&v);
    let r = v.zone_rect(3).unwrap();
    let start = Point::new(r.x0 + 1.0, r.center().y);
    let a = gesture(&mut v, &d, &reg, &drag_by(start, Vec2::new(-4.0 * k, 0.0)));
    assert_eq!(last_list(&a)[3].key_lo, 92);
    apply_zones(&mut d, &reg, &a);
}

#[test]
fn the_selected_zones_waveform_and_markers_are_drawn() {
    let (d, reg, mut v) = setup(vec![looped_zone()]);
    let t = texts(&mut v, &d, &reg);
    for s in ["LOOP", "TRIGGER", "FWD", "ONE-SHOT", "XFADE", "TUNE"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}");
    }
    assert!(t.iter().any(|x| x.starts_with("piano_C3.wav  ·  2ch")));
}

#[test]
fn wheel_zooms_and_scrolls_the_key_axis() {
    let (d, reg, mut v) = setup(layered_zones());
    let p = Point::new(v.key_center_x(60), v.velocity_y(64));
    let wheel = |delta: Vec2, modifiers| UiEvent::Wheel {
        delta,
        pos: p,
        modifiers,
    };
    let fit = ppk(&v);
    let _ = v.handle(&wheel(Vec2::new(0.0, 6.0), Modifiers::CTRL), d.doc(), &reg);
    assert!(ppk(&v) > fit * 1.5, "zoomed in: {}", ppk(&v));
    assert!(
        (v.key_x(60) - v.map_rect().x0).abs() < v.map_rect().width(),
        "the anchor key stays on screen"
    );
    let before = v.key_x(0);
    let _ = v.handle(&wheel(Vec2::new(0.0, -3.0), Modifiers::NONE), d.doc(), &reg);
    assert!((v.key_x(0) - before).abs() > 1e-6, "scrolled");
    // Zoomed out fully, the whole keyboard fits the map again.
    let _ = v.handle(
        &wheel(Vec2::new(0.0, -200.0), Modifiers::CTRL),
        d.doc(),
        &reg,
    );
    assert!((v.key_x(0) - v.map_rect().x0).abs() < 1e-6);
    assert!((v.key_x(128) - v.map_rect().x1).abs() < 1e-6);
}

// ---------------------------------------------------------------- property test

#[derive(Debug, Clone)]
enum G {
    Zone {
        pick: usize,
        part: u8,
        dx: i32,
        dy: i32,
    },
    Marker {
        which: u8,
        dx: i32,
    },
    Field {
        which: usize,
        dy: i32,
        shift: bool,
    },
    Loop(u8),
    Select(usize),
    Reset(usize),
}

fn gestures() -> impl Strategy<Value = Vec<G>> {
    let g =
        prop_oneof![
            (0usize..6, 0u8..6, -900i32..900, -300i32..300)
                .prop_map(|(pick, part, dx, dy)| G::Zone { pick, part, dx, dy }),
            (0u8..5, -1200i32..1200).prop_map(|(which, dx)| G::Marker { which, dx }),
            (0usize..10, -600i32..600, any::<bool>()).prop_map(|(which, dy, shift)| G::Field {
                which,
                dy,
                shift
            }),
            (0u8..3).prop_map(G::Loop),
            (0usize..4).prop_map(G::Select),
            (0usize..10).prop_map(G::Reset),
        ];
    prop::collection::vec(g, 1..14)
}

fn run_gesture(
    v: &mut SampleEditorView,
    d: &mut Document,
    reg: &Registry,
    g: &G,
    merges: &mut BTreeSet<u64>,
) {
    let actions = match g {
        G::Zone { pick, part, dx, dy } => {
            let n = v.zones().len();
            let r = v.zone_rect(pick % n).unwrap();
            let c = r.center();
            let p = match part {
                0 => c,
                1 => Point::new(r.x0 + 1.0, c.y),
                2 => Point::new(r.x1 - 1.0, c.y),
                3 => Point::new(c.x, r.y0 + 1.0),
                4 => Point::new(c.x, r.y1 - 1.0),
                _ => Point::new(r.x0 + 1.0, r.y0 + 1.0),
            };
            let by = Vec2::new(f64::from(*dx), f64::from(*dy));
            gesture(v, d, reg, &drag_by(p, by))
        }
        G::Marker { which, dx } => {
            let m = [
                Marker::Start,
                Marker::End,
                Marker::LoopStart,
                Marker::LoopEnd,
                Marker::Crossfade,
            ][usize::from(*which)];
            match v.marker_point(m) {
                Some(p) => gesture(v, d, reg, &drag_by(p, Vec2::new(f64::from(*dx), 0.0))),
                None => Vec::new(),
            }
        }
        G::Field { which, dy, shift } => {
            let p = v.field_center(Field::ALL[*which]);
            let mods = if *shift {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            };
            let events = [
                down(p),
                mv_with(p + Vec2::new(0.0, f64::from(*dy) / 2.0), mods),
                mv_with(p + Vec2::new(0.0, f64::from(*dy)), mods),
                up(p),
            ];
            run(|e| v.handle(e, d.doc(), reg), &events)
        }
        G::Loop(m) => {
            let mode = [LoopMode::Off, LoopMode::Forward, LoopMode::PingPong][usize::from(*m)];
            let p = v.loop_button_center(mode);
            run(|e| v.handle(e, d.doc(), reg), &[down(p), up(p)])
        }
        G::Select(i) => {
            let i = i % v.zones().len();
            let p = v.list_row_center(i).unwrap();
            run(|e| v.handle(e, d.doc(), reg), &[down(p), up(p)])
        }
        G::Reset(i) => {
            let p = v.field_center(Field::ALL[*i]);
            v.handle(&down_with(p, PointerButton::Primary, 2), d.doc(), reg)
        }
    };
    let lists = zone_lists(&actions);
    let is_drag = matches!(g, G::Zone { .. } | G::Marker { .. } | G::Field { .. });
    if is_drag && !lists.is_empty() {
        let merge = lists[0].1.expect("drags merge");
        assert!(
            lists.iter().all(|(_, m)| *m == Some(merge)),
            "one id per gesture"
        );
        assert!(merges.insert(merge), "fresh id per gesture: {merge}");
        let ends = actions.iter().filter(|a| **a == UiAction::EndMerge).count();
        assert_eq!(ends, 1);
    }
    for (zones, _) in &lists {
        assert_eq!(
            zones.len(),
            v.zones().len(),
            "edits never add or remove zones"
        );
    }
    // The model's own validation judges every list.
    let applied = apply_zones(d, reg, &actions);
    assert_eq!(applied.len(), lists.len());
    resync(v, d, reg);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_gestures_never_emit_an_invalid_list(gs in gestures()) {
        let mut zones = layered_zones();
        zones.push(looped_zone());
        let (mut d, reg, mut v) = setup(zones);
        let mut merges = BTreeSet::new();
        for g in &gs {
            run_gesture(&mut v, &mut d, &reg, g, &mut merges);
        }
        let mut fresh = Document::new(d.doc().clone());
        let cmd = Command::SetZones {
            device: key("sampler"),
            zones: d.doc().devices[0].zones.clone(),
            merge: None,
        };
        prop_assert!(fresh.apply(cmd, &reg).is_ok());
    }
}

// ---- the synth page: group, crossfade, round-robin, own envelope and filter -----------------------

/// Zones 0 and 1 of the layered set share group 3.
fn grouped_zones() -> Vec<ZoneDoc> {
    let mut zones = layered_zones();
    zones[0].ext.group = 3;
    zones[1].ext.group = 3;
    zones
}

#[test]
fn the_synth_button_switches_the_field_panel_and_back() {
    let (d, reg, mut v) = setup(grouped_zones());
    assert!(!v.synth_page());
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|x| x == "XFADE") && !t.iter().any(|x| x == "ROUND-ROBIN"));
    v.set_synth_page(true);
    let t = texts(&mut v, &d, &reg);
    for s in [
        "GROUP",
        "VEL XFADE",
        "ROUND-ROBIN",
        "RR SLOT",
        "ATTACK",
        "CUTOFF",
        "ENV AMOUNT",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(!t.iter().any(|x| x == "XFADE"));
    // A zone with no envelope or filter of its own shows the device ones.
    assert!(t.iter().any(|x| x == "device"), "{t:?}");
    v.set_synth_page(false);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|x| x == "XFADE"));
}

#[test]
fn dragging_a_synth_field_emits_a_valid_list_and_an_envelope_edit_reaches_the_group() {
    let (d, reg, mut v) = setup(grouped_zones());
    v.set_synth_page(true);
    let p = v.field_center(Field::EnvA);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -40.0)));
    let list = last_list(&a);
    assert_eq!(list.len(), 4);
    let e0 = list[0]
        .ext
        .env
        .expect("the edited zone has its own envelope");
    assert!(e0.attack_ms > 1.0, "dragging up lengthens the attack");
    // Zone 1 is in the same group: it follows. The zones outside the group do not.
    assert_eq!(list[1].ext.env, Some(e0));
    assert_eq!(list[2].ext.env, None);
    assert_eq!(list[3].ext.env, None);
    // The model accepts the list.
    let mut doc = d;
    doc.apply(
        Command::SetZones {
            device: key("sampler"),
            zones: list,
            merge: None,
        },
        &reg,
    )
    .unwrap();
}

#[test]
fn a_zone_outside_any_group_edits_only_itself_and_a_double_click_gives_the_filter_back() {
    let mut zones = layered_zones();
    zones[1].ext.filter = Some(atomik_device_sdk::ZoneFilter {
        mode: 1,
        cutoff: 2_000.0,
        resonance: 0.5,
        key_track: 0.0,
        env_amount: 0.0,
    });
    let (mut d, reg, mut v) = setup(zones);
    v.set_synth_page(true);
    select(&mut v, &d, &reg, 1);
    let p = v.field_center(Field::FltCutoff);
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), d.doc(), &reg);
    let list = last_list(&a);
    assert_eq!(list[1].ext.filter, None);
    d.apply(
        Command::SetZones {
            device: key("sampler"),
            zones: list,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    resync(&mut v, &d, &reg);
    // Editing the cutoff of a zone without a filter starts from the default filter.
    let fc = v.field_center(Field::FltCutoff);
    let a = gesture(&mut v, &d, &reg, &drag_by(fc, Vec2::new(0.0, -20.0)));
    let f = last_list(&a)[1].ext.filter.expect("a filter of its own");
    assert!(f.cutoff > 8_000.0 && f.mode == 0);
}

#[test]
fn the_crossfade_and_round_robin_fields_stay_inside_the_zone_rules() {
    let mut z = layered_zones();
    z[0].vel_lo = 10;
    z[0].vel_hi = 20;
    let (d, reg, mut v) = setup(z);
    v.set_synth_page(true);
    let fc = v.field_center(Field::VelXfade);
    let a = gesture(&mut v, &d, &reg, &drag_by(fc, Vec2::new(0.0, -400.0)));
    assert_eq!(
        last_list(&a)[0].ext.vel_xfade,
        10,
        "at most the width of the velocity range"
    );
    let fc = v.field_center(Field::RrLen);
    let a = gesture(&mut v, &d, &reg, &drag_by(fc, Vec2::new(0.0, -400.0)));
    assert_eq!(last_list(&a)[0].ext.rr_len, 16);
}
