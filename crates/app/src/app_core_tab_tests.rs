//! Tab, score and chord sheet files in the app core: the import as tracks and as a song, one undo
//! step each, the exports, and the round trip of a score through the song and through files.

use super::*;
use crate::sink::RecordingSink;
use crate::tab_file::tests::fixture_score;
use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipId, DeviceKey, Note, empty_doc};
use atomik_tab::score::{SlideKind, TechFlags};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-tab-{tag}-{}-{n}", std::process::id()));
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

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

#[test]
fn a_score_imports_as_tracks_in_one_undo_step() {
    let mut c = core();
    let before = c.doc().clone();
    c.apply_tab(
        &fixture_score(),
        vec!["a thing was ignored".into()],
        false,
        "x.gp5",
    )
    .unwrap();
    let song = &c.doc().song;
    assert_eq!(song.tracks.len(), 3);
    // Tempo and meter stay as they were when the file is not imported as a song.
    assert_eq!(song.tempo, before.song.tempo);
    let msg = c.last_rejection().unwrap();
    assert!(msg.starts_with("Imported 3 tracks"), "{msg}");
    assert!(msg.contains("a thing was ignored"), "{msg}");
    let clip = &song.tracks[0].lanes[0].clips[0];
    assert_eq!(clip.midi.tab.as_ref().unwrap().tuning.len(), 6);
    assert!(c.undo());
    assert_eq!(c.doc(), &before, "one undo takes the whole import back");
}

#[test]
fn a_score_imports_as_a_song_with_tempo_meter_markers_and_chords() {
    let mut c = core();
    c.apply_tab(&fixture_score(), Vec::new(), true, "x.gp5")
        .unwrap();
    let song = &c.doc().song;
    assert_eq!(song.tempo.len(), 2);
    assert_eq!(song.tempo[1].tick, 8 * PPQ);
    assert_eq!(song.meter.len(), 2);
    assert_eq!(song.markers.len(), 2);
    assert_eq!(song.harmony.chords.len(), 2);
    assert!(song.harmony.key.is_some());
    assert!(c.undo());
    assert!(c.doc().song.markers.is_empty());
    assert!(c.doc().song.harmony.chords.is_empty());
}

#[test]
fn an_empty_or_unreadable_file_is_reported_by_name_and_changes_nothing() {
    let tmp = Tmp::new("bad");
    let mut c = core();
    let before = c.doc().clone();
    std::fs::write(tmp.path("junk.gp5"), b"definitely not a score").unwrap();
    c.import_tab(&tmp.path("junk.gp5"), false);
    assert!(c.last_rejection().unwrap().contains("junk.gp5"));
    c.import_tab(&tmp.path("missing.gpx"), true);
    assert!(c.last_rejection().unwrap().contains("missing.gpx"));
    let empty = atomik_tab::score::TabScore::new();
    assert!(c.apply_tab(&empty, Vec::new(), false, "e").is_err());
    assert_eq!(c.doc(), &before);
}

#[test]
fn loading_a_tab_file_from_the_browser_is_the_file_menu_import() {
    let tmp = Tmp::new("browser");
    let mut c = core();
    std::fs::write(tmp.path("song.gp4"), b"junk").unwrap();
    c.browse(atomik_ui::BrowseAction::Load(tmp.path("song.gp4")));
    // The file goes to the tab import (which reports it), not refused as "not a library item".
    let msg = c.last_rejection().unwrap();
    assert!(
        msg.contains("song.gp4") && !msg.contains("library item"),
        "{msg}"
    );
}

#[test]
fn text_tab_exports_the_selected_clip_or_the_song() {
    let tmp = Tmp::new("text");
    let mut c = core();
    assert!(!c.can_export_tab());
    c.apply_tab(&fixture_score(), Vec::new(), false, "x.gp5")
        .unwrap();
    assert!(c.can_export_tab());
    // The new clips are selected after an import: the scope is the clip until it is deselected.
    assert_eq!(c.tab_export_scope(), Some(MidiExport::Clip));
    c.arrangement_mut().set_selected_clips(&[]);
    assert_eq!(c.tab_export_scope(), Some(MidiExport::Song));
    c.export_tab(TabExportFormat::Text, &tmp.path("song.txt"));
    let text = std::fs::read_to_string(tmp.path("song.txt")).unwrap();
    assert!(text.contains('|'), "{text}");
    assert!(c.last_rejection().unwrap().starts_with("Exported the song"));
    // With a clip selected only that clip is written.
    let id = c.doc().song.tracks[1].lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    assert_eq!(c.tab_export_scope(), Some(MidiExport::Clip));
    c.export_tab(TabExportFormat::Text, &tmp.path("bass.txt"));
    let bass = std::fs::read_to_string(tmp.path("bass.txt")).unwrap();
    assert!(bass.len() < text.len(), "one track is shorter than three");
    assert!(c.last_rejection().unwrap().starts_with("Exported the clip"));
}

#[test]
fn a_chord_sheet_imports_and_exports() {
    let tmp = Tmp::new("sheet");
    let mut c = core();
    assert!(!c.can_export_sheet());
    c.export_sheet(&tmp.path("none.txt"));
    assert!(c.last_rejection().unwrap().contains("no chord sheet"));
    let text = "Am       G\nla la la la\n\nF       C\nde de de de\n";
    std::fs::write(tmp.path("tune.txt"), text).unwrap();
    c.import_sheet(&tmp.path("tune.txt"));
    assert_eq!(c.doc().song.sheets.len(), 1);
    assert_eq!(c.doc().song.sheets[0].title, "tune");
    assert!(
        c.last_rejection()
            .unwrap()
            .starts_with("Imported the chord sheet")
    );
    assert!(c.can_export_sheet());
    c.export_sheet(&tmp.path("out.txt"));
    let out = std::fs::read_to_string(tmp.path("out.txt")).unwrap();
    assert!(out.contains("Am") && out.contains("la la la la"), "{out}");
    // The bracket form is detected.
    std::fs::write(
        tmp.path("inline.cho"),
        "{title: Made Up}\n[C]Hello [G]there\n",
    )
    .unwrap();
    c.import_sheet(&tmp.path("inline.cho"));
    assert_eq!(c.doc().song.sheets.len(), 2);
    assert_eq!(c.doc().song.sheets[1].title, "Made Up");
    assert!(c.undo());
    assert_eq!(c.doc().song.sheets.len(), 1);
    c.import_sheet(&tmp.path("gone.txt"));
    assert!(c.last_rejection().unwrap().contains("gone.txt"));
}

#[test]
fn a_clip_without_tab_is_fretted_for_the_instrument_its_track_name_suggests() {
    let mut c = core();
    let key = DeviceKey("b".into());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_owned(),
        key: Some(key.clone()),
    })
    .unwrap();
    c.apply_command(Command::Rename {
        key: key.clone(),
        name: "Slap Bass".to_owned(),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key.clone(),
    }))
    .unwrap();
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * PPQ,
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
        controllers: Vec::new(),
        content: ClipContent::Notes(
            vec![Note::new(0, PPQ, 40, 0.8), Note::new(PPQ, PPQ, 43, 0.8)].into(),
        ),
    };
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key,
        lane: 0,
        clip,
    }))
    .unwrap();
    let (score, warnings) = crate::tab_file::score_from_song(
        &c.doc().song,
        &crate::tab_file::TabSource::Song,
        "T",
        &|k| c.device_name(k),
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let t = &score.tracks[0];
    assert_eq!(t.instrument_id, "bass4");
    assert_eq!(t.notes.len(), 2);
    assert_eq!(t.key_of(&t.notes[0]), Some(40));
    assert_eq!(t.key_of(&t.notes[1]), Some(43));
}

#[test]
fn a_score_survives_the_trip_through_the_song_and_back() {
    let mut c = core();
    let original = fixture_score();
    c.apply_tab(&original, Vec::new(), true, "x.gp5").unwrap();
    let (back, warnings) = crate::tab_file::score_from_song(
        &c.doc().song,
        &crate::tab_file::TabSource::Song,
        "T",
        &|k| c.device_name(k),
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(back.tracks.len(), 3);
    let g = &back.tracks[0];
    assert_eq!(g.instrument_id, "guitar6");
    assert_eq!(g.capo.fret, 2);
    assert_eq!(g.tuning, original.tracks[0].tuning);
    // Ties are one long note now; everything else keeps its tick, string, fret and techniques.
    let at = |t: i64| g.notes.iter().find(|n| n.tick == t).unwrap();
    assert_eq!((at(0).string, at(0).fret), (1, 3));
    assert_eq!(at(0).tech.bend, original.tracks[0].notes[0].tech.bend);
    assert!(at(PPQ).tech.has(TechFlags::HAMMER_ON));
    assert_eq!(at(2 * PPQ).tech.slide, Some(SlideKind::Legato));
    assert_eq!(at(3 * PPQ).len, 2 * PPQ);
    assert_eq!(g.lyrics.len(), 2);
    assert_eq!(back.tempo.len(), 2);
    assert_eq!(back.meter.len(), 2);
    assert_eq!(back.markers.len(), 2);
    assert_eq!(g.chords.len(), 2);
    // The drum track comes back as percussion with its keys.
    let d = back.tracks.iter().find(|t| t.is_percussion).unwrap();
    assert_eq!(d.notes.iter().map(|n| n.fret).collect::<Vec<_>>(), [36, 38]);
}

#[test]
fn musicxml_exported_from_the_song_imports_again_through_a_file() {
    let tmp = Tmp::new("xml");
    let mut c = core();
    c.apply_tab(&fixture_score(), Vec::new(), true, "x.gp5")
        .unwrap();
    for name in ["round.musicxml", "round.mxl"] {
        c.export_tab_of(MidiExport::Song, TabExportFormat::MusicXml, &tmp.path(name));
        let msg = c.last_rejection().unwrap();
        assert!(msg.starts_with("Exported the song as MusicXML"), "{msg}");
        let mut other = core();
        other.import_tab(&tmp.path(name), true);
        let msg = other.last_rejection().unwrap();
        assert!(msg.starts_with("Imported 3 tracks"), "{msg}");
        assert_eq!(other.doc().song.tracks.len(), 3);
        let tab = |core: &AppCore<RecordingSink>, i: usize| {
            core.doc().song.tracks[i].lanes[0].clips[0]
                .midi
                .tab
                .clone()
                .map(|t| (t.instrument_id, t.tuning, t.capo.fret, t.notes.len()))
        };
        assert_eq!(tab(&other, 0), tab(&c, 0), "{name}");
        assert_eq!(tab(&other, 1), tab(&c, 1), "{name}");
    }
}
