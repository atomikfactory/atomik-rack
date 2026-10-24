//! MIDI file import and export in the app core: what each command picks, the files it writes and
//! reads, one undo step per import, and the round trip of a whole song through a file.

use super::*;
use crate::midi_file::{clip_file, read_song, track_file};
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfEvent, SmfFile, SmfMessage, SmfTrack};
use atomik_model::song::{AutomationPoint, Curve, Marker, TempoEvent};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ControllerKind, ControllerLane, DeviceKey, Note,
    SongCommand, empty_doc,
};
use std::sync::atomic::{AtomicU64, Ordering};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-midi-{tag}-{}-{n}", std::process::id()));
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

fn add_synth(c: &mut AppCore<RecordingSink>, k: &str, name: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_owned(),
        key: Some(key(k)),
    })
    .unwrap();
    c.apply_command(Command::Rename {
        key: key(k),
        name: name.to_owned(),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
        .unwrap();
}

fn point(tick: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        tick,
        value,
        curve: Curve::Step,
    }
}

fn add_clip(c: &mut AppCore<RecordingSink>, k: &str, start: i64, notes: Vec<Note>) -> ClipId {
    let clip = Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![point(0, vel(64)), point(PPQ, 1.0)],
        )],
        content: ClipContent::Notes(notes),
    };
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key(k),
        lane: 0,
        clip,
    }))
    .unwrap();
    c.document.last_clip_id().unwrap()
}

fn vel(v: u8) -> f32 {
    f32::from(v) / 127.0
}

fn notes_in(c: &AppCore<RecordingSink>, id: ClipId) -> Vec<Note> {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Notes(n) => n.clone(),
        _ => panic!("not notes"),
    }
}

/// A song with two named tracks, a tempo change, a meter, a marker and loose notes.
fn song_core() -> AppCore<RecordingSink> {
    let mut c = core();
    add_synth(&mut c, "lead", "Lead");
    add_synth(&mut c, "bass", "Bass");
    add_clip(
        &mut c,
        "lead",
        0,
        vec![
            Note::new(0, PPQ, 60, vel(100)),
            Note::new(PPQ + 3, PPQ, 62, vel(90)),
        ],
    );
    add_clip(
        &mut c,
        "bass",
        BAR,
        vec![Note::new(7, 2 * PPQ, 36, vel(127))],
    );
    c.apply_command(Command::Song(SongCommand::SetTempoMap(vec![
        TempoEvent::step(0, 90.0),
        TempoEvent::step(2 * BAR, 140.0),
    ])))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetMeter {
        beats_per_bar: 3,
        beat_unit: 4,
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetMarkers(vec![Marker {
        id: 1,
        tick: BAR,
        name: "Verse".into(),
        color: None,
    }])))
    .unwrap();
    c
}

#[test]
fn export_needs_something_to_export() {
    let mut c = core();
    for what in [MidiExport::Clip, MidiExport::Track, MidiExport::Song] {
        assert!(!c.can_export_midi(what));
    }
    let tmp = Tmp::new("nothing");
    c.export_midi(MidiExport::Song, &tmp.path("x.mid"));
    assert!(!tmp.path("x.mid").exists());
    assert!(c.last_rejection().unwrap().contains("no note tracks"));
    c.export_midi(MidiExport::Clip, &tmp.path("x.mid"));
    assert!(c.last_rejection().unwrap().contains("select a note clip"));
    c.export_midi(MidiExport::Track, &tmp.path("x.mid"));
    assert!(c.last_rejection().unwrap().contains("select a note track"));
}

#[test]
fn export_picks_the_selected_clip_and_track() {
    let mut c = song_core();
    let lead = c.doc().song.tracks[0].lanes[0].clips[0].id;
    let bass = c.doc().song.tracks[1].lanes[0].clips[0].id;
    assert!(c.can_export_midi(MidiExport::Song));
    assert!(!c.can_export_midi(MidiExport::Clip));
    c.arrangement_mut().set_selected_clips(&[bass]);
    assert_eq!(c.midi_clip_source(), Some(bass));
    assert_eq!(c.midi_track_source(), Some(key("bass")));
    assert_eq!(c.midi_export_name(MidiExport::Clip), "Bass");
    assert_eq!(c.midi_export_name(MidiExport::Song), "Song");
    c.arrangement_mut().set_selected_clips(&[lead]);
    c.arrangement_mut().set_selected_track(Some(key("bass")));
    assert_eq!(
        c.midi_track_source(),
        Some(key("bass")),
        "the selected track first"
    );
    assert_eq!(c.midi_export_name(MidiExport::Track), "Bass");
}

#[test]
fn an_exported_song_imports_back_as_the_same_song_in_one_undo_step() {
    let tmp = Tmp::new("song");
    let source = song_core();
    let mut source = source;
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    assert!(
        source
            .last_rejection()
            .unwrap()
            .starts_with("Exported the song to song.mid")
    );

    let mut c = core();
    let before = c.doc().clone();
    c.import_midi(&tmp.path("song.mid"), true);
    let doc = c.doc();
    assert_eq!(doc.song.tempo, source.doc().song.tempo);
    assert_eq!(doc.song.meter, source.doc().song.meter);
    assert_eq!(doc.song.markers[0].name, "Verse");
    assert_eq!(doc.song.markers[0].tick, BAR);
    let names: Vec<_> = doc
        .song
        .tracks
        .iter()
        .map(|t| doc.device(&t.device).unwrap().name.clone())
        .collect();
    assert_eq!(names, ["Lead", "Bass"]);
    assert!(doc.song.tracks.iter().all(|t| !t.armed));
    for (new, old) in doc.song.tracks.iter().zip(&source.doc().song.tracks) {
        let (n, o) = (&new.lanes[0].clips[0], &old.lanes[0].clips[0]);
        let shifted: Vec<Note> = notes_in(&source, o.id)
            .iter()
            .map(|x| Note::new(x.tick + o.start, x.length, x.key, x.velocity))
            .collect();
        assert_eq!(notes_in(&c, n.id), shifted);
        assert_eq!(n.start, 0, "a song import starts at tick 0");
    }
    assert!(
        c.last_rejection()
            .unwrap()
            .starts_with("Imported 2 tracks (3 notes) from song.mid")
    );
    assert_eq!(
        c.arrangement.selected_clips().len(),
        2,
        "the new clips are selected"
    );
    assert!(doc.song.end >= BAR + 2 * PPQ);

    c.undo();
    assert_eq!(c.doc(), &before, "one undo step undoes the whole import");
}

#[test]
fn import_midi_without_a_selected_track_makes_tracks_at_the_playhead_and_leaves_the_tempo() {
    let tmp = Tmp::new("tracks");
    let mut source = song_core();
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    let mut c = core();
    c.locate_bars(2);
    let tempo = c.doc().song.tempo.clone();
    c.import_midi(&tmp.path("song.mid"), false);
    assert_eq!(
        c.doc().song.tempo,
        tempo,
        "tempo only comes with Import as Song"
    );
    assert!(c.doc().song.markers.is_empty());
    assert_eq!(c.doc().song.tracks.len(), 2);
    let start = c.doc().song.tracks[0].lanes[0].clips[0].start;
    assert_eq!(start, 2 * BAR);
}

#[test]
fn import_midi_with_a_selected_track_is_one_merged_clip_on_it() {
    let tmp = Tmp::new("clip");
    let mut source = song_core();
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    c.arrangement_mut().set_selected_track(Some(key("pad")));
    c.locate_bars(1);
    let tracks = c.doc().song.tracks.len();
    c.import_midi(&tmp.path("song.mid"), false);
    assert_eq!(c.doc().song.tracks.len(), tracks, "no new track");
    let clips = &c.doc().song.track(&key("pad")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].start, BAR);
    assert_eq!(
        notes_in(&c, clips[0].id).len(),
        3,
        "both tracks' notes are in it"
    );
    assert_eq!(c.arrangement.selected_clips(), vec![clips[0].id]);
    assert!(c.last_rejection().unwrap().contains("as one clip"));
    c.undo();
    assert!(
        c.doc().song.track(&key("pad")).unwrap().lanes[0]
            .clips
            .is_empty()
    );
}

#[test]
fn a_clip_exported_and_imported_on_its_track_keeps_notes_and_controllers() {
    let tmp = Tmp::new("roundtrip");
    let mut c = song_core();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    c.export_midi(MidiExport::Clip, &tmp.path("clip.mid"));
    c.arrangement_mut().set_selected_clips(&[]);
    c.arrangement_mut().set_selected_track(Some(key("bass")));
    c.import_midi(&tmp.path("clip.mid"), false);
    let clips = &c.doc().song.track(&key("bass")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 2);
    let new = clips.iter().max_by_key(|x| x.id).unwrap();
    let (_, _, old) = c.doc().song.clip(id).unwrap();
    assert_eq!(new.controllers, old.controllers);
    assert_eq!(new.length, old.length);
    assert_eq!(notes_in(&c, new.id), notes_in(&c, id));
}

#[test]
fn import_reports_bad_files_and_changes_nothing() {
    let tmp = Tmp::new("bad");
    let mut c = core();
    let before = c.doc().clone();
    std::fs::write(tmp.path("junk.mid"), b"this is not midi at all").unwrap();
    c.import_midi(&tmp.path("junk.mid"), true);
    assert!(
        c.last_rejection()
            .unwrap()
            .starts_with("junk.mid: not a MIDI file")
    );
    c.import_midi(&tmp.path("missing.mid"), false);
    assert!(c.last_rejection().unwrap().starts_with("missing.mid:"));
    // A valid file with a tempo and nothing to play.
    let only_tempo = SmfFile {
        format: 0,
        ppq: 96,
        tracks: vec![SmfTrack {
            events: vec![SmfEvent {
                tick: 0,
                message: SmfMessage::Tempo(500_000),
            }],
            end_tick: 0,
        }],
    };
    std::fs::write(tmp.path("tempo.mid"), only_tempo.to_bytes()).unwrap();
    c.import_midi(&tmp.path("tempo.mid"), true);
    assert!(c.last_rejection().unwrap().contains("no notes"));
    assert_eq!(c.doc(), &before);
}

#[test]
fn truncated_and_corrupted_files_never_panic_the_import() {
    let tmp = Tmp::new("fuzz");
    let mut source = song_core();
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    let good = std::fs::read(tmp.path("song.mid")).unwrap();
    let mut c = core();
    for cut in (0..good.len()).step_by(7) {
        let _ = read_song(&good[..cut]);
    }
    for i in (0..good.len()).step_by(5) {
        let mut bad = good.clone();
        bad[i] ^= 0xFF;
        std::fs::write(tmp.path("bad.mid"), &bad).unwrap();
        c.import_midi(&tmp.path("bad.mid"), i % 2 == 0);
    }
    // Whatever was imported, the document is still valid.
    let json = atomik_model::save_project(c.doc());
    atomik_model::load_project(&json, &default_registry()).unwrap();
}

#[test]
fn the_clip_file_of_the_core_matches_the_converter() {
    let tmp = Tmp::new("match");
    let mut c = song_core();
    let id = c.doc().song.tracks[1].lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    c.export_midi(MidiExport::Clip, &tmp.path("c.mid"));
    let (t, _, clip) = c.doc().song.clip(id).unwrap();
    let want = clip_file(&c.doc().song, clip, &c.device_name(&t.device));
    assert_eq!(std::fs::read(tmp.path("c.mid")).unwrap(), want);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    c.export_midi(MidiExport::Track, &tmp.path("t.mid"));
    let track = c.doc().song.track(&key("lead")).unwrap();
    assert_eq!(
        std::fs::read(tmp.path("t.mid")).unwrap(),
        track_file(&c.doc().song, track, "Lead")
    );
}
