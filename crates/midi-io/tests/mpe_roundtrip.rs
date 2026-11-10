//! What the MPE output of `atomik-core` sends, read back by the MPE input of this crate: the
//! zone from the configuration message, and each note with its own expression.

use atomik_core::mpe_out::{Initial, Out, mcm};
use atomik_core::{ExprKind, MpeEncoder, MpeZones, NoteKind, ZoneSide};
use atomik_midi_io::mpe::MpeInput;

fn feed(input: &mut MpeInput, out: &Out) -> Vec<NoteKind> {
    out.as_slice()
        .iter()
        .flat_map(|m| input.feed(m.bytes()).as_slice().to_vec())
        .collect()
}

#[test]
fn the_configuration_message_sets_the_zone_the_input_reads() {
    for (side, members) in [(ZoneSide::Lower, 7), (ZoneSide::Upper, 4)] {
        let mut input = MpeInput::new(MpeZones::default());
        for m in mcm(side, members) {
            input.feed(&m);
        }
        assert_eq!(input.zones(), MpeZones::one(side, members));
    }
}

#[test]
fn notes_with_their_expression_come_back_as_the_same_note_events() {
    let mut encoder = MpeEncoder::new(ZoneSide::Lower, 8);
    let mut input = MpeInput::new(MpeZones::one(ZoneSide::Lower, 8));
    let mut out = Out::new();
    // Two notes at once, the first bent up an octave before it sounds.
    encoder.note_on(
        60,
        60,
        100,
        Initial {
            tuning: Some(12.0),
            ..Initial::default()
        },
        &mut out,
    );
    encoder.note_on(64, 64, 100, Initial::default(), &mut out);
    encoder.pressure(64, 1.0, &mut out);
    encoder.slide(60, 0.5, &mut out);
    let events = feed(&mut input, &out);
    assert!(events.contains(&NoteKind::On {
        key: 60,
        velocity: 100.0 / 127.0
    }));
    let tuning = events
        .iter()
        .find_map(|e| match e {
            NoteKind::NoteExpr {
                key: 60,
                kind: ExprKind::Tuning,
                value,
            } => Some(*value),
            _ => None,
        })
        .expect("the bend of the first note");
    assert!((tuning - 12.0).abs() < 0.01, "{tuning}");
    assert!(events.iter().any(|e| matches!(
        e,
        NoteKind::NoteExpr { key: 64, kind: ExprKind::Pressure, value } if (*value - 1.0).abs() < 1e-6
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        NoteKind::NoteExpr { key: 60, kind: ExprKind::Slide, value } if (*value - 0.504).abs() < 0.01
    )));
    // The releases come back on the right keys.
    let mut off = Out::new();
    encoder.note_off(60, &mut off);
    encoder.note_off(64, &mut off);
    assert_eq!(
        feed(&mut input, &off),
        vec![NoteKind::Off { key: 60 }, NoteKind::Off { key: 64 }]
    );
}
