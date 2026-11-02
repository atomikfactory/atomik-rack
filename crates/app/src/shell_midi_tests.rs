//! `Shell` MIDI file tests: the File menu entries, the dialog requests, the message when there is
//! nothing to export and MIDI files dropped on the window.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfEvent, SmfFile, SmfMessage, SmfTrack};
use std::sync::atomic::{AtomicU64, Ordering};

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn item(s: &Shell<NullBackend>, label: &str) -> crate::commands::MenuItem {
    s.menu_model()
        .into_iter()
        .flat_map(|m| m.entries)
        .find_map(|e| match e {
            MenuEntry::Item(i) if i.label == label => Some(i),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no menu item {label}"))
}

/// A one-track type 0 file with two notes, a tempo and a marker, written to a temp folder.
fn midi_file(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-midi-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let on = |tick, key, velocity| SmfEvent {
        tick,
        message: SmfMessage::NoteOn {
            channel: 0,
            key,
            velocity,
        },
    };
    let file = SmfFile {
        format: 0,
        ppq: 480,
        tracks: vec![SmfTrack {
            events: vec![
                SmfEvent {
                    tick: 0,
                    message: SmfMessage::Tempo(400_000),
                },
                on(0, 60, 100),
                on(480, 60, 0),
                on(480, 62, 90),
                on(960, 62, 0),
            ],
            end_tick: 960,
        }],
    };
    let path = dir.join("tune.MID");
    std::fs::write(&path, file.to_bytes()).unwrap();
    path
}

#[test]
fn the_file_menu_has_the_midi_entries_and_asks_for_dialogs() {
    let mut s = shell();
    for label in [
        "Import MIDI\u{2026}",
        "Import MIDI as Song\u{2026}",
        "Export MIDI Clip\u{2026}",
        "Export MIDI Track\u{2026}",
        "Export MIDI Song\u{2026}",
    ] {
        assert!(
            s.menu_model()[0].items().any(|i| i.label == label),
            "{label}"
        );
    }
    assert!(item(&s, "Import MIDI\u{2026}").enabled);
    assert!(
        !item(&s, "Export MIDI Clip\u{2026}").enabled,
        "nothing to export yet"
    );
    assert!(!item(&s, "Export MIDI Track\u{2026}").enabled);
    assert!(!item(&s, "Export MIDI Song\u{2026}").enabled);
    assert_eq!(
        s.execute(AppCommand::ImportMidi),
        vec![ShellRequest::PickImportMidi { as_song: false }]
    );
    assert_eq!(
        s.execute(AppCommand::ImportMidiSong),
        vec![ShellRequest::PickImportMidi { as_song: true }]
    );
}

#[test]
fn an_export_with_nothing_to_write_says_what_to_select() {
    let mut s = shell();
    assert!(s.execute(AppCommand::ExportMidiSong).is_empty());
    assert_eq!(
        s.core().last_rejection(),
        Some("the song has no note tracks to export")
    );
    assert!(s.execute(AppCommand::ExportMidiClip).is_empty());
    assert_eq!(
        s.core().last_rejection(),
        Some("select a note clip to export")
    );
}

#[test]
fn importing_then_exporting_goes_through_the_requests() {
    let mut s = shell();
    let path = midi_file("flow");
    s.import_midi(&path, true);
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert!((s.core().doc().tempo_bpm - 150.0).abs() < 1e-6);
    assert!(item(&s, "Export MIDI Song\u{2026}").enabled);
    assert_eq!(
        s.execute(AppCommand::ExportMidiSong),
        vec![ShellRequest::PickExportMidi(MidiExport::Song)]
    );
    let out = path.with_file_name("out.mid");
    s.export_midi_to(MidiExport::Song, &out);
    let again = crate::midi_file::read_song(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(again.tracks[0].notes.len(), 2);
    assert_eq!(again.tracks[0].notes[1].tick, PPQ);
    assert_eq!(s.midi_export_name(MidiExport::Song), "Song");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_midi_file_dropped_on_the_window_imports_as_tracks() {
    let mut s = shell();
    let path = midi_file("drop");
    let song = s.layout().song.expect("the arrangement shows").center();
    s.handle(&UiEvent::FileHover {
        pos: song,
        paths: vec![path.clone()],
    });
    s.handle(&UiEvent::FileDrop {
        pos: song,
        paths: vec![path.clone()],
    });
    assert!(
        s.core().arrangement().drop_hover().is_none(),
        "the hover is cleared"
    );
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert_eq!(s.core().doc().song.tracks[0].lanes[0].clips.len(), 1);
    // Dropped over the rack it does the same.
    let rack = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: rack,
        paths: vec![path.clone()],
    });
    assert_eq!(s.core().doc().song.tracks.len(), 2);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

fn add_note_track(s: &mut Shell<NullBackend>, name: &str) -> atomik_model::DeviceKey {
    let key = atomik_model::DeviceKey(name.to_owned());
    let core = s.core_mut();
    core.apply_command(atomik_model::Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_owned(),
        key: Some(key.clone()),
    })
    .unwrap();
    core.apply_command(atomik_model::Command::Song(
        atomik_model::SongCommand::AddTrack {
            device: key.clone(),
        },
    ))
    .unwrap();
    key
}

#[test]
fn a_midi_file_dropped_on_a_note_lane_becomes_a_clip_there_and_hovers_with_the_lane_marker() {
    let mut s = shell();
    let path = midi_file("drop-lane");
    let pad = add_note_track(&mut s, "pad");
    let other = add_note_track(&mut s, "other");
    let lane = s
        .core()
        .arrangement()
        .lane_rect(&pad, 0)
        .expect("the lane is shown");
    let at = atomik_ui::kurbo::Point::new(lane.x0 + 300.0, lane.center().y);
    s.handle(&UiEvent::FileHover {
        pos: at,
        paths: vec![path.clone()],
    });
    assert!(s.core().arrangement().clip_drop_hover().is_some());
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![path.clone()],
    });
    assert!(s.core().arrangement().clip_drop_hover().is_none());
    let song = &s.core().doc().song;
    assert_eq!(
        song.tracks.len(),
        2,
        "no new track: the clip goes on the lane"
    );
    let clips = &song.track(&pad).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert!(clips[0].start > 0, "at the drop position, not the playhead");
    assert!(song.track(&other).unwrap().lanes[0].clips.is_empty());
    assert!(s.core_mut().undo());
    assert!(
        s.core().doc().song.track(&pad).unwrap().lanes[0]
            .clips
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_midi_file_dropped_below_the_tracks_makes_new_tracks_at_the_drop_position() {
    let mut s = shell();
    let path = midi_file("drop-below");
    let pad = add_note_track(&mut s, "pad");
    let lane = s.core().arrangement().lane_rect(&pad, 0).unwrap();
    // Well below the last track row, inside the arrangement.
    let below = s.layout().song.expect("the arrangement shows");
    let at = atomik_ui::kurbo::Point::new(lane.x0 + 300.0, below.y1 - 6.0);
    assert!(at.y > lane.y1);
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![path.clone()],
    });
    let song = &s.core().doc().song;
    assert_eq!(song.tracks.len(), 2, "one new track made from the file");
    assert!(song.track(&pad).unwrap().lanes[0].clips.is_empty());
    assert!(song.tracks[1].lanes[0].clips[0].start > 0);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
