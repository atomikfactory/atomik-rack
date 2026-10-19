//! SeqEditorView behaviour through its public contract: hit testing
//! of the note lane, the three bar lanes, the tie row and the toolbar, key set / move,
//! velocity / gate / probability bar drags, tie, length, resolution and swing, the pattern
//! slots (select, add, duplicate, remove, copy, paste, clear, rename), one command per gesture,
//! merged drags, Esc cancel, audition, the playing-step cursor, and display-list goldens.
//! Every emitted command is applied to a real model `Document`, so the model validates it.
//! Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test seq_editor`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, PatternDoc, RackDoc, SeqResolution, StepDoc};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton, SeqEditorView, UiAction, UiEvent};
use proptest::prelude::*;
use std::path::PathBuf;
use support::seq::*;
use support::song::{press, run};
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn setup() -> (Document, Registry, SeqEditorView) {
    let doc = players_doc();
    let reg = registry();
    let v = editor(&doc, &reg);
    (Document::new(doc), reg, v)
}

fn feed(v: &mut SeqEditorView, d: &Document, reg: &Registry, events: &[UiEvent]) -> Vec<UiAction> {
    run(|e| v.handle(e, d.doc(), reg), events)
}

macro_rules! click {
    ($v:ident, $d:expr, $reg:expr, $p:expr) => {{
        let p = $p;
        click_fn(&mut $v, $d, $reg, p)
    }};
}

macro_rules! click_with {
    ($v:ident, $d:expr, $reg:expr, $p:expr, $n:expr) => {{
        let p = $p;
        click_with_fn(&mut $v, $d, $reg, p, $n)
    }};
}

fn click_with_fn(
    v: &mut SeqEditorView,
    d: &Document,
    reg: &Registry,
    p: Point,
    clicks: u8,
) -> Vec<UiAction> {
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks,
        },
        d.doc(),
        reg,
    );
    out.extend(v.handle(&up(p), d.doc(), reg));
    out
}

fn click_fn(v: &mut SeqEditorView, d: &Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    click_with_fn(v, d, reg, p, 1)
}

/// Applies `actions` and syncs the view.
fn settle(
    v: &mut SeqEditorView,
    d: &mut Document,
    reg: &Registry,
    actions: &[UiAction],
) -> Vec<Option<u64>> {
    let (_, merges) = apply_all(d, reg, actions);
    v.sync(d.doc(), reg);
    merges
}

fn paint(v: &mut SeqEditorView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_bass_pattern_matches_golden() {
    let (d, _, mut v) = setup();
    check("seq_editor_bass_1x.json", &paint(&mut v, d.doc()).to_json());
}

#[test]
fn a_short_swung_eighth_pattern_matches_golden() {
    let (d, _, mut v) = setup();
    v.set_pattern_index(1);
    check("seq_editor_fill_1x.json", &paint(&mut v, d.doc()).to_json());
}

#[test]
fn an_empty_pattern_matches_golden() {
    let (d, _, mut v) = setup();
    v.set_pattern_index(2);
    check(
        "seq_editor_empty_1x.json",
        &paint(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_playing_cursor_matches_golden() {
    let (d, _, mut v) = setup();
    v.set_meter_values(&[0.0, 5.0, 1.0]);
    check(
        "seq_editor_playing_1x.json",
        &paint(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn a_closed_editor_paints_its_empty_state() {
    let reg = registry();
    let doc = players_doc();
    let mut v = SeqEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    let dl = paint(&mut v, &doc);
    assert!(
        dl.texts().iter().any(|t| t.contains("No sequencer")),
        "{:?}",
        dl.texts()
    );
    let d = Document::new(doc);
    assert!(
        feed(
            &mut v,
            &d,
            &reg,
            &[down(Point::new(500.0, 300.0)), up(Point::new(500.0, 300.0))]
        )
        .is_empty()
    );
    assert!(!v.is_present());
}

// ------------------------------------------------------------------ open, sync, present

#[test]
fn it_reads_the_edited_pattern_and_follows_the_document() {
    let (mut d, reg, mut v) = setup();
    assert!(v.is_present());
    assert_eq!(v.device(), Some(&key("seq")));
    assert_eq!(v.pattern_index(), 0);
    assert_eq!(v.pattern_count(), 3);
    assert_eq!(v.pattern(), Some(&bass()));
    v.set_pattern_index(1);
    assert_eq!(v.pattern(), Some(&fill()));
    v.set_pattern_index(9);
    assert_eq!(v.pattern_index(), 2, "clamped to the last pattern");
    // An external edit shows up after sync.
    let mut p = fill();
    p.swing = 50;
    let _ = d.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 1,
            pattern: p.clone(),
            merge: None,
        },
        &reg,
    );
    v.set_pattern_index(1);
    v.sync(d.doc(), &reg);
    assert_eq!(v.pattern(), Some(&p));
    // Removing the device ends the session; so does the wrong type.
    let _ = d.apply(Command::RemoveDevice { key: key("seq") }, &reg);
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
    assert_eq!(v.pattern(), None);
    let mut other = SeqEditorView::new(atomik_ui::Theme::dark());
    other.open(key("arp"));
    other.sync(&players_doc(), &reg);
    assert!(!other.is_present(), "an arpeggiator has no patterns");
}

#[test]
fn the_key_axis_fits_the_pattern_with_room_and_is_never_smaller_than_two_octaves() {
    let (d, reg, mut v) = setup();
    let (lo, hi) = v.key_range();
    assert!(lo <= 36 - 3 && hi >= 48 + 3, "{lo}..{hi}");
    assert!(hi - lo >= 23);
    v.set_pattern_index(2);
    let (lo, hi) = v.key_range();
    assert!(
        lo <= 60 && 60 <= hi && hi - lo >= 23,
        "empty pattern: {lo}..{hi}"
    );
    // Steps beyond the length do not count.
    let mut p = fill();
    p.steps[40] = step(120, 100, 80, 100);
    let mut doc = d;
    let _ = doc.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 1,
            pattern: p,
            merge: None,
        },
        &reg,
    );
    v.set_pattern_index(1);
    v.sync(doc.doc(), &reg);
    assert!(v.key_range().1 < 100, "{:?}", v.key_range());
    let (lo, hi) = v.key_range();
    assert!(hi <= 127 && lo <= hi);
}

// ------------------------------------------------------------------ note lane

#[test]
fn a_click_on_an_empty_step_switches_it_on_at_that_key_keeping_the_other_values() {
    let (mut d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.note_cell_center(1, 40));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "one command: {a:?}");
    let mut expect = bass();
    expect.steps[1] = StepDoc {
        on: true,
        key: 40,
        ..StepDoc::default()
    };
    assert_eq!(sets[0].0, 0);
    assert_eq!(sets[0].1, expect);
    assert!(sets[0].2.is_some(), "a drag merge id");
    assert!(a.contains(&UiAction::EndMerge));
    assert_eq!(auditions(&a), vec![(40, 100, true), (40, 100, false)]);
    let merges = settle(&mut v, &mut d, &reg, &a);
    assert_eq!(merges.len(), 1);
    assert_eq!(pattern_of(d.doc(), 0), expect);
    assert_eq!(v.pattern(), Some(&expect));
}

#[test]
fn a_click_on_an_on_step_at_the_same_key_switches_it_off_and_at_another_key_moves_it() {
    let (mut d, reg, mut v) = setup();
    // Step 5 is on at key 46.
    let a = click!(v, &d, &reg, v.note_cell_center(5, 46));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    let mut expect = bass();
    expect.steps[5].on = false;
    assert_eq!(sets[0].1, expect);
    settle(&mut v, &mut d, &reg, &a);
    // Click again (now off) at another key: on at that key.
    let a = click!(v, &d, &reg, v.note_cell_center(5, 50));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1);
    assert!(sets[0].1.steps[5].on && sets[0].1.steps[5].key == 50);
    settle(&mut v, &mut d, &reg, &a);
    // An on step clicked at another key moves its key and keeps its values.
    let a = click!(v, &d, &reg, v.note_cell_center(0, 41));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1);
    assert_eq!(
        sets[0].1.steps[0],
        StepDoc {
            key: 41,
            ..bass().steps[0]
        }
    );
}

#[test]
fn a_vertical_drag_in_a_column_moves_the_key_as_one_merged_undo_step() {
    let (mut d, reg, mut v) = setup();
    let col = 3;
    let pts = [
        v.note_cell_center(col, 36),
        v.note_cell_center(col, 38),
        v.note_cell_center(col, 41),
        v.note_cell_center(col, 44),
    ];
    let a = drag_through(|e| v.handle(e, d.doc(), &reg), &pts);
    let sets = set_patterns(&a);
    assert!(sets.len() >= 2, "{a:?}");
    let id = sets[0].2;
    assert!(id.is_some());
    assert!(sets.iter().all(|s| s.2 == id), "one merge id: {sets:?}");
    assert_eq!(sets.last().unwrap().1.steps[col].key, 44);
    assert_eq!(
        a.iter().filter(|x| matches!(x, UiAction::EndMerge)).count(),
        1
    );
    // The audition follows the key: every on has an off, and the last key is 44.
    let au = auditions(&a);
    assert_eq!(
        au.iter().filter(|x| x.2).count(),
        au.iter().filter(|x| !x.2).count()
    );
    assert_eq!(au.iter().rev().find(|x| x.2).map(|x| x.0), Some(44));
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!(pattern_of(d.doc(), 0).steps[col].key, 44);
    d.undo(&reg);
    assert_eq!(pattern_of(d.doc(), 0), bass(), "one undo step");
}

#[test]
fn escape_during_a_note_drag_restores_the_pattern_and_the_audition() {
    let (mut d, reg, mut v) = setup();
    let col = 3;
    let mut a = v.handle(&down(v.note_cell_center(col, 36)), d.doc(), &reg);
    a.extend(v.handle(&mv(v.note_cell_center(col, 45)), d.doc(), &reg));
    assert!(v.is_dragging());
    a.extend(v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg));
    assert!(!v.is_dragging());
    let sets = set_patterns(&a);
    assert_eq!(sets.last().unwrap().1, bass(), "the base comes back");
    assert!(a.iter().any(|x| matches!(x, UiAction::EndMerge)));
    let au = auditions(&a);
    assert_eq!(
        au.iter().filter(|x| x.2).count(),
        au.iter().filter(|x| !x.2).count()
    );
    assert!(
        !a.iter().any(|x| matches!(x, UiAction::CloseSeqEditor)),
        "{a:?}"
    );
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!(pattern_of(d.doc(), 0), bass());
    // The release after the cancel does nothing.
    assert!(
        v.handle(&up(Point::new(300.0, 200.0)), d.doc(), &reg)
            .is_empty()
    );
}

#[test]
fn the_key_header_auditions_its_key_until_release() {
    let (d, reg, mut v) = setup();
    let p = v.key_header_center(48);
    let a = v.handle(&down(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(48, 100, true)]);
    assert!(set_patterns(&a).is_empty());
    let a = v.handle(&up(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(48, 100, false)]);
}

#[test]
fn steps_beyond_the_length_are_outside_the_grid() {
    let (d, reg, mut v) = setup();
    v.set_pattern_index(1); // 12 steps
    let beyond = v.note_cell_center(12, 60);
    let a = click!(v, &d, &reg, beyond);
    assert!(set_patterns(&a).is_empty(), "{a:?}");
}

// ------------------------------------------------------------------ bar lanes

#[test]
fn a_press_in_the_velocity_lane_sets_the_bar_and_a_drag_paints_across_columns() {
    let (mut d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.velocity_point(2, 0.25));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    assert_eq!(sets[0].1.steps[2].velocity, 32);
    assert!(
        sets[0].1.steps[2].on == bass().steps[2].on,
        "only the value changes"
    );
    settle(&mut v, &mut d, &reg, &a);
    // Drag from step 0 to step 4 along a falling line.
    let pts = [
        v.velocity_point(0, 1.0),
        v.velocity_point(1, 0.8),
        v.velocity_point(2, 0.6),
        v.velocity_point(3, 0.4),
        v.velocity_point(4, 0.2),
    ];
    let a = drag_through(|e| v.handle(e, d.doc(), &reg), &pts);
    let sets = set_patterns(&a);
    let id = sets[0].2;
    assert!(sets.iter().all(|s| s.2 == id && id.is_some()));
    assert_eq!(
        a.iter().filter(|x| matches!(x, UiAction::EndMerge)).count(),
        1
    );
    let last = &sets.last().unwrap().1;
    for (k, frac) in [(0usize, 1.0f64), (1, 0.8), (2, 0.6), (3, 0.4), (4, 0.2)] {
        let want = (frac * 127.0).round().max(1.0) as u8;
        assert_eq!(last.steps[k].velocity, want, "step {k}");
    }
    settle(&mut v, &mut d, &reg, &a);
    d.undo(&reg);
    assert_eq!(
        pattern_of(d.doc(), 0).steps[2].velocity,
        32,
        "undo of the drag only"
    );
}

#[test]
fn a_fast_drag_fills_the_columns_it_skipped() {
    let (d, reg, mut v) = setup();
    let pts = [v.gate_point(0, 0.5), v.gate_point(8, 0.5)];
    let a = drag_through(|e| v.handle(e, d.doc(), &reg), &pts);
    let sets = set_patterns(&a);
    let last = &sets.last().unwrap().1;
    for k in 0..=8 {
        assert_eq!(last.steps[k].gate, 50, "step {k}");
    }
    assert_eq!(last.steps[9], bass().steps[9]);
}

#[test]
fn the_lanes_clamp_to_their_ranges() {
    let (d, reg, mut v) = setup();
    let top = |v: &SeqEditorView, f: fn(&SeqEditorView, usize, f64) -> Point| f(v, 1, 1.0);
    let bottom = |v: &SeqEditorView, f: fn(&SeqEditorView, usize, f64) -> Point| f(v, 1, 0.0);
    let a = click!(v, &d, &reg, top(&v, SeqEditorView::velocity_point));
    assert_eq!(set_patterns(&a)[0].1.steps[1].velocity, 127);
    let a = click!(v, &d, &reg, bottom(&v, SeqEditorView::velocity_point));
    assert_eq!(
        set_patterns(&a)[0].1.steps[1].velocity,
        1,
        "velocity 0 is not allowed"
    );
    let a = click!(v, &d, &reg, top(&v, SeqEditorView::gate_point));
    assert_eq!(set_patterns(&a)[0].1.steps[1].gate, 100);
    let a = click!(v, &d, &reg, bottom(&v, SeqEditorView::gate_point));
    assert_eq!(set_patterns(&a)[0].1.steps[1].gate, 1);
    // Step 8 plays with a probability of 50 %.
    let a = click!(v, &d, &reg, v.probability_point(8, 1.0));
    assert_eq!(set_patterns(&a)[0].1.steps[8].prob, 100);
    let a = click!(v, &d, &reg, bottom(&v, SeqEditorView::probability_point));
    assert_eq!(
        set_patterns(&a)[0].1.steps[1].prob,
        0,
        "probability 0 is allowed"
    );
    // A pointer far above or below the lane while dragging still clamps.
    let p = v.probability_point(2, 0.5);
    let a = drag_through(
        |e| v.handle(e, d.doc(), &reg),
        &[p, p + Vec2::new(0.0, 400.0), p + Vec2::new(0.0, -400.0)],
    );
    assert_eq!(set_patterns(&a).last().unwrap().1.steps[2].prob, 100);
}

#[test]
fn escape_during_a_lane_drag_restores_the_pattern() {
    let (mut d, reg, mut v) = setup();
    let mut a = v.handle(&down(v.velocity_point(0, 0.3)), d.doc(), &reg);
    a.extend(v.handle(&mv(v.velocity_point(5, 0.9)), d.doc(), &reg));
    a.extend(v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg));
    assert_eq!(set_patterns(&a).last().unwrap().1, bass());
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!(pattern_of(d.doc(), 0), bass());
    assert!(!v.is_dragging());
}

#[test]
fn a_click_on_a_tie_toggles_it_in_one_command() {
    let (mut d, reg, mut v) = setup();
    // Step 4 of the bass line is tied to step 5.
    let a = click!(v, &d, &reg, v.tie_center(4));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    assert!(!sets[0].1.steps[4].tie);
    assert_eq!(sets[0].2, None, "a click is a standalone command");
    settle(&mut v, &mut d, &reg, &a);
    let a = click!(v, &d, &reg, v.tie_center(7));
    assert!(set_patterns(&a)[0].1.steps[7].tie);
    // The tie sits between two neighbours: its point is at the column boundary side.
    assert!(v.tie_center(4).x < v.tie_center(5).x);
}

// ------------------------------------------------------------------ toolbar: length, resolution, swing

#[test]
fn the_length_buttons_step_the_length_within_one_to_sixty_four() {
    let (mut d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.length_button_center(true));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1);
    assert_eq!((sets[0].1.length, sets[0].2), (17, None));
    // Only the length changes: the 64 steps stay.
    let mut expect = bass();
    expect.length = 17;
    assert_eq!(sets[0].1, expect);
    settle(&mut v, &mut d, &reg, &a);
    let a = click!(v, &d, &reg, v.length_button_center(false));
    assert_eq!(set_patterns(&a)[0].1.length, 16);
    settle(&mut v, &mut d, &reg, &a);
    // At the limits nothing is emitted.
    let mut p = bass();
    p.length = 64;
    let _ = d.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 0,
            pattern: p,
            merge: None,
        },
        &reg,
    );
    v.sync(d.doc(), &reg);
    assert!(set_patterns(&click!(v, &d, &reg, v.length_button_center(true))).is_empty());
    let mut p = bass();
    p.length = 1;
    let _ = d.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 0,
            pattern: p,
            merge: None,
        },
        &reg,
    );
    v.sync(d.doc(), &reg);
    assert!(set_patterns(&click!(v, &d, &reg, v.length_button_center(false))).is_empty());
}

#[test]
fn the_length_field_drags_wheels_and_resets() {
    let (mut d, reg, mut v) = setup();
    let c = v.length_field_center();
    let a = drag_through(
        |e| v.handle(e, d.doc(), &reg),
        &[c, c + Vec2::new(4.0, 0.0), c + Vec2::new(8.0, 0.0)],
    );
    let sets = set_patterns(&a);
    assert!(!sets.is_empty());
    let id = sets[0].2;
    assert!(id.is_some() && sets.iter().all(|s| s.2 == id));
    assert!(sets.last().unwrap().1.length > 16);
    assert_eq!(
        a.iter().filter(|x| matches!(x, UiAction::EndMerge)).count(),
        1
    );
    settle(&mut v, &mut d, &reg, &a);
    d.undo(&reg);
    assert_eq!(pattern_of(d.doc(), 0).length, 16);
    v.sync(d.doc(), &reg);
    // Wheel steps by one.
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: c,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &reg,
    );
    assert_eq!(set_patterns(&a)[0].1.length, 17);
    // Double-click resets to 16 (no command when it already is).
    let a = click_with!(v, &d, &reg, c, 2);
    assert!(set_patterns(&a).is_empty(), "already 16: {a:?}");
    let mut p = fill();
    p.length = 5;
    let _ = d.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 1,
            pattern: p,
            merge: None,
        },
        &reg,
    );
    v.set_pattern_index(1);
    v.sync(d.doc(), &reg);
    let a = click_with!(v, &d, &reg, c, 2);
    assert_eq!(set_patterns(&a).last().unwrap().1.length, 16);
}

#[test]
fn each_resolution_button_sets_the_resolution_in_one_command() {
    let (mut d, reg, mut v) = setup();
    for res in SeqResolution::ALL {
        let a = click!(v, &d, &reg, v.resolution_button_center(res));
        if res == SeqResolution::Sixteenth {
            assert!(set_patterns(&a).is_empty(), "the current one: {a:?}");
            continue;
        }
        let sets = set_patterns(&a);
        assert_eq!(sets.len(), 1, "{res:?}: {a:?}");
        let mut expect = bass();
        expect.resolution = res;
        assert_eq!((&sets[0].1, sets[0].2), (&expect, None));
        settle(&mut v, &mut d, &reg, &a);
        // Back to 1/16 for the next one.
        let a = click!(
            v,
            &d,
            &reg,
            v.resolution_button_center(SeqResolution::Sixteenth)
        );
        settle(&mut v, &mut d, &reg, &a);
    }
}

#[test]
fn the_swing_field_drags_merges_clamps_wheels_and_resets() {
    let (mut d, reg, mut v) = setup();
    let c = v.swing_field_center();
    let a = drag_through(
        |e| v.handle(e, d.doc(), &reg),
        &[c, c + Vec2::new(10.0, 0.0), c + Vec2::new(25.0, 0.0)],
    );
    let sets = set_patterns(&a);
    let id = sets[0].2;
    assert!(id.is_some() && sets.iter().all(|s| s.2 == id));
    assert_eq!(sets.last().unwrap().1.swing, 25);
    assert_eq!(
        a.iter().filter(|x| matches!(x, UiAction::EndMerge)).count(),
        1
    );
    settle(&mut v, &mut d, &reg, &a);
    // Clamped at 75 whatever the drag.
    let a = drag_through(
        |e| v.handle(e, d.doc(), &reg),
        &[c, c + Vec2::new(500.0, 0.0)],
    );
    assert_eq!(set_patterns(&a).last().unwrap().1.swing, 75);
    settle(&mut v, &mut d, &reg, &a);
    // Esc restores the value of the press.
    let mut a = v.handle(&down(c), d.doc(), &reg);
    a.extend(v.handle(&mv(c + Vec2::new(-30.0, 0.0)), d.doc(), &reg));
    a.extend(v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg));
    assert_eq!(set_patterns(&a).last().unwrap().1.swing, 75);
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!(pattern_of(d.doc(), 0).swing, 75);
    // Wheel and double-click reset.
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: c,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &reg,
    );
    assert_eq!(set_patterns(&a)[0].1.swing, 74);
    let a = click_with!(v, &d, &reg, c, 2);
    assert_eq!(set_patterns(&a).last().unwrap().1.swing, 0);
}

// ------------------------------------------------------------------ pattern slots and commands

#[test]
fn the_slots_select_the_edited_pattern_and_report_it() {
    let (d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.slot_center(1));
    assert_eq!(v.pattern_index(), 1);
    assert_eq!(v.pattern(), Some(&fill()));
    assert!(
        a.contains(&UiAction::SelectPattern {
            device: key("seq"),
            index: 1
        }),
        "{a:?}"
    );
    assert!(commands_of(&a).is_empty());
    let a = click!(v, &d, &reg, v.slot_center(1));
    assert!(
        !a.iter()
            .any(|x| matches!(x, UiAction::SelectPattern { .. })),
        "no change"
    );
    // A slot without a pattern is dead.
    let a = click!(v, &d, &reg, v.slot_center(5));
    assert_eq!(v.pattern_index(), 1);
    assert!(a.is_empty(), "{a:?}");
}

fn commands_of(a: &[UiAction]) -> Vec<Command> {
    commands(a)
}

#[test]
fn add_duplicate_and_remove_work_like_the_panels() {
    let (mut d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Add));
    assert_eq!(
        commands_of(&a),
        vec![Command::AddPattern { device: key("seq") }]
    );
    assert!(a.contains(&UiAction::SelectPattern {
        device: key("seq"),
        index: 3
    }));
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!((v.pattern_count(), v.pattern_index()), (4, 3));
    let a = click!(
        v,
        &d,
        &reg,
        v.pattern_button_center(PatternButton::Duplicate)
    );
    assert_eq!(
        commands_of(&a),
        vec![Command::DuplicatePattern {
            device: key("seq"),
            index: 3
        }]
    );
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!((v.pattern_count(), v.pattern_index()), (5, 4));
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Remove));
    assert_eq!(
        commands_of(&a),
        vec![Command::RemovePattern {
            device: key("seq"),
            index: 4
        }]
    );
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!((v.pattern_count(), v.pattern_index()), (4, 3));
}

use atomik_ui::seq_editor::PatternButton;

#[test]
fn copy_paste_and_clear() {
    let (mut d, reg, mut v) = setup();
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Paste));
    assert!(commands_of(&a).is_empty(), "empty clipboard");
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Copy));
    assert!(commands_of(&a).is_empty());
    assert_eq!(v.clipboard(), Some(&bass()));
    v.set_pattern_index(2);
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Paste));
    let sets = set_patterns(&a);
    let mut expect = bass();
    expect.name = "Pattern 3".into();
    assert_eq!((sets[0].0, &sets[0].1, sets[0].2), (2, &expect, None));
    settle(&mut v, &mut d, &reg, &a);
    let a = click!(v, &d, &reg, v.pattern_button_center(PatternButton::Clear));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1);
    assert!(sets[0].1.steps.iter().all(|s| *s == StepDoc::default()));
    assert_eq!(sets[0].1.length, 16);
    // The clipboard can be set from outside (the panel's clipboard).
    v.set_clipboard(Some(fill()));
    assert_eq!(v.clipboard(), Some(&fill()));
}

#[test]
fn a_double_click_on_a_slot_renames_the_pattern_inline() {
    let (mut d, reg, mut v) = setup();
    let _ = click_with!(v, &d, &reg, v.slot_center(1), 2);
    assert_eq!(v.renaming(), Some("Fill"));
    for _ in 0..4 {
        let _ = v.handle(&press(Key::Backspace, Modifiers::NONE), d.doc(), &reg);
    }
    for ch in "Ride".chars() {
        let _ = v.handle(&press(Key::Char(ch), Modifiers::NONE), d.doc(), &reg);
    }
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    let sets = set_patterns(&a);
    let mut expect = fill();
    expect.name = "Ride".into();
    assert_eq!((sets[0].0, &sets[0].1, sets[0].2), (1, &expect, None));
    settle(&mut v, &mut d, &reg, &a);
    assert_eq!(v.renaming(), None);
    // Esc cancels without a command and without closing.
    let _ = click_with!(v, &d, &reg, v.slot_center(1), 2);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert!(a.is_empty(), "{a:?}");
    assert_eq!(v.renaming(), None);
}

#[test]
fn the_view_says_when_it_owns_the_keyboard_as_a_text_field() {
    // single-letter commands must not fire while a pattern name is typed.
    let (d, reg, mut v) = setup();
    assert!(!v.is_text_owner());
    let _ = click_with!(v, &d, &reg, v.slot_center(1), 2);
    assert!(v.is_text_owner(), "the name is being edited");
    for ch in "rl1k s".chars() {
        let key = if ch == ' ' { Key::Space } else { Key::Char(ch) };
        let _ = v.handle(&press(key, Modifiers::NONE), d.doc(), &reg);
    }
    assert!(
        v.renaming().is_some_and(|t| t.ends_with("rl1k s")),
        "{:?}",
        v.renaming()
    );
    let _ = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert!(!v.is_text_owner(), "Esc gives the field up");
}

// ------------------------------------------------------------------ cursor, close, keys, scroll

#[test]
fn the_cursor_shows_the_playing_step_of_the_edited_pattern_only() {
    let (_, _, mut v) = setup();
    assert_eq!(v.cursor(), None);
    v.set_meter_values(&[0.0, 5.0, 1.0]);
    assert_eq!(v.cursor(), Some(5));
    v.set_meter_values(&[1.0, 5.0, 1.0]);
    assert_eq!(v.cursor(), None, "another pattern is playing");
    v.set_meter_values(&[0.0, 5.0, 0.0]);
    assert_eq!(v.cursor(), None, "stopped");
    v.set_meter_values(&[0.0, 99.0, 1.0]);
    assert_eq!(v.cursor(), None, "beyond the length");
    v.set_meter_values(&[]);
    assert_eq!(v.cursor(), None);
}

#[test]
fn a_changed_cursor_asks_for_a_repaint_and_an_unchanged_one_does_not() {
    let (d, _, mut v) = setup();
    let _ = paint(&mut v, d.doc());
    assert!(!v.needs_repaint());
    v.set_meter_values(&[0.0, 5.0, 1.0]);
    assert!(v.needs_repaint());
    let _ = paint(&mut v, d.doc());
    v.set_meter_values(&[0.0, 5.0, 1.0]);
    assert!(!v.needs_repaint());
    v.set_meter_values(&[0.0, 6.0, 1.0]);
    assert!(v.needs_repaint());
}

#[test]
fn escape_and_the_close_button_close_the_editor_and_undo_redo_pass_through() {
    let (d, reg, mut v) = setup();
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert_eq!(a, vec![UiAction::CloseSeqEditor]);
    let a = click!(v, &d, &reg, v.close_button_center());
    assert_eq!(a, vec![UiAction::CloseSeqEditor]);
    let a = v.handle(&press(Key::Char('z'), Modifiers::CTRL), d.doc(), &reg);
    assert_eq!(a, vec![UiAction::Undo]);
    let a = v.handle(
        &press(
            Key::Char('z'),
            Modifiers {
                shift: true,
                ctrl: true,
                ..Modifiers::NONE
            },
        ),
        d.doc(),
        &reg,
    );
    assert_eq!(a, vec![UiAction::Redo]);
    let a = v.handle(&press(Key::Char('y'), Modifiers::CTRL), d.doc(), &reg);
    assert_eq!(a, vec![UiAction::Redo]);
}

#[test]
fn the_wheel_scrolls_the_note_lane_and_a_long_pattern_scrolls_sideways() {
    let (mut d, reg, mut v) = setup();
    let y0 = v.scroll_y();
    let p = v.note_cell_center(2, 48);
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -2.0),
            pos: p,
            modifiers: Modifiers::NONE,
        },
        d.doc(),
        &reg,
    );
    assert!(v.scroll_y() >= y0);
    // 64 steps in a narrow pane scroll horizontally.
    let mut pat = bass();
    pat.length = 64;
    let _ = d.apply(
        Command::SetPattern {
            device: key("seq"),
            index: 0,
            pattern: pat,
            merge: None,
        },
        &reg,
    );
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 600.0, 560.0));
    v.sync(d.doc(), &reg);
    assert_eq!(v.scroll_x(), 0.0);
    let p = v.note_cell_center(2, 48);
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -3.0),
            pos: p,
            modifiers: Modifiers::SHIFT,
        },
        d.doc(),
        &reg,
    );
    assert!(v.scroll_x() > 0.0, "{}", v.scroll_x());
    // The cell centres move with the scroll.
    let c63 = v.note_cell_center(63, 48);
    assert!(c63.x > 0.0);
}

// ------------------------------------------------------------------ the model accepts everything

#[derive(Debug, Clone)]
enum Act {
    Note(usize, u8),
    Vel(usize, f64),
    Gate(usize, f64),
    Prob(usize, f64),
    Tie(usize),
    LenInc,
    LenDec,
    Res(usize),
    Slot(usize),
    Add,
    Dup,
    Rem,
    Undo,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        (0usize..16, 0u8..128).prop_map(|(s, k)| Act::Note(s, k)),
        (0usize..16, 0.0f64..1.0).prop_map(|(s, f)| Act::Vel(s, f)),
        (0usize..16, 0.0f64..1.0).prop_map(|(s, f)| Act::Gate(s, f)),
        (0usize..16, 0.0f64..1.0).prop_map(|(s, f)| Act::Prob(s, f)),
        (0usize..16).prop_map(Act::Tie),
        Just(Act::LenInc),
        Just(Act::LenDec),
        (0usize..7).prop_map(Act::Res),
        (0usize..8).prop_map(Act::Slot),
        Just(Act::Add),
        Just(Act::Dup),
        Just(Act::Rem),
        Just(Act::Undo),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn no_gesture_sequence_makes_the_model_refuse_a_command(acts in proptest::collection::vec(act(), 1..30)) {
        let (mut d, reg, mut v) = setup();
        for a in acts {
            let step_cols = usize::from(v.pattern().map_or(1, |p| p.length)).max(1);
            let p = match a {
                Act::Note(s, k) => Some(v.note_cell_center(s % step_cols, k.clamp(v.key_range().0, v.key_range().1))),
                Act::Vel(s, f) => Some(v.velocity_point(s % step_cols, f)),
                Act::Gate(s, f) => Some(v.gate_point(s % step_cols, f)),
                Act::Prob(s, f) => Some(v.probability_point(s % step_cols, f)),
                Act::Tie(s) => Some(v.tie_center(s % step_cols)),
                Act::LenInc => Some(v.length_button_center(true)),
                Act::LenDec => Some(v.length_button_center(false)),
                Act::Res(i) => Some(v.resolution_button_center(SeqResolution::ALL[i])),
                Act::Slot(i) => Some(v.slot_center(i)),
                Act::Add => Some(v.pattern_button_center(PatternButton::Add)),
                Act::Dup => Some(v.pattern_button_center(PatternButton::Duplicate)),
                Act::Rem => Some(v.pattern_button_center(PatternButton::Remove)),
                Act::Undo => None,
            };
            let actions = match p {
                Some(p) => click!(v, &d, &reg, p),
                None => { d.undo(&reg); Vec::new() }
            };
            for a in &actions {
                if let UiAction::Command(c) = a {
                    d.apply(c.clone(), &reg).unwrap_or_else(|e| panic!("refused {c:?}: {e}"));
                }
            }
            v.sync(d.doc(), &reg);
            if let Some(p) = v.pattern() {
                prop_assert!(p.check().is_ok());
            }
        }
    }
}

// Keep the type used by the fixtures referenced.
const _: fn() -> PatternDoc = bass;
