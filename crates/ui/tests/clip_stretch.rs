//! Alt+drag on a clip's start or end edge stretches the clip: the gesture emits trial steps
//! (`StretchStep`) that the host applies without an undo step, the release keeps the last one
//! (`StretchEnd`), Escape drops it. The body keeps the alternate tool.

mod support;

use atomik_model::ClipId;
use atomik_ui::keymap::{Os, Roles};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, CursorHint, Key, Modifiers, UiAction, UiEvent};
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

fn hold_alt(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) {
    let _ = v.handle(&UiEvent::ModifiersChanged { modifiers: alt() }, doc, reg);
}

fn steps(actions: &[UiAction]) -> Vec<(Vec<ClipId>, f64, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::StretchStep {
                ids,
                percent,
                from_start,
            } => Some((ids.clone(), *percent, *from_start)),
            _ => None,
        })
        .collect()
}

#[test]
fn alt_dragging_the_end_edge_stretches_by_the_ratio_and_the_release_keeps_it() {
    let (mut v, doc, reg) = setup();
    hold_alt(&mut v, &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    // Clip 1 is four bars: one bar more is 5/4 as long, a tempo of 80 per cent.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), alt()),
        ],
    );
    let got = steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert_eq!(got[0].0, vec![ClipId(1)]);
    assert!((got[0].1 - 80.0).abs() < 1e-9, "{}", got[0].1);
    assert!(!got[0].2);
    // No ordinary resize command: the host previews the stretch.
    assert!(song_commands(&a).is_empty());
    assert_eq!(v.cursor(), CursorHint::Stretch);
    // The release accepts it as one step.
    let a = v.handle(&up(p), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: true }]);
}

#[test]
fn alt_dragging_the_start_edge_keeps_the_end_and_cannot_pass_tick_zero() {
    let (mut v, doc, reg) = setup();
    hold_alt(&mut v, &doc, &reg);
    let bp = bar_px(&v);
    let p = left_edge(&v, 1);
    // Four bars from tick 0, the start edge one bar to the right: three bars, 4/3 the tempo.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), alt()),
        ],
    );
    let got = steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert!(got[0].2, "the start edge is the grabbed one");
    assert!((got[0].1 - 100.0 * 4.0 / 3.0).abs() < 1e-6, "{}", got[0].1);
    // Past the original start would put the start before tick 0: held at the original.
    let a = v.handle(&mv_with(p - Vec2::new(bp, 0.0), alt()), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: false }]);
    let a = v.handle(&up(p), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn every_selected_clip_stretches_by_the_same_ratio() {
    let (mut v, doc, reg) = setup();
    v.set_selected_clips(&[ClipId(1), ClipId(3)]);
    hold_alt(&mut v, &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 3.9, 0.0), alt()),
        ],
    );
    let got = steps(&a);
    assert_eq!(got.len(), 1, "{a:?}");
    assert_eq!(got[0].0, vec![ClipId(1), ClipId(3)]);
    // Eight bars over four: twice as long.
    assert!((got[0].1 - 50.0).abs() < 1e-9, "{}", got[0].1);
}

#[test]
fn escape_drops_the_preview_and_going_back_to_the_start_length_drops_it_too() {
    let (mut v, doc, reg) = setup();
    hold_alt(&mut v, &doc, &reg);
    let bp = bar_px(&v);
    let p = right_edge(&v, 1);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), alt()),
        ],
    );
    assert_eq!(steps(&a).len(), 1);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(a, vec![UiAction::StretchEnd { commit: false }]);
    // After the release nothing is open: no second end.
    let a = v.handle(&up(p), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
    // Out and back: the preview is dropped and the release has nothing to keep.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(p, alt(), 1),
            mv_with(p + Vec2::new(bp * 1.1, 0.0), alt()),
            mv_with(p, alt()),
            up(p),
        ],
    );
    assert_eq!(steps(&a).len(), 1);
    assert_eq!(
        a.iter()
            .filter(|x| matches!(x, UiAction::StretchEnd { .. }))
            .collect::<Vec<_>>(),
        vec![&UiAction::StretchEnd { commit: false }]
    );
}

#[test]
fn the_edge_shows_the_stretch_cursor_only_with_alt_and_the_body_keeps_the_pencil() {
    let (mut v, doc, reg) = setup();
    let p = right_edge(&v, 1);
    let _ = v.handle(&mv(p), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
    hold_alt(&mut v, &doc, &reg);
    let _ = v.handle(&mv_with(p, alt()), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Stretch);
    // Alt on the body: the pencil, and a press there does not stretch.
    let r = v.clip_rect(ClipId(1)).unwrap();
    let body = Point::new(r.center().x, r.y1 - 6.0);
    let one_bar = bar_px(&v);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            mv_with(body, alt()),
            down_mods(body, alt(), 1),
            mv_with(body + Vec2::new(one_bar, 0.0), alt()),
            up(body),
        ],
    );
    assert!(steps(&a).is_empty(), "{a:?}");
    assert_eq!(v.cursor(), CursorHint::Pen);
}
