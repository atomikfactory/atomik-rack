//! The message lane of the piano roll: the MSG chip, adding, moving, stepping, deleting and
//! clearing the program changes and SysEx messages of a clip. Gestures run through a real
//! [`Document`], so "one undo step" is checked on the model.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipId, Command, Document, MidiData, ProgramChange, SongCommand, SysexEvent};
use atomik_ui::controller_lane::Chip;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::midi_lane::MsgRef;
use atomik_ui::timeline::Snap;
use atomik_ui::{Modifiers, PianoRollView, PointerButton, UiAction};
use support::song::*;
use support::*;

fn prog(tick: i64, program: u8) -> ProgramChange {
    ProgramChange::new(tick, program)
}

fn setup(midi: MidiData) -> (PianoRollView, Document, Registry) {
    let reg = registry();
    let mut doc = song_doc();
    doc.song_mut().tracks[0].lanes[0].clips[0].midi = midi.into();
    let d = Document::new(doc);
    let mut v = piano_roll(d.doc(), &reg);
    v.set_controllers_open(true);
    v.set_snap(Snap::Beat);
    v.set_messages_open(true);
    (v, d, reg)
}

fn data(d: &Document) -> MidiData {
    (*d.doc().song.clip(ClipId(1)).unwrap().2.midi).clone()
}

fn gesture(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    events: &[atomik_ui::UiEvent],
) -> Vec<UiAction> {
    let mut all = Vec::new();
    for e in events {
        let a = v.handle(e, d.doc(), reg);
        for c in song_commands(&a) {
            d.apply(Command::Song(c), reg).unwrap();
        }
        v.sync(d.doc(), reg);
        all.extend(a);
    }
    all
}

fn click(v: &mut PianoRollView, d: &mut Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    gesture(v, d, reg, &[down(p), up(p)])
}

fn up_with(pos: Point, button: PointerButton) -> atomik_ui::UiEvent {
    atomik_ui::UiEvent::PointerUp {
        pos,
        button,
        modifiers: Modifiers::NONE,
    }
}

fn merges(actions: &[UiAction]) -> Vec<Option<u64>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipMidi { merge, .. } => Some(merge),
            _ => None,
        })
        .collect()
}

fn two() -> MidiData {
    MidiData {
        programs: vec![
            prog(0, 5),
            ProgramChange {
                bank_msb: Some(3),
                bank_lsb: Some(0),
                ..prog(2 * PPQ, 9)
            },
        ],
        sysex: vec![SysexEvent {
            tick: PPQ,
            data: vec![0x7E, 0x7F, 0x09, 0x01],
        }],
    }
}

#[test]
fn the_msg_chip_switches_the_area_to_the_messages_and_back() {
    let reg = registry();
    let d = Document::new(song_doc());
    let mut v = piano_roll(d.doc(), &reg);
    v.set_controllers_open(true);
    assert!(!v.messages_open());
    let msg = v.controller_chip_center(Chip::Messages);
    let a = v.handle(&down(msg), d.doc(), &reg);
    v.handle(&up(msg), d.doc(), &reg);
    assert!(a.is_empty() && v.messages_open());
    let back = v.controller_chip_center(Chip::Back);
    v.handle(&down(back), d.doc(), &reg);
    v.handle(&up(back), d.doc(), &reg);
    assert!(!v.messages_open());
    assert!(v.controllers_open(), "the area stays");
}

#[test]
fn a_press_on_the_program_row_adds_a_program_change_with_the_program_in_force() {
    let (mut v, mut d, reg) = setup(two());
    let before = d.doc().clone();
    // Just after the first beat (it snaps back to it): the program in force there is 5.
    let at = v.message_row_point(PPQ + 1000);
    let a = click(&mut v, &mut d, &reg, at);
    let sent = song_commands(&a);
    assert_eq!(sent.len(), 1);
    assert!(matches!(
        sent[0],
        SongCommand::SetClipMidi { merge: None, .. }
    ));
    let m = data(&d);
    assert_eq!(m.programs.len(), 3);
    assert_eq!(m.programs[1], prog(PPQ, 5));
    assert_eq!(v.selected_message(), Some(MsgRef::Program(1)));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
}

#[test]
fn a_marker_drag_moves_the_message_on_the_grid_as_one_undo_step() {
    let (mut v, mut d, reg) = setup(two());
    let before = d.doc().clone();
    let from = v.message_point(MsgRef::Program(1)).unwrap();
    let to = Point::new(v.message_row_point(3 * PPQ).x, from.y);
    let a = gesture(&mut v, &mut d, &reg, &[down(from), mv(to), up(to)]);
    let ids = merges(&a);
    assert!(!ids.is_empty() && ids.iter().all(|m| *m == ids[0] && m.is_some()));
    let m = data(&d);
    assert_eq!(m.programs[1].tick, 3 * PPQ);
    assert_eq!(m.programs[1].bank_msb, Some(3), "the bank travels with it");
    assert_eq!(v.selected_message(), Some(MsgRef::Program(1)));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    // A still click selects and sends nothing.
    let before = d.doc().clone();
    let p = v.message_point(MsgRef::Sysex(0)).unwrap();
    let a = click(&mut v, &mut d, &reg, p);
    assert!(song_commands(&a).is_empty());
    assert_eq!(v.selected_message(), Some(MsgRef::Sysex(0)));
    assert_eq!(d.doc(), &before);
    // A SysEx marker drags too.
    let to = Point::new(v.message_row_point(2 * PPQ).x, p.y);
    gesture(&mut v, &mut d, &reg, &[down(p), mv(to), up(to)]);
    assert_eq!(data(&d).sysex[0].tick, 2 * PPQ);
    assert_eq!(data(&d).sysex[0].data, [0x7E, 0x7F, 0x09, 0x01]);
}

#[test]
fn the_steppers_change_the_selected_program_change_one_undo_step_each() {
    let (mut v, mut d, reg) = setup(two());
    // Nothing selected: the steppers do nothing.
    let prg = v.controller_chip_center(Chip::Program);
    let a = click(&mut v, &mut d, &reg, Point::new(prg.x + 28.0, prg.y));
    assert!(song_commands(&a).is_empty());
    let p = v.message_point(MsgRef::Program(0)).unwrap();
    click(&mut v, &mut d, &reg, p);
    let depth = d.undo_depth();
    // Right arrow: program 6; left arrow twice: 4.
    click(&mut v, &mut d, &reg, Point::new(prg.x + 28.0, prg.y));
    assert_eq!(data(&d).programs[0].program, 6);
    click(&mut v, &mut d, &reg, Point::new(prg.x - 28.0, prg.y));
    click(&mut v, &mut d, &reg, Point::new(prg.x - 28.0, prg.y));
    assert_eq!(data(&d).programs[0].program, 4);
    assert_eq!(d.undo_depth(), depth + 3);
    // The bank: MSB up from none gives 0, the LSB likewise; the body drops the byte.
    let msb = v.controller_chip_center(Chip::BankMsb);
    click(&mut v, &mut d, &reg, Point::new(msb.x + 28.0, msb.y));
    assert_eq!(data(&d).programs[0].bank_msb, Some(0));
    click(&mut v, &mut d, &reg, Point::new(msb.x + 28.0, msb.y));
    assert_eq!(data(&d).programs[0].bank_msb, Some(1));
    click(&mut v, &mut d, &reg, msb);
    assert_eq!(data(&d).programs[0].bank_msb, None);
    let lsb = v.controller_chip_center(Chip::BankLsb);
    click(&mut v, &mut d, &reg, Point::new(lsb.x - 28.0, lsb.y));
    assert_eq!(data(&d).programs[0].bank_lsb, Some(127));
    assert_eq!(v.selected_message(), Some(MsgRef::Program(0)));
}

#[test]
fn the_secondary_button_and_delete_remove_a_message_and_clear_removes_them_all() {
    let (mut v, mut d, reg) = setup(two());
    let before = d.doc().clone();
    let p = v.message_point(MsgRef::Program(1)).unwrap();
    let e = [
        down_with(p, PointerButton::Secondary, 1),
        up_with(p, PointerButton::Secondary),
    ];
    gesture(&mut v, &mut d, &reg, &e);
    assert_eq!(data(&d).programs, [prog(0, 5)]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    v.sync(d.doc(), &reg);
    // Select the SysEx message, then Delete.
    let s = v.message_point(MsgRef::Sysex(0)).unwrap();
    click(&mut v, &mut d, &reg, s);
    assert!(v.message_focus());
    let out = v.command("edit.delete", None, d.doc(), &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("Delete is the roll's while a message is selected");
    };
    for c in song_commands(&actions) {
        d.apply(Command::Song(c), &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
    assert!(data(&d).sysex.is_empty());
    assert_eq!(data(&d).programs.len(), 2);
    // CLEAR empties the rest in one step.
    let clear = v.controller_chip_center(Chip::ClearMessages);
    click(&mut v, &mut d, &reg, clear);
    assert!(data(&d).is_empty());
    let depth = d.undo_depth();
    let a = click(&mut v, &mut d, &reg, clear);
    assert!(song_commands(&a).is_empty(), "nothing to clear");
    assert_eq!(d.undo_depth(), depth);
    d.undo().unwrap();
    assert_eq!(data(&d).programs.len(), 2);
}

#[test]
fn the_sysex_chip_adds_a_general_midi_message_at_the_playhead() {
    let (mut v, mut d, reg) = setup(MidiData::default());
    let add = v.controller_chip_center(Chip::AddSysex);
    let a = click(&mut v, &mut d, &reg, add);
    assert_eq!(song_commands(&a).len(), 1);
    let m = data(&d);
    assert_eq!(m.sysex.len(), 1);
    assert_eq!(m.sysex[0].data, [0x7E, 0x7F, 0x09, 0x01]);
    assert_eq!(v.selected_message(), Some(MsgRef::Sysex(0)));
}

#[test]
fn a_press_outside_the_lane_forgets_the_selection_and_hands_the_keys_back() {
    let (mut v, mut d, reg) = setup(two());
    let p = v.message_point(MsgRef::Program(0)).unwrap();
    click(&mut v, &mut d, &reg, p);
    assert!(v.message_focus());
    // A press in the grid hands the keys back to the notes.
    let grid = Point::new(v.bounds().x0 + 400.0, v.bounds().y0 + 200.0);
    v.handle(&down(grid), d.doc(), &reg);
    v.handle(&up(grid), d.doc(), &reg);
    assert!(!v.message_focus());
    assert_eq!(v.selected_message(), None);
}
