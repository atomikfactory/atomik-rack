//! Random gesture sequences on the arrangement and the piano roll (pointer down, move and up with
//! modifiers, keys, wheel scroll and zoom, undo and redo of the document). Whatever the order:
//!
//! - nothing panics and every command the view emits is accepted by the model;
//! - the selection names clips (notes) that exist after every sync;
//! - a view that is told the document revision (the gated sync) draws and answers exactly like
//!   one that syncs on every call;
//! - a pointer move that changes nothing leaves `needs_repaint` false once the hover has settled;
//! - undoing every step restores the document the gestures started from.
//!
//! The properties run 256 cases each; a failing case is saved to
//! `view_properties.proptest-regressions` (keep that file in the repository).

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, ClipId, Document, RackDoc};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    ArrangementView, DisplayList, Key, Modifiers, PianoRollView, PointerButton, UiAction, UiEvent,
};
use proptest::prelude::*;
use support::audio::*;
use support::song::*;
use support::*;

/// One step of a gesture script.
#[derive(Debug, Clone)]
enum Step {
    Move(f64, f64, u8),
    Down(f64, f64, u8, bool, bool),
    Up(f64, f64, u8),
    Wheel(f64, f64, f64, f64, u8),
    Key(u8, u8),
    Leave,
    Undo,
    Redo,
}

fn arb_step(bounds: Rect) -> impl Strategy<Value = Step> {
    let (w, h) = (bounds.width(), bounds.height());
    prop_oneof![
        4 => (0.0..w, 0.0..h, 0_u8..8).prop_map(|(x, y, m)| Step::Move(x, y, m)),
        4 => (0.0..w, 0.0..h, 0_u8..8, any::<bool>(), any::<bool>())
            .prop_map(|(x, y, m, c, r)| Step::Down(x, y, m, c, r)),
        4 => (0.0..w, 0.0..h, 0_u8..8).prop_map(|(x, y, m)| Step::Up(x, y, m)),
        2 => (0.0..w, 0.0..h, -6.0..6.0, -6.0..6.0, 0_u8..8)
            .prop_map(|(x, y, dx, dy, m)| Step::Wheel(x, y, dx, dy, m)),
        3 => (0_u8..40, 0_u8..8).prop_map(|(k, m)| Step::Key(k, m)),
        1 => Just(Step::Leave),
        1 => Just(Step::Undo),
        1 => Just(Step::Redo),
    ]
}

fn modifiers(m: u8) -> Modifiers {
    Modifiers {
        ctrl: m & 1 != 0,
        shift: m & 2 != 0,
        alt: m & 4 != 0,
        ..Modifiers::NONE
    }
}

fn key_of(k: u8) -> Key {
    match k {
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
        10 => Key::Enter,
        11 => Key::Backspace,
        12 => Key::PageUp,
        13 => Key::PageDown,
        14 => Key::Char('+'),
        15 => Key::Char('-'),
        n => Key::Char(char::from(b'a' + (n - 16) % 26)),
    }
}

/// The event of a step; `None` for the document steps (undo, redo).
fn event(s: &Step) -> Option<UiEvent> {
    Some(match *s {
        Step::Move(x, y, m) => UiEvent::PointerMove {
            pos: Point::new(x, y),
            modifiers: modifiers(m),
        },
        Step::Down(x, y, m, double, right) => UiEvent::PointerDown {
            pos: Point::new(x, y),
            button: if right {
                PointerButton::Secondary
            } else {
                PointerButton::Primary
            },
            modifiers: modifiers(m),
            clicks: if double { 2 } else { 1 },
        },
        Step::Up(x, y, m) => UiEvent::PointerUp {
            pos: Point::new(x, y),
            button: PointerButton::Primary,
            modifiers: modifiers(m),
        },
        Step::Wheel(x, y, dx, dy, m) => UiEvent::Wheel {
            delta: Vec2::new(dx, dy),
            pos: Point::new(x, y),
            modifiers: modifiers(m),
        },
        Step::Key(k, m) => UiEvent::Key {
            key: key_of(k),
            modifiers: modifiers(m),
            pressed: true,
        },
        Step::Leave => UiEvent::PointerLeave,
        Step::Undo | Step::Redo => return None,
    })
}

/// Applies the commands among `actions` the way the app does: the model must accept each one.
fn apply(d: &mut Document, reg: &Registry, actions: &[UiAction]) -> Result<(), TestCaseError> {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                if let Err(e) = d.apply(c.clone(), reg) {
                    return Err(TestCaseError::fail(format!("{c:?} refused: {e}")));
                }
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
    Ok(())
}

/// A view pair under test: `gated` is told the document revision, `fresh` syncs on every call.
trait Pane {
    fn pane_handle(&mut self, e: &UiEvent, doc: &RackDoc, reg: &Registry) -> Vec<UiAction>;
    fn pane_paint(&mut self, doc: &RackDoc, reg: &Registry) -> String;
    fn pane_sync(&mut self, doc: &RackDoc, reg: &Registry);
    fn gate(&mut self, revision: Option<u64>);
    fn repaint(&self) -> bool;
    /// The selection as a string, and whether it is valid for `doc`.
    fn selection(&self, doc: &RackDoc) -> (String, bool);
}

impl Pane for ArrangementView {
    fn pane_handle(&mut self, e: &UiEvent, doc: &RackDoc, reg: &Registry) -> Vec<UiAction> {
        self.handle(e, doc, reg)
    }
    fn pane_paint(&mut self, doc: &RackDoc, reg: &Registry) -> String {
        let mut dl = DisplayList::new();
        self.paint(&mut dl, doc, reg);
        dl.to_json()
    }
    fn pane_sync(&mut self, doc: &RackDoc, reg: &Registry) {
        self.sync(doc, reg);
    }
    fn gate(&mut self, revision: Option<u64>) {
        self.set_doc_revision(revision);
    }
    fn repaint(&self) -> bool {
        self.needs_repaint()
    }
    fn selection(&self, doc: &RackDoc) -> (String, bool) {
        let clips = self.selected_clips();
        let valid = clips.iter().all(|id| doc.song.clip(*id).is_some())
            && self
                .selected_track()
                .is_none_or(|t| doc.song.track(t).is_some());
        (format!("{clips:?} {:?}", self.selected_track()), valid)
    }
}

impl Pane for PianoRollView {
    fn pane_handle(&mut self, e: &UiEvent, doc: &RackDoc, reg: &Registry) -> Vec<UiAction> {
        self.handle(e, doc, reg)
    }
    fn pane_paint(&mut self, doc: &RackDoc, reg: &Registry) -> String {
        let mut dl = DisplayList::new();
        self.paint(&mut dl, doc, reg);
        dl.to_json()
    }
    fn pane_sync(&mut self, doc: &RackDoc, reg: &Registry) {
        self.sync(doc, reg);
    }
    fn gate(&mut self, revision: Option<u64>) {
        self.set_doc_revision(revision);
    }
    fn repaint(&self) -> bool {
        self.needs_repaint()
    }
    fn selection(&self, doc: &RackDoc) -> (String, bool) {
        let notes = self.selected_notes();
        // The clip may be gone (undone); then nothing can be selected.
        let count = match doc.song.clip(ClipId(1)).map(|(_, _, c)| &c.content) {
            Some(ClipContent::Notes(n)) => n.len(),
            _ => 0,
        };
        (format!("{notes:?}"), notes.iter().all(|i| *i < count))
    }
}

/// Runs `steps` on a gated and on an always-syncing view over one document and checks the
/// properties after every step.
fn run<P: Pane>(
    mut gated: P,
    mut fresh: P,
    doc: RackDoc,
    reg: &Registry,
    steps: &[Step],
) -> Result<(), TestCaseError> {
    let mut d = Document::new(doc.clone());
    for s in steps {
        let mut acts = [Vec::new(), Vec::new()];
        match event(s) {
            Some(ev) => {
                let current = d.doc().clone();
                gated.gate(Some(d.revision()));
                acts[0] = gated.pane_handle(&ev, &current, reg);
                gated.gate(None);
                acts[1] = fresh.pane_handle(&ev, &current, reg);
                prop_assert_eq!(&acts[0], &acts[1], "the gated view answers like the other");
                apply(&mut d, reg, &acts[0])?;
            }
            None => {
                let ops = if matches!(s, Step::Undo) {
                    d.undo()
                } else {
                    d.redo()
                };
                let _ = ops;
            }
        }
        let current = d.doc().clone();
        gated.pane_sync(&current, reg);
        fresh.pane_sync(&current, reg);
        for v in [&gated, &fresh] {
            let (sel, valid) = v.selection(&current);
            prop_assert!(
                valid,
                "the selection {} names something that is gone after {:?}",
                sel,
                s
            );
        }
        gated.gate(Some(d.revision()));
        let (a, b) = (
            gated.pane_paint(&current, reg),
            fresh.pane_paint(&current, reg),
        );
        gated.gate(None);
        prop_assert!(
            a == b,
            "the gated view draws what the other does after {:?}",
            s
        );
    }
    // Undo every step: the document is the one the gestures started from.
    while d.undo().is_some() {}
    prop_assert_eq!(d.doc(), &doc, "undoing every step restores the document");
    Ok(())
}

/// A pointer move that changes nothing: after the hover settled at `at`, moving to `at` again
/// asks for no repaint.
fn assert_quiet_move<P: Pane>(
    v: &mut P,
    doc: &RackDoc,
    reg: &Registry,
    at: Point,
    m: u8,
) -> Result<(), TestCaseError> {
    let ev = UiEvent::PointerMove {
        pos: at,
        modifiers: modifiers(m),
    };
    v.pane_handle(&ev, doc, reg);
    v.pane_paint(doc, reg);
    v.pane_handle(&ev, doc, reg);
    v.pane_paint(doc, reg);
    v.pane_handle(&ev, doc, reg);
    prop_assert!(
        !v.repaint(),
        "an unchanged pointer position asks for a repaint at {:?}",
        at
    );
    Ok(())
}

fn arrangement_pair(doc: &RackDoc, reg: &Registry) -> (ArrangementView, ArrangementView) {
    (arrangement(doc, reg), arrangement(doc, reg))
}

fn roll_pair(doc: &RackDoc, reg: &Registry) -> (PianoRollView, PianoRollView) {
    (piano_roll(doc, reg), piano_roll(doc, reg))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arrangement_gestures_keep_the_view_valid(
        steps in proptest::collection::vec(arb_step(ARR_BOUNDS), 1..30)
    ) {
        let (doc, reg) = (song_doc(), registry());
        let (a, b) = arrangement_pair(&doc, &reg);
        run(a, b, doc, &reg, &steps)?;
    }

    #[test]
    fn arrangement_gestures_with_audio_keep_the_view_valid(
        steps in proptest::collection::vec(arb_step(ARR_BOUNDS), 1..30)
    ) {
        let (doc, reg) = (audio_doc(), registry());
        let (a, b) = (audio_arrangement(&doc, &reg), audio_arrangement(&doc, &reg));
        run(a, b, doc, &reg, &steps)?;
    }

    #[test]
    fn piano_roll_gestures_keep_the_view_valid(
        steps in proptest::collection::vec(arb_step(ROLL_BOUNDS), 1..30)
    ) {
        let (doc, reg) = (song_doc(), registry());
        let (a, b) = roll_pair(&doc, &reg);
        run(a, b, doc, &reg, &steps)?;
    }

    #[test]
    fn a_pointer_move_that_changes_nothing_repaints_nothing_in_the_arrangement(
        x in 0.0..ARR_BOUNDS.width(), y in 0.0..ARR_BOUNDS.height(), m in 0_u8..8
    ) {
        let (doc, reg) = (song_doc(), registry());
        let mut v = arrangement(&doc, &reg);
        assert_quiet_move(&mut v, &doc, &reg, Point::new(x, y), m)?;
    }

    #[test]
    fn a_pointer_move_that_changes_nothing_repaints_nothing_in_the_piano_roll(
        x in 0.0..ROLL_BOUNDS.width(), y in 0.0..ROLL_BOUNDS.height(), m in 0_u8..8
    ) {
        let (doc, reg) = (song_doc(), registry());
        let mut v = piano_roll(&doc, &reg);
        assert_quiet_move(&mut v, &doc, &reg, Point::new(x, y), m)?;
    }
}

#[test]
fn the_properties_notice_a_view_that_goes_stale() {
    // A gated view that is never told about a change draws a stale picture: the comparison the
    // properties make would catch it.
    let (doc, reg) = (song_doc(), registry());
    let mut stale = arrangement(&doc, &reg);
    let mut fresh = arrangement(&doc, &reg);
    stale.gate(Some(1));
    let before = stale.pane_paint(&doc, &reg);
    let mut changed = doc.clone();
    changed.song_mut().loop_enabled = !changed.song.loop_enabled;
    changed.song_mut().loop_range = (BAR, 3 * BAR);
    let stale_after = stale.pane_paint(&changed, &reg);
    let fresh_after = fresh.pane_paint(&changed, &reg);
    assert_eq!(
        before, stale_after,
        "the same revision replays what it knew"
    );
    assert_ne!(
        stale_after, fresh_after,
        "a stale revision is visible to the comparison"
    );
}

// ------------------------------------------------------------------ regressions the properties found

#[test]
fn a_double_click_on_an_empty_audio_lane_draws_nothing() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let lane = v.lane_rect(&key("audio-track-2"), 0).expect("lane");
    let at = Point::new(lane.x1 - 20.0, lane.center().y);
    let e = UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 2,
    };
    let acts = v.handle(&e, &doc, &reg);
    assert!(
        !acts.iter().any(|a| matches!(a, UiAction::Command(_))),
        "{acts:?}"
    );
}

#[test]
fn an_undo_in_the_middle_of_a_start_trim_does_not_send_a_trim_before_tick_zero() {
    // The trim sends the change since its last move; an undo mid-drag takes the trim back, and
    // the next move must measure from what the document holds, not from what was sent.
    let (doc, reg) = (song_doc(), registry());
    let (a, b) = arrangement_pair(&doc, &reg);
    let steps = [
        Step::Wheel(0.0, 0.0, 0.0, 2.919_028_531_575_441, 3),
        Step::Down(
            210.015_296_455_336_68,
            198.426_340_121_755_74,
            0,
            false,
            false,
        ),
        Step::Move(611.601_282_294_467, 0.0, 0),
        Step::Undo,
        Step::Move(0.0, 0.0, 0),
    ];
    run(a, b, doc, &reg, &steps).expect("the trim stays valid after an undo");
}

#[test]
fn a_note_dropped_on_another_replaces_it_and_stays_selected() {
    use atomik_model::Note;
    use atomik_ui::seq_edit::normalize_notes;
    let note = |tick, key, length| Note {
        tick,
        length,
        key,
        velocity: 0.8,
        muted: false,
    };
    let notes = vec![note(0, 36, 100), note(0, 36, 200), note(480, 40, 100)];
    // The second note was just moved onto the first.
    let (list, sel) = normalize_notes(notes.clone(), &[1].into());
    assert_eq!(list, vec![note(0, 36, 200), note(480, 40, 100)]);
    assert_eq!(sel.into_iter().collect::<Vec<_>>(), vec![0]);
    // Two untracked notes on one spot: the longer stays and nothing is selected.
    let (list, sel) = normalize_notes(notes, &Default::default());
    assert_eq!(list.len(), 2);
    assert!(sel.is_empty());
}

#[test]
fn a_band_that_outlives_an_undo_does_not_select_notes_that_are_gone() {
    let (doc, reg) = (song_doc(), registry());
    let (a, b) = roll_pair(&doc, &reg);
    let steps = [
        Step::Down(940.531_771_768_571_4, 91.994_530_304_030_18, 0, true, false),
        Step::Up(0.0, 0.0, 0),
        Step::Down(
            526.912_150_869_650_8,
            101.538_264_025_009_81,
            2,
            false,
            false,
        ),
        Step::Undo,
        Step::Move(0.0, 0.0, 0),
    ];
    run(a, b, doc, &reg, &steps).expect("the selection stays valid after the undo");
}

#[test]
fn committing_an_unchanged_clip_name_sends_nothing() {
    let (doc, reg) = (song_doc(), registry());
    let (a, b) = arrangement_pair(&doc, &reg);
    let steps = [
        Step::Down(403.162_898_747_330_74, 160.650_733_248_032, 0, true, false),
        Step::Key(9, 0),
        Step::Down(0.0, 0.0, 0, false, false),
    ];
    run(a, b, doc, &reg, &steps).expect("an unchanged name is not a command");
}

#[test]
fn a_selection_dragged_and_undone_mid_gesture_names_only_clips_that_exist() {
    let (doc, reg) = (song_doc(), registry());
    let (a, b) = arrangement_pair(&doc, &reg);
    let steps = [
        Step::Down(752.360210358058, 205.72625343128817, 0, true, false),
        Step::Wheel(0.0, 0.0, -4.8836286008679926, 0.0, 2),
        Step::Down(632.8414329912573, 275.54728886814155, 0, false, false),
        Step::Move(0.0, 0.0, 0),
        Step::Up(0.0, 0.0, 0),
        Step::Down(750.4928051096111, 396.61738931534865, 1, false, false),
        Step::Move(0.0, 0.0, 0),
        Step::Undo,
        Step::Up(0.0, 0.0, 0),
    ];
    run(a, b, doc, &reg, &steps).expect("the selection stays valid");
}
