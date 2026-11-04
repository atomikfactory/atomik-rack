//! Takes follow-ups in the app core: the takes clipboard (menu actions, Edit > Copy, Cut and Paste
//! on a focused take row, across New and Open), a file imported as a take (audio and MIDI) and
//! a take opened in the piano roll.

use super::*;
use crate::audio_import::ticks_for_frames;
use crate::clipboard::ClipboardKind;
use crate::midi_file::clip_file;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand, TakeGroup, empty_doc,
};
use atomik_ui::{ImportTarget, UiAction};
use std::sync::atomic::{AtomicU64, Ordering};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-takes-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.view_mut().set_show_console_devices(false);
    c
}

fn add_track(c: &mut AppCore<RecordingSink>, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_owned(),
        key: Some(key(k)),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
        .unwrap();
}

fn note_take(start: i64, length: i64, k: u8) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(vec![Note::new(0, PPQ / 2, k, 0.8)].into()),
    }
}

/// Synth `a` with a group of three note takes (keys 60..62, two bars from bar 2), and synth `b`
/// with a group of one take.
fn two_groups() -> AppCore<RecordingSink> {
    let mut c = core();
    add_track(&mut c, "a");
    add_track(&mut c, "b");
    for (device, takes) in [
        (
            "a",
            (0..3).map(|i| note_take(BAR, 2 * BAR, 60 + i)).collect(),
        ),
        ("b", vec![note_take(8 * BAR, BAR, 36)]),
    ] {
        c.apply_command(Command::Song(SongCommand::NewTakeGroup {
            device: key(device),
            lane: 0,
            takes,
        }))
        .unwrap();
    }
    c.after_view_change();
    c
}

fn group(c: &AppCore<RecordingSink>, id: u32) -> TakeGroup {
    c.doc().song.take_group(id).expect("a group").2.clone()
}

fn saved(c: &AppCore<RecordingSink>) -> String {
    atomik_model::save_project(c.doc())
}

fn command(c: &mut AppCore<RecordingSink>, pane: Pane, id: &str) {
    let _ = c.pane_command(pane, id, None);
}

// ------------------------------------------------------------------ the clipboard

#[test]
fn the_menu_actions_copy_takes_to_the_clipboard_and_paste_them_into_another_group() {
    let mut c = two_groups();
    c.handle_action(UiAction::CopyTakes {
        group: 1,
        takes: vec![0, 2],
    });
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Takes));
    assert_eq!(c.clipboard().get().unwrap().len(), 2);
    let before = saved(&c);
    let depth = c.document().undo_depth();
    c.handle_action(UiAction::PasteTakes { group: 2 });
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let g = group(&c, 2);
    assert_eq!(g.takes.len(), 3);
    assert_eq!(
        g.takes[1].start,
        8 * BAR,
        "the earliest starts where the group does"
    );
    assert!(c.undo());
    assert_eq!(saved(&c), before);
}

#[test]
fn a_paste_with_nothing_on_the_clipboard_says_so_and_changes_nothing() {
    let mut c = two_groups();
    let before = saved(&c);
    c.handle_action(UiAction::PasteTakes { group: 2 });
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("clipboard is empty")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(saved(&c), before);
}

#[test]
fn edit_copy_cut_and_paste_work_on_the_focused_take_row() {
    let mut c = two_groups();
    c.arrangement_mut().set_take_focus(1, 1);
    assert!(c.has_selection(Pane::Arrangement));
    command(&mut c, Pane::Arrangement, "edit.copy");
    assert_eq!(c.clipboard().kind(), Some(ClipboardKind::Takes));
    assert!(c.can_paste(Pane::Arrangement));
    // Paste goes into the group of the focused row.
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert_eq!(group(&c, 1).takes.len(), 4);
    // Cut deletes the take after copying it.
    c.arrangement_mut().set_take_focus(2, 0);
    command(&mut c, Pane::Arrangement, "edit.cut");
    assert!(
        c.doc().song.take_group(2).is_none(),
        "the last take took its group along"
    );
    assert_eq!(c.clipboard().get().unwrap().len(), 1);
}

#[test]
fn takes_paste_across_new_into_a_group_started_on_the_selected_lane() {
    let mut c = two_groups();
    c.handle_action(UiAction::CopyTakes {
        group: 1,
        takes: vec![0, 1, 2],
    });
    c.new_document();
    add_track(&mut c, "z");
    c.after_view_change();
    // No take row is focused and no track selected: the paste says what to select.
    command(&mut c, Pane::Arrangement, "edit.paste");
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Select a take row")),
        "{:?}",
        c.last_rejection()
    );
    c.arrangement_mut().set_selected_track(Some(key("z")));
    c.locate(3 * BAR);
    command(&mut c, Pane::Arrangement, "edit.paste");
    let (device, _, g) = c.doc().song.take_groups().next().expect("a new group");
    assert_eq!(device, &key("z"));
    assert_eq!(g.takes.len(), 3);
    assert_eq!(g.range(), (3 * BAR, 5 * BAR));
}

// ------------------------------------------------------------------ import

fn wav(dir: &Tmp, name: &str, frames: usize) -> PathBuf {
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

#[test]
fn an_audio_file_imports_as_a_take_where_the_group_starts() {
    let t = Tmp::new("audio");
    let mut c = core();
    // A group of one audio take, made from an imported clip on a new audio track.
    c.import_audio(
        vec![wav(&t, "first.wav", 96_000)],
        ImportTarget::NewTracks,
        2 * BAR,
    );
    c.wait_background();
    let track = c.doc().song.tracks[0].device.clone();
    let clip = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.apply_command(Command::Song(SongCommand::GroupAsTakes {
        device: track,
        lane: 0,
        clips: vec![clip],
        extra: Vec::new(),
    }))
    .unwrap();
    assert_eq!(group(&c, 1).range().0, 2 * BAR);
    let depth = c.document().undo_depth();
    c.set_take_import(Some(1));
    assert_eq!(c.take_import_request(), Some(1));
    assert_eq!(c.take_import_request(), None, "the request is used up");
    c.import_takes(vec![wav(&t, "second.wav", 48_000)], 1);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let g = group(&c, 1);
    assert_eq!(g.takes.len(), 2);
    assert_eq!(g.takes[1].start, 2 * BAR);
    assert_eq!(
        g.takes[1].length,
        ticks_for_frames(&c.doc().song, 2 * BAR, 48_000, c.doc().sample_rate)
    );
    assert_eq!(
        c.document().undo_depth(),
        depth + 1,
        "the take is one undo step"
    );
    assert_eq!(c.doc().assets.len(), 2);
}

#[test]
fn a_file_of_the_wrong_kind_for_the_group_is_refused() {
    let t = Tmp::new("kind");
    let mut c = two_groups();
    let before = saved(&c);
    c.import_takes(vec![wav(&t, "x.wav", 48_000)], 1);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("holds notes")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(saved(&c), before);
    c.import_takes(vec![wav(&t, "x.wav", 48_000)], 99);
    assert!(c.last_rejection().is_some_and(|m| m.contains("gone")));
}

#[test]
fn a_midi_file_imports_as_a_note_take_and_is_refused_by_an_audio_group() {
    let t = Tmp::new("midi");
    let mut c = two_groups();
    // A file holding one clip's notes.
    let source = Clip {
        content: ClipContent::Notes(
            vec![Note::new(0, PPQ, 70, 0.8), Note::new(PPQ, PPQ, 72, 0.8)].into(),
        ),
        ..note_take(0, BAR, 70)
    };
    let bytes = clip_file(
        &c.doc().song,
        &source,
        "take",
        crate::midi_file::MidiResolution::Native,
    );
    let path = t.path("take.mid");
    std::fs::write(&path, bytes).unwrap();
    let depth = c.document().undo_depth();
    c.import_midi_take(&path, 2);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Imported 2 notes"))
    );
    let g = group(&c, 2);
    assert_eq!(g.takes.len(), 2);
    assert_eq!(
        g.takes[1].start,
        8 * BAR,
        "it starts where the group starts"
    );
    let ClipContent::Notes(n) = &g.takes[1].content else {
        panic!("notes")
    };
    assert_eq!(n.len(), 2);
    assert_eq!(c.document().undo_depth(), depth + 1);
    // A missing group and a bad file are reported.
    c.import_midi_take(&path, 99);
    assert!(c.last_rejection().is_some_and(|m| m.contains("gone")));
    std::fs::write(&path, b"junk").unwrap();
    let before = saved(&c);
    c.import_midi_take(&path, 2);
    assert!(c.last_rejection().is_some());
    assert_eq!(saved(&c), before);
}

// ------------------------------------------------------------------ open

#[test]
fn a_take_opens_in_the_piano_roll_and_the_edit_goes_to_the_take() {
    let mut c = two_groups();
    let id = group(&c, 1).takes[2].id;
    c.handle_action(UiAction::OpenPianoRoll(id));
    assert_eq!(c.piano_roll_clip(), Some(id));
    assert_eq!(c.piano_roll().notes().len(), 1);
    // An edit through the clip command the roll sends reaches the take and the roll follows it.
    let notes: atomik_model::NoteList =
        vec![Note::new(0, PPQ, 72, 0.5), Note::new(PPQ, PPQ, 74, 0.5)].into();
    c.apply_command(Command::Song(SongCommand::SetClipContent {
        id,
        content: ClipContent::Notes(notes),
        merge: None,
    }))
    .unwrap();
    assert_eq!(c.piano_roll_clip(), Some(id), "the roll stays open");
    assert_eq!(c.piano_roll().notes().len(), 2);
    assert_eq!(group(&c, 1).takes[2].id, id);
    // Deleting the take closes the roll.
    c.apply_command(Command::Song(SongCommand::DeleteTake { group: 1, take: 2 }))
        .unwrap();
    assert_eq!(c.piano_roll_clip(), None);
}

#[test]
fn open_take_picks_the_editor_by_the_content_and_ignores_unknown_ids() {
    let mut c = two_groups();
    let id = group(&c, 2).takes[0].id;
    c.open_take(id);
    assert_eq!(c.piano_roll_clip(), Some(id));
    c.open_take(ClipId(9999));
    assert_eq!(
        c.piano_roll_clip(),
        Some(id),
        "an unknown id changes nothing"
    );
}
