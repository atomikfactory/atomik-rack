//! MIDI file import and export in the app core: what each command picks, the files it writes and
//! reads, one undo step per import, and the round trip of a whole song through a file.

use super::*;
use crate::midi_file::{clip_file, read_song, track_file};
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfEvent, SmfFile, SmfMessage, SmfTrack};
use atomik_model::song::{AutomationPoint, Curve, Marker, TempoEvent};
use atomik_model::theory::{ChordKind, ScaleKind};
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
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
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
        name: None,
        color: None,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![point(0, vel(64)), point(PPQ, 1.0)],
        )],
        content: ClipContent::Notes(notes.into()),
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
        ClipContent::Notes(n) => n.to_vec(),
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
    c.apply_command(Command::Song(SongCommand::SetRanges(vec![
        atomik_model::song::RangeMarker {
            id: 1,
            start: BAR,
            end: 2 * BAR,
            name: "Chorus".into(),
            color: None,
        },
    ])))
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
    assert_eq!(doc.song.markers.len(), 1, "the range is not also a marker");
    let ranges: Vec<_> = doc
        .song
        .ranges
        .iter()
        .map(|r| (r.start, r.end, r.name.as_str()))
        .collect();
    assert_eq!(
        ranges,
        [(BAR, 2 * BAR, "Chorus")],
        "range markers come back as ranges"
    );
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
    c.sink_mut().take();
    c.import_midi(&tmp.path("song.mid"), false);
    crate::sink::assert_new_devices_routed(&c.sink_mut().take());
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
    let want = clip_file(
        &c.doc().song,
        clip,
        &c.device_name(&t.device),
        crate::midi_file::MidiResolution::Native,
    );
    assert_eq!(std::fs::read(tmp.path("c.mid")).unwrap(), want);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    c.export_midi(MidiExport::Track, &tmp.path("t.mid"));
    let track = c.doc().song.track(&key("lead")).unwrap();
    assert_eq!(
        std::fs::read(tmp.path("t.mid")).unwrap(),
        track_file(
            &c.doc().song,
            track,
            "Lead",
            crate::midi_file::MidiResolution::Native
        )
    );
}

#[test]
fn import_midi_into_a_track_goes_on_its_selected_note_lane() {
    let tmp = Tmp::new("lane");
    let mut source = song_core();
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    c.apply_command(Command::Song(SongCommand::AddNoteLane {
        device: key("pad"),
        name: None,
    }))
    .unwrap();
    c.arrangement_mut().set_selected_track(Some(key("pad")));
    c.arrangement_mut().set_selected_lane(&key("pad"), 1);
    c.import_midi(&tmp.path("song.mid"), false);
    let pad = c.doc().song.track(&key("pad")).unwrap();
    assert!(pad.lanes[0].clips.is_empty());
    assert_eq!(pad.lanes[1].clips.len(), 1);
    assert_eq!(
        c.arrangement.selected_clips(),
        vec![pad.lanes[1].clips[0].id]
    );
}

#[test]
fn an_exported_track_merges_its_note_lanes_and_skips_a_muted_one() {
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    add_clip(&mut c, "pad", 0, vec![Note::new(0, PPQ, 60, vel(100))]);
    c.apply_command(Command::Song(SongCommand::AddNoteLane {
        device: key("pad"),
        name: None,
    }))
    .unwrap();
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Notes(vec![Note::new(PPQ, PPQ, 64, vel(100))].into()),
    };
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("pad"),
        lane: 1,
        clip,
    }))
    .unwrap();
    let keys = |c: &AppCore<RecordingSink>| {
        let song = &c.doc().song;
        let track = song.track(&key("pad")).unwrap();
        let midi = read_song(&track_file(
            song,
            track,
            "Pad",
            crate::midi_file::MidiResolution::Native,
        ))
        .unwrap();
        let mut keys: Vec<u8> = midi.merged().notes.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        keys
    };
    assert_eq!(keys(&c), vec![60, 64], "both lanes play");
    c.apply_command(Command::Song(SongCommand::SetLaneMuted {
        device: key("pad"),
        lane: 1,
        muted: true,
    }))
    .unwrap();
    assert_eq!(keys(&c), vec![60], "the muted lane is left out");
}

fn song_file(tmp: &Tmp) -> PathBuf {
    let mut source = song_core();
    source.export_midi(MidiExport::Song, &tmp.path("song.mid"));
    tmp.path("song.mid")
}

#[test]
fn a_midi_file_dropped_on_a_note_lane_is_one_clip_there_at_the_drop_tick() {
    let tmp = Tmp::new("drop-lane");
    let file = song_file(&tmp);
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    c.apply_command(Command::Song(SongCommand::AddNoteLane {
        device: key("pad"),
        name: None,
    }))
    .unwrap();
    // The playhead and the selection are elsewhere: only the drop counts.
    c.locate_bars(5);
    c.arrangement_mut().set_selected_track(None);
    let before = c.doc().clone();
    c.handle_action(UiAction::ImportMidi {
        paths: vec![file],
        lane: Some((key("pad"), 1)),
        tick: 2 * BAR,
    });
    let pad = c.doc().song.track(&key("pad")).unwrap();
    assert!(pad.lanes[0].clips.is_empty());
    assert_eq!(pad.lanes[1].clips.len(), 1);
    assert_eq!(pad.lanes[1].clips[0].start, 2 * BAR);
    assert_eq!(c.doc().song.tracks.len(), 1, "no new track");
    assert_eq!(
        notes_in(&c, pad.lanes[1].clips[0].id).len(),
        3,
        "both MIDI tracks merge into the one clip, as File > Import MIDI does"
    );
    assert_eq!(
        c.arrangement.selected_clips(),
        vec![pad.lanes[1].clips[0].id]
    );
    c.undo();
    assert_eq!(c.doc(), &before, "one undo step");
}

#[test]
fn a_midi_file_dropped_below_the_tracks_makes_new_tracks_at_the_drop_tick() {
    let tmp = Tmp::new("drop-new");
    let file = song_file(&tmp);
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    // A selected track does not matter: new tracks, not a clip on it.
    c.arrangement_mut().set_selected_track(Some(key("pad")));
    c.locate_bars(7);
    c.sink_mut().take();
    c.handle_action(UiAction::ImportMidi {
        paths: vec![file],
        lane: None,
        tick: BAR,
    });
    crate::sink::assert_new_devices_routed(&c.sink_mut().take());
    let song = &c.doc().song;
    assert_eq!(song.tracks.len(), 3, "the pad and two imported tracks");
    assert!(song.track(&key("pad")).unwrap().lanes[0].clips.is_empty());
    for t in &song.tracks[1..] {
        assert_eq!(t.lanes[0].clips[0].start, BAR);
    }
}

#[test]
fn a_midi_drop_on_a_lane_that_is_gone_or_not_notes_changes_nothing_and_says_so() {
    let tmp = Tmp::new("drop-bad");
    let file = song_file(&tmp);
    let mut c = core();
    add_synth(&mut c, "pad", "Pad");
    let before = c.doc().clone();
    for lane in [Some((key("pad"), 5)), Some((key("ghost"), 0))] {
        c.handle_action(UiAction::ImportMidi {
            paths: vec![file.clone()],
            lane,
            tick: 0,
        });
        assert_eq!(c.doc(), &before);
        assert!(c.last_rejection().unwrap().contains("note lane"));
    }
    // An unreadable file is reported by name and also changes nothing.
    c.import_midi_at(&tmp.path("missing.mid"), None, 0);
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().unwrap().contains("missing.mid"));
}

#[test]
fn loading_a_midi_file_from_the_browser_is_the_file_menu_import() {
    let tmp = Tmp::new("browser");
    let file = song_file(&tmp);
    let (mut browser, mut menu) = (core(), core());
    for c in [&mut browser, &mut menu] {
        add_synth(c, "pad", "Pad");
        c.arrangement_mut().set_selected_track(Some(key("pad")));
        c.locate_bars(1);
    }
    browser.browse(atomik_ui::BrowseAction::Load(file.clone()));
    menu.import_midi(&file, false);
    assert_eq!(browser.doc(), menu.doc());
    let clips = &browser.doc().song.track(&key("pad")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].start, BAR);
    // A broken file is reported like the menu does.
    std::fs::write(tmp.path("junk.mid"), b"nope").unwrap();
    browser.browse(atomik_ui::BrowseAction::Load(tmp.path("junk.mid")));
    assert!(browser.last_rejection().unwrap().contains("junk.mid"));
}

#[test]
fn the_export_resolution_is_the_cores_setting_and_the_native_one_by_default() {
    let tmp = Tmp::new("resolution");
    let division = |name: &str| {
        let bytes = std::fs::read(tmp.path(name)).unwrap();
        u16::from_be_bytes([bytes[12], bytes[13]])
    };
    let mut c = song_core();
    assert_eq!(
        c.midi_resolution(),
        crate::midi_file::MidiResolution::Native
    );
    c.export_midi(MidiExport::Song, &tmp.path("native.mid"));
    assert_eq!(division("native.mid"), 15_360);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    for (res, want) in [
        (crate::midi_file::MidiResolution::Ppq960, 960),
        (crate::midi_file::MidiResolution::Ppq480, 480),
    ] {
        c.set_midi_resolution(res);
        assert_eq!(c.midi_resolution(), res);
        c.export_midi(MidiExport::Song, &tmp.path("song.mid"));
        assert_eq!(division("song.mid"), want);
        c.export_midi(MidiExport::Track, &tmp.path("track.mid"));
        assert_eq!(division("track.mid"), want);
    }
}

#[test]
fn program_changes_and_sysex_travel_through_a_song_file_and_back_into_clips() {
    let tmp = Tmp::new("program");
    let mut source = core();
    add_synth(&mut source, "pad", "Pad");
    let id = add_clip(&mut source, "pad", 0, vec![Note::new(0, PPQ, 60, vel(100))]);
    let mut clip = source.doc().song.clip(id).unwrap().2.clone();
    clip.midi = atomik_model::MidiData {
        programs: vec![atomik_model::ProgramChange {
            bank_msb: Some(2),
            bank_lsb: Some(1),
            ..atomik_model::ProgramChange::new(0, 11)
        }],
        sysex: vec![atomik_model::SysexEvent {
            tick: PPQ,
            data: vec![0x41, 0x10, 0x42],
        }],
        ..Default::default()
    }
    .into();
    source
        .apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    source
        .apply_command(Command::Song(SongCommand::AddClip {
            device: key("pad"),
            lane: 0,
            clip: clip.clone(),
        }))
        .unwrap();
    source.set_midi_resolution(crate::midi_file::MidiResolution::Ppq960);
    source.export_midi(MidiExport::Song, &tmp.path("data.mid"));

    let mut target = core();
    target.import_midi(&tmp.path("data.mid"), true);
    let track = &target.doc().song.tracks[0];
    assert_eq!(track.lanes[0].clips[0].midi, clip.midi);
    // The import is one undo step, data included.
    assert!(target.undo());
    assert!(target.doc().song.tracks.is_empty());
}

// ----- groove from a MIDI file ---------------------------------------------------------------

const STEP: i64 = PPQ / 4;

/// A MIDI file of one bar of 1/16 notes whose odd steps are `late` ticks late, the odd ones a
/// little softer; written by the clip export of a scratch core.
fn groove_file(tmp: &Tmp, name: &str, late: i64) -> PathBuf {
    let mut c = core();
    add_synth(&mut c, "lead", "Lead");
    let notes = (0..8)
        .map(|i| {
            let (delay, velocity) = if i % 2 == 1 { (late, 60) } else { (0, 100) };
            Note::new(i * STEP + delay, STEP / 2, 60, vel(velocity))
        })
        .collect();
    let id = add_clip(&mut c, "lead", 0, notes);
    let (t, _, clip) = c.doc().song.clip(id).unwrap();
    let bytes = clip_file(
        &c.doc().song,
        clip,
        &c.device_name(&t.device),
        crate::midi_file::MidiResolution::Native,
    );
    let path = tmp.path(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_groove_from_a_midi_file_measures_its_onsets_and_adds_it_to_the_song() {
    let tmp = Tmp::new("groove");
    let file = groove_file(&tmp, "Late seconds.mid", STEP / 4);
    let mut c = core();
    let name = c
        .groove_from_midi(&file, StepResolution::Sixteenth, 2)
        .expect("a groove");
    assert_eq!(name, "Late seconds");
    let groove = c.doc().song.groove("Late seconds").unwrap();
    assert_eq!(groove.step, StepResolution::Sixteenth);
    assert_eq!(groove.slots.len(), 2);
    // The odd step is a quarter of a step late and softer than the average of all the notes.
    assert!((groove.slots[0].offset).abs() < 1e-6);
    assert!((groove.slots[1].offset - 0.25).abs() < 1e-3);
    assert!(groove.slots[0].velocity > 1.0 && groove.slots[1].velocity < 1.0);
    assert!(c.last_rejection().unwrap().contains("Late seconds.mid"));
    // No clip was added and one undo takes the groove away.
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.undo());
    assert!(c.doc().song.groove("Late seconds").is_none());
}

#[test]
fn a_taken_groove_name_gets_a_number_and_a_long_one_is_cut() {
    let tmp = Tmp::new("groove-names");
    let a = groove_file(&tmp, "beat.mid", STEP / 4);
    let mut c = core();
    for want in ["beat", "beat 2", "beat 3"] {
        let name = c
            .groove_from_midi(&a, StepResolution::Sixteenth, 2)
            .unwrap();
        assert_eq!(name, want);
    }
    // A built-in name is taken too.
    let straight = groove_file(&tmp, "Straight.mid", 0);
    let name = c
        .groove_from_midi(&straight, StepResolution::Eighth, 2)
        .unwrap();
    assert_eq!(name, "Straight 2");
    let long = groove_file(
        &tmp,
        "a very long file name that goes on and on for ever.mid",
        0,
    );
    let name = c
        .groove_from_midi(&long, StepResolution::Sixteenth, 4)
        .unwrap();
    // Cut to 32 characters and trimmed (the cut falls after a space here).
    assert!((30..=32).contains(&name.chars().count()), "{name}");
    assert!(name.starts_with("a very long file name"));
    assert_eq!(c.doc().song.groove(&name).unwrap().slots.len(), 4);
}

#[test]
fn a_midi_file_that_cannot_make_a_groove_is_reported_and_changes_nothing() {
    let tmp = Tmp::new("groove-bad");
    let mut c = core();
    let before = c.doc().clone();
    std::fs::write(tmp.path("junk.mid"), b"not midi").unwrap();
    assert!(
        c.groove_from_midi(&tmp.path("junk.mid"), StepResolution::Sixteenth, 2)
            .is_none()
    );
    assert!(c.last_rejection().unwrap().starts_with("junk.mid:"));
    assert!(
        c.groove_from_midi(&tmp.path("missing.mid"), StepResolution::Sixteenth, 2)
            .is_none()
    );
    // A file with a tempo and no notes has nothing to measure.
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
    assert!(
        c.groove_from_midi(&tmp.path("tempo.mid"), StepResolution::Sixteenth, 2)
            .is_none()
    );
    assert!(c.last_rejection().unwrap().starts_with("tempo.mid:"));
    // Too many slots are refused by the groove, not applied.
    let good = groove_file(&tmp, "good.mid", 0);
    assert!(
        c.groove_from_midi(&good, StepResolution::Sixteenth, 99)
            .is_none()
    );
    assert_eq!(c.doc(), &before);
}

#[test]
fn a_channel_of_controllers_alone_imports_to_the_track_lanes_in_one_undo_step() {
    let tmp = Tmp::new("own-lanes");
    let bend = |tick, value| SmfEvent {
        tick,
        message: SmfMessage::PitchBend { channel: 5, value },
    };
    let file = SmfFile {
        format: 1,
        ppq: 96,
        tracks: vec![SmfTrack {
            events: vec![
                SmfEvent {
                    tick: 0,
                    message: SmfMessage::NoteOn {
                        channel: 0,
                        key: 60,
                        velocity: 90,
                    },
                },
                bend(48, 12288),
                SmfEvent {
                    tick: 96,
                    message: SmfMessage::NoteOff {
                        channel: 0,
                        key: 60,
                        velocity: 0,
                    },
                },
            ],
            end_tick: 96,
        }],
    };
    let path = tmp.path("bend.mid");
    std::fs::write(&path, file.to_bytes()).unwrap();
    let mut c = core();
    c.locate_bars(2);
    let before = c.doc().clone();
    c.import_midi(&path, false);
    let t = c.doc().song.tracks.last().unwrap();
    assert_eq!(t.lanes[0].clips.len(), 1);
    assert_eq!(t.controllers.len(), 1);
    assert_eq!(
        t.controllers[0].points[0].tick,
        2 * BAR + PPQ / 2,
        "in song ticks, from the import start"
    );
    assert!(c.last_rejection().unwrap().contains("Imported 1 track"));
    c.undo();
    assert_eq!(c.doc(), &before, "one undo step");
}

/// A type 0 file at 480 ticks per quarter: a key signature, chord symbols as a text event, a
/// marker and a lyric, and a bar of C major then a bar of A minor held as block chords.
fn chord_file() -> SmfFile {
    let ev = |tick: u64, message: SmfMessage| SmfEvent { tick, message };
    let on = |tick: u64, key: u8| {
        [
            ev(
                tick,
                SmfMessage::NoteOn {
                    channel: 0,
                    key,
                    velocity: 100,
                },
            ),
            ev(
                tick + 1920,
                SmfMessage::NoteOff {
                    channel: 0,
                    key,
                    velocity: 0,
                },
            ),
        ]
    };
    let mut events = vec![
        ev(
            0,
            SmfMessage::KeySignature {
                sharps: -1,
                minor: true,
            },
        ),
        ev(0, SmfMessage::Text("C".into())),
        ev(0, SmfMessage::Marker("Intro".into())),
        ev(1920, SmfMessage::Marker("Am7".into())),
        ev(3840, SmfMessage::Lyric("F/A".into())),
    ];
    for (bar, keys) in [[48u8, 52, 55], [45, 48, 52]].iter().enumerate() {
        for k in keys {
            events.extend(on(bar as u64 * 1920, *k));
        }
    }
    events.sort_by_key(|e| e.tick);
    SmfFile {
        format: 0,
        ppq: 480,
        tracks: vec![SmfTrack {
            events,
            end_tick: 4800,
        }],
    }
}

#[test]
fn a_midi_import_reads_chord_symbols_and_the_key_signature_in_one_undo_step() {
    let tmp = Tmp::new("chords");
    std::fs::write(tmp.path("chords.mid"), chord_file().to_bytes()).unwrap();
    let mut c = core();
    c.import_midi(&tmp.path("chords.mid"), true);
    let h = c.doc().song.harmony.clone();
    // One flat, minor: D minor.
    assert_eq!(
        h.key,
        Some(atomik_model::Key::new(2, ScaleKind::NaturalMinor))
    );
    let chords: Vec<String> = h.chords.iter().map(|ch| ch.symbol()).collect();
    assert_eq!(chords, ["C", "Am7", "F/A"]);
    assert_eq!(
        h.chords
            .iter()
            .map(|ch| (ch.tick, ch.end()))
            .collect::<Vec<_>>(),
        [(0, BAR), (BAR, 2 * BAR), (2 * BAR, 2 * BAR + 2 * PPQ)]
    );
    // The marker that is not a chord stays a marker.
    let markers: Vec<&str> = c
        .doc()
        .song
        .markers
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(markers, ["Intro"]);
    c.undo();
    assert!(c.doc().song.harmony.is_empty(), "one undo step");
    assert!(c.doc().song.tracks.is_empty());
}

#[test]
fn a_midi_import_keeps_a_song_key_that_is_already_set() {
    let tmp = Tmp::new("keep-key");
    std::fs::write(tmp.path("chords.mid"), chord_file().to_bytes()).unwrap();
    let mut c = core();
    let g = atomik_model::Key::new(7, ScaleKind::Major);
    c.set_song_key(Some(g)).unwrap();
    c.import_midi(&tmp.path("chords.mid"), true);
    assert_eq!(c.doc().song.harmony.key, Some(g));
    assert_eq!(c.doc().song.harmony.chords.len(), 3);
}

#[test]
fn a_midi_import_detects_chords_from_the_notes_when_asked() {
    let mut file = chord_file();
    file.tracks[0].events.retain(|e| {
        !matches!(
            e.message,
            SmfMessage::Text(_) | SmfMessage::Lyric(_) | SmfMessage::Marker(_)
        )
    });
    let tmp = Tmp::new("detect");
    std::fs::write(tmp.path("plain.mid"), file.to_bytes()).unwrap();
    let mut c = core();
    c.import_midi(&tmp.path("plain.mid"), true);
    assert!(c.doc().song.harmony.chords.is_empty(), "off by default");
    let mut c = core();
    c.set_midi_chords_from_notes(true);
    c.import_midi(&tmp.path("plain.mid"), true);
    let roots: Vec<(u8, ChordKind)> = c
        .doc()
        .song
        .harmony
        .chords
        .iter()
        .map(|ch| (ch.root, ch.quality))
        .collect();
    assert_eq!(roots, [(0, ChordKind::Major), (9, ChordKind::Minor)]);
    c.undo();
    assert!(c.doc().song.harmony.chords.is_empty(), "one undo step");
}
