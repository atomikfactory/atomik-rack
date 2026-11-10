//! File > Restore from Backup in the shell: the menu row, the refusals, the list that fills in from
//! a worker, restoring (after the question about unsaved changes), and the settings that reach the
//! core. The shell is built as Windows, so Ctrl is the command key on every host.

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
            "atomik-shell-backup-{tag}-{}-{n}",
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

/// A shell whose song is saved as `<tmp>/song.atomik` at tune 1 with backups on in
/// `<tmp>/Backups`, then saved again at tune 2: two copies.
fn saved(t: &Tmp) -> Shell<NullBackend> {
    let mut s = shell(t);
    s.core_mut().set_backup(Some(t.path("Backups")), 10, true);
    tune(&mut s, 1.0);
    s.core_mut().save_as(&t.path("song.atomik")).unwrap();
    tune(&mut s, 2.0);
    s.core_mut().save().unwrap();
    s.core_mut().wait_background();
    s
}

fn key(s: &mut Shell<NullBackend>, key: Key) -> Vec<ShellRequest> {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
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

/// Frames until the list of copies has been filled in.
fn wait_for_rows(s: &mut Shell<NullBackend>) {
    for _ in 0..400 {
        s.frame();
        if s.backup_view().is_some_and(|v| !v.rows().is_empty()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the list never filled in");
}

#[test]
fn the_file_menu_has_restore_from_backup_after_versions() {
    let t = Tmp::new("menu");
    let s = shell(&t);
    let file = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "File")
        .unwrap();
    let labels: Vec<String> = file.items().map(|i| i.label.to_string()).collect();
    let at = labels.iter().position(|l| l == "Versions\u{2026}").unwrap();
    assert_eq!(labels[at + 1], "Restore from Backup\u{2026}");
    let item = file
        .items()
        .find(|i| i.command == AppCommand::RestoreBackup)
        .unwrap();
    assert!(item.enabled);
}

#[test]
fn without_a_folder_or_a_project_file_it_says_what_to_do() {
    let t = Tmp::new("refuse");
    let mut s = shell(&t);
    assert!(s.execute(AppCommand::RestoreBackup).is_empty());
    assert!(!s.backup_open());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("No backup folder is set")),
        "{:?}",
        s.core().last_rejection()
    );
    s.core_mut().set_backup(Some(t.path("Backups")), 10, true);
    s.execute(AppCommand::RestoreBackup);
    assert!(!s.backup_open());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("Save the song first"))
    );
}

#[test]
fn the_list_opens_empty_fills_in_from_the_worker_and_esc_closes_it() {
    let t = Tmp::new("open");
    let mut s = saved(&t);
    assert!(s.execute(AppCommand::RestoreBackup).is_empty());
    assert!(s.backup_open());
    assert!(s.backup_view().unwrap().rows().is_empty());
    assert!(s.backup_view().unwrap().note().starts_with("Looking"));
    wait_for_rows(&mut s);
    let v = s.backup_view().unwrap();
    assert_eq!(v.rows().len(), 2);
    assert_eq!(v.note(), "2 copies, newest first");
    assert_eq!(v.rows()[0].when.len(), 19);
    assert!(!s.wants_text_input());
    // A key does not reach the rack behind the panel; Esc closes.
    key(&mut s, Key::Char('z'));
    assert!(s.backup_open());
    key(&mut s, Key::Escape);
    assert!(!s.backup_open());
}

#[test]
fn restoring_a_clean_song_opens_the_copy_as_a_new_song_and_closes_the_list() {
    let t = Tmp::new("restore-clean");
    let mut s = saved(&t);
    assert!(!s.core().is_dirty());
    s.execute(AppCommand::RestoreBackup);
    wait_for_rows(&mut s);
    // The oldest copy holds tune 1 (the first save), the newest tune 2.
    key(&mut s, Key::End);
    let at = s.backup_view().unwrap().restore_rect().center();
    let requests = click(&mut s, at);
    assert!(requests.is_empty(), "{requests:?}");
    assert!(!s.backup_open());
    assert_eq!(s.core().doc().tune_cents, 1.0);
    assert_eq!(s.core().file(), None, "a new song, not the project");
    // The project file still holds the later state.
    let text = std::fs::read_to_string(t.path("song.atomik")).unwrap();
    assert!(text.contains('2'), "{text}");
}

#[test]
fn restoring_with_unsaved_changes_asks_first_and_each_answer_carries_on() {
    let t = Tmp::new("restore-dirty");
    let mut s = saved(&t);
    tune(&mut s, 7.0);
    s.execute(AppCommand::RestoreBackup);
    wait_for_rows(&mut s);
    let path = s.backup_rows[0].path.clone();
    let at = s.backup_view().unwrap().restore_rect().center();
    let requests = click(&mut s, at);
    assert_eq!(
        requests,
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::RestoreBackup(path.clone())
        }]
    );
    assert_eq!(s.core().doc().tune_cents, 7.0, "asking changes nothing");
    assert!(s.backup_open());
    // Don't Save: the newest copy opens.
    assert!(
        s.discard_then(PendingAction::RestoreBackup(path))
            .is_empty()
    );
    assert_eq!(s.core().doc().tune_cents, 2.0);
    assert_eq!(s.core().file(), None);
    assert!(!s.backup_open());
}

#[test]
fn saving_first_writes_the_song_then_opens_the_copy() {
    let t = Tmp::new("restore-save");
    let mut s = saved(&t);
    tune(&mut s, 7.0);
    s.execute(AppCommand::RestoreBackup);
    wait_for_rows(&mut s);
    key(&mut s, Key::End);
    let path = s.backup_rows[1].path.clone();
    assert!(s.save_then(PendingAction::RestoreBackup(path)).is_empty());
    assert_eq!(s.core().doc().tune_cents, 1.0);
    let text = std::fs::read_to_string(t.path("song.atomik")).unwrap();
    let doc = atomik_model::load_project(&text, s.core().registry()).unwrap();
    assert_eq!(doc.tune_cents, 7.0, "the unsaved state was saved first");
}

#[test]
fn a_copy_that_went_away_is_reported_and_the_list_stays() {
    let t = Tmp::new("restore-gone");
    let mut s = saved(&t);
    s.execute(AppCommand::RestoreBackup);
    wait_for_rows(&mut s);
    let gone = s.backup_rows[0].path.clone();
    std::fs::remove_file(&gone).unwrap();
    let at = s.backup_view().unwrap().restore_rect().center();
    click(&mut s, at);
    assert!(s.backup_open());
    assert!(s.core().last_rejection().is_some());
    assert_eq!(s.core().doc().tune_cents, 2.0);
    assert!(s.core().file().is_some());
}

#[test]
fn the_backup_settings_reach_the_core_when_the_dialog_is_applied() {
    let t = Tmp::new("apply");
    let mut s = shell(&t);
    assert_eq!(s.core().backup_folder(), None);
    let mut prefs = s.workflow().clone();
    prefs.backup_folder = Some(t.path("Elsewhere"));
    prefs.backup_keep = 4;
    prefs.backup_audio = false;
    let user = s.user_keymap().clone();
    s.apply_preferences(Some((user, prefs)));
    assert_eq!(
        s.core().backup_folder(),
        Some(t.path("Elsewhere").as_path())
    );
    assert_eq!(
        (s.core().backup_keep(), s.core().backup_audio()),
        (4, false)
    );
    // workflow.json holds them for the next start.
    let text = std::fs::read_to_string(t.path("workflow.json")).unwrap();
    assert!(text.contains("\"backup_keep\": 4"), "{text}");
    assert!(text.contains("\"backup_audio\": false"), "{text}");
    assert!(text.contains("backup_folder"), "{text}");
}

#[test]
fn choose_in_the_backup_tab_asks_for_a_folder_and_the_pick_lands_in_the_draft() {
    let t = Tmp::new("pick");
    let mut s = shell(&t);
    s.open_settings(Some(crate::settings_dialog::Tab::Backup));
    let size = Size::new(1000.0, 800.0);
    let at = s
        .dialog
        .as_ref()
        .unwrap()
        .backup_layout(size)
        .unwrap()
        .folder
        .choose
        .center();
    assert_eq!(click(&mut s, at), vec![ShellRequest::PickBackupFolder]);
    // The pick goes into the open dialog's draft, not into the core.
    s.set_backup_folder(t.path("Picked"));
    assert_eq!(s.core().backup_folder(), None);
    let draft = s.dialog.as_ref().unwrap().preferences().unwrap().1;
    assert_eq!(draft.backup_folder, Some(t.path("Picked")));
}

#[test]
fn a_folder_picked_without_the_dialog_is_applied_and_saved_at_once() {
    let t = Tmp::new("pick-direct");
    let mut s = shell(&t);
    s.set_backup_folder(t.path("Picked"));
    assert_eq!(s.core().backup_folder(), Some(t.path("Picked").as_path()));
    assert_eq!(s.workflow().backup_folder, Some(t.path("Picked")));
}
