//! PianoRollView behaviour through its public contract: drawing, selecting,
//! moving, resizing, velocity editing, delete, quantize, audition and validity of every
//! emitted note list.

mod support;

use atomik_model::{ClipContent, ClipId, Command, Note, SongCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{
    Key, Modifiers, PianoRollView, PointerButton, QuantizeAction, Tool, TransportAction, UiAction,
};
use proptest::prelude::*;
use support::song::*;
use support::*;

/// Every note list emitted for clip 1, with its merge id.
fn contents(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                id: ClipId(1),
                content: ClipContent::Notes(n),
                merge,
            } => Some((n.into_vec(), merge)),
            _ => None,
        })
        .collect()
}

fn auditions(actions: &[UiAction]) -> Vec<(u8, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Audition { device, key, on } => {
                assert_eq!(device, &key_of("bass"));
                Some((*key, *on))
            }
            _ => None,
        })
        .collect()
}

fn key_of(k: &str) -> atomik_model::DeviceKey {
    key(k)
}

fn grid_point(v: &PianoRollView, tick: i64, key: u8) -> Point {
    Point::new(v.tick_to_x(tick), v.key_center_y(key))
}

fn assert_valid(notes: &[Note]) {
    for n in notes {
        assert!(n.key <= 127 && n.length > 0 && n.tick >= 0, "{n:?}");
        assert!(n.velocity > 0.0 && n.velocity <= 1.0, "{n:?}");
    }
    for w in notes.windows(2) {
        assert!(
            (w[0].tick, w[0].key) <= (w[1].tick, w[1].key),
            "unsorted {w:?}"
        );
    }
}

#[test]
fn opens_on_a_note_clip_and_fits_it() {
    let (doc, reg) = (song_doc(), registry());
    let v = piano_roll(&doc, &reg);
    assert_eq!(v.clip(), Some(ClipId(1)));
    let notes = bass_line();
    let r0 = v.note_rect(0).unwrap();
    assert!((r0.x0 - v.tick_to_x(0)).abs() < 1e-6);
    assert!((r0.width() - (v.tick_to_x(notes[0].length) - v.tick_to_x(0))).abs() < 1e-6);
    assert!(
        v.key_center_y(48) < v.key_center_y(36),
        "higher keys are higher up"
    );
    assert_eq!(v.key_at(v.key_center_y(43)), 43);
    for i in 0..notes.len() {
        let r = v.note_rect(i).unwrap();
        assert!(
            ROLL_BOUNDS.contains(r.center()),
            "note {i} is visible: {r:?}"
        );
    }
    assert!(v.note_rect(notes.len()).is_none());
}

#[test]
fn clicking_empty_space_draws_a_snapped_note_and_auditions_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Pencil);
    v.set_snap(Snap::Sixteenth);
    let p = grid_point(&v, 2 * PPQ + PPQ / 4 + 300, 50);
    let a = v.handle(&down(p), &doc, &reg);
    let c = contents(&a);
    assert_eq!(c.len(), 1);
    let (notes, merge) = &c[0];
    assert!(merge.is_some());
    assert_valid(notes);
    assert_eq!(notes.len(), 17);
    let added = Note::new(2 * PPQ + PPQ / 4, PPQ / 4, 50, 0.8);
    let i = notes.iter().position(|n| *n == added).expect("new note");
    assert_eq!(v.selected_notes(), vec![i]);
    assert_eq!(auditions(&a), vec![(50, true)]);
    let a = v.handle(&up(p), &doc, &reg);
    assert_eq!(auditions(&a), vec![(50, false)]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn dragging_while_drawing_sets_the_length() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Pencil);
    v.set_snap(Snap::Sixteenth);
    let p = grid_point(&v, 8 * PPQ + 10, 52);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(beat_px, 0.0))],
    );
    let c = contents(&a);
    let last = &c.last().unwrap().0;
    let n = last.iter().find(|n| n.key == 52).unwrap();
    assert_eq!((n.tick, n.length), (8 * PPQ, PPQ));
    assert_eq!(c[0].1, c.last().unwrap().1, "one merge group");
}

#[test]
fn dragging_a_note_moves_it_in_time_and_key_with_audition() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let to = Point::new(p.x + beat_px * 4.2, v.key_center_y(38));
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(1.0, 0.0)), mv(to)],
    );
    let c = contents(&a);
    assert_eq!(c.len(), 1, "the 1 px wiggle emits nothing");
    let notes = &c[0].0;
    assert_valid(notes);
    let moved = Note {
        tick: 4 * PPQ,
        key: 38,
        ..bass_line()[0]
    };
    let i = notes.iter().position(|n| *n == moved).expect("moved note");
    assert_eq!(v.selected_notes(), vec![i]);
    assert!(!notes.contains(&bass_line()[0]));
    assert_eq!(auditions(&a), vec![(36, true), (36, false), (38, true)]);
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(auditions(&a), vec![(38, false)]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn a_selected_group_moves_together_and_keeps_its_shape_at_the_key_limit() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_selected_notes(&(0..16).collect::<Vec<_>>());
    let r = v.note_rect(5).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(Point::new(p.x, p.y + 4000.0))],
    );
    let c = contents(&a);
    let notes = &c.last().unwrap().0;
    assert_valid(notes);
    let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    let mut expected: Vec<u8> = bass_line().iter().map(|n| n.key - 36).collect();
    keys.sort_unstable();
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert_eq!(v.selected_notes().len(), 16);
}

#[test]
fn right_edge_resizes_on_the_grid() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.x1 - 2.0, r.center().y);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(beat_px, 0.0))],
    );
    let c = contents(&a);
    let notes = &c.last().unwrap().0;
    assert_eq!(notes[0].length, 2 * PPQ);
    assert_eq!(notes[0].tick, 0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[mv(p - Vec2::new(beat_px * 5.0, 0.0)), up(p)],
    );
    let c = contents(&a);
    assert_eq!(c.last().unwrap().0[0].length, PPQ, "at least one step");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn shift_drag_rubber_bands_without_commands() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let a0 = Point::new(v.tick_to_x(0) + 1.0, v.key_center_y(56));
    let a1 = Point::new(v.tick_to_x(4 * PPQ) - 30.0, v.key_center_y(32));
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(a0, Modifiers::SHIFT, 1),
            mv_with(a1, Modifiers::SHIFT),
            up(a1),
        ],
    );
    assert!(commands(&a).is_empty());
    assert!(auditions(&a).is_empty());
    assert_eq!(v.selected_notes(), vec![0, 1, 2, 3]);
}

#[test]
fn velocity_lane_drag_changes_velocity() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let p = v.velocity_point(5, 0.25).unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    let c = contents(&a);
    let notes = &c[0].0;
    assert!(
        (notes[5].velocity - 0.25).abs() < 0.02,
        "{}",
        notes[5].velocity
    );
    assert_eq!(notes[4], bass_line()[4]);
    let merge = c[0].1;
    assert!(merge.is_some());
    let p2 = v.velocity_point(5, 0.9).unwrap();
    let a = v.handle(&mv(p2), &doc, &reg);
    let c = contents(&a);
    assert!((c[0].0[5].velocity - 0.9).abs() < 0.02);
    assert_eq!(c[0].1, merge);
    assert_eq!(v.handle(&up(p2), &doc, &reg), vec![UiAction::EndMerge]);
    // With 5 and 6 selected, both shift by the same amount.
    v.set_selected_notes(&[5, 6]);
    let base = bass_line();
    let target = base[5].velocity - 0.2;
    let a = v.handle(&down(v.velocity_point(5, target).unwrap()), &doc, &reg);
    let n = &contents(&a)[0].0;
    assert!((n[6].velocity - (base[6].velocity - 0.2)).abs() < 0.02);
    assert!(auditions(&a).is_empty());
}

fn off_grid_doc() -> atomik_model::RackDoc {
    let mut doc = song_doc();
    doc.song_mut().tracks[0].lanes[0].clips[0].content = ClipContent::Notes(
        vec![
            n(100, PPQ, 60, 0.8),
            n(PPQ - 200, PPQ, 62, 0.8),
            n(2 * PPQ + 1000, PPQ, 64, 0.8),
        ]
        .into(),
    );
    doc
}

#[test]
fn quantize_asks_the_app_for_the_panels_last_settings() {
    let (doc, reg) = (off_grid_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let apply = UiAction::Quantize(QuantizeAction::ApplyLast);
    // The key and the button say the same thing; the app owns the settings and the notes.
    let a = v.command("editor.quantize", None, &doc, &reg);
    assert_eq!(a.actions(), std::slice::from_ref(&apply));
    let q = v.quantize_button_center();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(q), up(q)]);
    assert_eq!(a, vec![apply]);
}

#[test]
fn quantize_options_open_from_the_neighbour_button_and_a_right_click() {
    let (doc, reg) = (off_grid_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let options = UiAction::Quantize(QuantizeAction::Options);
    let o = v.quantize_options_button_center();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(o), up(o)]);
    assert_eq!(a, vec![options.clone()]);
    let q = v.quantize_button_center();
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_with(q, PointerButton::Secondary, 1)],
    );
    assert_eq!(a, vec![options]);
}

#[test]
fn delete_removes_the_selection_and_arrows_transpose() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_selected_notes(&[0, 2]);
    let a = v.command("edit.delete", None, &doc, &reg);
    let notes = &contents(a.actions())[0].0;
    assert_eq!(notes.len(), 14);
    assert!(!notes.contains(&bass_line()[0]) && !notes.contains(&bass_line()[2]));
    assert!(
        v.selected_notes().is_empty(),
        "deleted notes leave the selection"
    );
    v.set_selected_notes(&[0, 2]);
    let a = v.command("editor.octave_up", None, &doc, &reg);
    let notes = &contents(a.actions())[0].0;
    assert_eq!(notes[0].key, 48);
    assert_eq!(notes[2].key, 55);
    assert_eq!(notes[1], bass_line()[1]);
}

#[test]
fn keyboard_strip_auditions_and_glides() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let (p, q) = (v.keyboard_point(40), v.keyboard_point(42));
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(q), up(q)]);
    assert!(commands(&a).is_empty());
    assert_eq!(
        auditions(&a),
        vec![(40, true), (40, false), (42, true), (42, false)]
    );
}

#[test]
fn escape_clears_then_closes_and_the_close_button_closes() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_selected_notes(&[1]);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(a.is_empty() && v.selected_notes().is_empty());
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(a, vec![UiAction::ClosePianoRoll]);
    let c = v.close_button_center();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(c), up(c)]);
    assert_eq!(a, vec![UiAction::ClosePianoRoll]);
}

#[test]
fn ruler_click_locates_and_transport_keys_pass_through() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    let p = Point::new(v.tick_to_x(2 * PPQ + 100), v.ruler_y());
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        a,
        vec![UiAction::Transport(TransportAction::Locate(2 * PPQ))]
    );
    // The transport and undo keys are registry commands the shell resolves, not view keys.
    let a = v.handle(&press(Key::Space, Modifiers::NONE), &doc, &reg);
    assert!(a.is_empty());
    let a = v.handle(&press(Key::Char('z'), Modifiers::CTRL), &doc, &reg);
    assert!(a.is_empty());
}

#[test]
fn a_closed_roll_ignores_input() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.close();
    assert_eq!(v.clip(), None);
    let p = Point::new(500.0, 300.0);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(a.is_empty());
    let _ = Command::SetTempo(120.0);
}

#[derive(Debug, Clone)]
enum Gesture {
    Click {
        x: f64,
        y: f64,
        shift: bool,
        ctrl: bool,
        alt: bool,
    },
    Drag {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        shift: bool,
        alt: bool,
    },
    Key(u8),
}

fn gesture() -> impl Strategy<Value = Gesture> {
    prop_oneof![
        (
            0.0..1.0f64,
            0.0..1.0f64,
            any::<bool>(),
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(x, y, shift, ctrl, alt)| Gesture::Click {
                x,
                y,
                shift,
                ctrl,
                alt
            }),
        (
            0.0..1.0f64,
            0.0..1.0f64,
            -600.0..600.0f64,
            -900.0..900.0f64,
            any::<bool>(),
            any::<bool>()
        )
            .prop_map(|(x, y, dx, dy, shift, alt)| Gesture::Drag {
                x,
                y,
                dx,
                dy,
                shift,
                alt
            }),
        (0u8..6).prop_map(Gesture::Key),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn every_emitted_note_list_is_valid_and_sorted(gs in prop::collection::vec(gesture(), 1..12)) {
        let (mut doc, reg) = (song_doc(), registry());
        let mut v = piano_roll(&doc, &reg);
        let b = ROLL_BOUNDS;
        for g in gs {
            let mut command_actions: Vec<UiAction> = Vec::new();
            let events = match g {
                Gesture::Click { x, y, shift, ctrl, alt } => {
                    let p = Point::new(b.x0 + x * b.width(), b.y0 + y * b.height());
                    let m = Modifiers { shift, ctrl, alt, meta: false };
                    vec![down_mods(p, m, 1), up(p)]
                }
                Gesture::Drag { x, y, dx, dy, shift, alt } => {
                    let p = Point::new(b.x0 + x * b.width(), b.y0 + y * b.height());
                    let m = Modifiers { shift, ctrl: false, alt, meta: false };
                    vec![
                        down_mods(p, m, 1),
                        mv_with(p + Vec2::new(dx / 2.0, dy / 2.0), m),
                        mv_with(p + Vec2::new(dx, dy), m),
                        up(p + Vec2::new(dx, dy)),
                    ]
                }
                Gesture::Key(k) => {
                    let id = [
                        "edit.delete",
                        "editor.quantize",
                        "editor.transpose_up",
                        "editor.octave_down",
                        "editor.nudge_left",
                        "editor.nudge_right",
                    ][k as usize];
                    command_actions = v.command(id, None, &doc, &reg).actions().to_vec();
                    Vec::new()
                }
            };
            let mut ons: i32 = 0;
            let mut batches: Vec<Vec<UiAction>> = vec![command_actions];
            batches.extend(events.iter().map(|e| v.handle(e, &doc, &reg)));
            for actions in batches {
                for a in &actions {
                    if let UiAction::Audition { on, key, .. } = a {
                        prop_assert!(*key <= 127);
                        ons += if *on { 1 } else { -1 };
                    }
                    if let UiAction::Command(Command::Song(SongCommand::SetClipContent {
                        id, content: ClipContent::Notes(notes), ..
                    })) = a {
                        prop_assert_eq!(*id, ClipId(1));
                        assert_valid(notes);
                        // Apply it, as the app would.
                        doc.song_mut().tracks[0].lanes[0].clips[0].content =
                            ClipContent::Notes(notes.clone());
                    }
                }
            }
            prop_assert_eq!(ons, 0, "every audition on is matched by an off");
        }
    }
}

// ---------------------------------------------------------------- dragging the playhead

/// The song with the bass clip (id 1) moved to bar 2, so content ticks and song ticks differ.
fn bass_at_bar_two() -> atomik_model::RackDoc {
    let mut doc = song_doc();
    for t in &mut doc.song_mut().tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(1) {
                    c.start = BAR;
                }
            }
        }
    }
    doc
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

#[test]
fn ruler_drag_keeps_locating_the_snapped_song_tick_once_per_change() {
    let (doc, reg) = (bass_at_bar_two(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    let y = v.ruler_y();
    let at = |t: i64| Point::new(v.tick_to_x(t), y);
    let events = [
        down(at(PPQ)),
        mv(at(PPQ + 100)),
        mv(at(2 * PPQ)),
        mv(at(2 * PPQ + 100)),
        mv(at(0)),
        up(at(0)),
    ];
    let a = run(|e| v.handle(e, &doc, &reg), &events);
    assert_eq!(locates(&a), vec![BAR + PPQ, BAR + 2 * PPQ, BAR]);
}

#[test]
fn playhead_line_drags_from_where_it_was_grabbed() {
    let (doc, reg) = (bass_at_bar_two(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    v.set_playhead(BAR + PPQ);
    let y = v.key_center_y(56);
    let x = v.tick_to_x(PPQ);
    let p = Point::new(x + 2.0, y);
    let _ = run(|e| v.handle(e, &doc, &reg), &[mv(p)]);
    assert_eq!(v.cursor(), atomik_ui::CursorHint::ResizeHorizontal);
    let step = PPQ as f64 * v.px_per_tick();
    let to = Point::new(x + 2.0 + step * 2.0, y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to), up(to)]);
    assert_eq!(locates(&a), vec![BAR + 3 * PPQ]);
    assert!(contents(&a).is_empty());
}

#[test]
fn pressing_away_from_the_playhead_line_or_on_a_note_does_not_drag_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_playhead(PPQ);
    let y = v.key_center_y(56);
    let p = Point::new(v.tick_to_x(PPQ) + 12.0, y);
    let to = Point::new(p.x + 60.0, y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to), up(to)]);
    assert!(locates(&a).is_empty());
    // A note starting under the line keeps its edge.
    v.set_playhead(PPQ);
    let r = v.note_rect(1).unwrap();
    let edge = Point::new(r.x0 + 1.0, r.center().y);
    let to = Point::new(edge.x + 40.0, edge.y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(edge), mv(to), up(to)]);
    assert!(locates(&a).is_empty(), "{a:?}");
}

fn right_click_note(
    v: &mut PianoRollView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    i: usize,
) -> (Point, Vec<UiAction>) {
    let at = v.note_rect(i).unwrap().center();
    let a = run(
        |e| v.handle(e, doc, reg),
        &[down_with(at, PointerButton::Secondary, 1)],
    );
    (at, a)
}

#[test]
fn right_click_on_an_unselected_note_with_select_selects_it_and_opens_the_notes_menu() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Select);
    v.set_selected_notes(&[0, 2]);
    let (at, a) = right_click_note(&mut v, &doc, &reg, 5);
    assert!(contents(&a).is_empty(), "no removal: {a:?}");
    assert_eq!(
        v.selected_notes(),
        vec![5],
        "the click replaces the selection"
    );
    assert_eq!(
        a,
        vec![UiAction::ContextMenu {
            target: atomik_ui::ContextTarget::Notes,
            at
        }]
    );
}

#[test]
fn right_click_on_a_selected_note_with_select_keeps_the_multi_selection() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Select);
    v.set_selected_notes(&[0, 2, 4]);
    let (at, a) = right_click_note(&mut v, &doc, &reg, 2);
    assert!(contents(&a).is_empty(), "no removal: {a:?}");
    assert_eq!(v.selected_notes(), vec![0, 2, 4]);
    assert_eq!(
        a,
        vec![UiAction::ContextMenu {
            target: atomik_ui::ContextTarget::Notes,
            at
        }]
    );
}

#[test]
fn right_click_on_a_note_with_pencil_still_removes_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Pencil);
    let (_, a) = right_click_note(&mut v, &doc, &reg, 1);
    let c = contents(&a);
    assert_eq!(c.len(), 1, "{a:?}");
    assert_eq!(c[0].0.len(), bass_line().len() - 1);
    assert!(!c[0].0.contains(&bass_line()[1]));
}

#[test]
fn dragging_or_wheeling_the_snap_selector_steps_the_resolutions() {
    use atomik_ui::UiEvent;
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    let s = v.snap_button_center();
    let end = Point::new(s.x, s.y - 10.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(s), mv(end), up(end)]);
    assert_eq!(v.snap(), Snap::Sixteenth);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(s), up(s)]);
    assert_eq!(
        v.snap(),
        Snap::ThirtySecond,
        "a click cycles forward as before"
    );
    let wheel = |dy: f64| UiEvent::Wheel {
        delta: Vec2::new(0.0, dy),
        pos: s,
        modifiers: Modifiers::NONE,
    };
    let _ = v.handle(&wheel(-1.0), &doc, &reg);
    assert_eq!(v.snap(), Snap::Sixteenth);
}

#[test]
fn a_dragged_note_ends_at_the_snapped_pointer_and_a_click_keeps_the_default_length() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_tool(Tool::Pencil);
    v.set_snap(Snap::Beat);
    let p = grid_point(&v, 8 * PPQ, 52);
    let (x_end, x_back) = (v.tick_to_x(11 * PPQ + PPQ / 5), v.tick_to_x(6 * PPQ));
    // Released at 11 beats and a bit: the end snaps to the beat grid.
    let to = Point::new(x_end, p.y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to), up(to)]);
    let n = *contents(&a)
        .last()
        .unwrap()
        .0
        .iter()
        .find(|n| n.key == 52)
        .unwrap();
    assert_eq!((n.tick, n.length), (8 * PPQ, 3 * PPQ));
    // Dragged back to the left of the start: one snap step.
    let to = Point::new(x_back, p.y);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to), up(to)]);
    let n = *contents(&a)
        .last()
        .unwrap()
        .0
        .iter()
        .find(|n| n.key == 52)
        .unwrap();
    assert_eq!(n.length, PPQ);
    // A click: the default length.
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let n = *contents(&a)
        .last()
        .unwrap()
        .0
        .iter()
        .find(|n| n.key == 52)
        .unwrap();
    assert_eq!(n.length, PPQ);
}
