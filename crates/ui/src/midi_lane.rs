//! The message lane of the piano roll: the program changes and SysEx messages of one note clip,
//! drawn in the controller area on the roll's time axis (the MSG chip switches to it).
//!
//! The roll owns the geometry and the pointer routing; this module holds the pure pieces so they
//! can be tested without a view: what a marker says ([`program_label`], [`sysex_label`]), where
//! it sits and what a press hits ([`marker_rect`], [`hit`]), and what the steppers of the chip
//! row do to the selected program change ([`step`]). Every edit returns the whole clip data
//! (`MidiData`), which goes into one `SongCommand::SetClipMidi`.
//!
//! Program changes sit on the upper row, SysEx messages on the lower one. A press on empty space
//! of the upper row adds a program change (the program in force there, or 0); a press on a
//! marker selects it and a drag moves it on the grid; the secondary button or the eraser deletes
//! it. The chips step the program and the bank select bytes of the selected program change, add
//! a SysEx message at the playhead and clear the lot.

use crate::controller_lane::{Chip, Part};
use atomik_model::{MidiData, ProgramChange, SysexEvent};
use vello::kurbo::Rect;

/// The SysEx message the add chip inserts: General MIDI System On.
pub const GM_SYSTEM_ON: [u8; 4] = [0x7E, 0x7F, 0x09, 0x01];
/// Height of a marker.
pub const MARKER_HEIGHT: f64 = 18.0;
/// Distance of the program row's top from the lane body's top.
pub const PROGRAM_ROW: f64 = 10.0;
/// Distance of the SysEx row's top from the lane body's top.
pub const SYSEX_ROW: f64 = 48.0;

/// One message of the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRef {
    /// Program change `i` of the clip's data.
    Program(usize),
    /// SysEx message `i` of the clip's data.
    Sysex(usize),
}

/// What a program change marker shows: `P5`, or `B3.0 P5` with the bank select (`-` for a byte
/// that is not sent).
pub fn program_label(p: &ProgramChange) -> String {
    if p.has_bank() {
        let byte = |b: Option<u8>| b.map_or("-".to_owned(), |b| b.to_string());
        format!("B{}.{} P{}", byte(p.bank_msb), byte(p.bank_lsb), p.program)
    } else {
        format!("P{}", p.program)
    }
}

/// What a SysEx marker shows: `SYX` and the number of data bytes.
pub fn sysex_label(s: &SysexEvent) -> String {
    format!("SYX {}", s.data.len())
}

/// The width of a marker with `label`.
pub fn marker_width(label: &str) -> f64 {
    10.0 + 5.6 * label.chars().count() as f64
}

/// The label of a message.
pub fn label_of(data: &MidiData, m: MsgRef) -> Option<String> {
    match m {
        MsgRef::Program(i) => data.programs.get(i).map(program_label),
        MsgRef::Sysex(i) => data.sysex.get(i).map(sysex_label),
    }
}

/// The marker of message `m` in a lane body `body`, with its tick at `x`: it starts just left of
/// the tick's line so the line points at the marker's corner.
pub fn marker_rect(body: Rect, x: f64, m: MsgRef, label: &str) -> Rect {
    let top = body.y0
        + match m {
            MsgRef::Program(_) => PROGRAM_ROW,
            MsgRef::Sysex(_) => SYSEX_ROW,
        };
    Rect::new(
        x - 2.0,
        top,
        x - 2.0 + marker_width(label),
        top + MARKER_HEIGHT,
    )
}

/// The message under `(x, y)`; a later message wins where markers overlap. `tick_to_x` maps a
/// content tick to a logical x.
pub fn hit(
    data: &MidiData,
    body: Rect,
    tick_to_x: impl Fn(i64) -> f64,
    x: f64,
    y: f64,
) -> Option<MsgRef> {
    let inside = |r: Rect| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
    let programs = data.programs.iter().enumerate().rev().map(|(i, p)| {
        let m = MsgRef::Program(i);
        (
            m,
            marker_rect(body, tick_to_x(p.tick), m, &program_label(p)),
        )
    });
    let sysex = data.sysex.iter().enumerate().rev().map(|(i, s)| {
        let m = MsgRef::Sysex(i);
        (m, marker_rect(body, tick_to_x(s.tick), m, &sysex_label(s)))
    });
    programs
        .chain(sysex)
        .find(|(_, r)| inside(*r))
        .map(|(m, _)| m)
}

/// True when `y` is on the program row of a lane body at `body` (where a press adds a program
/// change).
pub fn on_program_row(body: Rect, y: f64) -> bool {
    y >= body.y0 + PROGRAM_ROW - 4.0 && y < body.y0 + PROGRAM_ROW + MARKER_HEIGHT + 4.0
}

/// The text of a message chip; the steppers show the selected program change's values.
pub fn chip_text(chip: Chip, selected: Option<&ProgramChange>) -> String {
    let byte = |b: Option<u8>| b.map_or("-".to_owned(), |b| b.to_string());
    match (chip, selected) {
        (Chip::Program, Some(p)) => format!("< PRG {} >", p.program),
        (Chip::BankMsb, Some(p)) => format!("< MSB {} >", byte(p.bank_msb)),
        (Chip::BankLsb, Some(p)) => format!("< LSB {} >", byte(p.bank_lsb)),
        _ => crate::controller_lane::chip_label(chip, &Default::default()),
    }
}

/// A bank byte after an arrow press: the values `none, 0, 1, .. 127` form a ring; a press on the
/// body of the chip drops the byte.
fn step_bank(b: Option<u8>, part: Part) -> Option<u8> {
    match (part, b) {
        (Part::Body, _) => None,
        (Part::Up, None) => Some(0),
        (Part::Up, Some(127)) => None,
        (Part::Up, Some(v)) => Some(v + 1),
        (Part::Down, None) => Some(127),
        (Part::Down, Some(0)) => None,
        (Part::Down, Some(v)) => Some(v - 1),
    }
}

/// The data after a press on a stepper chip (`Program`, `BankMsb` or `BankLsb`) for program
/// change `index`; `None` when nothing changes (another chip, no such change, the body of the
/// program stepper).
pub fn step(data: &MidiData, index: usize, chip: Chip, part: Part) -> Option<MidiData> {
    let mut p = *data.programs.get(index)?;
    match chip {
        Chip::Program => {
            p.program = match part {
                Part::Up => (p.program + 1) % 128,
                Part::Down => p.program.checked_sub(1).unwrap_or(127),
                Part::Body => return None,
            }
        }
        Chip::BankMsb => p.bank_msb = step_bank(p.bank_msb, part),
        Chip::BankLsb => p.bank_lsb = step_bank(p.bank_lsb, part),
        _ => return None,
    }
    let mut out = data.clone();
    out.programs[index] = p;
    (out != *data).then_some(out)
}

/// The data after a press on empty space of the program row at content `tick`: a program change
/// with the program in force there (0 before the first), and its index.
pub fn add_program(data: &MidiData, tick: i64) -> Option<(MidiData, usize)> {
    let program = data.program_at(tick).map_or(0, |p| p.program);
    data.with_program(ProgramChange::new(tick, program))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(tick: i64, program: u8) -> ProgramChange {
        ProgramChange::new(tick, program)
    }

    fn data() -> MidiData {
        MidiData {
            programs: vec![
                prog(0, 5),
                ProgramChange {
                    bank_msb: Some(3),
                    bank_lsb: None,
                    ..prog(100, 9)
                },
            ],
            sysex: vec![SysexEvent {
                tick: 50,
                data: vec![1, 2, 3],
            }],
        }
    }

    #[test]
    fn labels_say_the_bank_and_the_program() {
        let d = data();
        assert_eq!(program_label(&d.programs[0]), "P5");
        assert_eq!(program_label(&d.programs[1]), "B3.- P9");
        assert_eq!(sysex_label(&d.sysex[0]), "SYX 3");
        assert_eq!(label_of(&d, MsgRef::Program(1)).as_deref(), Some("B3.- P9"));
        assert_eq!(label_of(&d, MsgRef::Sysex(4)), None);
        assert_eq!(chip_text(Chip::Program, d.programs.first()), "< PRG 5 >");
        assert_eq!(chip_text(Chip::BankMsb, d.programs.get(1)), "< MSB 3 >");
        assert_eq!(chip_text(Chip::BankLsb, d.programs.get(1)), "< LSB - >");
        assert_eq!(chip_text(Chip::Program, None), "< PRG >");
        assert_eq!(chip_text(Chip::ClearMessages, None), "CLEAR");
    }

    #[test]
    fn a_press_hits_the_marker_on_its_row() {
        let d = data();
        let body = Rect::new(60.0, 100.0, 900.0, 184.0);
        let x = |t: i64| 60.0 + t as f64;
        let prog_y = body.y0 + PROGRAM_ROW + 5.0;
        let sysex_y = body.y0 + SYSEX_ROW + 5.0;
        assert_eq!(hit(&d, body, x, 62.0, prog_y), Some(MsgRef::Program(0)));
        assert_eq!(hit(&d, body, x, 170.0, prog_y), Some(MsgRef::Program(1)));
        assert_eq!(hit(&d, body, x, 112.0, sysex_y), Some(MsgRef::Sysex(0)));
        assert_eq!(hit(&d, body, x, 112.0, prog_y), None, "wrong row");
        assert_eq!(hit(&d, body, x, 500.0, prog_y), None);
        assert!(on_program_row(body, prog_y) && !on_program_row(body, sysex_y));
    }

    #[test]
    fn the_program_stepper_wraps_and_the_bank_steppers_pass_through_none() {
        let d = data();
        let up = step(&d, 0, Chip::Program, Part::Up).unwrap();
        assert_eq!(up.programs[0].program, 6);
        let down = step(&d, 0, Chip::Program, Part::Down).unwrap();
        assert_eq!(down.programs[0].program, 4);
        let mut zero = d.clone();
        zero.programs[0].program = 0;
        assert_eq!(
            step(&zero, 0, Chip::Program, Part::Down).unwrap().programs[0].program,
            127
        );
        assert!(step(&d, 0, Chip::Program, Part::Body).is_none());
        // The bank: none, 0 .. 127, none.
        let b = step(&d, 0, Chip::BankMsb, Part::Up).unwrap();
        assert_eq!(b.programs[0].bank_msb, Some(0));
        let b = step(&b, 0, Chip::BankMsb, Part::Down).unwrap();
        assert_eq!(b.programs[0].bank_msb, None);
        let b = step(&b, 0, Chip::BankMsb, Part::Down).unwrap();
        assert_eq!(b.programs[0].bank_msb, Some(127));
        let b = step(&b, 0, Chip::BankMsb, Part::Up).unwrap();
        assert_eq!(b.programs[0].bank_msb, None);
        let c = step(&d, 1, Chip::BankMsb, Part::Body).unwrap();
        assert_eq!(c.programs[1].bank_msb, None, "the body drops the byte");
        assert!(
            step(&d, 0, Chip::BankLsb, Part::Body).is_none(),
            "already none"
        );
        assert!(step(&d, 9, Chip::Program, Part::Up).is_none());
        assert!(step(&d, 0, Chip::Thin, Part::Up).is_none());
    }

    #[test]
    fn a_new_program_change_takes_the_program_in_force() {
        let d = data();
        let (next, i) = add_program(&d, 150).unwrap();
        assert_eq!(i, 2);
        assert_eq!(next.programs[2], prog(150, 9));
        let (next, i) = add_program(&MidiData::default(), 7).unwrap();
        assert_eq!((i, next.programs[0].program), (0, 0));
        assert!(add_program(&d, -1).is_none());
    }
}
