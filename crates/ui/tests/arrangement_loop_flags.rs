//! The loop as two flags in the arrangement ruler: an L flag at the left locator and an R flag
//! at the right one, drawn like the song End's E flag, with a tint between them instead of a
//! bar. Hit tests (also where flags meet), drags (snap, the minimum length, one command, Escape),
//! the commands that set them at the playhead and from the ruler's menu, and the band's press.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{RackDoc, SongCommand};
use atomik_ui::action::ContextTarget;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, CursorHint, DisplayList, Key, Modifiers, TransportAction, UiAction, UiEvent,
};
use std::path::PathBuf;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

/// Ctrl+Alt: the disable-snap role of the standard roles.
const NO_SNAP: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: true,
    meta: false,
};

/// The song with the loop on bars 2 to 4 (on) and the End at bar 7.
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

fn loops(actions: &[UiAction]) -> Vec<((i64, i64), bool)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetLoop { range, enabled } => Some((range, enabled)),
            _ => None,
        })
        .collect()
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

fn locates(actions: &[UiAction]) -> Vec<i64> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Transport(TransportAction::Locate(t)) => Some(*t),
            _ => None,
        })
        .collect()
}

/// The left flag of a loop starting at `tick` hangs to the right of its line, `dx` pixels in.
fn left_flag(v: &ArrangementView, tick: i64, row: usize, dx: f64) -> Point {
    Point::new(v.tick_to_x(tick) + dx, v.flag_row_y(row))
}

/// The right flag hangs to the left of its line.
fn right_flag(v: &ArrangementView, tick: i64, row: usize, dx: f64) -> Point {
    Point::new(v.tick_to_x(tick) - dx, v.flag_row_y(row))
}

fn drag(
    from: Point,
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Registry,
    to_tick: i64,
    mods: Modifiers,
) -> Vec<UiAction> {
    let to = Point::new(v.tick_to_x(to_tick), from.y);
    send(v, doc, reg, &[down(from), mv_with(to, mods), up(to)])
}

#[test]
fn the_ruler_draws_an_l_an_r_and_an_e_flag() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let ruler = ruler_ops(&dl);
    for letter in ["L", "R", "E"] {
        let text = format!("\"text\":\"{letter}\"");
        assert_eq!(ruler.matches(&text).count(), 1, "{letter}");
    }
}

/// The alpha byte of the ruler fills that span `x0` to `x1` and sit inside the ruler (above the
/// lanes, which start at y 40).
fn tint_alphas(dl: &DisplayList, x0: f64, x1: f64) -> Vec<u8> {
    let at = format!("[{x0:.2},");
    dl.to_json()
        .lines()
        .filter(|l| l.starts_with("{\"fill\":{\"rect\":") && l.contains(&at))
        .filter(|l| l.contains(&format!(",{x1:.2},")))
        .filter(|l| {
            let rect = l.split('[').nth(1).unwrap();
            let y1: f64 = rect
                .split(',')
                .nth(3)
                .unwrap()
                .trim_end_matches("]}")
                .parse()
                .unwrap();
            y1 <= 40.5
        })
        .map(|l| {
            let b = l.split("\"brush\":\"#").nth(1).unwrap();
            u8::from_str_radix(&b[6..8], 16).unwrap()
        })
        .collect()
}

#[test]
fn the_loop_is_a_tint_between_the_flags_stronger_when_on() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    let (x0, x1) = (v.tick_to_x(BAR), v.tick_to_x(4 * BAR));
    let mut on = DisplayList::new();
    v.paint(&mut on, &doc, &reg);
    doc.song_mut().loop_enabled = false;
    v.sync(&doc, &reg);
    let mut off = DisplayList::new();
    v.paint(&mut off, &doc, &reg);
    let (a_on, a_off) = (tint_alphas(&on, x0, x1), tint_alphas(&off, x0, x1));
    assert_eq!((a_on.len(), a_off.len()), (1, 1), "{a_on:?} {a_off:?}");
    assert!(a_off[0] > 0, "off is still faintly tinted");
    assert!(
        a_on[0] > 2 * a_off[0],
        "on is clearly stronger: {a_on:?} {a_off:?}"
    );
}

#[test]
fn the_bar_is_gone_so_a_band_press_between_the_flags_does_not_toggle_the_loop() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = Point::new(v.tick_to_x(2 * BAR + PPQ), v.flag_row_y(1) + 2.0);
    let a = send(&mut v, &doc, &reg, &[down(p), up(p)]);
    assert!(loops(&a).is_empty(), "{a:?}");
    assert_eq!(locates(&a), vec![2 * BAR], "it locates like the labels");
}

#[test]
fn a_band_drag_scrubs_the_playhead_and_draws_no_loop() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let y = v.flag_row_y(1) + 2.0;
    let p0 = Point::new(v.tick_to_x(8 * BAR), y);
    let p1 = Point::new(v.tick_to_x(10 * BAR), y);
    let a = send(&mut v, &doc, &reg, &[down(p0), mv(p1), up(p1)]);
    assert!(loops(&a).is_empty(), "{a:?}");
    assert_eq!(locates(&a), vec![8 * BAR, 10 * BAR]);
}

#[test]
fn dragging_the_left_flag_moves_the_left_locator_snapped_in_one_command() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = left_flag(&v, BAR, 0, 5.0);
    let to = |t: i64| Point::new(v.tick_to_x(t) + 5.0, p.y);
    let events = [
        down(p),
        mv(to(2 * BAR + PPQ)),
        mv(to(2 * BAR + 3 * PPQ)),
        mv(to(2 * BAR + PPQ)),
        up(to(2 * BAR + PPQ)),
    ];
    let a = send(&mut v, &doc, &reg, &events);
    assert_eq!(loops(&a), vec![((2 * BAR, 4 * BAR), true)], "{a:?}");
    assert_eq!(song_commands(&a).len(), 1, "one command, so one undo step");
    assert!(locates(&a).is_empty());
}

#[test]
fn dragging_the_right_flag_moves_the_right_locator_and_keeps_the_enabled_flag() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().loop_enabled = false;
    v.sync(&doc, &reg);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let a = drag(p, &mut v, &doc, &reg, 6 * BAR + PPQ, Modifiers::NONE);
    assert_eq!(loops(&a), vec![((BAR, 6 * BAR), false)], "{a:?}");
}

#[test]
fn with_the_disable_snap_role_a_flag_goes_to_the_free_tick() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let a = drag(p, &mut v, &doc, &reg, 6 * BAR + 100, NO_SNAP);
    let ((l, r), _) = loops(&a)[0];
    assert_eq!(l, BAR);
    assert!((r - (6 * BAR + 100)).abs() <= 1, "{r}");
}

#[test]
fn a_flag_dragged_past_the_other_leaves_the_minimum_length() {
    let (doc, reg, mut v) = setup(Snap::Beat);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let a = drag(p, &mut v, &doc, &reg, 0, Modifiers::NONE);
    assert_eq!(loops(&a), vec![((0, PPQ), true)]);
    let p = left_flag(&v, BAR, 0, 5.0);
    let a = drag(p, &mut v, &doc, &reg, 5 * BAR, Modifiers::NONE);
    assert_eq!(loops(&a), vec![((5 * BAR, 5 * BAR + PPQ), true)]);
}

#[test]
fn escape_cancels_a_flag_drag_and_the_flag_is_drawn_back() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let to = Point::new(v.tick_to_x(7 * BAR), p.y);
    let a = send(&mut v, &doc, &reg, &[down(p), mv(to)]);
    assert!(a.is_empty());
    let a = send(&mut v, &doc, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert!(song_commands(&a).is_empty(), "{a:?}");
    let a = send(&mut v, &doc, &reg, &[up(to)]);
    assert!(a.is_empty(), "{a:?}");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let x = v.tick_to_x(4 * BAR).round() + 0.5;
    assert!(
        dl.to_json().contains(&format!("[{x:.2},")),
        "R is back at bar 5"
    );
}

#[test]
fn clicking_a_flag_without_moving_changes_nothing() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    for p in [left_flag(&v, BAR, 0, 5.0), right_flag(&v, 4 * BAR, 0, 5.0)] {
        let a = send(&mut v, &doc, &reg, &[down(p), up(p)]);
        assert!(a.is_empty(), "{a:?}");
    }
}

#[test]
fn the_dragged_flag_and_tint_follow_the_pointer_before_the_commit() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let to = Point::new(v.tick_to_x(7 * BAR), p.y);
    send(&mut v, &doc, &reg, &[down(p), mv(to)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert_eq!(
        tint_alphas(&dl, v.tick_to_x(BAR), v.tick_to_x(7 * BAR)).len(),
        1
    );
}

#[test]
fn the_cursor_is_the_horizontal_arrows_over_each_flag() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    for p in [left_flag(&v, BAR, 0, 5.0), right_flag(&v, 4 * BAR, 0, 5.0)] {
        send(&mut v, &doc, &reg, &[mv(Point::new(0.0, 500.0))]);
        assert_eq!(v.cursor(), CursorHint::Default);
        send(&mut v, &doc, &reg, &[mv(p)]);
        assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
    }
}

#[test]
fn the_line_below_a_flag_grabs_its_locator_too() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let y = v.flag_row_y(1) + 3.0;
    let a = drag(
        Point::new(v.tick_to_x(4 * BAR) + 1.0, y),
        &mut v,
        &doc,
        &reg,
        6 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(loops(&a), vec![((BAR, 6 * BAR), true)], "{a:?}");
}

// ------------------------------------------------------ flags that meet

#[test]
fn r_and_e_at_the_same_tick_hang_on_opposite_sides_and_each_is_grabbed() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().end = 4 * BAR;
    v.sync(&doc, &reg);
    let a = drag(
        right_flag(&v, 4 * BAR, 0, 5.0),
        &mut v,
        &doc,
        &reg,
        6 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        (loops(&a).len(), ends(&a).len()),
        (1, 0),
        "R is on the left: {a:?}"
    );
    let a = drag(
        left_flag(&v, 4 * BAR, 0, 5.0),
        &mut v,
        &doc,
        &reg,
        6 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        (loops(&a).len(), ends(&a).len()),
        (0, 1),
        "E is on the right: {a:?}"
    );
}

#[test]
fn on_the_shared_line_of_r_and_e_the_locator_wins() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().end = 4 * BAR;
    v.sync(&doc, &reg);
    // On the flag row, exactly on the line, and below the flags on the bare line.
    for y in [v.flag_row_y(0), v.flag_row_y(1) + 3.0] {
        let a = drag(
            Point::new(v.tick_to_x(4 * BAR), y),
            &mut v,
            &doc,
            &reg,
            6 * BAR,
            Modifiers::NONE,
        );
        assert_eq!((loops(&a).len(), ends(&a).len()), (1, 0), "y {y}: {a:?}");
    }
}

#[test]
fn l_at_the_end_goes_one_row_down_so_both_flags_stay_readable() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().loop_range = (6 * BAR, 8 * BAR);
    v.sync(&doc, &reg);
    let a = drag(
        left_flag(&v, 6 * BAR, 0, 5.0),
        &mut v,
        &doc,
        &reg,
        9 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        (loops(&a).len(), ends(&a).len()),
        (0, 1),
        "E keeps the top row: {a:?}"
    );
    let a = drag(
        left_flag(&v, 6 * BAR, 1, 5.0),
        &mut v,
        &doc,
        &reg,
        9 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        (loops(&a).len(), ends(&a).len()),
        (1, 0),
        "L is in the second row: {a:?}"
    );
}

#[test]
fn two_locators_closer_than_a_flag_do_not_cover_each_other() {
    let (mut doc, reg, mut v) = setup(Snap::Beat);
    doc.song_mut().loop_range = (2 * BAR, 2 * BAR + PPQ);
    v.sync(&doc, &reg);
    let a = drag(
        right_flag(&v, 2 * BAR + PPQ, 0, 4.0),
        &mut v,
        &doc,
        &reg,
        3 * BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        loops(&a),
        vec![((2 * BAR, 3 * BAR), true)],
        "R on top: {a:?}"
    );
    let a = drag(
        left_flag(&v, 2 * BAR, 1, 2.0),
        &mut v,
        &doc,
        &reg,
        BAR,
        Modifiers::NONE,
    );
    assert_eq!(
        loops(&a),
        vec![((BAR, 2 * BAR + PPQ), true)],
        "L below: {a:?}"
    );
}

#[test]
fn a_flag_at_the_far_left_stays_grabbable() {
    let (mut doc, reg, mut v) = setup(Snap::Bar);
    doc.song_mut().loop_range = (0, 2 * BAR);
    v.sync(&doc, &reg);
    let a = drag(
        left_flag(&v, 0, 0, 5.0),
        &mut v,
        &doc,
        &reg,
        BAR,
        Modifiers::NONE,
    );
    assert_eq!(loops(&a), vec![((BAR, 2 * BAR), true)], "{a:?}");
}

// ------------------------------------------------------ commands

fn command(v: &mut ArrangementView, doc: &RackDoc, id: &str) -> CommandOutcome {
    v.command(id, None, doc, &registry())
}

fn handled_loops(outcome: CommandOutcome) -> Vec<((i64, i64), bool)> {
    match outcome {
        CommandOutcome::Handled(a) => loops(&a),
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_left_and_right_locator_at_playhead_set_one_locator_each() {
    let (doc, _, mut v) = setup(Snap::Bar);
    v.set_playhead(2 * BAR + 7);
    let out = command(&mut v, &doc, "transport.set_left_locator");
    assert_eq!(handled_loops(out), vec![((2 * BAR + 7, 4 * BAR), true)]);
    v.set_playhead(5 * BAR + 9);
    let out = command(&mut v, &doc, "transport.set_right_locator");
    assert_eq!(handled_loops(out), vec![((BAR, 5 * BAR + 9), true)]);
}

#[test]
fn the_playhead_commands_keep_the_minimum_length_and_the_enabled_flag() {
    let (mut doc, _, mut v) = setup(Snap::Beat);
    doc.song_mut().loop_enabled = false;
    v.sync(&doc, &registry());
    v.set_playhead(6 * BAR);
    let out = command(&mut v, &doc, "transport.set_left_locator");
    assert_eq!(handled_loops(out), vec![((6 * BAR, 6 * BAR + PPQ), false)]);
    v.set_playhead(0);
    let out = command(&mut v, &doc, "transport.set_right_locator");
    assert_eq!(handled_loops(out), vec![((0, PPQ), false)]);
}

#[test]
fn the_playhead_commands_decline_where_nothing_would_change() {
    let (doc, _, mut v) = setup(Snap::Bar);
    v.set_playhead(BAR);
    assert!(matches!(
        command(&mut v, &doc, "transport.set_left_locator"),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_playhead(4 * BAR);
    assert!(matches!(
        command(&mut v, &doc, "transport.set_right_locator"),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn the_ruler_menu_sets_a_locator_at_the_snapped_click_and_left_can_be_zero() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    for y in [v.ruler_label_y(), v.flag_row_y(1) + 3.0] {
        let p = Point::new(v.tick_to_x(2 * BAR) + 5.0, y);
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
        let out = command(&mut v, &doc, "sequencer.locator_left_here");
        assert_eq!(handled_loops(out), vec![((2 * BAR, 4 * BAR), true)]);
        let out = command(&mut v, &doc, "sequencer.locator_right_here");
        assert_eq!(handled_loops(out), vec![((BAR, 2 * BAR), true)]);
    }
    let p = Point::new(v.tick_to_x(0) + 1.0, v.ruler_label_y());
    send(&mut v, &doc, &reg, &[right_down(p), right_up(p)]);
    let out = command(&mut v, &doc, "sequencer.locator_left_here");
    assert_eq!(handled_loops(out), vec![((0, 4 * BAR), true)]);
}

#[test]
fn a_right_click_on_a_flag_opens_the_ruler_menu_and_does_not_drag_it() {
    let (doc, reg, mut v) = setup(Snap::Bar);
    let p = right_flag(&v, 4 * BAR, 0, 5.0);
    let a = send(
        &mut v,
        &doc,
        &reg,
        &[right_down(p), mv(Point::new(p.x + 80.0, p.y)), right_up(p)],
    );
    assert!(
        a.iter().any(|a| matches!(a, UiAction::ContextMenu { .. })),
        "{a:?}"
    );
    assert!(loops(&a).is_empty(), "{a:?}");
}

// ------------------------------------------------------ golden

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
    assert_eq!(
        expected, actual,
        "{name} differs (regenerate with {UPDATE_ENV}=1)"
    );
}

/// The ops of the ruler: what is drawn between its clip and the pop that ends it.
fn ruler_ops(dl: &DisplayList) -> String {
    let json = dl.to_json();
    let mut lines = json
        .lines()
        .skip_while(|l| !l.starts_with("{\"clip\":{\"rect\":[208.00,0.00,"));
    let mut out = Vec::new();
    for l in lines.by_ref().skip(1) {
        if l.starts_with("{\"pop_clip\"") {
            break;
        }
        out.push(l);
    }
    out.join(
        "
",
    )
}

#[test]
fn golden_of_the_ruler_with_the_flags_apart_met_and_crowded() {
    let mut out = String::new();
    for (name, loop_range, end, on) in [
        ("apart_on", (BAR, 4 * BAR), 6 * BAR, true),
        ("apart_off", (BAR, 4 * BAR), 6 * BAR, false),
        ("r_at_end", (BAR, 6 * BAR), 6 * BAR, true),
        ("crowded", (6 * BAR, 6 * BAR + PPQ), 6 * BAR, true),
    ] {
        let (mut doc, reg, mut v) = setup(Snap::Bar);
        doc.song_mut().loop_range = loop_range;
        doc.song_mut().end = end;
        doc.song_mut().loop_enabled = on;
        v.sync(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        out.push_str(&format!(
            "# {name}
{}
",
            ruler_ops(&dl)
        ));
    }
    check("arrangement_loop_flags_1x.json", &out);
}

/// Writes previews of the ruler to `target/preview`: `cargo test -p atomik-ui --test
/// arrangement_loop_flags -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    for (name, loop_range, end, on) in [
        ("apart", (BAR, 4 * BAR), 6 * BAR, true),
        ("apart_off", (BAR, 4 * BAR), 6 * BAR, false),
        ("r_at_end", (BAR, 6 * BAR), 6 * BAR, true),
        ("crowded", (6 * BAR, 6 * BAR + PPQ), 6 * BAR, true),
    ] {
        let (mut doc, reg, mut v) = setup(Snap::Bar);
        doc.song_mut().loop_range = loop_range;
        doc.song_mut().end = end;
        doc.song_mut().loop_enabled = on;
        v.sync(&doc, &reg);
        v.set_playhead(2 * BAR + PPQ);
        let mut text = TextEngine::new();
        v.set_scale(2.0);
        let (w, h) = (
            (ARR_BOUNDS.width() * 2.0) as u16,
            (ARR_BOUNDS.height() * 2.0) as u16,
        );
        let mut canvas = CpuCanvas::new(w, h, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/preview/loop_flags_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
}
