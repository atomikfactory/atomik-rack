//! SliceEditorView behaviour through its public contract:
//! marker hit testing and dragging (one merge id per gesture, EndMerge, Esc), add and delete,
//! slice selection with audition, per-slice fields, the toolbar (SliceLoop in both modes,
//! SlicesToTrack, `loop.beats`), drops, empty state and open/close. Every emitted zone list is
//! applied to a real model `Document`, so the model's own `SetZones` validation judges it.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, ZoneDoc, markers_of};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::zone_edit::Field;
use atomik_ui::{DisplayList, Key, Modifiers, SliceEditorView, SliceMode, UiAction, UiEvent};
use proptest::prelude::*;
use std::path::PathBuf;
use support::slicer::*;
use support::song::{down_mods, press, run};
use support::*;

const MERGE_FLOOR: u64 = 6 << 40;

fn setup(zones: Vec<ZoneDoc>) -> (Document, Registry, SliceEditorView) {
    let (doc, reg) = (slicer_doc(zones), registry());
    let v = editor(&doc, &reg);
    (Document::new(doc), reg, v)
}

fn default_setup() -> (Document, Registry, SliceEditorView) {
    setup(sliced_zones(&MARKERS))
}

fn send(v: &mut SliceEditorView, d: &Document, reg: &Registry, e: &UiEvent) -> Vec<UiAction> {
    v.handle(e, d.doc(), reg)
}

/// Press at `points[0]`, move through the rest, release at the last point.
fn gesture(
    v: &mut SliceEditorView,
    d: &Document,
    reg: &Registry,
    points: &[Point],
) -> Vec<UiAction> {
    let mut events = vec![down(points[0])];
    events.extend(points[1..].iter().map(|p| mv(*p)));
    events.push(up(*points.last().unwrap()));
    run(|e| v.handle(e, d.doc(), reg), &events)
}

fn drag_by(p: Point, d: Vec2) -> Vec<Point> {
    vec![p, p + d * 0.3, p + d * 0.6, p + d]
}

fn click(
    v: &mut SliceEditorView,
    d: &Document,
    reg: &Registry,
    p: Point,
    clicks: u8,
) -> Vec<UiAction> {
    let events = [down_mods(p, Modifiers::NONE, clicks), up(p)];
    run(|e| v.handle(e, d.doc(), reg), &events)
}

fn wheel(
    v: &mut SliceEditorView,
    d: &Document,
    reg: &Registry,
    p: Point,
    dy: f64,
    m: Modifiers,
) -> Vec<UiAction> {
    send(
        v,
        d,
        reg,
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos: p,
            modifiers: m,
        },
    )
}

fn texts(v: &mut SliceEditorView, d: &Document, reg: &Registry) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn last_list(a: &[UiAction]) -> Vec<ZoneDoc> {
    zone_lists(a).pop().expect("a zone list").0
}

fn audition(on: bool, slice: u8) -> UiAction {
    UiAction::AuditionVelocity {
        device: key("slicer"),
        key: 24 + slice,
        velocity: 100,
        on,
    }
}

fn param_of(a: &UiAction) -> Option<(&str, f32, Option<u64>)> {
    match a {
        UiAction::Command(Command::SetParam {
            param,
            value,
            merge,
            ..
        }) => Some((param.as_str(), *value, *merge)),
        _ => None,
    }
}

fn only_slice_changed(before: &[ZoneDoc], after: &[ZoneDoc], slice: usize) {
    assert_eq!(before.len(), after.len());
    for (i, (a, b)) in before.iter().zip(after).enumerate() {
        if i == slice {
            assert_ne!(a, b, "slice {slice} should change");
        } else {
            assert_eq!(a, b, "slice {i} must not change");
        }
    }
}

// ------------------------------------------------------------------ open and select

#[test]
fn opens_with_the_first_slice_selected_and_the_markers_from_the_zones() {
    let (_, _, v) = default_setup();
    assert!(v.is_present());
    assert_eq!(v.slice_count(), 4);
    assert_eq!(v.selected_slice(), Some(0));
    assert_eq!(v.markers(), MARKERS.to_vec());
    let w = v.waveform_rect();
    assert!((v.frame_x(0) - w.x0).abs() < 1e-9);
    assert!((v.frame_x(LOOP_FRAMES) - w.x1).abs() < 1e-6);
}

#[test]
fn clicking_a_slice_selects_it_and_auditions_its_key_at_velocity_100() {
    let (d, reg, mut v) = default_setup();
    let p = v.slice_point(2).unwrap();
    let a = click(&mut v, &d, &reg, p, 1);
    assert_eq!(a, vec![audition(true, 2), audition(false, 2)]);
    assert_eq!(v.selected_slice(), Some(2));
    // The strip selects and auditions too.
    let p = v.strip_point(3).unwrap();
    let a = click(&mut v, &d, &reg, p, 1);
    assert_eq!(a, vec![audition(true, 3), audition(false, 3)]);
    assert_eq!(v.selected_slice(), Some(3));
}

#[test]
fn the_audition_note_off_is_sent_on_escape_during_the_hold() {
    let (d, reg, mut v) = default_setup();
    let p = v.slice_point(1).unwrap();
    let on = send(&mut v, &d, &reg, &down(p));
    assert_eq!(on, vec![audition(true, 1)]);
    assert!(v.is_dragging());
    let esc = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert_eq!(esc, vec![audition(false, 1)]);
    assert!(!v.is_dragging());
}

// ------------------------------------------------------------------ markers

#[test]
fn dragging_a_marker_moves_it_with_one_merge_id_and_ends_the_merge() {
    let (mut d, reg, mut v) = default_setup();
    let before = d.doc().devices[0].zones.clone();
    let p = v.marker_point(2).unwrap();
    let actions = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(40.0, 0.0)));
    let lists = zone_lists(&actions);
    assert!(!lists.is_empty());
    let id = lists[0].1.expect("a merge id");
    assert!((MERGE_FLOOR..7 << 40).contains(&id));
    assert!(lists.iter().all(|(_, m)| *m == Some(id)));
    assert_eq!(actions.last(), Some(&UiAction::EndMerge));
    assert_eq!(
        actions.iter().filter(|a| **a == UiAction::EndMerge).count(),
        1
    );
    apply_all(&mut d, &reg, &actions);
    let now = d.doc().devices[0].zones.clone();
    let markers = markers_of(&now);
    assert_eq!(markers.len(), 4, "a drag never changes the slice count");
    let expect = 48_000 + (40.0 * 96_000.0 / v.waveform_rect().width()) as u64;
    assert!(
        markers[2].abs_diff(expect) <= 100,
        "{markers:?} vs {expect}"
    );
    // Fields stay with their slice.
    assert!(now[1].reverse && !now[0].reverse && !now[2].reverse);
    assert_eq!(now[2].gain_db, -3.0);
    assert_eq!(
        (now[0].start, now[1].start, now[3].start),
        (0, 24_000, 72_000)
    );
    assert_ne!(before, now);
}

#[test]
fn a_marker_drag_stops_one_millisecond_from_its_neighbours() {
    let (d, reg, mut v) = default_setup();
    let p = v.marker_point(1).unwrap();
    let far = v.frame_x(60_000) - p.x;
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(far, 0.0)));
    let m = markers_of(&last_list(&a));
    assert_eq!(m[1], 48_000 - MIN_LEN);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(-400.0, 0.0)));
    let m = markers_of(&last_list(&a));
    assert_eq!(m[1], MIN_LEN, "never reaches the first marker");
    assert_eq!(m.len(), 4);
}

#[test]
fn the_first_marker_cannot_be_moved() {
    let (d, reg, mut v) = default_setup();
    let p = v.marker_point(0).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(60.0, 0.0)));
    assert!(zone_lists(&a).is_empty(), "{a:?}");
    assert!(!a.contains(&UiAction::EndMerge));
}

#[test]
fn escape_cancels_a_marker_drag_and_restores_the_markers() {
    let (mut d, reg, mut v) = default_setup();
    let original = d.doc().devices[0].zones.clone();
    let p = v.marker_point(3).unwrap();
    let mut actions = send(&mut v, &d, &reg, &down(p));
    actions.extend(send(&mut v, &d, &reg, &mv(p + Vec2::new(-30.0, 0.0))));
    actions.extend(send(&mut v, &d, &reg, &mv(p + Vec2::new(-60.0, 0.0))));
    apply_all(&mut d, &reg, &actions);
    assert_ne!(d.doc().devices[0].zones, original);
    assert!(v.is_dragging());
    let esc = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    let lists = zone_lists(&esc);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].0, original);
    assert!(lists[0].1.is_some_and(|m| m >= MERGE_FLOOR));
    assert_eq!(esc.last(), Some(&UiAction::EndMerge));
    assert!(!v.is_dragging());
    apply_all(&mut d, &reg, &esc);
    assert_eq!(d.doc().devices[0].zones, original);
    // The release after Esc does nothing.
    assert!(send(&mut v, &d, &reg, &up(p)).is_empty());
}

#[test]
fn separate_marker_drags_use_separate_merge_ids() {
    let (mut d, reg, mut v) = default_setup();
    let mut ids = Vec::new();
    for slice in [1, 2] {
        let p = v.marker_point(slice).unwrap();
        let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(-12.0, 0.0)));
        ids.push(zone_lists(&a)[0].1.unwrap());
        apply_all(&mut d, &reg, &a);
    }
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn double_click_on_the_waveform_adds_one_marker_in_one_step() {
    let (mut d, reg, mut v) = default_setup();
    let p = Point::new(v.frame_x(60_000), v.waveform_rect().center().y);
    let first = click(&mut v, &d, &reg, p, 1);
    assert_eq!(first, vec![audition(true, 2), audition(false, 2)]);
    let a = click(&mut v, &d, &reg, p, 2);
    let lists = zone_lists(&a);
    assert_eq!(lists.len(), 1, "{a:?}");
    assert_eq!(lists[0].1, None, "a standalone step");
    assert_eq!(lists[0].0.len(), 5);
    let m = markers_of(&lists[0].0);
    assert!(m[3].abs_diff(60_000) <= 100, "{m:?}");
    assert_eq!(v.selected_slice(), Some(3), "the new slice is selected");
    assert!(
        !a.iter()
            .any(|x| matches!(x, UiAction::AuditionVelocity { .. }))
    );
    apply_all(&mut d, &reg, &a);
    assert_eq!(d.doc().devices[0].zones.len(), 5);
}

#[test]
fn no_marker_is_added_at_64_slices() {
    let markers: Vec<u64> = (0..64).map(|i| i * 1_500).collect();
    let (d, reg, mut v) = setup(sliced_zones(&markers));
    assert_eq!(v.slice_count(), 64);
    let p = v.slice_point(10).unwrap();
    let a = click(&mut v, &d, &reg, p, 2);
    assert!(zone_lists(&a).is_empty());
}

#[test]
fn delete_merges_the_selected_slice_into_the_previous_one() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.slice_point(2).unwrap();
    let _ = click(&mut v, &d, &reg, p, 1);
    let a = send(&mut v, &d, &reg, &press(Key::Delete, Modifiers::NONE));
    let lists = zone_lists(&a);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].1, None);
    assert_eq!(markers_of(&lists[0].0), vec![0, 24_000, 72_000]);
    assert_eq!(v.selected_slice(), Some(1));
    apply_all(&mut d, &reg, &a);
    assert_eq!(d.doc().devices[0].zones.len(), 3);
    // Backspace removes too.
    let _ = send(&mut v, &d, &reg, &mv(Point::new(-9.0, -9.0)));
    let a = send(&mut v, &d, &reg, &press(Key::Backspace, Modifiers::NONE));
    assert_eq!(markers_of(&last_list(&a)), vec![0, 72_000]);
}

#[test]
fn the_first_slice_cannot_be_removed() {
    let (d, reg, mut v) = default_setup();
    assert_eq!(v.selected_slice(), Some(0));
    let a = send(&mut v, &d, &reg, &press(Key::Delete, Modifiers::NONE));
    assert!(a.is_empty());
    let a = send(&mut v, &d, &reg, &press(Key::Backspace, Modifiers::NONE));
    assert!(a.is_empty());
}

#[test]
fn ctrl_wheel_zooms_the_waveform_and_markers_stay_grabbable() {
    let (mut d, reg, mut v) = default_setup();
    let p = Point::new(v.frame_x(48_000), v.waveform_rect().center().y);
    let _ = wheel(&mut v, &d, &reg, p, 4.0, Modifiers::CTRL);
    let x = v.frame_x(48_000);
    assert!((x - p.x).abs() < 1.0, "the anchor stays under the pointer");
    assert!(v.frame_x(72_000) - v.frame_x(48_000) > 24_000.0 / 97.2 * 1.5);
    let m = v.marker_point(2).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(m, Vec2::new(10.0, 0.0)));
    apply_all(&mut d, &reg, &a);
    assert_eq!(markers_of(&d.doc().devices[0].zones).len(), 4);
    // Plain wheel scrolls.
    let before = v.frame_x(48_000);
    let _ = wheel(&mut v, &d, &reg, p, -3.0, Modifiers::NONE);
    assert_ne!(v.frame_x(48_000), before);
}

// ------------------------------------------------------------------ per-slice fields

#[test]
fn the_fields_edit_only_the_selected_slice() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.slice_point(1).unwrap();
    let _ = click(&mut v, &d, &reg, p, 1);
    let before = d.doc().devices[0].zones.clone();

    let p = v.field_center(Field::Gain);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -20.0)));
    let lists = zone_lists(&a);
    let id = lists[0].1.expect("merge id");
    assert!(id >= MERGE_FLOOR);
    assert!(lists.iter().all(|(_, m)| *m == Some(id)));
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    only_slice_changed(&before, &last_list(&a), 1);
    assert!(last_list(&a)[1].gain_db > 0.0);
    apply_all(&mut d, &reg, &a);

    let before = d.doc().devices[0].zones.clone();
    let p = v.field_center(Field::Pan);
    let a = wheel(&mut v, &d, &reg, p, -1.0, Modifiers::NONE);
    only_slice_changed(&before, &last_list(&a), 1);
    assert!(last_list(&a)[1].pan != 0.0);
    apply_all(&mut d, &reg, &a);

    let before = d.doc().devices[0].zones.clone();
    let p = v.field_center(Field::Transpose);
    let a = wheel(&mut v, &d, &reg, p, 2.0, Modifiers::NONE);
    only_slice_changed(&before, &last_list(&a), 1);
    assert_eq!(last_list(&a)[1].transpose, 2);
    apply_all(&mut d, &reg, &a);

    let before = d.doc().devices[0].zones.clone();
    let p = v.field_center(Field::Tune);
    let a = wheel(&mut v, &d, &reg, p, 5.0, Modifiers::NONE);
    only_slice_changed(&before, &last_list(&a), 1);
    assert_eq!(last_list(&a)[1].tune_cents, 5.0);
    apply_all(&mut d, &reg, &a);

    // Double-click resets the field.
    let before = d.doc().devices[0].zones.clone();
    let p = v.field_center(Field::Transpose);
    let a = click(&mut v, &d, &reg, p, 2);
    only_slice_changed(&before, &last_list(&a), 1);
    assert_eq!(last_list(&a)[1].transpose, 0);
    apply_all(&mut d, &reg, &a);
    // The slice boundaries never changed.
    assert_eq!(markers_of(&d.doc().devices[0].zones), MARKERS.to_vec());
}

#[test]
fn field_values_are_clamped_valid() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.field_center(Field::Gain);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -2000.0)));
    assert_eq!(last_list(&a)[0].gain_db, 12.0);
    apply_all(&mut d, &reg, &a);
    let p = v.field_center(Field::Pan);
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, 2000.0)));
    assert_eq!(last_list(&a)[0].pan, -1.0);
    apply_all(&mut d, &reg, &a);
}

#[test]
fn the_reverse_button_toggles_the_selected_slice() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.slice_point(3).unwrap();
    let _ = click(&mut v, &d, &reg, p, 1);
    let before = d.doc().devices[0].zones.clone();
    let p = v.reverse_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    let after = last_list(&a);
    only_slice_changed(&before, &after, 3);
    assert!(after[3].reverse);
    assert_eq!(zone_lists(&a)[0].1, None);
    apply_all(&mut d, &reg, &a);
    // And back (slice 2 is reversed already: toggling clears it).
    let p = v.slice_point(1).unwrap();
    let _ = click(&mut v, &d, &reg, p, 1);
    let p = v.reverse_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    assert!(!last_list(&a)[1].reverse);
}

#[test]
fn field_edits_are_ignored_without_a_selection() {
    let (d, reg, mut v) = default_setup();
    let _ = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert_eq!(v.selected_slice(), None);
    let p = v.reverse_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    assert!(a.is_empty());
}

// ------------------------------------------------------------------ toolbar

fn slice_loop(mode: SliceMode) -> UiAction {
    UiAction::SliceLoop {
        device: key("slicer"),
        mode,
    }
}

#[test]
fn the_slice_button_emits_transients_with_the_sensitivity() {
    let (d, reg, mut v) = default_setup();
    assert_eq!(v.sensitivity(), 50);
    let p = v.slice_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    assert_eq!(
        a,
        vec![slice_loop(SliceMode::Transients { sensitivity: 50 })]
    );
    // Drag the sensitivity field up by 25 px: +20 %.
    let p = v.sensitivity_center();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -25.0)));
    assert!(a.is_empty(), "the sensitivity is view state: {a:?}");
    assert_eq!(v.sensitivity(), 70);
    let b = v.slice_button_center();
    let a = click(&mut v, &d, &reg, b, 1);
    assert_eq!(
        a,
        vec![slice_loop(SliceMode::Transients { sensitivity: 70 })]
    );
    // Limits and reset.
    let _ = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -500.0)));
    assert_eq!(v.sensitivity(), 100);
    let _ = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, 500.0)));
    assert_eq!(v.sensitivity(), 0);
    let _ = click(&mut v, &d, &reg, p, 2);
    assert_eq!(v.sensitivity(), 50);
}

#[test]
fn escape_restores_the_sensitivity_of_a_drag() {
    let (d, reg, mut v) = default_setup();
    let p = v.sensitivity_center();
    let _ = send(&mut v, &d, &reg, &down(p));
    let _ = send(&mut v, &d, &reg, &mv(p + Vec2::new(0.0, -30.0)));
    assert_ne!(v.sensitivity(), 50);
    let a = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert!(a.is_empty());
    assert_eq!(v.sensitivity(), 50);
}

#[test]
fn the_grid_mode_and_divisions_set_the_slice_mode() {
    let (d, reg, mut v) = default_setup();
    let p = v.mode_button_center(true);
    let _ = click(&mut v, &d, &reg, p, 1);
    assert_eq!(v.slice_mode(), SliceMode::Grid { division: 4 });
    for div in [1, 2, 4, 8] {
        let p = v.division_button_center(div).unwrap();
        let _ = click(&mut v, &d, &reg, p, 1);
        let b = v.slice_button_center();
        let a = click(&mut v, &d, &reg, b, 1);
        assert_eq!(a, vec![slice_loop(SliceMode::Grid { division: div })]);
    }
    assert!(v.division_button_center(3).is_none());
    let p = v.mode_button_center(false);
    let _ = click(&mut v, &d, &reg, p, 1);
    assert_eq!(v.slice_mode(), SliceMode::Transients { sensitivity: 50 });
    // A division button also selects the grid mode.
    let p = v.division_button_center(2).unwrap();
    let _ = click(&mut v, &d, &reg, p, 1);
    assert_eq!(v.slice_mode(), SliceMode::Grid { division: 2 });
}

#[test]
fn to_track_emits_slices_to_track() {
    let (d, reg, mut v) = default_setup();
    let p = v.to_track_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    assert_eq!(
        a,
        vec![UiAction::SlicesToTrack {
            device: key("slicer")
        }]
    );
}

#[test]
fn slice_and_to_track_do_nothing_without_a_loop() {
    let (d, reg, mut v) = setup(Vec::new());
    let p = v.slice_button_center();
    assert!(click(&mut v, &d, &reg, p, 1).is_empty());
    let p = v.to_track_button_center();
    assert!(click(&mut v, &d, &reg, p, 1).is_empty());
}

#[test]
fn the_beats_field_edits_loop_beats_with_set_param() {
    let (mut d, reg, mut v) = default_setup();
    assert_eq!(v.beats(), 4);
    let p = v.beats_center();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -8.0)));
    let params: Vec<_> = a.iter().filter_map(param_of).collect();
    assert!(!params.is_empty());
    let id = params[0].2.expect("merge id");
    assert!(id >= MERGE_FLOOR);
    assert!(
        params
            .iter()
            .all(|(k, _, m)| *k == "loop.beats" && *m == Some(id))
    );
    assert_eq!(params.last().unwrap().1, 6.0);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    apply_all(&mut d, &reg, &a);
    assert_eq!(d.doc().devices[0].params["loop.beats"], 6.0);
    let _ = send(&mut v, &d, &reg, &mv(Point::new(-9.0, -9.0)));
    assert_eq!(v.beats(), 6);

    let a = wheel(&mut v, &d, &reg, p, 1.0, Modifiers::NONE);
    assert_eq!(
        a.iter().filter_map(param_of).collect::<Vec<_>>(),
        vec![("loop.beats", 7.0, None)]
    );
    apply_all(&mut d, &reg, &a);

    let a = click(&mut v, &d, &reg, p, 2);
    assert_eq!(
        a.iter().filter_map(param_of).collect::<Vec<_>>(),
        vec![("loop.beats", 4.0, None)]
    );

    // Limits.
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(0.0, -2000.0)));
    assert_eq!(a.iter().filter_map(param_of).next_back().unwrap().1, 64.0);
}

#[test]
fn escape_restores_loop_beats_of_a_drag() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.beats_center();
    let mut a = send(&mut v, &d, &reg, &down(p));
    a.extend(send(&mut v, &d, &reg, &mv(p + Vec2::new(0.0, -12.0))));
    apply_all(&mut d, &reg, &a);
    assert_eq!(d.doc().devices[0].params["loop.beats"], 7.0);
    let esc = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert_eq!(esc.iter().filter_map(param_of).next().unwrap().1, 4.0);
    assert_eq!(esc.last(), Some(&UiAction::EndMerge));
    apply_all(&mut d, &reg, &esc);
    assert_eq!(d.doc().devices[0].params["loop.beats"], 4.0);
}

// ------------------------------------------------------------------ drops, load, empty state

#[test]
fn dropped_files_import_into_the_slicer_without_a_key() {
    let (d, reg, mut v) = default_setup();
    let pos = Point::new(400.0, 300.0);
    let hover = UiEvent::FileHover {
        pos,
        paths: vec!["a.wav".into(), "b.wav".into()],
    };
    assert!(send(&mut v, &d, &reg, &hover).is_empty());
    assert_eq!(v.drop_hover(), Some(2));
    assert!(v.needs_repaint());
    let drop = UiEvent::FileDrop {
        pos,
        paths: vec![PathBuf::from("a.wav")],
    };
    let a = send(&mut v, &d, &reg, &drop);
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths: vec![PathBuf::from("a.wav")],
            device: key("slicer"),
            at_key: None
        }]
    );
    assert_eq!(v.drop_hover(), None);
    // Outside the bounds, empty drops and cancelled hovers do nothing.
    let out = UiEvent::FileDrop {
        pos: Point::new(1500.0, 10.0),
        paths: vec!["a.wav".into()],
    };
    assert!(send(&mut v, &d, &reg, &out).is_empty());
    let _ = send(&mut v, &d, &reg, &hover);
    let _ = send(&mut v, &d, &reg, &UiEvent::FileHoverCancel);
    assert_eq!(v.drop_hover(), None);
}

#[test]
fn the_load_buttons_ask_for_the_file_dialog() {
    let load = vec![UiAction::LoadSamplesDialog {
        device: key("slicer"),
    }];
    let (d, reg, mut v) = default_setup();
    let p = v.load_button_center();
    assert_eq!(click(&mut v, &d, &reg, p, 1), load);
    let (d, reg, mut v) = setup(Vec::new());
    let p = v.empty_load_center();
    assert_eq!(click(&mut v, &d, &reg, p, 1), load);
}

#[test]
fn the_empty_state_invites_a_drop() {
    let (d, reg, mut v) = setup(Vec::new());
    assert!(v.is_present());
    assert_eq!(v.slice_count(), 0);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|s| s == "Drop a loop here"), "{t:?}");
    assert!(t.iter().any(|s| s == "LOAD..."));
    // Drops work in the empty state.
    let drop = UiEvent::FileDrop {
        pos: Point::new(300.0, 300.0),
        paths: vec!["loop.wav".into()],
    };
    assert_eq!(send(&mut v, &d, &reg, &drop).len(), 1);
    // Clicks on the empty pane do nothing.
    assert!(click(&mut v, &d, &reg, Point::new(100.0, 200.0), 2).is_empty());
}

#[test]
fn the_view_shows_slice_numbers_and_note_names() {
    let (d, reg, mut v) = default_setup();
    let t = texts(&mut v, &d, &reg);
    for s in [
        "1",
        "2",
        "3",
        "4",
        "C1",
        "C#1",
        "D1",
        "D#1",
        "R",
        "SLICE",
        "TO TRACK",
        "TRANSIENTS",
        "GRID",
        "REVERSE",
        "LEVEL",
        "PAN",
        "PITCH",
        "TUNE",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?} in {t:?}");
    }
    assert!(t.iter().any(|x| x == "1/16") && t.iter().any(|x| x == "50 %"));
    assert!(
        t.iter()
            .any(|x| x.contains("4 slices") && x.contains("amen_loop.wav"))
    );
}

#[test]
fn a_missing_asset_is_reported() {
    let (d, reg, mut v) = default_setup();
    v.set_missing_assets([LOOP.to_string()]);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|s| s.starts_with("Missing asset")), "{t:?}");
    v.set_missing_assets(Vec::new());
    v.clear_peaks(LOOP);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|s| s == "no waveform"));
}

// ------------------------------------------------------------------ open, close, undo

#[test]
fn close_button_and_escape_close_the_editor() {
    let (d, reg, mut v) = default_setup();
    let p = v.close_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    assert_eq!(a, vec![UiAction::CloseSliceEditor]);
    // Esc deselects first, then closes.
    let a = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert!(a.is_empty());
    assert_eq!(v.selected_slice(), None);
    let a = send(&mut v, &d, &reg, &press(Key::Escape, Modifiers::NONE));
    assert_eq!(a, vec![UiAction::CloseSliceEditor]);
}

#[test]
fn undo_and_redo_shortcuts_are_forwarded() {
    let (d, reg, mut v) = default_setup();
    let ctrl = Modifiers::CTRL;
    assert_eq!(
        send(&mut v, &d, &reg, &press(Key::Char('z'), ctrl)),
        vec![UiAction::Undo]
    );
    let sc = Modifiers {
        shift: true,
        ..ctrl
    };
    assert_eq!(
        send(&mut v, &d, &reg, &press(Key::Char('z'), sc)),
        vec![UiAction::Redo]
    );
    assert_eq!(
        send(&mut v, &d, &reg, &press(Key::Char('y'), ctrl)),
        vec![UiAction::Redo]
    );
}

#[test]
fn the_view_follows_the_document_and_goes_away_with_the_device() {
    let (mut d, reg, mut v) = default_setup();
    let p = v.marker_point(1).unwrap();
    let a = gesture(&mut v, &d, &reg, &drag_by(p, Vec2::new(30.0, 0.0)));
    apply_all(&mut d, &reg, &a);
    d.undo(&reg);
    v.sync(d.doc(), &reg);
    assert_eq!(v.markers(), MARKERS.to_vec());
    d.apply(Command::RemoveDevice { key: key("slicer") }, &reg)
        .unwrap();
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
    assert!(send(&mut v, &d, &reg, &down(Point::new(300.0, 300.0))).is_empty());
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|s| s == "No loop slicer open"));
}

#[test]
fn a_sampler_is_not_a_slicer() {
    let mut doc = slicer_doc(sliced_zones(&MARKERS));
    doc.devices.push(device("smp", "atomik.sampler", "Sampler"));
    let reg = registry();
    let mut v = SliceEditorView::new(atomik_ui::Theme::dark());
    v.open(key("smp"));
    v.sync(&doc, &reg);
    assert!(!v.is_present());
}

#[test]
fn zones_on_other_keys_are_not_slices_and_are_kept_by_field_edits() {
    let mut zones = sliced_zones(&MARKERS);
    let mut stray = ZoneDoc::new(LOOP, LOOP_FRAMES);
    stray.key_lo = 100;
    stray.key_hi = 100;
    zones.push(stray.clone());
    let (mut d, reg, mut v) = setup(zones);
    assert_eq!(v.slice_count(), 4);
    let p = v.reverse_button_center();
    let a = click(&mut v, &d, &reg, p, 1);
    let list = last_list(&a);
    assert_eq!(list.len(), 5);
    assert_eq!(list[4], stray);
    assert!(list[0].reverse);
    apply_all(&mut d, &reg, &a);
}

// ------------------------------------------------------------------ random gestures

#[derive(Debug, Clone)]
enum Step {
    Click(f64, f64, u8),
    Drag(f64, f64, f64, f64),
    DragMarker(usize, f64, f64),
    Key(u8),
    Wheel(f64, f64, f64, bool),
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    let pt = (0.0..1000.0f64, 0.0..560.0f64);
    let step = prop_oneof![
        (pt.clone(), 1u8..=2).prop_map(|((x, y), c)| Step::Click(x, y, c)),
        (pt.clone(), pt.clone()).prop_map(|((a, b), (c, d))| Step::Drag(a, b, c, d)),
        (0usize..8, -300.0..300.0f64, -50.0..50.0f64)
            .prop_map(|(i, dx, dy)| Step::DragMarker(i, dx, dy)),
        (0u8..3).prop_map(Step::Key),
        (pt, -6.0..6.0f64, proptest::bool::ANY).prop_map(|((x, y), d, z)| Step::Wheel(x, y, d, z)),
    ];
    proptest::collection::vec(step, 1..24)
}

fn run_step(v: &mut SliceEditorView, d: &mut Document, reg: &Registry, s: &Step) {
    let events: Vec<UiEvent> = match s {
        Step::Click(x, y, c) => {
            let p = Point::new(*x, *y);
            vec![down_mods(p, Modifiers::NONE, *c), up(p)]
        }
        Step::Drag(a, b, c, e) => {
            let (p, q) = (Point::new(*a, *b), Point::new(*c, *e));
            vec![down(p), mv(p.lerp(q, 0.5)), mv(q), up(q)]
        }
        Step::DragMarker(i, dx, dy) => {
            let Some(p) = v.marker_point(*i) else { return };
            let q = p + Vec2::new(*dx, *dy);
            vec![down(p), mv(p.lerp(q, 0.5)), mv(q), up(q)]
        }
        Step::Key(k) => {
            let key = [Key::Delete, Key::Escape, Key::Backspace][usize::from(*k)].clone();
            vec![press(key, Modifiers::NONE)]
        }
        Step::Wheel(x, y, dy, zoom) => vec![UiEvent::Wheel {
            delta: Vec2::new(0.0, *dy),
            pos: Point::new(*x, *y),
            modifiers: if *zoom {
                Modifiers::CTRL
            } else {
                Modifiers::NONE
            },
        }],
    };
    for e in &events {
        let actions = v.handle(e, d.doc(), reg);
        for a in &actions {
            if let UiAction::Command(c @ Command::SetZones { merge, .. }) = a {
                if let Some(m) = merge {
                    assert!(*m >= MERGE_FLOOR);
                }
                d.apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            if let UiAction::Command(c @ Command::SetParam { .. }) = a {
                d.apply(c.clone(), reg).unwrap();
            }
            if matches!(a, UiAction::EndMerge) {
                d.end_merge();
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_gestures_never_emit_an_invalid_list(ss in steps()) {
        let (mut d, reg, mut v) = default_setup();
        for s in &ss {
            run_step(&mut v, &mut d, &reg, s);
        }
        let zones = d.doc().devices[0].zones.clone();
        let mut fresh = Document::new(d.doc().clone());
        let cmd = Command::SetZones { device: key("slicer"), zones: zones.clone(), merge: None };
        prop_assert!(fresh.apply(cmd, &reg).is_ok());
        // Always contiguous slices from frame 0 to the end.
        let m = markers_of(&zones);
        prop_assert_eq!(m.first().copied(), Some(0));
        prop_assert!(m.windows(2).all(|w| w[0] < w[1]));
        prop_assert_eq!(zones.last().map(|z| z.end), Some(LOOP_FRAMES));
    }
}
