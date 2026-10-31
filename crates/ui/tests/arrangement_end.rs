//! The song End in the arrangement: its flag in the ruler (drag, snap), the shading past it in
//! the lanes, the command that sets it at the playhead and the ruler's context-menu entry.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{RackDoc, SongCommand};
use atomik_ui::action::ContextTarget;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, CursorHint, DisplayList, Modifiers, TransportAction, UiAction, UiEvent,
};
use support::song::*;
use support::*;

/// Ctrl+Alt: the disable-snap role of the standard roles.
const NO_SNAP: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: true,
    meta: false,
};

/// The song with the End at bar 7 (tick `6 * BAR`) and the loop on bars 2 to 4.
fn setup(snap: Snap) -> (RackDoc, Registry, ArrangementView) {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song_mut().end = 6 * BAR;
    doc.song_mut().loop_range = (BAR, 4 * BAR);
    let mut v = arrangement(&doc, &reg);
    v.set_snap(snap);
    (doc, reg, v)
}

fn send(
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Registry,
    events: &[UiEvent],
) -> Vec<UiAction> {
    run(|e| v.handle(e, doc, reg), events)
}

fn ends(actions: &[UiAction]) -> Vec<i64> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetSongEnd(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn loops(actions: &[UiAction]) -> usize {
    song_commands(actions)
        .iter()
        .filter(|c| matches!(c, SongCommand::SetLoop { .. }))
        .count()
}

/// A point on the End flag of `v`, `dx` pixels right of the End line at `end` (the flag hangs
/// to the right of its line).
fn flag(v: &ArrangementView, end: i64, dx: f64) -> Point {
    Point::new(v.tick_to_x(end) + dx, v.end_flag_y())
}

/// Whether a fill of the End shade starts at x `x` in `dl`.
fn shaded_from(dl: &DisplayList, x: f64) -> bool {
    let at = format!("[{x:.2},");
    dl.to_json().lines().any(|l| {
        l.starts_with("{\"fill\":{\"rect\":")
            && l.contains(&at)
            && l.contains("\"brush\":\"#0000004d\"")
    })
}

#[test]
fn the_flag_is_drawn_in_the_ruler_as_an_e() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert_eq!(dl.texts().iter().filter(|t| **t == "E").count(), 1);
}

#[test]
fn the_area_past_the_end_is_shaded_in_the_lanes() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(
        shaded_from(&dl, v.tick_to_x(6 * BAR)),
        "a shade starts at the End"
    );
    // Dragged, the shade follows the pointer before anything is committed.
    let p = flag(&v, 6 * BAR, 4.0);
    let to = Point::new(v.tick_to_x(9 * BAR), p.y);
    send(&mut v, &doc, &reg, &[down(p), mv(to)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(shaded_from(&dl, v.tick_to_x(9 * BAR)));
    assert!(!shaded_from(&dl, v.tick_to_x(6 * BAR)));
}

#[test]
fn dragging_the_flag_sets_the_snapped_end_once_on_release() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = flag(&v, 6 * BAR, 4.0);
    let at = |t: i64| Point::new(v.tick_to_x(t) + 3.0, p.y);
    let events = [
        down(p),
        mv(at(8 * BAR + PPQ)),
        mv(at(10 * BAR + PPQ)),
        up(at(10 * BAR + PPQ)),
    ];
    let a = send(&mut v, &doc, &reg, &events);
    assert_eq!(ends(&a), vec![10 * BAR], "{a:?}");
    assert_eq!(song_commands(&a).len(), 1, "one command for the drag");
}

#[test]
fn the_line_below_the_flag_grabs_the_end_too() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = Point::new(v.tick_to_x(6 * BAR), v.loop_band_y() + 4.0);
    let to = Point::new(v.tick_to_x(12 * BAR), p.y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(ends(&a), vec![12 * BAR], "{a:?}");
}

#[test]
fn with_the_disable_snap_role_the_end_is_the_free_tick() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = flag(&v, 6 * BAR, 4.0);
    let to = Point::new(v.tick_to_x(9 * BAR + 100), p.y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv_with(to, NO_SNAP), up(to)]);
    let t = ends(&a)[0];
    assert!((t - (9 * BAR + 100)).abs() <= 1, "{t}");
}

#[test]
fn the_end_never_goes_to_zero() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = flag(&v, 6 * BAR, 4.0);
    let to = Point::new(v.tick_to_x(0) - 30.0, p.y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to), up(to)]);
    let t = ends(&a)[0];
    assert!(t > 0 && t <= BAR, "{t}");
}

#[test]
fn clicking_the_flag_without_moving_changes_nothing() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = flag(&v, 6 * BAR, 4.0);
    let a = send(&mut v, &doc, &reg, &[down(p), up(p)]);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn the_flag_wins_over_the_line_of_a_locator_and_the_band_below_keeps_the_locator() {
    // The right locator's line is at the End: the End's flag hangs on the other side of it.
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().end = 4 * BAR;
    v.sync(&doc, &reg);
    let x = v.tick_to_x(4 * BAR);
    let drag = |v: &mut ArrangementView, from: Point| {
        let to = Point::new(x + 2.0 * (v.tick_to_x(BAR) - v.tick_to_x(0)), from.y);
        send(v, &doc, &reg, &[down(from), mv(to), up(to)])
    };
    let on_flag = flag(&v, 4 * BAR, 4.0);
    let a = drag(&mut v, on_flag);
    assert_eq!((ends(&a).len(), loops(&a)), (1, 0), "{a:?}");
    let on_band = Point::new(x, v.loop_band_y() + 4.0);
    let a = drag(&mut v, on_band);
    assert_eq!((ends(&a).len(), loops(&a)), (0, 1), "{a:?}");
}

#[test]
fn the_cursor_is_the_horizontal_arrows_over_the_flag() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = flag(&v, 6 * BAR, 4.0);
    send(&mut v, &doc, &reg, &[mv(p)]);
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
}

fn command(v: &mut ArrangementView, doc: &RackDoc, id: &str) -> CommandOutcome {
    v.command(id, None, doc, &registry())
}

fn handled_ends(outcome: CommandOutcome) -> Vec<i64> {
    match outcome {
        CommandOutcome::Handled(a) => ends(&a),
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_end_marker_at_playhead_sets_the_playhead_tick() {
    let (doc, _, mut v) = setup(Snap::Bar);
    v.set_playhead(9 * BAR + 7);
    let out = command(&mut v, &doc, "transport.set_end_marker");
    assert_eq!(handled_ends(out), vec![9 * BAR + 7]);
}

#[test]
fn set_end_marker_at_playhead_declines_at_zero_and_where_it_already_is() {
    let (doc, _, mut v) = setup(Snap::Bar);
    v.set_playhead(0);
    assert!(matches!(
        command(&mut v, &doc, "transport.set_end_marker"),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_playhead(6 * BAR);
    assert!(matches!(
        command(&mut v, &doc, "transport.set_end_marker"),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn the_ruler_menu_opens_on_a_right_click_and_sets_the_end_at_the_snapped_click() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    for y in [v.ruler_label_y(), v.loop_band_y() + 4.0] {
        let p = Point::new(v.tick_to_x(8 * BAR) + 5.0, y);
        let a = send(&mut v, &doc, &reg, &[right_down(p), right_up(p)]);
        assert!(
            a.iter().any(|a| matches!(
                a,
                UiAction::ContextMenu {
                    target: ContextTarget::RulerMeter,
                    ..
                }
            )),
            "{a:?}"
        );
        let out = command(&mut v, &doc, "sequencer.marker_end_here");
        assert_eq!(handled_ends(out), vec![8 * BAR]);
    }
}

#[test]
fn a_ruler_press_beside_the_flag_still_locates() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = Point::new(v.tick_to_x(6 * BAR) - 40.0, v.ruler_label_y());
    let a = send(&mut v, &doc, &reg, &[down(p), up(p)]);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::Transport(TransportAction::Locate(_))))
    );
}
