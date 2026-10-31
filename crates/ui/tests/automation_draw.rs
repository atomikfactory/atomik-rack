//! Drawing automation with the pencil: a drag writes a thinned series of points, Shift lays the
//! points on the line from the press, the flat-range role holds the pressed value, Escape puts
//! the clip back, and Cleanup Automation is one workflow command.

mod support;

use atomik_model::{
    AutomationPoint, ClipContent, ClipId, Command, SongCommand, ThinLevel, WorkflowCommand,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Key, Modifiers, Tool, UiAction};
use support::song::*;
use support::*;

const CLIP: ClipId = ClipId(4);
/// Where a stroke begins, as a fraction of the clip: past its start edge and its first point.
const START: f64 = 0.02;

fn last_points(actions: &[UiAction]) -> Option<Vec<AutomationPoint>> {
    song_commands(actions)
        .into_iter()
        .rev()
        .find_map(|c| match c {
            SongCommand::SetClipContent {
                id: CLIP,
                content: ClipContent::Automation(p),
                ..
            } => Some(p),
            _ => None,
        })
}

/// A view with the pencil, 1/16 snap and no thinning, and the screen position of a tick at the
/// middle of the cutoff clip's value range.
fn pencil() -> (
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
    ArrangementView,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Sixteenth);
    v.set_tool(Tool::Pencil);
    v.set_automation_thinning(0.0);
    (doc, reg, v)
}

/// Draws across the first four bars in 32 pointer samples; `y` gives the height at fraction
/// `f` of the way. Returns the points after the last move.
fn stroke_across(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    y: impl Fn(f64) -> f64,
    modifiers: Modifiers,
) -> Vec<AutomationPoint> {
    let (top, bottom) = v.automation_value_range(CLIP).unwrap();
    let h = bottom - top;
    let (x0, x1) = (v.tick_to_x(0), v.tick_to_x(4 * BAR));
    let at = |f: f64| Point::new(x0 + (x1 - x0) * f, top + y(f) * h);
    let mut out = v.handle(&down(at(START)), doc, reg);
    for i in 1..=32 {
        let f = START + f64::from(i) / 32.0 * (0.999 - START);
        out.extend(v.handle(&mv_with(at(f), modifiers), doc, reg));
    }
    let up_at = at(0.999);
    let ended = v.handle(&up(up_at), doc, reg);
    assert_eq!(
        ended,
        vec![UiAction::EndMerge],
        "the stroke is one undo step"
    );
    last_points(&out).expect("the stroke wrote points")
}

#[test]
fn the_pencil_draws_a_point_per_snap_step_and_thinning_keeps_fewer() {
    let (doc, reg, mut v) = pencil();
    let wave = |f: f64| 0.5 + 0.4 * (f * 12.0).sin();
    let all = stroke_across(&mut v, &doc, &reg, wave, Modifiers::NONE);
    // Nearly four bars of sixteenths: about 64 points.
    assert!((55..=70).contains(&all.len()), "{}", all.len());
    assert!(
        all.windows(2).all(|w| w[0].tick < w[1].tick),
        "sorted, unique"
    );
    // The same stroke thinned at the default level keeps fewer, and at Maximum fewer still.
    let counts: Vec<usize> = [0.005_f32, 0.05]
        .into_iter()
        .map(|t| {
            let (doc, reg, mut v) = pencil();
            v.set_automation_thinning(t);
            stroke_across(&mut v, &doc, &reg, wave, Modifiers::NONE).len()
        })
        .collect();
    assert!(counts[0] < all.len(), "{counts:?} vs {}", all.len());
    assert!(counts[1] < counts[0], "{counts:?}");
}

#[test]
fn a_press_without_a_drag_still_places_one_point() {
    let (doc, reg, mut v) = pencil();
    let (top, bottom) = v.automation_value_range(CLIP).unwrap();
    let p = Point::new(v.tick_to_x(BAR + PPQ), (top + bottom) / 2.0);
    let a = v.handle(&down(p), &doc, &reg);
    assert_eq!(
        last_points(&a).unwrap().len(),
        5,
        "the clip had four points"
    );
    assert_eq!(v.handle(&up(p), &doc, &reg), vec![UiAction::EndMerge]);
}

#[test]
fn shift_puts_every_point_on_the_line_from_the_press() {
    let (doc, reg, mut v) = pencil();
    // The hand wobbles, the points do not.
    let pts = stroke_across(
        &mut v,
        &doc,
        &reg,
        |f| 0.8 - 0.5 * f + 0.1 * (f * 30.0).sin(),
        Modifiers::SHIFT,
    );
    // The clip's first point lies before the stroke and stays.
    let pts = &pts[1..];
    assert!(pts.len() > 40);
    let (a, b) = (pts[0], *pts.last().unwrap());
    for p in pts {
        let f = (p.tick - a.tick) as f32 / (b.tick - a.tick) as f32;
        let want = a.value + (b.value - a.value) * f;
        assert!(
            (p.value - want).abs() <= want.abs().max(1.0) * 1e-3,
            "{p:?} vs {want}"
        );
    }
}

#[test]
fn the_flat_range_holds_the_pressed_value_between_two_points() {
    let (doc, reg, mut v) = pencil();
    // Ctrl+Shift is the default flat-range role on the OS the tests pin.
    let flat = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };
    let all = stroke_across(&mut v, &doc, &reg, |f| 0.2 + 0.6 * f, flat);
    // The clip's first point lies before the stroke and stays.
    assert_eq!(all[0].tick, 0);
    let pts = &all[1..];
    assert_eq!(pts.len(), 2, "{pts:?}");
    assert!((pts[0].value - pts[1].value).abs() < 1e-3);
    assert!(pts[1].tick > 3 * BAR);
}

#[test]
fn escape_puts_the_clip_back_and_a_selection_tool_still_drags_one_point() {
    let (doc, reg, mut v) = pencil();
    let original = match &doc.song.clip(CLIP).unwrap().2.content {
        ClipContent::Automation(p) => p.clone(),
        _ => unreachable!(),
    };
    let (top, bottom) = v.automation_value_range(CLIP).unwrap();
    let p = Point::new(v.tick_to_x(BAR + PPQ), (top + bottom) / 2.0);
    let _ = v.handle(&down(p), &doc, &reg);
    let _ = v.handle(&mv(Point::new(p.x + 80.0, p.y - 20.0)), &doc, &reg);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(last_points(&a).unwrap(), original);
    // With the selection tool a click adds one point and the drag moves that point.
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let a = v.handle(&down(p), &doc, &reg);
    assert_eq!(last_points(&a).unwrap().len(), 5);
    let a = v.handle(&mv(Point::new(p.x + 200.0, p.y)), &doc, &reg);
    assert_eq!(last_points(&a).unwrap().len(), 5, "moved, not drawn");
}

#[test]
fn cleanup_automation_acts_on_the_selected_clips_or_the_selected_tracks() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    // Nothing selected: the command says what to select.
    let out = v.command("sequencer.cleanup_automation", Some("heavy"), &doc, &reg);
    assert!(matches!(out, CommandOutcome::NotApplicable(_)), "{out:?}");
    // A bad level is declined.
    assert_eq!(
        v.command("sequencer.cleanup_automation", Some("extreme"), &doc, &reg),
        CommandOutcome::Declined
    );
    // The cutoff clip, selected by a click on its header.
    let r = v.clip_rect(CLIP).unwrap();
    let p = Point::new(r.x0 + 30.0, r.y0 + 4.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    for level in ThinLevel::ALL {
        let out = v.command("sequencer.cleanup_automation", Some(level.id()), &doc, &reg);
        let CommandOutcome::Handled(actions) = out else {
            panic!("{out:?}");
        };
        assert_eq!(
            commands(&actions),
            [Command::Workflow(WorkflowCommand::CleanupAutomation {
                ids: vec![CLIP],
                level
            })]
        );
    }
}
