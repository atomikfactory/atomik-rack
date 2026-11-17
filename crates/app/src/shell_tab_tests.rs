//! `Shell` tab file tests: the File menu entries, the dialog requests, the messages when there is
//! nothing to export, and tab files dropped on the window.

use super::*;
use crate::session::NullBackend;
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
        .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
        .find(|i| i.label == label)
        .unwrap_or_else(|| panic!("no menu item {label}"))
}

fn tmp_file(tag: &str, name: &str, bytes: &[u8]) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-tab-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn the_file_menu_has_the_tab_entries_and_asks_for_dialogs() {
    let mut s = shell();
    for label in [
        "Import Tab or Score File\u{2026}",
        "Import Tab or Score as Song\u{2026}",
        "Import Chord Sheet\u{2026}",
        "Export Tab as MusicXML\u{2026}",
        "Export Tab as Text\u{2026}",
        "Export Chord Sheet as Text\u{2026}",
    ] {
        assert!(
            s.menu_model()[0].items().any(|i| i.label == label),
            "{label}"
        );
    }
    assert!(item(&s, "Import Tab or Score File\u{2026}").enabled);
    assert!(item(&s, "Import Chord Sheet\u{2026}").enabled);
    assert!(!item(&s, "Export Tab as MusicXML\u{2026}").enabled);
    assert!(!item(&s, "Export Tab as Text\u{2026}").enabled);
    assert!(!item(&s, "Export Chord Sheet as Text\u{2026}").enabled);
    assert_eq!(
        s.execute(AppCommand::ImportTab),
        vec![ShellRequest::PickImportTab { as_song: false }]
    );
    assert_eq!(
        s.execute(AppCommand::ImportTabSong),
        vec![ShellRequest::PickImportTab { as_song: true }]
    );
    assert_eq!(
        s.execute(AppCommand::ImportSheet),
        vec![ShellRequest::PickImportSheet]
    );
}

#[test]
fn an_export_with_nothing_to_write_says_so_and_asks_for_no_dialog() {
    let mut s = shell();
    assert!(s.execute(AppCommand::ExportTabMusicXml).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no note tracks")
    );
    assert!(s.execute(AppCommand::ExportTabText).is_empty());
    assert!(s.execute(AppCommand::ExportSheetText).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no chord sheet")
    );
}

#[test]
fn commands_have_stable_ids_that_round_trip() {
    for (id, command) in [
        ("file.import_tab", AppCommand::ImportTab),
        ("file.import_tab_song", AppCommand::ImportTabSong),
        ("file.import_sheet", AppCommand::ImportSheet),
        ("file.export_tab_musicxml", AppCommand::ExportTabMusicXml),
        ("file.export_tab_text", AppCommand::ExportTabText),
        ("file.export_sheet_text", AppCommand::ExportSheetText),
    ] {
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
    }
}

#[test]
fn a_tab_file_dropped_on_the_window_is_imported_not_taken_for_audio() {
    let mut s = shell();
    let path = tmp_file("drop", "tune.gp5", b"not really a score");
    let song = s.layout().song.expect("the arrangement shows").center();
    s.handle(&UiEvent::FileDrop {
        pos: song,
        paths: vec![path.clone()],
    });
    // The tab import ran (and refused the bytes by name); no audio import was tried.
    let msg = s.core().last_rejection().unwrap();
    assert!(msg.contains("tune.gp5"), "{msg}");
    assert!(!msg.to_lowercase().contains("audio"), "{msg}");
    assert!(s.core().doc().song.tracks.is_empty());
    // An .xml that is not MusicXML is left to the audio import (which does not take it).
    let other = tmp_file("drop-xml", "plain.xml", b"<?xml version=\"1.0\"?><note/>");
    assert!(!crate::tab_file::is_tab_path(&other));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let _ = std::fs::remove_dir_all(other.parent().unwrap());
}

#[test]
fn a_musicxml_file_with_any_extension_but_xml_is_found_by_its_bytes_only_as_xml() {
    let path = tmp_file(
        "sniff",
        "score.xml",
        b"<?xml version=\"1.0\"?><score-partwise version=\"4.0\"><part-list/></score-partwise>",
    );
    // True once the MusicXML reader is there: the root element decides.
    assert_eq!(
        crate::tab_file::is_tab_path(&path),
        atomik_tab::musicxml::sniff(std::fs::read(&path).unwrap().as_slice()).is_some()
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn the_chord_sheet_pane_opens_from_the_view_menu_takes_the_text_context_and_closes() {
    let mut s = shell();
    assert!(item(&s, "Chord Sheet").enabled);
    assert!(!s.menu_state().chord_sheet);
    s.execute(AppCommand::ToggleChordSheet);
    assert!(s.core().chord_sheet_open());
    assert_eq!(s.lower_pane(), Pane::ChordSheet);
    assert!(s.menu_state().chord_sheet);
    // The pane binds no keys of its own: the global ones run while it is read.
    assert_eq!(s.context_stack().contexts(), [Context::Global]);
    // A text in the pane owns the keys: the context is Text, and Esc leaves it.
    let sheet = atomik_tab::sheet::parse_sheet("title: Made Up\n\nAm      F\nla la la la\n")
        .unwrap()
        .sheet;
    s.core_mut()
        .apply_command(atomik_model::Command::Song(
            atomik_model::SongCommand::SetSheets(vec![sheet]),
        ))
        .unwrap();
    s.run_command("editor.sheet_edit_text", None);
    assert!(s.core().chord_sheet().editing());
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Text, Context::Global]
    );
    s.handle(&UiEvent::Key {
        key: atomik_ui::Key::Escape,
        modifiers: atomik_ui::Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.core().chord_sheet().editing());
    assert_eq!(s.context_stack().contexts(), [Context::Global]);
    // The same command closes it and the arrangement shows again.
    s.execute(AppCommand::ToggleChordSheet);
    assert!(!s.core().chord_sheet_open());
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn the_chord_trainer_pane_opens_from_the_view_menu_and_its_scores_go_to_the_window_state() {
    let mut s = shell();
    assert!(item(&s, "Chord Trainer").enabled);
    assert!(!s.menu_state().chord_trainer);
    s.execute(AppCommand::ToggleChordTrainer);
    assert!(s.core().chord_trainer_open());
    assert_eq!(s.lower_pane(), Pane::ChordTrainer);
    assert!(s.menu_state().chord_trainer);
    assert_eq!(s.context_stack().contexts(), [Context::Global]);
    // A list being typed owns the keys: the context is Text, and Esc leaves it.
    s.run_command("editor.trainer_edit_list", None);
    assert!(s.core().chord_trainer().is_text_owner());
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Text, Context::Global]
    );
    s.handle(&UiEvent::Key {
        key: atomik_ui::Key::Escape,
        modifiers: atomik_ui::Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.core().chord_trainer().is_text_owner());
    // A verified drill on one pair, played right, leaves a score in the window state.
    s.core_mut().set_input_channels(2);
    s.core_mut()
        .chord_trainer_mut()
        .set_input(Some("In 1".to_owned()));
    for (id, arg) in [
        ("editor.trainer_mode", Some("verified")),
        ("editor.trainer_order", Some("pair")),
    ] {
        s.run_command(id, arg);
    }
    for _ in 0..12 {
        s.run_command("editor.trainer_bpm", Some("up"));
    }
    for _ in 0..2 {
        s.run_command("editor.trainer_beats", Some("down"));
    }
    s.run_command("editor.trainer_start", None);
    for k in 0..8 {
        let t = f64::from(k);
        let view = s.core_mut().chord_trainer_mut();
        view.tick(t + 0.05);
        let chord = view.drill().expect("running").current();
        let mask = atomik_tab::grade::pc_mask(&view.chords()[chord].pitch_classes());
        let mut chroma = [0.03f32; 12];
        for (pc, v) in chroma.iter_mut().enumerate() {
            if mask >> pc & 1 == 1 {
                *v = 1.0;
            }
        }
        view.hear(t + 0.1, chroma);
    }
    s.core_mut().chord_trainer_mut().tick(8.2);
    s.run_command("editor.trainer_start", None);
    assert!(s.ui.chord_trainer_scores.is_empty(), "saved by the frame");
    s.frame();
    assert_eq!(s.ui.chord_trainer_scores.len(), 1);
    let score = s.ui.chord_trainer_scores.values().next().unwrap();
    assert!(score.hit_rate > 0.9 && score.cpm > 50.0, "{score:?}");
    // The same command closes it and the arrangement shows again.
    s.execute(AppCommand::ToggleChordTrainer);
    assert!(!s.core().chord_trainer_open());
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}
