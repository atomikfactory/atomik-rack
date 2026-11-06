//! The scale-selection role (Ctrl+Alt+Shift by default) on a clip's end edge scales the whole
//! selection about its earliest start, and on a start edge about its latest end: the gesture
//! emits trial steps (`ScaleStep`) that the host applies without an undo step, the release keeps
//! the last one (`StretchEnd`), going back to the start drops it. Alt alone keeps scaling each
//! clip about its own fixed edge.

mod support;

use atomik_model::ClipId;
use atomik_ui::keymap::{Os, Roles};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, CursorHint, Modifiers, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_snap(Snap::Bar);
    (v, doc, reg)
}

fn scale() -> Modifiers {
    Modifiers {
        shift: true,
        ctrl: true,
        alt: true,
        meta: false,
    }
}

fn hold(
    v: &mut ArrangementView,
    m: Modifiers,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) {
    let _ = v.handle(&UiEvent::ModifiersChanged { modifiers: m }, doc, reg);
}

fn bar_px(v: &ArrangementView) -> f64 {
    v.tick_to_x(BAR) - v.tick_to_x(0)
}

fn right_edge(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x1 - 2.0, r.center().y + 6.0)
}

fn left_edge(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x0 + 2.0, r.center().y + 6.0)
}

fn scale_steps(actions: &[UiAction]) -> Vec<(Vec<ClipId>, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::ScaleStep {
                ids,
                percent,
                from_end: false,
            } => Some((ids.clone(), *percent)),
            _ => None,
        })
        .collect()
}

/// The steps of a drag of a start edge: the clips, the tempo and that it is about the end.
fn end_steps(actions: &[UiAction]) -> Vec<(Vec<ClipId>, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::ScaleStep {
                ids,
                percent,
                from_end: true,
            } => Some((ids.clone(), *percent)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_selection_scales_about_its_earliest_start_not_about_the_grabbed_clip() {
    let (mut v, doc, reg) = setup();
    // Clip 3 spans bars 0..4, clip 2 spans bars 1..4: the selection starts at bar 0.
    v.set_selected_clips(&[ClipId(2), ClipId(3)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 2);
    // Clip 2's end one bar out is bar 5 of the selection's five-bar span from bar 0 over the
    // four it was: 5/4 as long, a tempo of 80 per cent (each clip about its own edge would
    // have given 4/3 for this clip).
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), scale()),
        ],
    );
    let got = scale_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert_eq!(got[0].0, vec![ClipId(2), ClipId(3)]);
    assert!((got[0].1 - 80.0).abs() < 1e-9, "{}", got[0].1);
    // The host previews it: no resize command, no stretch step.
    assert!(song_commands(&a).is_empty());
    assert!(a.iter().all(|x| !matches!(x, UiAction::StretchStep { .. })));
    assert_eq!(v.cursor(), CursorHint::Stretch);
    let a = v.handle(&up(p), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: true }]);
}

#[test]
fn a_single_clip_scales_about_its_own_start() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p + Vec2::new(bp * 3.9, 0.0), scale()),
        ],
    );
    let got = scale_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert!((got[0].1 - 50.0).abs() < 1e-9, "{}", got[0].1);
}

#[test]
fn pressing_an_unselected_clip_scales_that_clip_alone() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 3);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p + Vec2::new(bp * 3.9, 0.0), scale()),
        ],
    );
    let got = scale_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert_eq!(got[0].0, vec![ClipId(3)]);
}

#[test]
fn going_back_to_the_start_length_drops_the_preview_and_the_release_sends_nothing() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1), ClipId(3)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), scale()),
        ],
    );
    assert_eq!(scale_steps(&a).len(), 1);
    let a = v.handle(&mv_with(p, scale()), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: false }]);
    let a = v.handle(&up(p), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn a_clip_is_never_scaled_shorter_than_the_grid_allows() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1), ClipId(3)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    // Dragging the end far to the left of the start keeps the shortest clip at one grid step,
    // inside the tempo range the model accepts.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p - Vec2::new(bp * 30.0, 0.0), scale()),
        ],
    );
    let got = scale_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert!(got[0].1 <= atomik_model::SCALE_PERCENT_MAX);
    assert!(got[0].1 > 100.0);
}

#[test]
fn alt_alone_still_stretches_each_clip_about_its_own_fixed_edge() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(2), ClipId(3)]);
    hold(&mut v, alt(), &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 2);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), alt()),
        ],
    );
    assert!(scale_steps(&a).is_empty(), "{a:?}");
    let stretched: Vec<f64> = a
        .iter()
        .filter_map(|x| match x {
            UiAction::StretchStep { percent, .. } => Some(*percent),
            _ => None,
        })
        .collect();
    // Clip 2 is three bars long: one more is 4/3, a tempo of 75 per cent.
    assert_eq!(stretched.len(), 1, "{a:?}");
    assert!((stretched[0] - 75.0).abs() < 1e-9, "{}", stretched[0]);
}

#[test]
fn the_start_edge_scales_the_selection_about_its_latest_end() {
    let (mut v, doc, reg) = setup();
    // Clip 3 spans bars 0..4, clip 2 spans bars 1..4: the selection ends at bar 4.
    v.set_selected_clips(&[ClipId(2), ClipId(3)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = left_edge(&v, 2);
    // Clip 2's start one bar in is bar 2 of the selection's two bars to the end over the three
    // it was from bar 1: 2/3 as long, a tempo of 150 per cent.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), scale()),
        ],
    );
    let got = end_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert_eq!(got[0].0, vec![ClipId(2), ClipId(3)]);
    assert!((got[0].1 - 150.0).abs() < 1e-9, "{}", got[0].1);
    assert!(scale_steps(&a).is_empty(), "not about the start");
    assert!(song_commands(&a).is_empty());
    assert_eq!(v.cursor(), CursorHint::Stretch);
    let a = v.handle(&up(p), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: true }]);
}

#[test]
fn the_start_edge_never_scales_a_clip_before_tick_zero() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(2), ClipId(3)]);
    hold(&mut v, scale(), &doc, &reg);
    let bp = bar_px(&v);
    let p = left_edge(&v, 2);
    // Clip 3 already starts at 0: pulling the edge out to the left changes nothing.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p - Vec2::new(bp * 0.9, 0.0), scale()),
        ],
    );
    assert!(end_steps(&a).is_empty(), "{a:?}");
    // A single clip that starts later can grow to the left up to tick 0.
    v.set_selected_clips(&[ClipId(2)]);
    let _ = v.handle(&up(p), &doc, &reg);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, scale(), 1),
            mv_with(p - Vec2::new(bp * 5.0, 0.0), scale()),
        ],
    );
    let got = end_steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    // Bars 0..4 over bars 1..4: 4/3 as long.
    assert!((got[0].1 - 75.0).abs() < 1e-9, "{}", got[0].1);
}

#[test]
fn the_role_needs_the_select_tool_and_an_edge() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1)]);
    hold(&mut v, scale(), &doc, &reg);
    let r = v.clip_rect(ClipId(1)).unwrap();
    // The body is not a handle.
    let body = Point::new(r.center().x, r.center().y + 6.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(body, scale(), 1),
            mv_with(body + Vec2::new(20.0, 0.0), scale()),
        ],
    );
    assert!(
        scale_steps(&a).is_empty() && end_steps(&a).is_empty(),
        "{a:?}"
    );
    let _ = v.handle(&up(body), &doc, &reg);
}
