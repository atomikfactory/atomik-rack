//! The controller area of the piano roll: showing it, the lane chips, drawing, dragging, deleting
//! and toggling points, snap, cancel, and what is painted. Gestures run through a real
//! [`Document`], so "one undo step" is checked on the model.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::{AutomationPoint, ClipId, Command, Curve, Document, SongCommand};
use atomik_ui::controller_lane::{Chip, PRESETS};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::viewport::Tool;
use atomik_ui::{DisplayList, Key, Modifiers, PianoRollView, UiAction};
use support::song::*;
use support::*;

fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn setup(lanes: Vec<ControllerLane>) -> (PianoRollView, Document, Registry) {
    let reg = registry();
    let mut doc = song_doc();
    doc.song.tracks[0].lanes[0].clips[0].controllers = lanes;
    let d = Document::new(doc);
    let mut v = piano_roll(d.doc(), &reg);
    v.set_controllers_open(true);
    v.set_snap(Snap::Beat);
    (v, d, reg)
}

fn lanes_of(d: &Document) -> Vec<ControllerLane> {
    d.doc().song.clip(ClipId(1)).unwrap().2.controllers.clone()
}

fn points(d: &Document, kind: ControllerKind) -> Vec<AutomationPoint> {
    lanes_of(d)
        .into_iter()
        .find(|l| l.kind == kind)
        .map(|l| l.points)
        .unwrap_or_default()
}

fn apply(v: &mut PianoRollView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for c in song_commands(actions) {
        d.apply(Command::Song(c), reg).unwrap();
    }
    v.sync(d.doc(), reg);
}

/// Sends `events` through the view, applying the commands after each one.
fn gesture(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    events: &[atomik_ui::UiEvent],
) -> Vec<UiAction> {
    let mut all = Vec::new();
    for e in events {
        let a = v.handle(e, d.doc(), reg);
        apply(v, d, reg, &a);
        all.extend(a);
    }
    all
}

/// The merge ids of the `SetClipControllers` commands among `actions`.
fn merges(actions: &[UiAction]) -> Vec<Option<u64>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipControllers { merge, .. } => Some(merge),
            _ => None,
        })
        .collect()
}

fn at(v: &PianoRollView, kind: ControllerKind, tick: i64, value: f32) -> Point {
    v.controller_point(kind, tick, value)
}

const BEND: ControllerKind = ControllerKind::PitchBend;
const MOD: ControllerKind = ControllerKind::Cc(1);

#[test]
fn the_area_is_hidden_until_the_top_bar_toggle_and_takes_room_from_the_roll() {
    let reg = registry();
    let d = Document::new(song_doc());
    let mut v = piano_roll(d.doc(), &reg);
    assert!(!v.controllers_open());
    assert_eq!(v.controller_area().height(), 0.0);
    assert_eq!(v.velocity_rect().y1, v.bounds().y1, "closed: as before");
    let grid_before = v.velocity_rect().y0;
    // The CONTROLLERS field of the scale bar toggles it.
    let toggle = Point::new(v.bounds().x0 + 600.0, v.bounds().y0 + 28.0 + 11.0);
    let a = v.handle(&down(toggle), d.doc(), &reg);
    assert!(a.is_empty());
    v.handle(&up(toggle), d.doc(), &reg);
    assert!(v.controllers_open());
    assert!(v.controller_area().height() > 90.0);
    assert_eq!(v.controller_area().y1, v.bounds().y1);
    assert_eq!(v.velocity_rect().y1, v.controller_area().y0);
    assert!(
        v.velocity_rect().y0 < grid_before,
        "the roll above gives way"
    );
    v.handle(&down(toggle), d.doc(), &reg);
    v.handle(&up(toggle), d.doc(), &reg);
    assert!(!v.controllers_open());
    assert_eq!(v.velocity_rect().y0, grid_before);
}

#[test]
fn chips_choose_the_lane_and_the_steppers_step_their_number() {
    let (mut v, d, reg) = setup(vec![]);
    assert_eq!(v.controller_kind(), BEND);
    for (i, kind) in PRESETS.iter().enumerate() {
        let p = v.controller_chip_center(Chip::Preset(i));
        v.handle(&down(p), d.doc(), &reg);
        v.handle(&up(p), d.doc(), &reg);
        assert_eq!(v.controller_kind(), *kind);
    }
    // The CC stepper: the middle shows CC 74, the right arrow steps up, the left down.
    let c = v.controller_chip_center(Chip::Cc);
    for (dx, want) in [(0.0, 74), (28.0, 75), (-28.0, 74), (-28.0, 73)] {
        let p = Point::new(c.x + dx, c.y);
        v.handle(&down(p), d.doc(), &reg);
        v.handle(&up(p), d.doc(), &reg);
        assert_eq!(v.controller_kind(), ControllerKind::Cc(want), "dx {dx}");
    }
    let p = v.controller_chip_center(Chip::Poly);
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    assert_eq!(v.controller_kind(), ControllerKind::PolyPressure(60));
    // The curve chip flips the curve of new points.
    assert_eq!(v.controller_curve(), Curve::Linear);
    let p = v.controller_chip_center(Chip::Curve);
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    assert_eq!(v.controller_curve(), Curve::Step);
    // None of this touched the document.
    assert!(lanes_of(&d).is_empty());
}

#[test]
fn a_click_in_the_lane_adds_a_snapped_point_at_midi_resolution_as_one_undo_step() {
    let (mut v, mut d, reg) = setup(vec![]);
    v.set_controller_kind(MOD);
    let before = d.doc().clone();
    let p = at(&v, MOD, PPQ + 1234, 0.5);
    let a = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(merges(&a).len(), 1);
    let pts = points(&d, MOD);
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0].tick, PPQ, "snapped to the beat");
    assert!(((pts[0].value * 127.0).round() - pts[0].value * 127.0).abs() < 1e-3);
    assert!((pts[0].value - 0.5).abs() < 0.02);
    assert_eq!(pts[0].curve, Curve::Linear);
    assert_eq!(v.controller_selection(), Some(0));
    assert!(a.iter().any(|x| matches!(x, UiAction::EndMerge)));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one step");
}

#[test]
fn dragging_with_the_pencil_draws_one_point_per_grid_step_in_one_merge_group() {
    let (mut v, mut d, reg) = setup(vec![]);
    v.set_controller_kind(MOD);
    v.set_tool(Tool::Pencil);
    let before = d.doc().clone();
    let start = at(&v, MOD, PPQ, 0.2);
    let mut events = vec![down(start)];
    for i in 1..=4 {
        events.push(mv(at(&v, MOD, PPQ + i * PPQ, 0.2 + 0.2 * i as f32)));
    }
    events.push(up(at(&v, MOD, 5 * PPQ, 1.0)));
    let a = gesture(&mut v, &mut d, &reg, &events);
    let ids = merges(&a);
    assert!(ids.len() >= 5 && ids.iter().all(|m| m.is_some() && *m == ids[0]));
    let pts = points(&d, MOD);
    let ticks: Vec<i64> = pts.iter().map(|p| p.tick).collect();
    assert_eq!(ticks, [PPQ, 2 * PPQ, 3 * PPQ, 4 * PPQ, 5 * PPQ]);
    assert!(
        pts.windows(2).all(|w| w[0].value < w[1].value),
        "a rising stroke"
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "the whole stroke is one step");
    d.redo(&reg).unwrap();
    assert_eq!(points(&d, MOD).len(), 5);
}

#[test]
fn a_new_point_takes_the_curve_chip_and_the_pedal_always_steps() {
    let (mut v, mut d, reg) = setup(vec![]);
    v.set_controller_kind(MOD);
    let c = v.controller_chip_center(Chip::Curve);
    v.handle(&down(c), d.doc(), &reg);
    v.handle(&up(c), d.doc(), &reg);
    let p = at(&v, MOD, PPQ, 0.5);
    gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(points(&d, MOD)[0].curve, Curve::Step);
    // With the chip back on ramp, the pedal lane still steps.
    v.handle(&down(c), d.doc(), &reg);
    v.handle(&up(c), d.doc(), &reg);
    let pedal = ControllerKind::Cc(64);
    v.set_controller_kind(pedal);
    let p = at(&v, pedal, 2 * PPQ, 1.0);
    gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(points(&d, pedal)[0].curve, Curve::Step);
}

#[test]
fn dragging_a_point_moves_it_in_time_and_value_and_a_still_click_changes_nothing() {
    let lane = ControllerLane::new(
        BEND,
        vec![
            pt(PPQ, 0.5, Curve::Linear),
            pt(3 * PPQ, -0.5, Curve::Linear),
        ],
    );
    let (mut v, mut d, reg) = setup(vec![lane]);
    v.set_controller_kind(BEND);
    let before = d.doc().clone();
    // A click on the point selects it and sends nothing.
    let p = at(&v, BEND, PPQ, 0.5);
    let a = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(song_commands(&a).is_empty());
    assert_eq!(v.controller_selection(), Some(0));
    assert_eq!(d.doc(), &before);
    // Drag it to beat 2 at full bend.
    let to = at(&v, BEND, 2 * PPQ + 100, 1.0);
    let a = gesture(&mut v, &mut d, &reg, &[down(p), mv(to), up(to)]);
    let ids = merges(&a);
    assert!(!ids.is_empty() && ids.iter().all(|m| *m == ids[0] && m.is_some()));
    let pts = points(&d, BEND);
    assert_eq!(
        pts.iter().map(|p| p.tick).collect::<Vec<_>>(),
        [2 * PPQ, 3 * PPQ]
    );
    assert!((pts[0].value - 1.0).abs() < 0.03, "{}", pts[0].value);
    assert_eq!(
        pts[0].curve,
        Curve::Linear,
        "the curve travels with the point"
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_dragged_point_snaps_to_the_bend_centre_and_replaces_a_point_it_lands_on() {
    let lane = ControllerLane::new(
        BEND,
        vec![pt(PPQ, 0.5, Curve::Step), pt(2 * PPQ, 0.9, Curve::Step)],
    );
    let (mut v, mut d, reg) = setup(vec![lane]);
    v.set_controller_kind(BEND);
    let p = at(&v, BEND, PPQ, 0.5);
    let to = at(&v, BEND, PPQ, 0.01); // a hair above the centre line
    gesture(&mut v, &mut d, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(points(&d, BEND)[0].value, 0.0, "the centre");
    // Drag the first point onto the second one's tick.
    let p = at(&v, BEND, PPQ, 0.0);
    let to = at(&v, BEND, 2 * PPQ, 0.3);
    gesture(&mut v, &mut d, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(points(&d, BEND).len(), 1);
}

#[test]
fn the_eraser_and_the_secondary_button_delete_a_point_and_the_last_one_drops_the_lane() {
    let lane = ControllerLane::new(
        MOD,
        vec![pt(PPQ, 0.5, Curve::Step), pt(2 * PPQ, 0.9, Curve::Step)],
    );
    let (mut v, mut d, reg) = setup(vec![lane]);
    v.set_controller_kind(MOD);
    let p = at(&v, MOD, PPQ, 0.5);
    let a = gesture(&mut v, &mut d, &reg, &[right_down(p), right_up(p)]);
    assert_eq!(merges(&a), vec![None]);
    assert_eq!(points(&d, MOD).len(), 1);
    v.set_tool(Tool::Eraser);
    let q = at(&v, MOD, 2 * PPQ, 0.9);
    gesture(&mut v, &mut d, &reg, &[down(q), up(q)]);
    assert!(lanes_of(&d).is_empty(), "no points, no lane");
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(points(&d, MOD).len(), 2);
}

#[test]
fn command_click_toggles_the_curve_of_a_point() {
    let lane = ControllerLane::new(MOD, vec![pt(PPQ, 0.5, Curve::Linear)]);
    let (mut v, mut d, reg) = setup(vec![lane]);
    v.set_controller_kind(MOD);
    let p = at(&v, MOD, PPQ, 0.5);
    let ctrl = Modifiers {
        ctrl: true,
        meta: true,
        ..Modifiers::NONE
    };
    gesture(&mut v, &mut d, &reg, &[down_mods(p, ctrl, 1), up(p)]);
    assert_eq!(points(&d, MOD)[0].curve, Curve::Step);
    gesture(&mut v, &mut d, &reg, &[down_mods(p, ctrl, 1), up(p)]);
    assert_eq!(points(&d, MOD)[0].curve, Curve::Linear);
}

#[test]
fn escape_during_a_drag_restores_the_lane() {
    let lane = ControllerLane::new(MOD, vec![pt(PPQ, 0.5, Curve::Step)]);
    let (mut v, mut d, reg) = setup(vec![lane]);
    v.set_controller_kind(MOD);
    let before = d.doc().clone();
    let p = at(&v, MOD, PPQ, 0.5);
    let to = at(&v, MOD, 3 * PPQ, 0.9);
    gesture(&mut v, &mut d, &reg, &[down(p), mv(to)]);
    assert_ne!(lanes_of(&d), lanes_of(&Document::new(before.clone())));
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(lanes_of(&d), lanes_of(&Document::new(before)));
    assert!(!v.is_dragging());
}

#[test]
fn snap_off_keeps_a_stroke_sparse_and_the_grid_snap_is_ignored_by_other_lanes() {
    let (mut v, mut d, reg) = setup(vec![]);
    v.set_controller_kind(MOD);
    v.set_snap(Snap::Off);
    let y = at(&v, MOD, 0, 0.5).y;
    let mut events = vec![down(at(&v, MOD, PPQ, 0.5))];
    for i in 1..=200 {
        events.push(mv(Point::new(v.tick_to_x(PPQ) + f64::from(i), y)));
    }
    events.push(up(Point::new(v.tick_to_x(PPQ) + 200.0, y)));
    gesture(&mut v, &mut d, &reg, &events);
    let n = points(&d, MOD).len();
    // 200 pixels, about three pixels per point.
    assert!(n > 30 && n < 100, "{n}");
    // Another lane is untouched by it.
    assert!(points(&d, BEND).is_empty());
}

#[test]
fn presses_in_the_lane_of_a_closed_area_or_with_the_razor_do_nothing() {
    let (mut v, mut d, reg) = setup(vec![]);
    v.set_controller_kind(MOD);
    v.set_tool(Tool::Razor);
    let p = at(&v, MOD, PPQ, 0.5);
    let a = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(song_commands(&a).is_empty());
    v.set_tool(Tool::Select);
    v.set_controllers_open(false);
    let a = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(
        song_commands(&a)
            .iter()
            .all(|c| !matches!(c, SongCommand::SetClipControllers { .. }))
    );
    assert!(lanes_of(&d).is_empty());
}

#[test]
fn the_painted_area_names_the_lane_and_draws_its_points() {
    let lane = ControllerLane::new(
        MOD,
        vec![pt(0, 0.0, Curve::Linear), pt(2 * PPQ, 1.0, Curve::Step)],
    );
    let (mut v, d, reg) = setup(vec![lane]);
    v.set_controller_kind(MOD);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let texts = dl.texts();
    for want in ["CONTROLLERS", "CTRL", "MOD 1", "BEND", "< CC 74 >", "CC 1"] {
        assert!(texts.contains(&want), "{want} in {texts:?}");
    }
    // The same roll with the area hidden paints less.
    v.set_controllers_open(false);
    let mut closed = DisplayList::new();
    v.paint(&mut closed, d.doc(), &reg);
    assert!(closed.texts().len() < texts.len());
    assert!(!closed.texts().contains(&"CTRL"));
}

/// Writes `target/preview/piano_roll_controllers.png`: the roll with a pitch bend lane (a ramp, a
/// hold and a return) and a mod wheel lane, the first shown with a point selected.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_controllers_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let lane = ControllerLane::new(
        BEND,
        vec![
            pt(0, 0.0, Curve::Linear),
            pt(PPQ, 0.6, Curve::Linear),
            pt(2 * PPQ, -0.3, Curve::Step),
            pt(3 * PPQ, 0.0, Curve::Step),
        ],
    );
    let wheel = ControllerLane::new(
        MOD,
        vec![pt(PPQ, 0.25, Curve::Linear), pt(3 * PPQ, 1.0, Curve::Step)],
    );
    let (mut v, mut d, reg) = setup(vec![lane, wheel]);
    v.set_controller_kind(BEND);
    let p = at(&v, BEND, PPQ, 0.6);
    gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    let scale = 1.5;
    v.set_scale(scale);
    let b = v.bounds();
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(
        (b.width() * scale) as u16,
        (b.height() * scale) as u16,
        &mut text,
    );
    v.paint(&mut canvas, d.doc(), &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join("piano_roll_controllers.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
}
