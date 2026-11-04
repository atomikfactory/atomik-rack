//! The automatic colour of new tracks: off by default, then a track made by any command takes
//! the palette colour the fewest tracks use, with its device, in the undo step of the command.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{
    CreateOptions, CreateSource, DeviceKey, SongCommand, VCA_PALETTE, WorkflowCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

fn instrument(c: &mut AppCore<RecordingSink>) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
}

fn colours(c: &AppCore<RecordingSink>) -> Vec<Option<[u8; 3]>> {
    let song = &c.doc().song;
    song.tracks
        .iter()
        .map(|t| song.track_color(&t.device))
        .collect()
}

#[test]
fn new_tracks_have_no_colour_of_their_own_by_default() {
    let mut c = core();
    assert!(!c.auto_color());
    instrument(&mut c);
    assert_eq!(colours(&c), [None]);
}

#[test]
fn new_tracks_go_round_the_palette_and_the_device_takes_the_colour_too() {
    let mut c = core();
    c.set_auto_color(true);
    for _ in 0..7 {
        instrument(&mut c);
    }
    let got = colours(&c);
    let want: Vec<Option<[u8; 3]>> = (0..7)
        .map(|i| Some(VCA_PALETTE[i % VCA_PALETTE.len()]))
        .collect();
    assert_eq!(got, want, "six colours, then the first again");
    let doc = c.doc();
    for t in &doc.song.tracks {
        let device = doc.device(&t.device).unwrap();
        assert_eq!(device.color, doc.song.track_color(&t.device));
    }
}

#[test]
fn a_deleted_tracks_colour_is_used_again_and_a_coloured_track_is_left_alone() {
    let mut c = core();
    c.set_auto_color(true);
    instrument(&mut c);
    instrument(&mut c);
    let first = c.doc().song.tracks[0].device.clone();
    // A track that already has a colour of its own keeps it.
    c.apply_command(Command::Song(SongCommand::SetTrackColor {
        device: first,
        color: Some([1, 2, 3]),
    }))
    .unwrap();
    instrument(&mut c);
    let got = colours(&c);
    assert_eq!(got[0], Some([1, 2, 3]));
    assert_eq!(got[1], Some(VCA_PALETTE[1]));
    assert_eq!(
        got[2],
        Some(VCA_PALETTE[0]),
        "the first colour is the least used"
    );
}

#[test]
fn the_colour_is_part_of_the_undo_step_that_made_the_track() {
    let mut c = core();
    c.set_auto_color(true);
    instrument(&mut c);
    assert!(colours(&c)[0].is_some());
    c.undo();
    assert!(c.doc().song.tracks.is_empty());
    c.redo();
    assert_eq!(colours(&c), [Some(VCA_PALETTE[0])], "redo brings both back");
}

#[test]
fn a_command_that_makes_no_track_colours_nothing() {
    let mut c = core();
    instrument(&mut c);
    c.set_auto_color(true);
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: DeviceKey(c.doc().song.tracks[0].device.0.clone()),
        muted: true,
        solo: false,
        armed: false,
    }))
    .unwrap();
    assert_eq!(colours(&c), [None]);
}
