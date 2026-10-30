//! The arrangement corner's master mute, solo and Manual Rec buttons and the master keyboard arrow
//! of a track header: what a click and each command ask the app for.

mod support;

use atomik_model::{Command, SongCommand};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, UiAction};
use support::song::*;
use support::*;

/// The corner buttons (the view sits at the origin): the master mute, the master solo and Manual
/// Rec, one row under the snap selector.
const MUTE_BUTTON: Point = Point::new(22.0, 32.5);
const SOLO_BUTTON: Point = Point::new(46.0, 32.5);
const MANUAL_REC_BUTTON: Point = Point::new(94.0, 32.5);
/// The arrow of the first track header (the first row starts under the 40 px ruler).
const FIRST_ARROW: Point = Point::new(180.0, 56.0);

fn flagged(muted: &[usize], soloed: &[usize]) -> (atomik_model::RackDoc, ArrangementView) {
    let (mut doc, reg) = (song_doc(), registry());
    for t in &mut doc.song.tracks {
        t.muted = false;
        t.solo = false;
    }
    for i in muted {
        doc.song.tracks[*i].muted = true;
    }
    for i in soloed {
        doc.song.tracks[*i].solo = true;
    }
    let v = arrangement(&doc, &reg);
    (doc, v)
}

fn click(v: &mut ArrangementView, doc: &atomik_model::RackDoc, at: Point) -> Vec<UiAction> {
    let reg = registry();
    run(|e| v.handle(e, doc, &reg), &[mv(at), down(at), up(at)])
}

/// The one batch of an action list: its flags as `(muted, solo, armed)` per track device.
fn batch(actions: &[UiAction]) -> Vec<(String, bool, bool, bool)> {
    let batches: Vec<&Vec<Command>> = actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::Batch(b)) => Some(b),
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 1, "one undo step: {actions:?}");
    batches[0]
        .iter()
        .map(|c| match c {
            Command::Song(SongCommand::SetTrackFlags {
                device,
                muted,
                solo,
                armed,
            }) => (device.to_string(), *muted, *solo, *armed),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn the_master_mute_clears_every_mute_in_one_batch_and_keeps_solos_and_arming() {
    let (doc, mut v) = flagged(&[0, 2], &[1]);
    let a = click(&mut v, &doc, MUTE_BUTTON);
    let flags = batch(&a);
    assert_eq!(flags.len(), 2, "only the muted tracks: {flags:?}");
    assert!(flags.iter().all(|(_, muted, solo, _)| !muted && !solo));
    // The track that is soloed but not muted is untouched.
    let soloed = doc.song.tracks[1].device.to_string();
    assert!(flags.iter().all(|(d, ..)| *d != soloed));
}

#[test]
fn the_master_solo_clears_every_solo_and_keeps_mutes() {
    let (doc, mut v) = flagged(&[0], &[0, 1]);
    let a = click(&mut v, &doc, SOLO_BUTTON);
    let flags = batch(&a);
    assert_eq!(flags.len(), 2, "{flags:?}");
    let first = doc.song.tracks[0].device.to_string();
    let first = flags.iter().find(|(d, ..)| *d == first).unwrap();
    assert!(first.1, "the mute of a soloed track stays: {first:?}");
    assert!(flags.iter().all(|(_, _, solo, _)| !solo));
}

#[test]
fn clearing_with_nothing_set_asks_for_nothing_and_the_commands_agree() {
    let (doc, mut v) = flagged(&[], &[]);
    assert!(click(&mut v, &doc, MUTE_BUTTON).is_empty());
    assert!(click(&mut v, &doc, SOLO_BUTTON).is_empty());
    let reg = registry();
    for id in ["sequencer.clear_mutes", "sequencer.clear_solos"] {
        assert!(matches!(
            v.command(id, None, &doc, &reg),
            CommandOutcome::Declined
        ));
    }
    let (doc, mut v) = flagged(&[1], &[]);
    match v.command("sequencer.clear_mutes", None, &doc, &reg) {
        CommandOutcome::Handled(a) => assert_eq!(batch(&a).len(), 1),
        other => panic!("{other:?}"),
    }
}

#[test]
fn manual_rec_is_asked_for_by_its_button_and_its_command() {
    let (doc, mut v) = flagged(&[], &[]);
    assert!(
        click(&mut v, &doc, MANUAL_REC_BUTTON)
            .iter()
            .any(|a| matches!(a, UiAction::ToggleManualRec))
    );
    match v.command("sequencer.manual_rec", None, &doc, &registry()) {
        CommandOutcome::Handled(a) => assert!(matches!(a[..], [UiAction::ToggleManualRec])),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_header_arrow_chooses_the_keyboard_track_only_in_separated_mode() {
    let (doc, mut v) = flagged(&[], &[]);
    let first = doc.song.tracks[0].device.clone();
    // Standard mode: the arrow is an indicator, a click asks for nothing.
    v.set_master_keyboard(Some(first.clone()), false);
    let a = click(&mut v, &doc, FIRST_ARROW);
    assert!(
        !a.iter()
            .any(|a| matches!(a, UiAction::SetMasterKeyboard(_))),
        "{a:?}"
    );
    // Separated mode: a click on another track's arrow chooses it...
    v.set_master_keyboard(None, true);
    let a = click(&mut v, &doc, FIRST_ARROW);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::SetMasterKeyboard(Some(d)) if *d == first)),
        "{a:?}"
    );
    // ...and a click on the lit one hands the keyboard back to the selection.
    v.set_master_keyboard(Some(first), true);
    let a = click(&mut v, &doc, FIRST_ARROW);
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::SetMasterKeyboard(None))),
        "{a:?}"
    );
}
