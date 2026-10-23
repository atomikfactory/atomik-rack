//! Vertical zoom: track heights and the global track zoom of the arrangement, the key zoom of
//! the piano roll. Rows, headers, hit testing, painting and the commands at several heights.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, SongCommand};
use atomik_ui::arrangement::{COMPACT_BELOW, MAX_ROW_HEIGHT, MIN_ROW_HEIGHT, TRACK_HEIGHT};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::piano_roll::{KEY_HEIGHT, MAX_KEY_HEIGHT, MIN_KEY_HEIGHT};
use atomik_ui::{
    ArrangementView, DisplayList, Modifiers, PianoRollView, TrackButton, UiAction, UiEvent,
};
use std::collections::BTreeMap;
use support::song::*;
use support::*;

fn setup() -> (ArrangementView, Document, Registry) {
    let reg = registry();
    let d = Document::new(song_doc());
    let v = arrangement(d.doc(), &reg);
    (v, d, reg)
}

fn heights(pairs: &[(&str, u16)]) -> BTreeMap<atomik_model::DeviceKey, u16> {
    pairs.iter().map(|(k, h)| (key(k), *h)).collect()
}

/// Applies the song commands among `actions` to the document and syncs the view.
fn apply(v: &mut ArrangementView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for c in song_commands(actions) {
        d.apply(Command::Song(c), reg).unwrap();
    }
    v.sync(d.doc(), reg);
}

fn h(v: &ArrangementView, device: &str, lane: usize) -> f64 {
    v.lane_rect(&key(device), lane).unwrap().height()
}

fn wheel(v: &mut ArrangementView, d: &Document, reg: &Registry, pos: Point, dy: f64, m: Modifiers) {
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos,
            modifiers: m,
        },
        d.doc(),
        reg,
    );
}

fn paint(v: &mut ArrangementView, d: &Document, reg: &Registry) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl
}

#[test]
fn rows_have_the_default_heights_at_zoom_one() {
    let (v, ..) = setup();
    assert_eq!(v.track_zoom(), 1.0);
    for t in ["bass", "lead", "keys"] {
        assert_eq!(h(&v, t, 0), TRACK_HEIGHT, "{t}");
        assert_eq!(v.track_height(&key(t)), Some(TRACK_HEIGHT));
    }
    assert_eq!(h(&v, "bass", 1), 52.0, "the automation lane");
    assert_eq!(v.track_height(&key("nobody")), None);
}

#[test]
fn the_zoom_scales_every_row_and_stays_between_the_limits() {
    let (mut v, ..) = setup();
    v.set_track_zoom(2.0);
    assert_eq!(h(&v, "bass", 0), 120.0);
    assert_eq!(h(&v, "bass", 1), 104.0);
    assert_eq!(h(&v, "lead", 0), 120.0);
    // Rows follow each other without gaps.
    let (a, b) = (
        v.lane_rect(&key("bass"), 1).unwrap(),
        v.lane_rect(&key("lead"), 0).unwrap(),
    );
    assert!((a.y1 - b.y0).abs() < 1e-9);
    v.set_track_zoom(0.0);
    assert_eq!(h(&v, "bass", 0), MIN_ROW_HEIGHT);
    assert_eq!(h(&v, "bass", 1), MIN_ROW_HEIGHT.max(52.0 * v.track_zoom()));
    v.set_track_zoom(100.0);
    assert!(v.track_zoom() <= atomik_ui::arrangement::MAX_TRACK_ZOOM);
    assert!(h(&v, "bass", 0) <= MAX_ROW_HEIGHT);
    v.set_track_zoom(f64::NAN);
    assert_eq!(v.track_zoom(), 1.0);
    assert_eq!(h(&v, "bass", 0), TRACK_HEIGHT);
}

#[test]
fn the_marker_strip_and_tempo_lane_do_not_move_with_the_zoom() {
    let (mut v, ..) = setup();
    let top = v.lane_rect(&key("bass"), 0).unwrap().y0;
    v.set_track_zoom(3.0);
    assert_eq!(v.lane_rect(&key("bass"), 0).unwrap().y0, top);
    v.set_tempo_lane(true);
    let with_lane = v.lane_rect(&key("bass"), 0).unwrap().y0;
    assert!(with_lane > top, "the tempo lane pushes the rows down");
    v.set_track_zoom(0.5);
    assert_eq!(v.lane_rect(&key("bass"), 0).unwrap().y0, with_lane);
}

#[test]
fn a_track_height_from_the_song_applies_before_the_zoom() {
    let (mut v, mut d, reg) = setup();
    d.apply(
        Command::Song(SongCommand::SetTrackHeights(heights(&[("lead", 100)]))),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(h(&v, "lead", 0), 100.0);
    assert_eq!(h(&v, "keys", 0), 60.0);
    v.set_track_zoom(2.0);
    assert_eq!(h(&v, "lead", 0), 200.0);
    v.set_track_zoom(4.0);
    assert_eq!(h(&v, "lead", 0), MAX_ROW_HEIGHT, "clamped at 400");
}

#[test]
fn alt_wheel_zooms_the_tracks_and_keeps_the_row_under_the_pointer() {
    let (mut v, d, reg) = setup();
    // A short view, so the rows can scroll.
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 260.0));
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let pos = Point::new(lead.center().x, lead.y0 + lead.height() * 0.25);
    let frac = |v: &ArrangementView| {
        let r = v.lane_rect(&key("lead"), 0).unwrap();
        (pos.y - r.y0) / r.height()
    };
    let before = frac(&v);
    wheel(&mut v, &d, &reg, pos, 2.0, alt());
    assert!(v.track_zoom() > 1.0);
    assert!((frac(&v) - before).abs() < 0.02, "{} vs {before}", frac(&v));
    let z = v.track_zoom();
    wheel(&mut v, &d, &reg, pos, -2.0, alt());
    assert!(v.track_zoom() < z);
    // The time zoom is untouched by Alt+wheel, and Ctrl+wheel leaves the track zoom alone.
    let (ppt, zoom) = (v.px_per_tick(), v.track_zoom());
    wheel(&mut v, &d, &reg, pos, 2.0, alt());
    assert_eq!(v.px_per_tick(), ppt);
    wheel(&mut v, &d, &reg, pos, 2.0, Modifiers::CTRL);
    assert!(v.px_per_tick() > ppt);
    assert!(
        v.track_zoom() > zoom,
        "only the Alt+wheel before it changed it"
    );
}

#[test]
fn the_zoom_commands_step_the_zoom_and_reset_clears_the_heights_too() {
    let (mut v, mut d, reg) = setup();
    assert!(
        v.command("sequencer.zoom_tracks_in", None, d.doc(), &reg)
            .is_taken()
    );
    let up = v.track_zoom();
    assert!(up > 1.0);
    assert!(
        v.command("sequencer.zoom_tracks_out", None, d.doc(), &reg)
            .is_taken()
    );
    assert!((v.track_zoom() - 1.0).abs() < 1e-9);
    // Reset with nothing to reset sends nothing.
    v.set_track_zoom(2.0);
    let out = v.command("sequencer.zoom_tracks_reset", None, d.doc(), &reg);
    assert!(out.is_taken() && song_commands(out.actions()).is_empty());
    assert_eq!(v.track_zoom(), 1.0);
    // Reset with heights set sends one command that clears them.
    d.apply(
        Command::Song(SongCommand::SetTrackHeights(heights(&[("keys", 150)]))),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    let out = v.command("sequencer.zoom_tracks_reset", None, d.doc(), &reg);
    assert_eq!(
        song_commands(out.actions()),
        vec![SongCommand::SetTrackHeights(BTreeMap::new())]
    );
    apply(&mut v, &mut d, &reg, out.actions());
    assert_eq!(h(&v, "keys", 0), TRACK_HEIGHT);
    // Other views' commands are not the arrangement's.
    assert_eq!(
        v.command("editor.zoom_keys_in", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn dragging_the_header_edge_resizes_the_track_in_one_undo_step() {
    let (mut v, mut d, reg) = setup();
    let edge = v.track_edge_center(&key("lead")).unwrap();
    let mut actions = Vec::new();
    for e in [
        down(edge),
        mv(Point::new(edge.x, edge.y + 10.0)),
        mv(Point::new(edge.x, edge.y + 30.0)),
        up(Point::new(edge.x, edge.y + 30.0)),
    ] {
        let a = v.handle(&e, d.doc(), &reg);
        apply(&mut v, &mut d, &reg, &a);
        actions.extend(a);
    }
    assert_eq!(h(&v, "lead", 0), 90.0, "60 + 30 px");
    assert_eq!(d.doc().song.track_heights, heights(&[("lead", 90)]));
    let merges: Vec<u64> = song_commands(&actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragTrackHeights { merge, .. } => Some(merge),
            _ => None,
        })
        .collect();
    assert_eq!(merges.len(), 2);
    assert_eq!(merges[0], merges[1], "one merge group");
    assert!(actions.contains(&UiAction::EndMerge));
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(h(&v, "lead", 0), 60.0, "a single undo step");
}

#[test]
fn the_drag_follows_the_zoom_and_stays_inside_the_limits() {
    let (mut v, mut d, reg) = setup();
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 900.0));
    v.set_track_zoom(2.0);
    let edge = v.track_edge_center(&key("keys")).unwrap();
    let a = v.handle(&down(edge), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    // 60 px more at zoom 2 is 30 more at zoom 1.
    let a = v.handle(&mv(Point::new(edge.x, edge.y + 60.0)), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(d.doc().song.track_heights, heights(&[("keys", 90)]));
    assert_eq!(h(&v, "keys", 0), 180.0);
    let a = v.handle(&mv(Point::new(edge.x, edge.y - 5000.0)), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(
        d.doc().song.track_heights,
        heights(&[("keys", MIN_ROW_HEIGHT as u16)])
    );
    let a = v.handle(&mv(Point::new(edge.x, edge.y + 50_000.0)), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(d.doc().song.track_heights[&key("keys")], 400);
    let a = v.handle(&up(edge), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
}

#[test]
fn escape_during_the_drag_puts_the_heights_back_and_a_double_click_resets_one() {
    let (mut v, mut d, reg) = setup();
    d.apply(
        Command::Song(SongCommand::SetTrackHeights(heights(&[("lead", 80)]))),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    let edge = v.track_edge_center(&key("keys")).unwrap();
    for e in [
        down(edge),
        mv(Point::new(edge.x, edge.y + 40.0)),
        press(atomik_ui::Key::Escape, Modifiers::NONE),
    ] {
        let a = v.handle(&e, d.doc(), &reg);
        apply(&mut v, &mut d, &reg, &a);
    }
    assert_eq!(d.doc().song.track_heights, heights(&[("lead", 80)]));
    let a = v.handle(&up(edge), d.doc(), &reg);
    assert!(song_commands(&a).is_empty());
    // A double-click on the edge of the track with an entry removes it.
    let lead_edge = v.track_edge_center(&key("lead")).unwrap();
    let a = v.handle(&down_mods(lead_edge, Modifiers::NONE, 2), d.doc(), &reg);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetTrackHeights(BTreeMap::new())]
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(h(&v, "lead", 0), TRACK_HEIGHT);
    // ... and does nothing for one without.
    let a = v.handle(&down_mods(lead_edge, Modifiers::NONE, 2), d.doc(), &reg);
    assert!(song_commands(&a).is_empty());
}

#[test]
fn clips_are_hit_at_every_height() {
    let (mut v, d, reg) = setup();
    for zoom in [0.4, 0.7, 1.0, 2.0, 4.0] {
        v.set_track_zoom(zoom);
        let r = v.clip_rect(atomik_model::ClipId(1)).unwrap();
        let lane = v.lane_rect(&key("bass"), 0).unwrap();
        assert!(
            r.y0 >= lane.y0 && r.y1 <= lane.y1,
            "{zoom}: the clip is in its row"
        );
        assert!((r.height() - (lane.height() - 6.0)).abs() < 1e-9);
        // The middle of the body and (when the clip is tall) the lowest pixel of the body.
        let body = Point::new(r.x0 + 20.0, (r.y0 + 14.0 + r.y1) / 2.0);
        for p in [body, Point::new(body.x, r.y1 - 1.0)] {
            let a = v.handle(&down(p), d.doc(), &reg);
            let _ = v.handle(&up(p), d.doc(), &reg);
            let _ = a;
            assert!(
                v.selected_clips().contains(&atomik_model::ClipId(1)),
                "{zoom} at {p:?}"
            );
            v.set_selected_clips(&[]);
        }
        // The pixel under the clip belongs to the lane: no clip.
        let below = Point::new(r.x0 + 20.0, r.y1 + 1.5);
        let _ = v.handle(&down(below), d.doc(), &reg);
        let _ = v.handle(&up(below), d.doc(), &reg);
        assert!(
            !v.selected_clips().contains(&atomik_model::ClipId(1)),
            "{zoom}"
        );
    }
}

#[test]
fn a_short_header_has_one_line_and_its_buttons_still_work() {
    let (mut v, d, reg) = setup();
    v.set_track_zoom(0.4);
    assert!(h(&v, "lead", 0) < COMPACT_BELOW);
    let lane = v.lane_rect(&key("lead"), 0).unwrap();
    for b in [
        TrackButton::Expand,
        TrackButton::Mute,
        TrackButton::Solo,
        TrackButton::Arm,
    ] {
        let c = v.track_button_center(&key("lead"), b).unwrap();
        assert!(c.y > lane.y0 && c.y < lane.y1, "{b:?} is inside the row");
    }
    assert_eq!(
        v.track_button_center(&key("lead"), TrackButton::AddLane),
        None
    );
    let c = v
        .track_button_center(&key("lead"), TrackButton::Mute)
        .unwrap();
    let a = v.handle(&down(c), d.doc(), &reg);
    let _ = v.handle(&up(c), d.doc(), &reg);
    assert!(
        song_commands(&a)
            .iter()
            .any(|c| matches!(c, SongCommand::SetTrackFlags { .. })),
        "mute reached the song: {a:?}"
    );
    // A tall header keeps the full set.
    v.set_track_zoom(1.0);
    assert!(
        v.track_button_center(&key("lead"), TrackButton::AddLane)
            .is_some()
    );
}

#[test]
fn painting_works_at_the_smallest_and_the_tallest_rows() {
    let (mut v, d, reg) = setup();
    let normal = paint(&mut v, &d, &reg);
    for zoom in [0.4, 4.0] {
        v.set_track_zoom(zoom);
        let dl = paint(&mut v, &d, &reg);
        assert!(!dl.is_empty());
        assert_ne!(dl, normal, "{zoom}");
    }
    // Every row at the tallest height still fits the content: scrolling is clamped.
    v.set_scroll_y(1e9);
    let last = v.lane_rect(&key("keys"), 0).unwrap();
    assert!(last.y1 >= ARR_BOUNDS.y1 - 1.0 || last.y1 > 0.0);
}

#[test]
fn an_audio_clip_and_its_waveform_follow_the_row_height() {
    use support::audio::*;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let mut last = 0.0;
    for zoom in [0.4, 1.0, 3.0] {
        v.set_track_zoom(zoom);
        let r = v.clip_rect(atomik_model::ClipId(10)).unwrap();
        assert!(r.height() > last, "{zoom}");
        last = r.height();
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        assert!(!dl.is_empty());
    }
}

// ---------------------------------------------------------------- the piano roll's key zoom

fn roll() -> (PianoRollView, atomik_model::RackDoc, Registry) {
    let (doc, reg) = (song_doc(), registry());
    let v = piano_roll(&doc, &reg);
    (v, doc, reg)
}

fn roll_wheel(
    v: &mut PianoRollView,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
    pos: Point,
    dy: f64,
    m: Modifiers,
) {
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos,
            modifiers: m,
        },
        doc,
        reg,
    );
}

#[test]
fn the_roll_starts_at_the_default_key_height_and_clamps_the_zoom() {
    let (mut v, ..) = roll();
    assert_eq!(v.key_height(), KEY_HEIGHT);
    v.set_key_height(1000.0);
    assert_eq!(v.key_height(), MAX_KEY_HEIGHT);
    v.set_key_height(0.1);
    assert_eq!(v.key_height(), MIN_KEY_HEIGHT);
    v.set_key_height(f64::NAN);
    assert_eq!(v.key_height(), KEY_HEIGHT);
    assert_eq!((MIN_KEY_HEIGHT, MAX_KEY_HEIGHT), (6.0, 48.0));
}

#[test]
fn alt_wheel_zooms_the_keys_keeping_the_key_under_the_pointer() {
    let (mut v, doc, reg) = roll();
    let pos = Point::new(500.0, 250.0);
    let key_here = v.key_at(pos.y);
    let off = pos.y - v.key_center_y(key_here);
    roll_wheel(&mut v, &doc, &reg, pos, 3.0, alt());
    assert!(v.key_height() > KEY_HEIGHT);
    assert_eq!(v.key_at(pos.y), key_here, "the key stays under the pointer");
    let after = pos.y - v.key_center_y(key_here);
    assert!(
        after.abs() <= v.key_height() / 2.0 + 1e-6,
        "{off} -> {after}"
    );
    let big = v.key_height();
    roll_wheel(&mut v, &doc, &reg, pos, -6.0, alt());
    assert!(v.key_height() < big);
    // Ctrl+wheel is still the time zoom and leaves the keys alone.
    let (kh, ppt) = (v.key_height(), v.px_per_tick());
    roll_wheel(&mut v, &doc, &reg, pos, 2.0, Modifiers::CTRL);
    assert_eq!(v.key_height(), kh);
    assert!(v.px_per_tick() > ppt);
}

#[test]
fn the_key_commands_step_and_reset_the_height() {
    let (mut v, doc, reg) = roll();
    assert!(
        v.command("editor.zoom_keys_in", None, &doc, &reg)
            .is_taken()
    );
    assert!((v.key_height() - KEY_HEIGHT * 1.25).abs() < 1e-9);
    assert!(
        v.command("editor.zoom_keys_out", None, &doc, &reg)
            .is_taken()
    );
    assert!((v.key_height() - KEY_HEIGHT).abs() < 1e-9);
    for _ in 0..30 {
        v.command("editor.zoom_keys_in", None, &doc, &reg);
    }
    assert_eq!(v.key_height(), MAX_KEY_HEIGHT);
    assert!(
        v.command("editor.zoom_keys_reset", None, &doc, &reg)
            .is_taken()
    );
    assert_eq!(v.key_height(), KEY_HEIGHT);
    assert_eq!(
        v.command("sequencer.zoom_tracks_in", None, &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn notes_keys_and_hit_testing_follow_the_key_height() {
    let (mut v, doc, reg) = roll();
    for kh in [MIN_KEY_HEIGHT, 9.0, KEY_HEIGHT, 24.0, MAX_KEY_HEIGHT] {
        v.set_key_height(kh);
        // Fit the clip: the notes come into view whatever the key height.
        let _ = v.command("sequencer.zoom_selection", None, &doc, &reg);
        v.set_key_height(kh);
        // The first note in view (the grid spans y 48..484, the keyboard 56 px on the left).
        let i = (0..v.notes().len())
            .find(|i| {
                let r = v.note_rect(*i).unwrap();
                r.y0 > 49.0 && r.y1 < 483.0 && r.x0 > 60.0 && r.x1 < 990.0
            })
            .expect("a note is in view");
        let r = v.note_rect(i).unwrap();
        assert!((r.height() - kh).abs() < 1e-9, "{kh}");
        let key0 = v.notes()[i].key;
        assert!((r.center().y - v.key_center_y(key0)).abs() < 1e-9);
        assert_eq!(v.key_at(r.center().y), key0, "{kh}");
        assert_eq!(v.key_at(r.y0 + 0.01), key0);
        assert_eq!(v.key_at(r.y1 - 0.01), key0);
        // A click on the note selects it, at every height.
        let p = r.center();
        let _ = v.handle(&down(p), &doc, &reg);
        let _ = v.handle(&up(p), &doc, &reg);
        assert!(v.selected_notes().contains(&i), "{kh}");
        v.set_selected_notes(&[]);
        // The keyboard strip: the key under a point on it is the one drawn there.
        let kb = v.keyboard_point(key0);
        assert!((kb.y - v.key_center_y(key0)).abs() < 1e-9);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        assert!(!dl.is_empty());
    }
}

#[test]
fn the_pencil_draws_on_the_key_under_the_pointer_at_any_key_height() {
    use atomik_ui::Tool;
    for kh in [MIN_KEY_HEIGHT, 30.0, MAX_KEY_HEIGHT] {
        let (mut v, doc, reg) = roll();
        v.set_tool(Tool::Pencil);
        v.set_key_height(kh);
        // A key in view that no note of the clip uses, so the press draws.
        let used: Vec<u8> = v.notes().iter().map(|n| n.key).collect();
        let target = (v.key_at(450.0)..=v.key_at(60.0))
            .find(|k| !used.contains(k))
            .unwrap();
        let (x, y) = (v.tick_to_x(2 * PPQ + 123), v.key_center_y(target));
        let acts = run(
            |e| v.handle(e, &doc, &reg),
            &[down(Point::new(x, y)), up(Point::new(x, y))],
        );
        let drawn: Vec<u8> = song_commands(&acts)
            .into_iter()
            .filter_map(|c| match c {
                SongCommand::SetClipContent {
                    content: atomik_model::ClipContent::Notes(n),
                    ..
                } => Some(n),
                _ => None,
            })
            .flatten()
            .filter(|n| n.tick >= 2 * PPQ)
            .map(|n| n.key)
            .collect();
        assert!(drawn.contains(&target), "{kh}: {drawn:?}");
    }
}
