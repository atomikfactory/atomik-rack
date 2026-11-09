//! The expression strip of the piano roll: a note that carries per-note expression shows its
//! curves in a small strip under it while it is the selected note, a press on the strip's button
//! clears them in one undo step, and a note without expression has no strip.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipId, Command, Document, ExpressionPoint, MidiData, NoteExpression, SongCommand,
};
use atomik_ui::{DisplayList, PianoRollView, UiAction};
use support::song::*;
use support::*;

fn pt(tick: i64, value: f32) -> ExpressionPoint {
    ExpressionPoint::new(tick, value)
}

/// The first note of clip 1 gets curves; returns the roll with that note selected.
fn setup() -> (PianoRollView, Document, Registry, i64, u8) {
    let reg = registry();
    let mut doc = song_doc();
    let clip = &mut doc.song_mut().tracks[0].lanes[0].clips[0];
    let atomik_model::ClipContent::Notes(notes) = &clip.content else {
        panic!("a note clip");
    };
    let (tick, key) = (notes[0].tick, notes[0].key);
    let mut e = NoteExpression::new(tick, key);
    e.pitch = vec![pt(0, 0.0), pt(PPQ / 8, 1.5), pt(PPQ / 4, -2.0)];
    e.pressure = vec![pt(0, 0.2), pt(PPQ / 4, 0.9)];
    e.timbre = vec![pt(0, 0.5)];
    clip.midi = MidiData {
        expression: vec![e],
        ..Default::default()
    }
    .into();
    let d = Document::new(doc);
    let mut v = piano_roll(d.doc(), &reg);
    v.set_selected_notes(&[0]);
    (v, d, reg, tick, key)
}

fn midi_of(d: &Document) -> MidiData {
    (*d.doc().song.clip(ClipId(1)).unwrap().2.midi).clone()
}

#[test]
fn the_selected_note_shows_its_strip_under_it() {
    let (v, _d, _reg, _, _) = setup();
    let (strip, clear) = v.expression_rects().expect("the note has expression");
    let note = v.note_rect(0).unwrap();
    assert!(strip.y0 >= note.y1, "under the note");
    assert!(strip.x0 <= note.x0 + 1.0 && strip.width() >= 72.0 - 0.5);
    assert!(strip.contains(clear.center()));
    assert_eq!(v.expression_clear_center(), Some(clear.center()));
}

#[test]
fn only_one_selected_note_with_expression_has_a_strip() {
    let (mut v, _d, _reg, _, _) = setup();
    // Two notes selected: no strip. A note without expression: none either.
    v.set_selected_notes(&[0, 1]);
    assert!(v.expression_rects().is_none());
    v.set_selected_notes(&[1]);
    assert!(v.expression_rects().is_none());
    v.set_selected_notes(&[]);
    assert!(v.expression_rects().is_none());
}

#[test]
fn the_strip_is_painted_and_the_button_has_a_tooltip() {
    let (mut v, d, reg, _, _) = setup();
    let mut with = DisplayList::new();
    v.paint(&mut with, d.doc(), &reg);
    v.set_selected_notes(&[1]);
    let mut without = DisplayList::new();
    v.paint(&mut without, d.doc(), &reg);
    assert!(
        with.len() > without.len(),
        "the strip adds shapes: {} vs {}",
        with.len(),
        without.len()
    );
    v.set_selected_notes(&[0]);
    let clear = v.expression_clear_center().unwrap();
    let help = v.help_at(clear).expect("a tooltip");
    assert!(help.text.contains("Clear"), "{}", help.text);
    assert!(help.text.len() <= 80, "{}", help.text.len());
}

#[test]
fn the_clear_button_clears_the_notes_expression_as_one_undo_step() {
    let (mut v, mut d, reg, tick, key) = setup();
    let before = d.doc().clone();
    let at = v.expression_clear_center().unwrap();
    let a = v.handle(&down(at), d.doc(), &reg);
    let b = v.handle(&up(at), d.doc(), &reg);
    let sent: Vec<SongCommand> = song_commands(&[a, b].concat());
    assert_eq!(sent.len(), 1, "one command");
    assert!(matches!(
        sent[0],
        SongCommand::SetClipMidi { merge: None, .. }
    ));
    d.apply(Command::Song(sent[0].clone()), &reg).unwrap();
    assert!(midi_of(&d).expression_of(tick, key).is_none());
    v.sync(d.doc(), &reg);
    assert!(v.expression_rects().is_none(), "the strip is gone");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step restores it");
}

#[test]
fn a_press_on_the_strip_changes_nothing() {
    let (mut v, d, reg, _, _) = setup();
    let (strip, clear) = v.expression_rects().unwrap();
    let at = atomik_ui::kurbo::Point::new(strip.x0 + 4.0, strip.center().y);
    assert!(!clear.contains(at));
    let a = v.handle(&down(at), d.doc(), &reg);
    let b = v.handle(&up(at), d.doc(), &reg);
    assert!(
        a.iter()
            .chain(&b)
            .all(|x| !matches!(x, UiAction::Command(_)))
    );
    assert_eq!(v.selected_notes(), vec![0], "the selection stays");
}
