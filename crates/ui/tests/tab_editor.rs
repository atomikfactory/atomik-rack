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
    let first = digit(&mut v, document.doc(), '1');
    let m1 = apply_tab(&mut document, &registry(), &first);
    v.sync(document.doc(), &registry());
    v.tick(1.0);
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
