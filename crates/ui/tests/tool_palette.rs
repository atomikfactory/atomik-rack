//! The arrangement's mute, magnifier and hand tools, their paired alternates, and the eraser's
//! rubber band.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipId, Command, RackDoc, WorkflowCommand};
use atomik_ui::keymap::{Keymap, Os, Profile, find_conflicts};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, CursorHint, Modifiers, Tool, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (RackDoc, Registry, ArrangementView) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    (doc, reg, v)
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

/// Inside a clip, clear of its edges and its header strip.
fn body(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.center().x, r.y1 - 6.0)
}

/// Empty lane space: right of every clip of the bass track.
fn empty_lane(v: &ArrangementView) -> Point {
    let r = v.lane_rect(&key("bass"), 0).unwrap();
    Point::new(r.x1 - 20.0, r.center().y)
}

fn click_with(
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Registry,
    p: Point,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut out = v.handle(&down_mods(p, modifiers, 1), doc, reg);
    out.extend(v.handle(&up(p), doc, reg));
    out
}

fn click(v: &mut ArrangementView, doc: &RackDoc, reg: &Registry, p: Point) -> Vec<UiAction> {
    click_with(v, doc, reg, p, Modifiers::NONE)
}

fn muted(ids: &[u64], muted: bool) -> UiAction {
    UiAction::Command(Command::Workflow(WorkflowCommand::SetClipsMuted {
        ids: ids.iter().map(|i| ClipId(*i)).collect(),
        muted,
    }))
}

#[test]
fn the_new_tools_have_commands_buttons_and_keys() {
    let (doc, reg, mut v) = setup();
    for (id, tool) in [
        ("tool.mute", Tool::Mute),
        ("tool.magnifier", Tool::Magnifier),
        ("tool.hand", Tool::Hand),
    ] {
        assert!(v.command(id, None, &doc, &reg).is_taken());
        assert_eq!(v.tool(), tool);
    }
    // A press on each button selects its tool.
    for tool in Tool::ALL {
        let c = v.tool_button_center(tool);
        click(&mut v, &doc, &reg, c);
        assert_eq!(v.tool(), tool, "{tool:?} button");
    }
    // The buttons do not overlap each other or the snap selector's corner buttons.
    let centers: Vec<Point> = Tool::ALL.iter().map(|t| v.tool_button_center(*t)).collect();
    for (i, a) in centers.iter().enumerate() {
        for b in &centers[i + 1..] {
            assert!(a.distance(*b) > 12.0, "{a:?} {b:?}");
        }
    }
}

#[test]
fn the_tool_keys_clash_with_no_standard_binding() {
    for os in Os::ALL {
        let map = Keymap::bundled(Profile::Standard, os);
        for id in ["tool.mute", "tool.magnifier", "tool.hand"] {
            assert_eq!(
                map.chords_of(id, None).len(),
                1,
                "{id} has one standard key on {os:?}"
            );
        }
        assert!(find_conflicts(&map).is_empty(), "no conflict on {os:?}");
    }
}

#[test]
fn the_mute_tool_toggles_the_clip_under_the_pointer_and_selects_nothing() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Mute);
    let p = body(&v, 1);
    assert_eq!(click(&mut v, &doc, &reg, p), [muted(&[1], true)]);
    assert!(v.selected_clips().is_empty());
    // On an empty lane nothing happens.
    let e = empty_lane(&v);
    assert!(click(&mut v, &doc, &reg, e).is_empty());
}

#[test]
fn the_mute_tool_mutes_all_the_selected_clips_when_it_hits_one_of_them() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Select);
    let (a, b) = (body(&v, 1), body(&v, 2));
    click(&mut v, &doc, &reg, a);
    click_with(&mut v, &doc, &reg, b, Modifiers::SHIFT);
    let mut want = v.selected_clips();
    want.sort();
    assert_eq!(want.len(), 2);
    v.set_tool(Tool::Mute);
    let out = click(&mut v, &doc, &reg, a);
    let ids: Vec<u64> = want.iter().map(|c| c.0).collect();
    assert_eq!(out, [muted(&ids, true)]);
}

#[test]
fn the_mute_tool_unmutes_a_muted_clip() {
    let (mut doc, reg, mut v) = setup();
    for t in &mut doc.song_mut().tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(1) {
                    c.muted = true;
                }
            }
        }
    }
    v.sync(&doc, &reg);
    v.set_tool(Tool::Mute);
    let p = body(&v, 1);
    assert_eq!(click(&mut v, &doc, &reg, p), [muted(&[1], false)]);
}

#[test]
fn the_alternate_modifier_pairs_each_tool_with_its_other() {
    let (doc, reg, mut v) = setup();
    for (tool, with_alt) in [
        (Tool::Select, Tool::Pencil),
        (Tool::Pencil, Tool::Select),
        (Tool::Eraser, Tool::Select),
        (Tool::Razor, Tool::Select),
        (Tool::Speaker, Tool::Select),
        (Tool::Mute, Tool::Select),
        (Tool::Magnifier, Tool::Hand),
        (Tool::Hand, Tool::Magnifier),
    ] {
        v.set_tool(tool);
        let _ = v.handle(&UiEvent::ModifiersChanged { modifiers: alt() }, &doc, &reg);
        assert_eq!(v.shown_tool(), with_alt, "{tool:?} with Alt");
        let _ = v.handle(
            &UiEvent::ModifiersChanged {
                modifiers: Modifiers::NONE,
            },
            &doc,
            &reg,
        );
        assert_eq!(v.shown_tool(), tool);
    }
}

#[test]
fn the_hand_scrolls_the_view_and_edits_nothing() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Hand);
    v.set_scroll_ticks(BAR as f64);
    let before = v.scroll_ticks();
    let p = body(&v, 1);
    let out = v.handle(&down(p), &doc, &reg);
    assert!(out.is_empty());
    let to = p + Vec2::new(-80.0, 0.0);
    v.handle(&mv(to), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Grabbing);
    let out = v.handle(&up(to), &doc, &reg);
    assert!(out.is_empty(), "no command: {out:?}");
    assert!(v.scroll_ticks() > before, "dragging left moves later");
    assert!(v.selected_clips().is_empty());
    v.handle(&mv(to + Vec2::new(1.0, 0.0)), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Hand);
}

#[test]
fn the_hand_pans_from_an_empty_lane_and_with_alt_the_magnifier_becomes_the_hand() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Magnifier);
    v.set_scroll_ticks(BAR as f64);
    let before = v.scroll_ticks();
    let e = empty_lane(&v);
    let out = v.handle(&down_mods(e, alt(), 1), &doc, &reg);
    assert!(out.is_empty());
    v.handle(&mv_with(e + Vec2::new(-60.0, 0.0), alt()), &doc, &reg);
    v.handle(&up(e + Vec2::new(-60.0, 0.0)), &doc, &reg);
    assert!(v.scroll_ticks() > before);
}

#[test]
fn a_magnifier_click_zooms_in_around_the_pointer_and_shift_zooms_out() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Magnifier);
    let p = body(&v, 1);
    let before = v.px_per_tick();
    let zoom_before = v.track_zoom();
    let under_before = (p.x - v.tick_to_x(0)) / v.px_per_tick();
    assert!(click(&mut v, &doc, &reg, p).is_empty());
    assert!(v.px_per_tick() > before * 1.9, "zoomed in");
    assert!(v.track_zoom() > zoom_before, "the rows grew");
    let under_after = (p.x - v.tick_to_x(0)) / v.px_per_tick();
    assert!(
        (under_after - under_before).abs() < PPQ as f64 / 4.0,
        "the tick under the pointer stays: {under_before} {under_after}"
    );
    let zoomed = v.px_per_tick();
    click_with(&mut v, &doc, &reg, p, Modifiers::SHIFT);
    assert!(v.px_per_tick() < zoomed, "Shift zooms out");
}

#[test]
fn a_magnifier_rectangle_fills_the_view_with_what_it_spans() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Magnifier);
    let (a, b) = (v.tick_to_x(BAR), v.tick_to_x(2 * BAR));
    let r = v.lane_rect(&key("bass"), 0).unwrap();
    let from = Point::new(a, r.y0 + 4.0);
    let to = Point::new(b, r.y1 - 4.0);
    let before = v.px_per_tick();
    v.handle(&down(from), &doc, &reg);
    v.handle(&mv(to), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Magnifier);
    let out = v.handle(&up(to), &doc, &reg);
    assert!(out.is_empty());
    assert!(v.px_per_tick() > before * 2.0, "one bar now fills the view");
    // The span's left edge is at the left of the timeline, within the fit margin.
    assert!(v.scroll_ticks() <= BAR as f64);
    assert!(v.scroll_ticks() > BAR as f64 / 2.0);
}

#[test]
fn the_eraser_still_deletes_a_clicked_clip_and_an_empty_click_does_nothing() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Eraser);
    let p = body(&v, 1);
    let out = click(&mut v, &doc, &reg, p);
    assert_eq!(out.len(), 1);
    let e = empty_lane(&v);
    assert!(click(&mut v, &doc, &reg, e).is_empty());
}

#[test]
fn the_eraser_band_deletes_the_clips_it_touches_in_one_command() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Eraser);
    let bass = v.lane_rect(&key("bass"), 0).unwrap();
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let from = Point::new(bass.x1 - 4.0, bass.y1 - 3.0);
    // Starts on empty space: the bass lane is empty at its right end.
    let to = Point::new(v.tick_to_x(BAR / 2), lead.y0 + 6.0);
    v.handle(&down(from), &doc, &reg);
    v.handle(&mv(to), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Eraser);
    let out = v.handle(&up(to), &doc, &reg);
    let [UiAction::Command(Command::Workflow(WorkflowCommand::RemoveClips { ids }))] =
        out.as_slice()
    else {
        panic!("one RemoveClips: {out:?}");
    };
    assert!(
        ids.contains(&ClipId(1)),
        "the bass clip is in the band: {ids:?}"
    );
    assert!(ids.contains(&ClipId(2)), "the lead clip too: {ids:?}");
}

#[test]
fn an_eraser_band_over_no_clip_emits_nothing() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Eraser);
    let e = empty_lane(&v);
    let to = e + Vec2::new(-5.0, 10.0);
    v.handle(&down(e), &doc, &reg);
    v.handle(&mv(to), &doc, &reg);
    assert!(v.handle(&up(to), &doc, &reg).is_empty());
}
