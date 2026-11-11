//! File > Save Version and File > Versions in the shell: the menu, the panel, saving a named
//! version, protecting, restoring (after the question about unsaved changes) and the
//! preference. The shell is built as Windows, so Ctrl is the command key on every host.

use super::*;
use crate::session::NullBackend;
use atomik_model::Command;
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-shell-versions-{tag}-{}-{n}",
            std::process::id()
        ));
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

fn shell(t: &Tmp) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(t.path("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(t.path("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn tune(s: &mut Shell<NullBackend>, cents: f32) {
    s.core_mut()
        .apply_command(Command::SetMasterTune(cents))
        .unwrap();
}

/// A shell whose song is saved as `<tmp>/song.atomik` (tune 1), then saved again at tune 2 so
/// that it holds one automatic version.
fn saved(t: &Tmp) -> Shell<NullBackend> {
    let mut s = shell(t);
    tune(&mut s, 1.0);
    s.core_mut().save_as(&t.path("song.atomik")).unwrap();
    tune(&mut s, 2.0);
    s.core_mut().save().unwrap();
    s
}

fn key(s: &mut Shell<NullBackend>, key: Key) -> Vec<ShellRequest> {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        key(s, Key::Char(c));
    }
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    let r = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    r
}

fn item(s: &Shell<NullBackend>, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
        .find(|i| i.command == command)
        .unwrap_or_else(|| panic!("{command:?} is in a menu"))
        .clone()
}

#[test]
fn the_file_menu_has_save_version_and_versions_after_save_as() {
    let t = Tmp::new("menu");
    let s = shell(&t);
    let file = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "File")
        .unwrap();
    let labels: Vec<String> = file.items().map(|i| i.label.to_string()).collect();
    let at = labels.iter().position(|l| l == "Save As\u{2026}").unwrap();
    assert_eq!(labels[at + 1], "Save Version\u{2026}");
    assert_eq!(labels[at + 2], "Versions\u{2026}");
    assert!(item(&s, AppCommand::SaveVersion).enabled);
    assert!(item(&s, AppCommand::Versions).enabled);
}

#[test]
fn a_song_without_a_project_file_says_to_save_it_first() {
    let t = Tmp::new("nofile");
    let mut s = shell(&t);
    for command in [AppCommand::SaveVersion, AppCommand::Versions] {
        assert!(s.execute(command).is_empty());
        assert!(!s.versions_open());
        assert!(
            s.core()
                .last_rejection()
                .is_some_and(|m| m.starts_with("Save the song first")),
            "{command:?}"
        );
    }
}

#[test]
fn the_panel_opens_lists_the_versions_and_takes_the_keys_and_esc_closes_it() {
    let t = Tmp::new("open");
    let mut s = saved(&t);
    assert!(s.execute(AppCommand::Versions).is_empty());
    assert!(s.versions_open());
    assert!(s.wants_text_input());
    let v = s.versions_view().unwrap();
    assert_eq!(v.items().len(), 1);
    assert!(!v.name_has_keys(), "File > Versions starts on the list");
    key(&mut s, Key::Escape);
    assert!(!s.versions_open());
    s.execute(AppCommand::SaveVersion);
    assert!(s.versions_view().unwrap().name_has_keys());
    // A key does not reach the rack behind the panel.
    key(&mut s, Key::Char('z'));
    assert!(s.versions_view().unwrap().name().ends_with('z'));
}

#[test]
fn saving_a_version_from_the_panel_keeps_it_named_and_leaves_the_song_unsaved() {
    let t = Tmp::new("save");
    let mut s = saved(&t);
    tune(&mut s, 9.0);
    s.execute(AppCommand::SaveVersion);
    type_text(&mut s, "Idea A");
    key(&mut s, Key::Enter);
    assert!(
        !s.versions_open(),
        "Save Version names and keeps, then closes"
    );
    let rows = s.core().versions().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "Idea A");
    assert!(!rows[0].automatic);
    assert!(s.core().is_dirty());
    // Browsing is the other command: the panel stays open to show versions, and the date is
    // shown to the minute (UTC).
    s.execute(AppCommand::Versions);
    assert!(!s.versions_view().unwrap().is_save_only());
    let when = &s.versions_view().unwrap().items()[0].when;
    assert_eq!(when.len(), "2026-11-06 12:34".len(), "{when}");
}

#[test]
fn save_version_is_a_naming_prompt_and_versions_is_the_browser() {
    let t = Tmp::new("distinct");
    let mut s = saved(&t);
    s.execute(AppCommand::SaveVersion);
    assert!(s.versions_view().unwrap().is_save_only());
    key(&mut s, Key::Escape);
    s.execute(AppCommand::Versions);
    assert!(!s.versions_view().unwrap().is_save_only());
}

#[test]
fn saving_with_an_empty_name_gives_a_default_name() {
    let t = Tmp::new("emptysave");
    let mut s = saved(&t);
    s.execute(AppCommand::SaveVersion);
    key(&mut s, Key::Enter);
    let rows = s.core().versions().unwrap();
    assert_eq!(rows[0].name, "Version 1");
    assert!(!rows[0].automatic);
}

#[test]
fn protecting_from_the_panel_marks_the_version() {
    let t = Tmp::new("protect");
    let mut s = saved(&t);
    s.execute(AppCommand::Versions);
    let at = s.versions_view().unwrap().protect_rect().center();
    click(&mut s, at);
    assert!(s.versions_view().unwrap().items()[0].protected);
    assert!(s.core().versions().unwrap()[0].protected);
}

#[test]
fn restoring_a_clean_song_opens_the_version_at_once_and_closes_the_panel() {
    let t = Tmp::new("restore-clean");
    let mut s = saved(&t);
    assert!(!s.core().is_dirty());
    s.execute(AppCommand::Versions);
    let at = s.versions_view().unwrap().restore_rect().center();
    let requests = click(&mut s, at);
    assert!(requests.is_empty(), "{requests:?}");
    assert!(!s.versions_open());
    assert_eq!(s.core().doc().tune_cents, 1.0);
    assert!(s.core().is_dirty(), "the restored song is not saved yet");
}

#[test]
fn restoring_with_unsaved_changes_asks_first_and_each_answer_carries_on() {
    let t = Tmp::new("restore-dirty");
    let mut s = saved(&t);
    tune(&mut s, 7.0);
    s.execute(AppCommand::Versions);
    let at = s.versions_view().unwrap().restore_rect().center();
    let id = s.core().versions().unwrap()[0].id;
    let requests = click(&mut s, at);
    assert_eq!(
        requests,
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::RestoreVersion(id)
        }]
    );
    assert_eq!(s.core().doc().tune_cents, 7.0, "asking changes nothing");
    // Cancel: nothing happens (the platform does not call back).
    // Don't Save: the version opens, and the unsaved state was kept as a version first.
    assert!(s.discard_then(PendingAction::RestoreVersion(id)).is_empty());
    assert_eq!(s.core().doc().tune_cents, 1.0);
    assert!(!s.versions_open());
    let rows = s.core().versions().unwrap();
    assert_eq!(rows[0].name, "Before restoring a version");

    // Save: the song is saved, then the version opens.
    let mut s = saved(&t);
    tune(&mut s, 7.0);
    let id = s.core().versions().unwrap()[0].id;
    assert!(s.save_then(PendingAction::RestoreVersion(id)).is_empty());
    assert_eq!(s.core().doc().tune_cents, 1.0);
    let text = std::fs::read_to_string(t.path("song.atomik")).unwrap();
    let saved_doc = atomik_model::load_project(&text, s.core().registry()).unwrap();
    assert_eq!(
        saved_doc.tune_cents, 7.0,
        "the unsaved state was saved first"
    );
}

#[test]
fn a_version_that_cannot_be_restored_is_reported_and_the_panel_stays() {
    let t = Tmp::new("restore-bad");
    let mut s = saved(&t);
    s.execute(AppCommand::Versions);
    let id = s.core().versions().unwrap()[0].id;
    let file = atomik_audio_file::VersionStore::for_project(s.core().file().unwrap())
        .unwrap()
        .path_of(id)
        .unwrap();
    std::fs::write(file, "not a project").unwrap();
    tune(&mut s, 5.0);
    s.core_mut().save().unwrap();
    s.execute(AppCommand::Versions);
    let at = s.versions_view().unwrap().restore_rect().center();
    let id = s.versions_view().unwrap().items().last().unwrap().id;
    // Choose the damaged one (the oldest) and restore it after the song is clean.
    let rows = s.versions_view().unwrap().items().len();
    for _ in 0..rows {
        key(&mut s, Key::Down);
    }
    assert_eq!(s.versions_view().unwrap().selected_id(), Some(id));
    click(&mut s, at);
    assert!(s.versions_open());
    assert_eq!(s.core().doc().tune_cents, 5.0);
}

#[test]
fn the_versions_kept_preference_reaches_the_core() {
    let t = Tmp::new("pref");
    let mut s = shell(&t);
    assert_eq!(s.core().keep_versions(), 20);
    let mut prefs = s.workflow().clone();
    prefs.keep_versions = 7;
    let user = s.user_keymap().clone();
    s.apply_preferences(Some((user, prefs)));
    assert_eq!(s.core().keep_versions(), 7);
}

#[test]
fn compare_shows_what_differs_from_the_song_now_and_esc_goes_back_before_it_closes() {
    let t = Tmp::new("compare");
    let mut s = saved(&t);
    // The file holds 2 cents; the version the second save kept holds 1. Unsaved: 7 cents.
    tune(&mut s, 7.0);
    s.execute(AppCommand::Versions);
    let at = s.versions_view().unwrap().compare_rect().center();
    assert!(click(&mut s, at).is_empty());
    let v = s.versions_view().unwrap();
    assert!(v.report_open());
    let lines = v.report_lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.trim() == "~ Master tune: +1 cents -> +7 cents"),
        "{lines:?}"
    );
    assert_eq!(
        s.core().versions().unwrap().len(),
        1,
        "comparing keeps nothing"
    );
    assert!(s.core().is_dirty());
    // Esc leaves the comparison; a second Esc closes the panel.
    key(&mut s, Key::Escape);
    assert!(s.versions_open());
    assert!(!s.versions_view().unwrap().report_open());
    key(&mut s, Key::Escape);
    assert!(!s.versions_open());
}

#[test]
fn compare_two_versions_with_shift_and_the_c_key() {
    let t = Tmp::new("compare-two");
    let mut s = saved(&t);
    tune(&mut s, 3.0);
    s.core_mut().save().unwrap();
    s.execute(AppCommand::Versions);
    assert_eq!(s.versions_view().unwrap().items().len(), 2);
    // Newest first: the newest holds 2 cents, the older 1 cent.
    key(&mut s, Key::Down);
    s.handle(&UiEvent::Key {
        key: Key::Up,
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
        pressed: true,
    });
    assert_eq!(s.versions_view().unwrap().marked_id(), Some(1));
    key(&mut s, Key::Char('c'));
    let lines = s.versions_view().unwrap().report_lines().unwrap().to_vec();
    assert!(
        lines
            .iter()
            .any(|l| l.trim() == "~ Master tune: +1 cents -> +2 cents"),
        "{lines:?}"
    );
    // Enter goes back to the list and restores nothing.
    key(&mut s, Key::Enter);
    assert!(s.versions_open());
    assert_eq!(s.core().doc().tune_cents, 3.0);
}

#[test]
fn a_comparison_that_cannot_be_made_is_reported_and_the_list_stays() {
    let t = Tmp::new("compare-bad");
    let mut s = saved(&t);
    s.execute(AppCommand::Versions);
    let id = s.versions_view().unwrap().selected_id().unwrap();
    let file = s.core().versions_file().unwrap();
    let store = atomik_audio_file::VersionStore::for_project(&file).unwrap();
    std::fs::write(store.path_of(id).unwrap(), "not a project").unwrap();
    key(&mut s, Key::Char('c'));
    assert!(s.versions_open());
    assert!(!s.versions_view().unwrap().report_open());
}

#[test]
fn the_preview_level_is_saved_in_the_settings_and_comes_back_next_time() {
    let t = Tmp::new("preview-level");
    let mut s = shell(&t);
    assert_eq!(s.core().preview_db(), 0.0);
    // The control moved in the browser: the core applies it, the shell saves it.
    let action = atomik_ui::BrowseAction::SetPreviewLevel(-9.0);
    s.core_mut().browse(action.clone());
    s.pane_actions(&[UiAction::Browse(action)]);
    assert_eq!(s.core().preview_db(), -9.0);
    assert_eq!(s.workflow().preview_db, -9.0);
    let text =
        std::fs::read_to_string(keymap_store::workflow_path(&t.path("settings.json"))).unwrap();
    assert!(text.contains("\"preview_db\": -9.0"), "{text}");
    // A new session reads it back into the core and onto the control.
    let again = shell(&t);
    assert_eq!(again.core().preview_db(), -9.0);
    assert_eq!(again.core().file_browser().preview_db(), -9.0);
}
