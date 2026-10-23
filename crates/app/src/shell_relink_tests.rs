//! `Shell` relink tests: File > Relink Missing Audio, the panel, LOCATE... and SEARCH FOLDER...
//! as platform requests, and the panel closing once nothing is missing.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_ui::ImportTarget;
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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-shell-relink-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wav(dir: &Path, name: &str, seed: f32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let data: Vec<f32> = (0..4_800).map(|i| (i as f32 / 4_800.0) * seed).collect();
    let path = dir.join(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

/// A saved project with one clip of `kick.wav`, its asset file deleted; the project file.
fn missing_project(t: &Tmp) -> PathBuf {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.import_audio(
        vec![wav(&t.0.join("source"), "kick.wav", 1.0)],
        ImportTarget::NewTracks,
        0,
    );
    c.wait_background();
    let hash = c.doc().assets.keys().next().unwrap().clone();
    c.save_as(&t.0.join("song.atomik")).unwrap();
    std::fs::remove_file(t.0.join("song").join("Audio").join(format!("{hash}.wav"))).unwrap();
    t.0.join("song").join("song.atomik")
}

fn opened(t: &Tmp) -> Shell<NullBackend> {
    let mut s = shell();
    s.open_file(&missing_project(t));
    s.core_mut().wait_background();
    s.frame();
    s
}

fn relink_item(s: &Shell<NullBackend>) -> crate::commands::MenuItem {
    s.menu_model()[0]
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Item(i) if i.command == AppCommand::RelinkAudio => Some(i.clone()),
            _ => None,
        })
        .expect("File > Relink Missing Audio")
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

#[test]
fn the_menu_item_is_in_the_file_menu_and_enabled_only_while_audio_is_missing() {
    let s = shell();
    let item = relink_item(&s);
    assert_eq!(item.label, "Relink Missing Audio\u{2026}");
    assert!(!item.enabled, "nothing is missing in a new project");
    assert_eq!(item.command.command_id(), Some(("file.relink_audio", None)));

    let t = Tmp::new();
    let s = opened(&t);
    assert!(relink_item(&s).enabled);
}

#[test]
fn the_command_opens_a_list_of_the_missing_files() {
    let t = Tmp::new();
    let mut s = opened(&t);
    assert!(!s.relink_open());
    assert!(s.execute(AppCommand::RelinkAudio).is_empty());
    assert!(s.relink_open());
    let entries = s.relink_view().unwrap().entries().to_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "kick.wav");
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    let texts = dl.texts();
    assert!(texts.contains(&"RELINK MISSING AUDIO"), "{texts:?}");
    assert!(texts.contains(&"kick.wav"));
    assert!(texts.contains(&"LOCATE..."));
    assert!(texts.contains(&"SEARCH FOLDER..."));
}

#[test]
fn with_nothing_missing_the_command_only_says_so() {
    let mut s = shell();
    assert!(s.execute(AppCommand::RelinkAudio).is_empty());
    assert!(!s.relink_open());
    assert_eq!(s.core().last_rejection(), Some("no missing audio files"));
}

#[test]
fn locate_asks_for_a_file_and_the_picked_file_relinks_the_asset() {
    let t = Tmp::new();
    let mut s = opened(&t);
    s.execute(AppCommand::RelinkAudio);
    let at = s.relink_view().unwrap().locate_rect(0).unwrap().center();
    assert_eq!(click(&mut s, at), vec![ShellRequest::PickRelinkFile]);
    s.relink_file(wav(&t.0.join("new"), "snare.wav", 0.5));
    s.core_mut().wait_background();
    s.frame();
    assert!(s.core().missing_assets().is_empty());
    assert!(!s.relink_open(), "the panel closes once nothing is missing");
    assert!(!relink_item(&s).enabled);
    assert!(s.core().is_dirty(), "the relink is an edit");
}

#[test]
fn search_folder_asks_for_a_folder_and_matches_by_name() {
    let t = Tmp::new();
    let mut s = opened(&t);
    s.execute(AppCommand::RelinkAudio);
    let at = s.relink_view().unwrap().search_rect().center();
    assert_eq!(click(&mut s, at), vec![ShellRequest::PickRelinkFolder]);
    wav(&t.0.join("pool").join("deep"), "Kick.WAV", 0.25);
    s.relink_search_folder(t.0.join("pool"));
    s.core_mut().wait_background();
    s.frame();
    assert!(s.core().missing_assets().is_empty());
    assert!(!s.relink_open());
}

#[test]
fn a_search_without_matches_keeps_the_panel_and_the_list() {
    let t = Tmp::new();
    let mut s = opened(&t);
    s.execute(AppCommand::RelinkAudio);
    wav(&t.0.join("pool"), "other.wav", 0.25);
    s.relink_search_folder(t.0.join("pool"));
    s.core_mut().wait_background();
    s.frame();
    assert!(s.relink_open());
    assert_eq!(s.core().missing_assets().len(), 1);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no matching audio files")
    );
}

#[test]
fn escape_and_close_dismiss_the_panel_and_a_picked_file_without_a_target_is_ignored() {
    let t = Tmp::new();
    let mut s = opened(&t);
    s.execute(AppCommand::RelinkAudio);
    let esc = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    assert!(s.handle(&esc).is_empty());
    assert!(!s.relink_open());
    s.execute(AppCommand::RelinkAudio);
    let at = s.relink_view().unwrap().close_rect().center();
    assert!(click(&mut s, at).is_empty());
    assert!(!s.relink_open());
    // No LOCATE... was pressed: a stray path does nothing.
    s.relink_file(wav(&t.0.join("new"), "x.wav", 0.5));
    s.core_mut().wait_background();
    assert_eq!(s.core().missing_assets().len(), 1);
}

#[test]
fn the_panel_takes_the_events_while_it_is_open() {
    let t = Tmp::new();
    let mut s = opened(&t);
    s.execute(AppCommand::RelinkAudio);
    // A click far from the panel neither reaches the rack nor closes it.
    let before = s.core().doc().clone();
    assert!(click(&mut s, atomik_ui::kurbo::Point::new(5.0, 700.0)).is_empty());
    assert!(s.relink_open());
    assert_eq!(s.core().doc(), &before);
}
