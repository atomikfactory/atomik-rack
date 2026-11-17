//! The tab editor through its public contract: the cursor, typing frets (one and two digits),
//! note values, delete, audition, the automatic fingering of a clip without a sidecar, and that
//! every edit is one undo step that keeps notes and sidecar together.

mod support;

use atomik_model::{Document, Note, RackDoc, TabData, TabNoteRef};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::{Key, Modifiers, NoteValue, PointerButton, TabEditorView, UiAction, UiEvent};
use support::tab::*;
use support::*;

fn digit(v: &mut TabEditorView, doc: &RackDoc, d: char) -> Vec<UiAction> {
    v.handle(
        &UiEvent::Key {
            key: Key::Char(d),
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        doc,
        &registry(),
    )
}

fn command(v: &mut TabEditorView, doc: &RackDoc, id: &str, arg: Option<&str>) -> Vec<UiAction> {
    match v.command(id, arg, doc, &registry()) {
        CommandOutcome::Handled(a) => a,
        other => panic!("{id}: {other:?}"),
    }
}

fn click(v: &mut TabEditorView, doc: &RackDoc, x: f64, y: f64) -> Vec<UiAction> {
    v.handle(
        &UiEvent::PointerDown {
            pos: Point::new(x, y),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        doc,
        &registry(),
    )
}

fn empty_doc() -> RackDoc {
    tab_doc("Guitar", Vec::new(), None)
}

#[test]
fn a_clip_without_a_sidecar_is_shown_fretted_and_not_changed() {
    let doc = tab_doc("Guitar", vec![Note::new(0, PPQ, 64, 0.8)], None);
    let v = tab_editor(&doc, &registry());
    assert!(v.is_automatic());
    assert_eq!(v.strings(), 6);
    let notes = v.notes();
    assert_eq!(notes.len(), 1);
    // E4 is the open top string: the fretter prefers open.
    assert_eq!((notes[0].1, notes[0].2), (0, 0));
    assert!(clip1(&doc).midi.tab.is_none(), "showing never writes");
}

#[test]
fn the_track_name_picks_the_instrument() {
    let doc = tab_doc("Banjo lead", Vec::new(), None);
    assert_eq!(tab_editor(&doc, &registry()).strings(), 5);
    let doc = tab_doc("Fat Bass", Vec::new(), None);
    assert_eq!(tab_editor(&doc, &registry()).strings(), 4);
}

#[test]
fn typing_a_fret_makes_a_note_with_the_pitch_of_the_string_and_fret() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    v.set_cursor(PPQ, 2); // the G string of a guitar
    let actions = digit(&mut v, &doc, '5');
    let mut document = Document::new(doc);
    apply_tab(&mut document, &registry(), &actions);
    let clip = clip1(document.doc());
    let notes: Vec<Note> = match &clip.content {
        atomik_model::ClipContent::Notes(n) => n.to_vec(),
        _ => panic!("notes"),
    };
    assert_eq!(notes.len(), 1);
    assert_eq!(
        (notes[0].tick, notes[0].key, notes[0].length),
        (PPQ, 60, PPQ)
    );
    let tab = clip
        .midi
        .tab
        .as_ref()
        .expect("the first edit makes the sidecar");
    assert_eq!(tab.instrument_id, "guitar6");
    let r = tab.note_of(PPQ, 60).expect("entry");
    assert_eq!((r.string, r.fret), (2, 5));
    // Audition: the typed note sounds on the track's device.
    assert!(actions.iter().any(|a| matches!(
        a,
        UiAction::Audition {
            key: 60,
            on: true,
            ..
        }
    )));
    // One undo step takes the whole edit back, sidecar included.
    assert!(document.undo().is_some());
    assert!(clip1(document.doc()).midi.tab.is_none());
}

#[test]
fn the_audition_note_is_released_by_the_clock() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    digit(&mut v, &doc, '3');
    assert!(
        !v.tick(1.0)
            .iter()
            .any(|a| matches!(a, UiAction::Audition { .. })),
        "the window starts at the first tick"
    );
    let out = v.tick(1.3);
    assert!(
        out.iter()
            .any(|a| matches!(a, UiAction::Audition { on: false, .. })),
        "{out:?}"
    );
}

#[test]
fn two_digits_typed_quickly_are_one_fret_in_one_undo_step() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let mut document = Document::new(doc);
    v.stamp(1.0);
    let first = digit(&mut v, document.doc(), '1');
    let m1 = apply_tab(&mut document, &registry(), &first);
    v.sync(document.doc(), &registry());
    v.stamp(1.2);
    let second = digit(&mut v, document.doc(), '2');
    let m2 = apply_tab(&mut document, &registry(), &second);
    assert_eq!(m1, m2, "both edits carry one merge id: {m1:?} {m2:?}");
    assert!(m1[0].is_some());
    v.sync(document.doc(), &registry());
    assert_eq!(v.note_at(0, 0).map(|n| n.0), Some(12));
    // The merged edits undo together.
    document.end_merge();
    assert!(document.undo().is_some());
    assert!(clip1(document.doc()).midi.tab.is_none());
}

#[test]
fn a_digit_after_the_timeout_starts_a_new_fret() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let mut document = Document::new(doc);
    v.stamp(1.0);
    let a = digit(&mut v, document.doc(), '1');
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    v.tick(1.0);
    let ended = v.tick(2.0);
    assert!(
        ended.iter().any(|a| matches!(a, UiAction::EndMerge)),
        "the merge ends with the timeout"
    );
    apply_tab(&mut document, &registry(), &ended);
    let b = digit(&mut v, document.doc(), '2');
    apply_tab(&mut document, &registry(), &b);
    v.sync(document.doc(), &registry());
    assert_eq!(v.note_at(0, 0).map(|n| n.0), Some(2), "2, not 12");
}

#[test]
fn a_fret_the_string_cannot_play_is_refused_with_a_status_and_no_command() {
    // The banjo drone cannot be pressed below its fifth fret.
    let doc = tab_doc("Banjo", Vec::new(), None);
    let mut v = tab_editor(&doc, &registry());
    v.set_cursor(0, 4); // the drone is the last line
    let out = digit(&mut v, &doc, '2');
    assert!(
        out.iter().all(|a| !matches!(a, UiAction::Command(_))),
        "{out:?}"
    );
    assert!(v.status().is_some());
    // "1" then "2" within the window is fret 12, which the drone can play.
    let mut v = tab_editor(&doc, &registry());
    v.set_cursor(0, 4);
    digit(&mut v, &doc, '1');
    let out = digit(&mut v, &doc, '2');
    assert!(
        out.iter().any(|a| matches!(a, UiAction::Command(_))),
        "{out:?}"
    );
}

#[test]
fn the_cursor_steps_by_the_note_value_and_stays_in_the_clip() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    command(&mut v, &doc, "editor.tab_cursor_right", None);
    assert_eq!(v.cursor_cell(), (PPQ, 0));
    command(&mut v, &doc, "editor.tab_duration", Some("eighth"));
    assert_eq!(v.note_value(), NoteValue::plain(8));
    command(&mut v, &doc, "editor.tab_cursor_right", None);
    assert_eq!(v.cursor_cell().0, PPQ + PPQ / 2);
    for _ in 0..3 {
        command(&mut v, &doc, "editor.tab_cursor_left", None);
    }
    assert_eq!(v.cursor_cell().0, 0, "never before the start");
    for _ in 0..40 {
        command(&mut v, &doc, "editor.tab_cursor_right", None);
    }
    assert!(v.cursor_cell().0 < 8 * PPQ, "never past the clip");
}

#[test]
fn up_and_down_move_between_strings_and_stop_at_the_ends() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    command(&mut v, &doc, "editor.tab_cursor_up", None);
    assert_eq!(v.cursor_cell().1, 0);
    for _ in 0..10 {
        command(&mut v, &doc, "editor.tab_cursor_down", None);
    }
    assert_eq!(v.cursor_cell().1, 5);
}

#[test]
fn bar_commands_jump_by_the_meter() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    command(&mut v, &doc, "editor.tab_bar_next", None);
    assert_eq!(v.cursor_cell().0, 4 * PPQ);
    command(&mut v, &doc, "editor.tab_bar_prev", None);
    assert_eq!(v.cursor_cell().0, 0);
}

#[test]
fn a_dotted_or_triplet_value_changes_the_step_and_the_length_of_the_next_note() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    command(&mut v, &doc, "editor.tab_duration", Some("eighth"));
    command(&mut v, &doc, "editor.tab_dotted", None);
    assert_eq!(v.note_value().ticks(), PPQ * 3 / 4);
    command(&mut v, &doc, "editor.tab_triplet", None);
    assert_eq!(v.note_value().ticks(), PPQ / 3);
    let actions = digit(&mut v, &doc, '0');
    let mut document = Document::new(doc);
    apply_tab(&mut document, &registry(), &actions);
    v.sync(document.doc(), &registry());
    assert_eq!(v.note_at(0, 0), Some((0, PPQ / 3)));
}

#[test]
fn a_value_command_resizes_the_note_under_the_cursor() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let mut document = Document::new(doc);
    let a = digit(&mut v, document.doc(), '3');
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    let a = command(&mut v, document.doc(), "editor.tab_duration", Some("half"));
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    assert_eq!(v.note_at(0, 0), Some((3, 2 * PPQ)));
}

#[test]
fn delete_removes_the_note_under_the_cursor_and_undo_brings_it_back() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let mut document = Document::new(doc);
    let a = digit(&mut v, document.doc(), '7');
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    v.tick(1.0);
    let ended = v.tick(3.0);
    apply_tab(&mut document, &registry(), &ended);
    let a = command(&mut v, document.doc(), "edit.delete", None);
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    assert!(v.notes().is_empty());
    assert!(document.undo().is_some());
    v.sync(document.doc(), &registry());
    assert_eq!(v.notes().len(), 1);
}

#[test]
fn a_click_puts_the_cursor_on_the_string_and_the_snapped_tick() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let x = v.tick_to_x(2 * PPQ) + 3.0;
    let y = v.string_y(3) + 2.0;
    click(&mut v, &doc, x, y);
    assert_eq!(v.cursor_cell(), (2 * PPQ, 3));
}

#[test]
fn a_click_near_a_note_picks_the_note_whatever_the_grid() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    v.set_cursor(PPQ / 2, 1);
    let a = digit(&mut v, &doc, '4');
    let mut document = Document::new(doc);
    apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    v.set_cursor(0, 0);
    let x = v.tick_to_x(PPQ / 2) + 4.0;
    let y = v.string_y(1);
    click(&mut v, document.doc(), x, y);
    assert_eq!(v.cursor_cell(), (PPQ / 2, 1));
}

#[test]
fn the_note_value_buttons_set_the_value() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let rects: Vec<_> = (0..1000)
        .step_by(2)
        .filter_map(|x| v.help_at(Point::new(f64::from(x), 14.0)))
        .filter(|h| h.text == "Note value of the next typed note")
        .map(|h| h.rect)
        .collect();
    assert!(!rects.is_empty(), "the buttons answer a tooltip query");
    // The buttons are laid out left to right: the first is the whole note.
    let first = rects
        .iter()
        .min_by(|a, b| a.x0.total_cmp(&b.x0))
        .copied()
        .expect("button");
    click(&mut v, &doc, first.center().x, first.center().y);
    assert_eq!(v.note_value().division, 1);
}

#[test]
fn follow_can_be_switched_off() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    assert!(v.follow());
    command(&mut v, &doc, "sequencer.follow", None);
    assert!(!v.follow());
}

#[test]
fn a_sidecar_note_keeps_its_string_in_the_view() {
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    // E4 on the B string, fret 5, though the open string would also do.
    tab.notes.push(TabNoteRef::new(0, 64, 1, 5));
    let doc = tab_doc("Guitar", vec![Note::new(0, PPQ, 64, 0.8)], Some(tab));
    let v = tab_editor(&doc, &registry());
    assert!(!v.is_automatic());
    assert_eq!(v.notes()[0], (0, 1, 5, PPQ));
}

#[test]
fn an_edit_on_a_banjo_clip_uses_five_strings_and_the_drone() {
    let doc = tab_doc("Banjo", Vec::new(), None);
    let mut v = tab_editor(&doc, &registry());
    v.set_cursor(0, 4);
    let a = digit(&mut v, &doc, '0');
    let mut document = Document::new(doc);
    apply_tab(&mut document, &registry(), &a);
    let clip = clip1(document.doc());
    let tab = clip.midi.tab.as_ref().unwrap();
    assert_eq!(tab.instrument_id, "banjo5");
    assert_eq!(tab.notes.len(), 1);
    // The open drone of an open-G banjo is the high g (G4 = 67).
    assert_eq!(tab.notes[0].key, 67);
}

#[test]
fn a_command_the_editor_does_not_own_is_declined() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    assert!(matches!(
        v.command("editor.chordify", None, &doc, &registry()),
        CommandOutcome::Declined
    ));
}

#[test]
fn the_zoom_commands_change_the_scale_of_the_staff() {
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let before = v.px_per_tick();
    command(&mut v, &doc, "editor.tab_zoom_in", None);
    assert!(v.px_per_tick() > before);
    command(&mut v, &doc, "editor.tab_zoom_out", None);
    command(&mut v, &doc, "editor.tab_zoom_out", None);
    assert!(v.px_per_tick() < before);
}

#[test]
fn a_digit_typed_after_an_idle_stretch_never_joins_the_one_before() {
    // The app stops ticking while idle: no frame runs between the two digits. The digit carries
    // the time it arrived at, so the second one is a fret of its own.
    let doc = empty_doc();
    let mut v = tab_editor(&doc, &registry());
    let mut document = Document::new(doc);
    v.stamp(10.0);
    let a = digit(&mut v, document.doc(), '1');
    let m1 = apply_tab(&mut document, &registry(), &a);
    v.sync(document.doc(), &registry());
    v.stamp(10.0 + 45.0);
    let b = digit(&mut v, document.doc(), '2');
    let m2 = apply_tab(&mut document, &registry(), &b);
    v.sync(document.doc(), &registry());
    assert_eq!(v.note_at(0, 0).map(|n| n.0), Some(2), "2, not 12");
    assert_ne!(m1, m2, "and not the same undo step");
}

// ---------------------------------------------------------------- editing tools (phase B2)

/// A guitar clip with four plain notes on four strings, one per beat: E4 (string 0, fret 0), D4
/// (string 1, fret 3), G3 (string 2, open) and E3 (string 3, fret 2).
fn song_doc() -> RackDoc {
    let mut tab = TabData::for_instrument("guitar6").unwrap();
    let mut notes = Vec::new();
    for (i, (s, f)) in [(0u8, 0u8), (1, 3), (2, 0), (3, 2)].iter().enumerate() {
        let tick = i as i64 * PPQ;
        let key = tab.tuning.pitch(usize::from(*s), *f, tab.capo).unwrap();
        notes.push(Note::new(tick, PPQ, key, 0.8));
        tab.notes.push(TabNoteRef::new(tick, key, *s, *f));
    }
    tab_doc("Guitar", notes, Some(tab))
}

/// The pair of a view and a document that follow each other through edits.
struct Session {
    v: TabEditorView,
    d: Document,
}

impl Session {
    fn new(doc: RackDoc) -> Self {
        let v = tab_editor(&doc, &registry());
        Session {
            v,
            d: Document::new(doc),
        }
    }

    fn run(&mut self, id: &str, arg: Option<&str>) -> Vec<UiAction> {
        let a = command(&mut self.v, self.d.doc(), id, arg);
        apply_tab(&mut self.d, &registry(), &a);
        self.d.end_merge();
        self.v.sync(self.d.doc(), &registry());
        a
    }

    fn key(&mut self, key: Key) -> Vec<UiAction> {
        let a = self.v.handle(
            &UiEvent::Key {
                key,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            self.d.doc(),
            &registry(),
        );
        apply_tab(&mut self.d, &registry(), &a);
        self.v.sync(self.d.doc(), &registry());
        a
    }

    fn pointer(&mut self, event: UiEvent) -> Vec<UiAction> {
        let a = self.v.handle(&event, self.d.doc(), &registry());
        apply_tab(&mut self.d, &registry(), &a);
        self.v.sync(self.d.doc(), &registry());
        a
    }

    fn down(&mut self, p: Point, button: PointerButton, shift: bool) -> Vec<UiAction> {
        self.pointer(UiEvent::PointerDown {
            pos: p,
            button,
            modifiers: if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            },
            clicks: 1,
        })
    }

    fn mv(&mut self, p: Point) -> Vec<UiAction> {
        self.pointer(UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        })
    }

    fn up(&mut self, p: Point) -> Vec<UiAction> {
        self.pointer(UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        })
    }

    fn cell(&self, tick: i64, string: u8) -> Point {
        Point::new(self.v.tick_to_x(tick), self.v.string_y(string))
    }

    fn tab(&self) -> TabData {
        clip1(self.d.doc()).midi.tab.clone().expect("a sidecar")
    }

    fn keys(&self) -> Vec<u8> {
        match clip1(self.d.doc()).content {
            atomik_model::ClipContent::Notes(n) => n.iter().map(|n| n.key).collect(),
            _ => panic!("notes"),
        }
    }
}

#[test]
fn shift_arrows_select_whole_columns_and_shift_up_down_narrow_to_a_rectangle() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1);
    assert!(s.v.selected().is_empty());
    s.run("editor.tab_extend", Some("right"));
    s.run("editor.tab_extend", Some("right"));
    assert_eq!(
        s.v.selected_range(),
        Some((PPQ, 3 * PPQ, 0, 5)),
        "every string"
    );
    assert_eq!(
        s.v.selected(),
        [(PPQ, 62), (2 * PPQ, 55), (3 * PPQ, 52)],
        "the notes inside, in time order"
    );
    // Down makes it the rectangle between the anchor (string 1) and the cursor (string 2).
    s.run("editor.tab_extend", Some("down"));
    assert_eq!(s.v.selected_range(), Some((PPQ, 3 * PPQ, 1, 2)));
    assert_eq!(s.v.selected(), [(PPQ, 62), (2 * PPQ, 55)]);
    // Left shrinks it again; a plain cursor move ends the selection.
    s.run("editor.tab_extend", Some("left"));
    assert_eq!(s.v.selected(), [(PPQ, 62), (2 * PPQ, 55)]);
    s.run("editor.tab_cursor_right", None);
    assert!(s.v.selected().is_empty() && s.v.selected_range().is_none());
}

#[test]
fn select_all_takes_every_note_and_escape_clears_it() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    assert_eq!(s.v.selected().len(), 4);
    s.key(Key::Escape);
    assert!(s.v.selected().is_empty());
}

#[test]
fn dragging_on_the_staff_selects_a_rectangle_and_a_click_selects_nothing() {
    let mut s = Session::new(song_doc());
    let (a, b) = (s.cell(0, 0), s.cell(PPQ, 1));
    s.down(a, PointerButton::Primary, false);
    assert!(s.v.is_dragging());
    s.mv(b);
    s.up(b);
    assert!(!s.v.is_dragging());
    assert_eq!(s.v.selected_range(), Some((0, PPQ, 0, 1)));
    assert_eq!(s.v.selected(), [(0, 64), (PPQ, 62)]);
    // A plain click clears it, and Shift+click grows a new one from the cursor.
    let c = s.cell(2 * PPQ, 2);
    s.down(c, PointerButton::Primary, false);
    s.up(c);
    assert!(s.v.selected().is_empty());
    let e = s.cell(3 * PPQ, 3);
    s.down(e, PointerButton::Primary, true);
    s.up(e);
    assert_eq!(s.v.selected(), [(2 * PPQ, 55), (3 * PPQ, 52)]);
}

#[test]
fn a_right_click_on_a_note_selects_it_and_asks_for_the_staff_menu() {
    let mut s = Session::new(song_doc());
    let p = s.cell(PPQ, 1);
    let a = s.down(p, PointerButton::Secondary, false);
    assert!(
        a.iter().any(|a| matches!(
            a,
            UiAction::ContextMenu {
                target: atomik_ui::ContextTarget::TabStaff,
                ..
            }
        )),
        "{a:?}"
    );
    assert_eq!(s.v.selected(), [(PPQ, 62)]);
    // A right click inside the selection keeps it whole.
    s.run("edit.select_all", None);
    let p = s.cell(2 * PPQ, 2);
    s.down(p, PointerButton::Secondary, false);
    assert_eq!(s.v.selected().len(), 4);
}

#[test]
fn a_technique_toggles_on_the_note_under_the_cursor_in_one_undo_step() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1);
    let a = s.run("editor.tab_technique", Some("palm_mute"));
    assert!(a.iter().any(|a| matches!(a, UiAction::Command(_))));
    let on = |s: &Session, tick, key| {
        s.tab()
            .note_of(tick, key)
            .unwrap()
            .tech
            .has(atomik_tab::score::TechFlags::PALM_MUTE)
    };
    assert!(on(&s, PPQ, 62));
    assert!(!on(&s, 0, 64), "only that note");
    // The same command again clears it; undo goes back one step at a time.
    s.run("editor.tab_technique", Some("palm_mute"));
    assert!(!on(&s, PPQ, 62));
    assert!(s.d.undo().is_some());
    assert!(on(&s, PPQ, 62));
    assert!(s.d.undo().is_some());
    assert!(
        clip1(s.d.doc())
            .midi
            .tab
            .as_ref()
            .is_some_and(|t| { t.note_of(PPQ, 62).is_some_and(|r| r.tech.is_empty()) })
    );
}

#[test]
fn a_technique_on_a_selection_sets_every_note_and_a_mixed_one_gets_it() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(0, 0);
    s.run("editor.tab_technique", Some("ghost"));
    s.run("edit.select_all", None);
    // One note has it already: the command sets it on all four, not clears.
    s.run("editor.tab_technique", Some("ghost"));
    let tab = s.tab();
    assert!(
        tab.notes
            .iter()
            .all(|r| r.tech.has(atomik_tab::score::TechFlags::GHOST))
    );
    s.run("editor.tab_technique", Some("ghost"));
    assert!(
        s.tab()
            .notes
            .iter()
            .all(|r| !r.tech.has(atomik_tab::score::TechFlags::GHOST)),
        "all had it: it clears"
    );
}

#[test]
fn hammer_on_goes_on_the_destination_note_and_excludes_pull_off() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1);
    s.run("editor.tab_technique", Some("pull_off"));
    s.run("editor.tab_technique", Some("hammer_on"));
    let t = s.tab();
    let r = t.note_of(PPQ, 62).unwrap();
    use atomik_tab::score::TechFlags;
    assert!(r.tech.has(TechFlags::HAMMER_ON) && !r.tech.has(TechFlags::PULL_OFF));
    assert!(t.note_of(0, 64).unwrap().tech.is_empty(), "not the origin");
}

#[test]
fn slides_vibrato_and_harmonics_set_their_curves() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(0, 0);
    s.run("editor.tab_technique", Some("slide_shift"));
    s.run("editor.tab_technique", Some("vibrato"));
    s.run("editor.tab_technique", Some("harmonic"));
    let t = s.tab();
    let r = t.note_of(0, 64).unwrap();
    assert_eq!(r.tech.slide, Some(atomik_tab::score::SlideKind::Shift));
    assert!(r.tech.vibrato.is_some());
    assert_eq!(
        r.tech.harmonic.map(|h| h.kind),
        Some(atomik_tab::score::HarmonicKind::Natural)
    );
    s.run("editor.tab_technique", Some("harmonic_artificial"));
    assert_eq!(
        s.tab()
            .note_of(0, 64)
            .unwrap()
            .tech
            .harmonic
            .map(|h| h.kind),
        Some(atomik_tab::score::HarmonicKind::Artificial),
        "another kind replaces it"
    );
    s.run("editor.tab_technique", Some("clear"));
    assert!(s.tab().note_of(0, 64).unwrap().tech.is_empty());
}

#[test]
fn bends_take_their_amount_from_the_chip_or_the_command() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(0, 0);
    s.run("editor.tab_bend_amount", Some("quarter_tone"));
    s.run("editor.tab_technique", Some("bend"));
    let peak = |s: &Session| {
        s.tab()
            .note_of(0, 64)
            .unwrap()
            .tech
            .bend
            .as_ref()
            .map(|b| b.peak())
    };
    assert_eq!(peak(&s), Some(50));
    // An explicit amount replaces it (and becomes the chip's amount).
    s.run("editor.tab_technique", Some("bend_one_half"));
    assert_eq!(peak(&s), Some(300));
    // Release and pre-bend use that amount.
    s.run("editor.tab_technique", Some("bend_release"));
    let kind = |s: &Session| {
        s.tab()
            .note_of(0, 64)
            .unwrap()
            .tech
            .bend
            .as_ref()
            .map(|b| (b.kind(), b.peak()))
    };
    assert_eq!(
        kind(&s),
        Some((atomik_tab::score::BendKind::BendRelease, 300))
    );
    s.run("editor.tab_technique", Some("pre_bend"));
    assert_eq!(kind(&s), Some((atomik_tab::score::BendKind::PreBend, 300)));
    // The bend is rendered into the clip as a curve.
    assert!(!clip1(s.d.doc()).midi.expression.is_empty());
}

#[test]
fn the_toolbar_buttons_run_the_technique_commands() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(0, 0);
    // Find the palm mute button by its tooltip and click it.
    let rect = (0..1000)
        .step_by(2)
        .filter_map(|x| s.v.help_at(Point::new(f64::from(x), 14.0)))
        .find(|h| h.text == "Palm mute")
        .expect("a palm mute button")
        .rect;
    let c = rect.center();
    s.down(c, PointerButton::Primary, false);
    s.up(c);
    assert!(
        s.tab()
            .note_of(0, 64)
            .unwrap()
            .tech
            .has(atomik_tab::score::TechFlags::PALM_MUTE)
    );
}

#[test]
fn alt_up_and_down_move_a_note_to_the_next_string_that_plays_its_pitch() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1); // D4: string 1 fret 3
    s.run("editor.tab_string_down", None);
    let r = s.tab().note_of(PPQ, 62).unwrap().clone();
    assert_eq!((r.string, r.fret), (2, 7));
    s.run("editor.tab_string_up", None);
    let r = s.tab().note_of(PPQ, 62).unwrap().clone();
    assert_eq!((r.string, r.fret), (1, 3));
    assert_eq!(s.keys(), [64, 62, 55, 52], "the pitches never change");
}

#[test]
fn changing_string_refuses_at_the_end_of_the_neck_with_a_message() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(0, 0); // E4 on the top string: nothing above it
    let a = s.run("editor.tab_string_up", None);
    assert!(a.is_empty(), "no edit: {a:?}");
    assert!(s.v.status().is_some_and(|m| m.contains("pitch")));
    // Down from the lowest string.
    s.v.set_cursor(3 * PPQ, 3);
    let before = s.tab();
    s.run("editor.tab_string_down", None);
    // E3 (52) is also playable on the A string (fret 7): that is allowed.
    assert_ne!(s.tab(), before);
}

#[test]
fn a_selection_changes_string_together_or_not_at_all() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    // The top note cannot go up: nothing changes.
    let before = s.tab();
    s.run("editor.tab_string_up", None);
    assert_eq!(s.tab(), before);
    assert!(s.v.status().is_some());
}

#[test]
fn transpose_moves_the_selection_by_a_semitone_or_an_octave_and_undoes() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    s.run("editor.tab_transpose", Some("octave_up"));
    assert_eq!(s.keys(), [76, 74, 67, 64]);
    s.run("editor.tab_transpose", Some("down"));
    assert_eq!(s.keys(), [75, 73, 66, 63]);
    // The positions follow: D#5 is fret 4 on the 'B' string?  Whatever it is, it plays that key.
    let t = s.tab();
    for r in &t.notes {
        assert_eq!(
            t.tuning.pitch(usize::from(r.string), r.fret, t.capo),
            Some(r.key)
        );
    }
    assert!(s.d.undo().is_some());
    assert_eq!(s.keys(), [76, 74, 67, 64]);
}

#[test]
fn transposing_past_the_midi_range_is_refused() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    for _ in 0..8 {
        s.run("editor.tab_transpose", Some("octave_up"));
    }
    assert!(s.keys().iter().all(|k| *k <= 127));
    assert!(s.v.status().is_some());
}

#[test]
fn refret_lowest_and_automatic_keep_the_pitches_and_the_techniques() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1);
    s.run("editor.tab_technique", Some("accent"));
    s.run("edit.select_all", None);
    s.run("editor.tab_refret", Some("lowest"));
    let t = s.tab();
    for r in &t.notes {
        // Nothing can be played lower on the neck.
        let lowest = (0..t.tuning.len())
            .filter_map(|st| t.tuning.fret_for_pitch(st, r.key, t.capo))
            .min()
            .unwrap();
        assert_eq!(r.fret, lowest, "key {}", r.key);
    }
    assert!(
        t.note_of(PPQ, 62)
            .unwrap()
            .tech
            .has(atomik_tab::score::TechFlags::ACCENT)
    );
    assert_eq!(s.keys(), [64, 62, 55, 52]);
    s.run("editor.tab_refret", Some("automatic"));
    assert_eq!(s.keys(), [64, 62, 55, 52]);
}

#[test]
fn re_fret_at_position_opens_a_chooser_and_a_click_on_a_position_applies_it() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    s.run("editor.tab_refret", Some("choose"));
    assert!(s.v.modal_open() && s.v.is_text_owner());
    // The keymap does not reach the view while the chooser is open.
    assert!(matches!(
        s.v.command("editor.tab_cursor_left", None, s.d.doc(), &registry()),
        CommandOutcome::Declined
    ));
    // Pick a position by its pixel: the panel is centred, the 5th position is in the grid.
    let centre = s.v.bounds().center();
    let mut done = false;
    for dy in (0..60).step_by(4) {
        for dx in (-130..130).step_by(6) {
            let p = Point::new(centre.x + f64::from(dx), centre.y + f64::from(dy));
            let a = s.down(p, PointerButton::Primary, false);
            if !a.is_empty() {
                done = true;
                break;
            }
            if !s.v.modal_open() {
                break;
            }
        }
        if done || !s.v.modal_open() {
            break;
        }
    }
    assert!(!s.v.modal_open(), "a choice closes the chooser");
    assert_eq!(s.keys(), [64, 62, 55, 52], "the pitches stay");
}

#[test]
fn escape_closes_a_panel_without_an_edit() {
    let mut s = Session::new(song_doc());
    s.run("editor.tab_tuning", None);
    assert!(s.v.modal_open());
    let a = s.key(Key::Escape);
    assert!(a.is_empty());
    assert!(!s.v.modal_open());
}

#[test]
fn the_tuning_dialog_applies_a_change_of_pitches_as_one_undo_step() {
    let mut s = Session::new(song_doc());
    s.run("editor.tab_tuning", None);
    assert!(s.v.modal_open());
    // Enter applies the dialog as it opened: the same tuning, a clean edit.
    let a = s.key(Key::Enter);
    assert!(!s.v.modal_open());
    assert!(a.iter().any(|a| matches!(a, UiAction::Command(_))), "{a:?}");
    assert_eq!(s.keys(), [64, 62, 55, 52]);
}

#[test]
fn clicking_the_instrument_label_opens_the_tuning_dialog() {
    let mut s = Session::new(song_doc());
    let p = Point::new(40.0, 14.0);
    s.down(p, PointerButton::Primary, false);
    s.up(p);
    assert!(s.v.modal_open());
    let help = s.v.help_at(Point::new(40.0, 14.0));
    assert!(help.is_none(), "no tooltips under a panel");
}

#[test]
fn copy_and_paste_keep_strings_and_techniques_and_paste_at_the_cursor() {
    let mut s = Session::new(song_doc());
    s.v.set_cursor(PPQ, 1);
    s.run("editor.tab_technique", Some("vibrato"));
    s.v.select_between((0, 0), (PPQ, 5));
    assert_eq!(s.v.selected().len(), 2);
    assert!(!s.v.can_paste());
    s.run("edit.copy", None);
    assert!(s.v.can_paste());
    s.v.set_cursor(4 * PPQ, 0);
    s.run("edit.paste", None);
    let t = s.tab();
    assert_eq!(t.notes.len(), 6);
    let r = t.note_of(5 * PPQ, 62).expect("the pasted second note");
    assert_eq!((r.string, r.fret), (1, 3), "string and fret kept");
    assert!(r.tech.vibrato.is_some(), "techniques kept");
    // What was pasted is selected, and undo takes it back in one step.
    assert_eq!(s.v.selected().len(), 2);
    assert!(s.d.undo().is_some());
    assert_eq!(s.tab().notes.len(), 4);
}

#[test]
fn cut_removes_the_selection_and_paste_brings_it_back_elsewhere() {
    let mut s = Session::new(song_doc());
    s.v.select_between((0, 0), (PPQ, 5));
    s.run("edit.cut", None);
    assert_eq!(s.tab().notes.len(), 2);
    assert!(s.v.selected().is_empty());
    s.v.set_cursor(5 * PPQ, 0);
    s.run("edit.paste", None);
    assert_eq!(s.tab().notes.len(), 4);
    assert!(s.tab().note_of(5 * PPQ, 64).is_some());
}

#[test]
fn paste_with_nothing_copied_says_so_and_paste_never_runs_past_the_clip() {
    let mut s = Session::new(song_doc());
    let a = s.run("edit.paste", None);
    assert!(a.is_empty() && s.v.status().is_some());
    s.v.select_between((0, 0), (3 * PPQ, 5));
    s.run("edit.copy", None);
    // The clip is eight beats long: pasting at beat seven keeps the notes that fit (two).
    s.v.set_cursor(6 * PPQ, 0);
    s.run("edit.paste", None);
    assert_eq!(s.tab().notes.len(), 4 + 2);
}

#[test]
fn delete_removes_a_whole_selection_in_one_step() {
    let mut s = Session::new(song_doc());
    s.v.select_between((0, 0), (PPQ, 5));
    s.run("edit.delete", None);
    assert_eq!(s.tab().notes.len(), 2);
    assert!(s.d.undo().is_some());
    assert_eq!(s.tab().notes.len(), 4);
}

#[test]
fn a_duration_key_resizes_the_whole_selection() {
    let mut s = Session::new(song_doc());
    s.run("edit.select_all", None);
    s.run("editor.tab_duration", Some("eighth"));
    for (_, _, _, len) in s.v.notes() {
        assert_eq!(len, PPQ / 2);
    }
}

#[test]
fn dragging_on_the_ruler_loops_the_bars_dragged_over_and_a_click_only_locates() {
    let mut s = Session::new(song_doc());
    let ruler_y = 28.0 + 11.0;
    let x = |tick: i64| s.v.tick_to_x(tick);
    // Press in bar 1, drag into bar 2: the loop is both bars (4/4, a bar is four beats).
    let (a, b) = (
        Point::new(x(PPQ / 2), ruler_y),
        Point::new(x(5 * PPQ), ruler_y),
    );
    let down = s.down(a, PointerButton::Primary, false);
    assert!(
        down.iter().any(|a| matches!(
            a,
            UiAction::Transport(atomik_ui::TransportAction::Locate(_))
        )),
        "the press still locates"
    );
    s.mv(b);
    assert_eq!(s.v.loop_preview(), Some((0, 8 * PPQ)));
    let up = s.up(b);
    let set: Vec<_> = up
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(atomik_model::Command::Song(
                atomik_model::SongCommand::SetLoop { range, enabled },
            )) => Some((*range, *enabled)),
            _ => None,
        })
        .collect();
    assert_eq!(set, [((0, 8 * PPQ), true)], "{up:?}");
    // A plain click sets no loop.
    s.down(a, PointerButton::Primary, false);
    let up = s.up(a);
    assert!(up.is_empty(), "{up:?}");
    assert!(s.v.loop_preview().is_none());
}

#[test]
fn dragging_backwards_over_the_ruler_loops_the_same_bars() {
    let mut s = Session::new(song_doc());
    let y = 28.0 + 11.0;
    let (a, b) = (
        Point::new(s.v.tick_to_x(5 * PPQ), y),
        Point::new(s.v.tick_to_x(PPQ), y),
    );
    s.down(a, PointerButton::Primary, false);
    s.mv(b);
    assert_eq!(s.v.loop_preview(), Some((0, 8 * PPQ)));
    s.up(b);
}
