//! The arrangement records its lanes and its track list into display lists that survive from
//! one frame to the next. A frame in which only the playhead (or a track meter) moved replays
//! them; every other change re-records them, and what is drawn is always what a paint without
//! the cache draws.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipId, RackDoc};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, DisplayList, Key, MeterSource, Modifiers, Theme, Tool, TrackLevel, UiEvent,
};
use proptest::prelude::*;
use support::audio::*;
use support::song::*;
use support::*;

fn setup() -> (RackDoc, Registry, ArrangementView) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    (doc, reg, v)
}

/// A paint with the layers as they are.
fn paint(v: &mut ArrangementView, doc: &RackDoc, reg: &Registry) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

/// A paint that records every layer afresh: what the cached paint has to equal.
fn reference(v: &mut ArrangementView, doc: &RackDoc, reg: &Registry) -> String {
    v.set_layer_cache(false);
    let out = paint(v, doc, reg);
    v.set_layer_cache(true);
    out
}

/// Paints once, runs `change`, paints again: the layers were recorded again and the picture is
/// the reference one.
fn assert_rerecords(
    what: &str,
    v: &mut ArrangementView,
    doc: &RackDoc,
    reg: &Registry,
    change: impl FnOnce(&mut ArrangementView),
) {
    paint(v, doc, reg);
    let before = v.layer_records();
    change(v);
    assert!(v.needs_repaint(), "{what}: asks for a repaint");
    let cached = paint(v, doc, reg);
    assert!(
        v.layer_records().0 > before.0,
        "{what}: the lanes were recorded again"
    );
    assert_eq!(
        cached,
        reference(v, doc, reg),
        "{what}: the cached paint draws what a fresh one does"
    );
}

#[test]
fn playback_frames_replay_the_recorded_lanes_and_track_list() {
    let (doc, reg, mut v) = setup();
    paint(&mut v, &doc, &reg);
    let recorded = v.layer_records();
    assert_eq!(recorded, (1, 1));
    // Two bars of playback, a frame per 20 ms.
    for i in 1..=200_i64 {
        v.set_playhead(i * 700);
        paint(&mut v, &doc, &reg);
    }
    assert_eq!(
        v.layer_records(),
        recorded,
        "200 playback frames recorded nothing"
    );
    let cached = paint(&mut v, &doc, &reg);
    assert_eq!(cached, reference(&mut v, &doc, &reg));
}

#[test]
fn a_moving_playhead_asks_for_a_repaint_but_not_a_new_recording() {
    let (doc, reg, mut v) = setup();
    paint(&mut v, &doc, &reg);
    v.set_playhead(BAR);
    assert!(v.needs_repaint());
    paint(&mut v, &doc, &reg);
    assert!(!v.needs_repaint());
    assert_eq!(v.layer_records(), (1, 1));
}

#[test]
fn the_playhead_line_is_drawn_over_the_lanes_in_the_same_place_as_before() {
    let (doc, reg, mut v) = setup();
    paint(&mut v, &doc, &reg);
    v.set_playhead(2 * BAR);
    let cached = paint(&mut v, &doc, &reg);
    assert_eq!(cached, reference(&mut v, &doc, &reg));
    // And the line moved with it.
    v.set_playhead(3 * BAR);
    let later = paint(&mut v, &doc, &reg);
    assert_ne!(cached, later);
}

#[test]
fn a_recording_playhead_changes_colour_without_a_stale_layer() {
    let (doc, reg, mut v) = setup();
    assert_rerecords("recording", &mut v, &doc, &reg, |v| v.set_recording(true));
}

#[test]
fn scroll_zoom_and_size_rerecord() {
    let (doc, reg, mut v) = setup();
    let at = Point::new(600.0, 200.0);
    assert_rerecords("scroll", &mut v, &doc, &reg, |v| {
        v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(-3.0, 0.0),
                pos: at,
                modifiers: Modifiers::SHIFT,
            },
            &doc,
            &reg,
        );
    });
    assert_rerecords("zoom", &mut v, &doc, &reg, |v| {
        v.handle(
            &UiEvent::Wheel {
                delta: Vec2::new(0.0, 2.0),
                pos: at,
                modifiers: Modifiers::CTRL,
            },
            &doc,
            &reg,
        );
    });
    assert_rerecords("size", &mut v, &doc, &reg, |v| {
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 380.0));
    });
    assert_rerecords("scale", &mut v, &doc, &reg, |v| v.set_scale(2.0));
}

#[test]
fn selection_hover_tool_snap_and_theme_rerecord() {
    let (doc, reg, mut v) = setup();
    let r = v.clip_rect(ClipId(1)).unwrap();
    let inside = Point::new(r.center().x, r.y1 - 6.0);
    assert_rerecords("hover", &mut v, &doc, &reg, |v| {
        v.handle(&mv(inside), &doc, &reg);
    });
    assert_rerecords("selection", &mut v, &doc, &reg, |v| {
        v.handle(&down(inside), &doc, &reg);
        v.handle(&up(inside), &doc, &reg);
    });
    assert!(!v.selected_clips().is_empty());
    assert_rerecords("tool", &mut v, &doc, &reg, |v| v.set_tool(Tool::Pencil));
    assert_rerecords("snap", &mut v, &doc, &reg, |v| v.set_snap(Snap::Beat));
    assert_rerecords("theme", &mut v, &doc, &reg, |v| {
        v.set_theme(Theme::light());
    });
}

#[test]
fn a_drag_in_progress_rerecords_on_every_move() {
    let (doc, reg, mut v) = setup();
    v.set_tool(Tool::Select);
    let r = v.clip_rect(ClipId(1)).unwrap();
    let start = Point::new(r.center().x, r.y1 - 6.0);
    v.handle(&down(start), &doc, &reg);
    for dx in [20.0, 70.0, 140.0] {
        let to = Point::new(start.x + dx, start.y);
        assert_rerecords("drag", &mut v, &doc, &reg, |v| {
            v.handle(&mv(to), &doc, &reg);
        });
    }
    v.handle(&up(Point::new(start.x + 140.0, start.y)), &doc, &reg);
}

#[test]
fn a_document_change_rerecords() {
    let (mut doc, reg, mut v) = setup();
    paint(&mut v, &doc, &reg);
    let before = v.layer_records();
    doc.song_mut().tracks[0].lanes[0].clips.push(notes_clip(
        99,
        6 * BAR,
        BAR,
        vec![n(0, PPQ, 60, 0.8)],
    ));
    let cached = paint(&mut v, &doc, &reg);
    assert!(v.layer_records().0 > before.0);
    assert_eq!(cached, reference(&mut v, &doc, &reg));
}

#[test]
fn another_document_revision_rerecords_even_when_the_view_saw_no_change() {
    let (doc, reg, mut v) = setup();
    v.set_doc_revision(Some(1));
    paint(&mut v, &doc, &reg);
    paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records().0, 1);
    v.set_doc_revision(Some(2));
    paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records().0, 2, "the revision is part of the stamp");
    v.set_doc_revision(Some(2));
    paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records().0, 2);
}

#[test]
fn a_repaint_requested_from_outside_rerecords() {
    let (doc, reg, mut v) = setup();
    assert_rerecords("request", &mut v, &doc, &reg, |v| v.request_repaint());
}

#[test]
fn a_meter_re_records_the_track_list_and_leaves_the_lanes() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records(), (1, 1));
    let level = |p: f32| TrackLevel {
        device: key("audio-track"),
        source: MeterSource::Playback,
        peaks: [p, p],
        stereo: true,
    };
    v.feed_track_levels(&[level(0.5)], 0.0);
    assert!(v.needs_repaint(), "a visible meter moved");
    let cached = paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records(), (1, 2), "only the track list");
    assert_eq!(cached, reference(&mut v, &doc, &reg));
    let lanes = v.layer_records().0;
    v.feed_track_levels(&[level(0.1)], 0.1);
    let cached = paint(&mut v, &doc, &reg);
    assert_eq!(v.layer_records().0, lanes);
    assert_eq!(cached, reference(&mut v, &doc, &reg));
}

#[test]
fn an_unchanged_view_painted_twice_records_once() {
    let (doc, reg, mut v) = setup();
    let a = paint(&mut v, &doc, &reg);
    let b = paint(&mut v, &doc, &reg);
    assert_eq!(a, b);
    assert_eq!(v.layer_records(), (1, 1));
}

#[derive(Debug, Clone)]
enum Ev {
    Move(f64, f64),
    Down(f64, f64, bool),
    Up(f64, f64),
    Wheel(f64, f64, f64, f64, u8),
    Key(u8, bool),
    Playhead(i64),
    Leave,
}

fn arb_event() -> impl Strategy<Value = Ev> {
    prop_oneof![
        (0.0..1200.0, 0.0..420.0).prop_map(|(x, y)| Ev::Move(x, y)),
        (0.0..1200.0, 0.0..420.0, any::<bool>()).prop_map(|(x, y, c)| Ev::Down(x, y, c)),
        (0.0..1200.0, 0.0..420.0).prop_map(|(x, y)| Ev::Up(x, y)),
        (0.0..1200.0, 0.0..420.0, -4.0..4.0, -4.0..4.0, 0_u8..4)
            .prop_map(|(x, y, dx, dy, m)| Ev::Wheel(x, y, dx, dy, m)),
        (0_u8..24, any::<bool>()).prop_map(|(k, s)| Ev::Key(k, s)),
        (0_i64..8 * BAR).prop_map(Ev::Playhead),
        Just(Ev::Leave),
    ]
}

fn modifiers(m: u8) -> Modifiers {
    Modifiers {
        ctrl: m & 1 != 0,
        shift: m & 2 != 0,
        ..Modifiers::NONE
    }
}

fn event(e: &Ev) -> Option<UiEvent> {
    Some(match *e {
        Ev::Move(x, y) => mv(Point::new(x, y)),
        Ev::Down(x, y, c) => down_mods(Point::new(x, y), Modifiers::NONE, if c { 2 } else { 1 }),
        Ev::Up(x, y) => up(Point::new(x, y)),
        Ev::Wheel(x, y, dx, dy, m) => UiEvent::Wheel {
            delta: Vec2::new(dx, dy),
            pos: Point::new(x, y),
            modifiers: modifiers(m),
        },
        Ev::Key(k, pressed) => {
            let key = match k {
                0 => Key::Escape,
                1 => Key::Up,
                2 => Key::Down,
                3 => Key::Left,
                4 => Key::Right,
                5 => Key::Delete,
                6 => Key::Home,
                7 => Key::End,
                8 => Key::Tab,
                9 => Key::Space,
                n => Key::Char(char::from(b'a' + (n - 10))),
            };
            UiEvent::Key {
                key,
                modifiers: Modifiers::NONE,
                pressed,
            }
        }
        Ev::Leave => UiEvent::PointerLeave,
        Ev::Playhead(_) => return None,
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// The cached paint is the paint of a view that records every layer on every frame, after
    /// any stream of events, playhead moves and paints.
    #[test]
    fn the_cached_paint_equals_the_uncached_one_on_any_event_stream(
        events in proptest::collection::vec(arb_event(), 1..40)
    ) {
        let (doc, reg) = (audio_doc(), registry());
        let mut v = audio_arrangement(&doc, &reg);
        paint(&mut v, &doc, &reg);
        for e in &events {
            match event(e) {
                Some(ev) => {
                    v.handle(&ev, &doc, &reg);
                }
                None => {
                    if let Ev::Playhead(t) = e {
                        v.set_playhead(*t);
                    }
                }
            }
            let cached = paint(&mut v, &doc, &reg);
            let fresh = reference(&mut v, &doc, &reg);
            prop_assert_eq!(cached, fresh, "after {:?}", e);
        }
    }
}
