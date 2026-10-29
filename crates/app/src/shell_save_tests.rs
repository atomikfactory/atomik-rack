//! The question about unsaved changes (Save, Don't Save, Cancel), File > Close, and the recent
//! songs: what the shell asks the platform for, and what happens after each answer.

use super::*;
use crate::commands::{MenuEntry, Submenu};
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{Key, Modifiers};

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

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-app-save-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn add(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(k.into())),
        })
        .unwrap();
}

fn has(s: &Shell<NullBackend>, k: &str) -> bool {
    s.core().doc().devices.iter().any(|d| d.key.0 == k)
}

/// A shell with a saved project `name` in `d` and one unsaved change (a synth called `lead`).
fn dirty_saved(d: &Path, name: &str) -> (Shell<NullBackend>, PathBuf) {
    let mut s = shell();
    let file = d.join(name);
    s.save_to(&file);
    assert!(!s.core().is_dirty());
    add(&mut s, "lead");
    assert!(s.core().is_dirty());
    (s, file)
}

fn asks(then: PendingAction) -> Vec<ShellRequest> {
    vec![ShellRequest::ConfirmSave { then }]
}

fn recent_menu(s: &Shell<NullBackend>) -> Submenu {
    let model = s.menu_model();
    model[0]
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu(sub) if sub.label == "Open Recent" => Some(sub.clone()),
            _ => None,
        })
        .expect("File has an Open Recent submenu")
}

fn recent_labels(s: &Shell<NullBackend>) -> Vec<String> {
    recent_menu(s)
        .items()
        .filter(|i| matches!(i.command, AppCommand::OpenRecent(_)))
        .map(|i| i.label.to_string())
        .collect()
}

// ------------------------------------------------------------------ the question

#[test]
fn closing_quitting_opening_and_creating_with_changes_ask_to_save() {
    for command in [
        AppCommand::New,
        AppCommand::Open,
        AppCommand::Close,
        AppCommand::Quit,
    ] {
        let mut s = shell();
        add(&mut s, "a");
        assert_eq!(
            s.execute(command),
            asks(PendingAction::Command(command)),
            "{command:?}"
        );
        assert!(has(&s, "a"), "asking changes nothing");
    }
}

#[test]
fn a_clean_song_closes_at_once() {
    let mut s = shell();
    assert!(!s.core().is_dirty());
    assert_eq!(s.execute(AppCommand::Close), vec![]);
    assert!(s.core().file().is_none());
    assert_eq!(s.execute(AppCommand::Quit), vec![ShellRequest::Quit]);
}

#[test]
fn ctrl_w_closes_the_song() {
    let mut s = shell();
    add(&mut s, "a");
    let ctrl_w = UiEvent::Key {
        key: Key::Char('w'),
        modifiers: Modifiers::CTRL,
        pressed: true,
    };
    assert_eq!(
        s.handle(&ctrl_w),
        asks(PendingAction::Command(AppCommand::Close))
    );
}

// ------------------------------------------------------------------ Save

#[test]
fn save_then_new_saves_the_song_and_starts_a_new_one() {
    let d = dir("save-new");
    let (mut s, file) = dirty_saved(&d, "song.atomik");
    let after = s.save_then(PendingAction::Command(AppCommand::New));
    assert_eq!(after, vec![]);
    assert!(
        std::fs::read_to_string(&file).unwrap().contains("lead"),
        "the change reached the file"
    );
    assert!(!has(&s, "lead"), "a new song follows");
    assert!(s.core().file().is_none() && !s.core().is_dirty());
}

#[test]
fn save_then_close_and_save_then_quit_continue_after_the_save() {
    let d = dir("save-close");
    let (mut s, file) = dirty_saved(&d, "song.atomik");
    assert_eq!(
        s.save_then(PendingAction::Command(AppCommand::Close)),
        vec![]
    );
    assert!(std::fs::read_to_string(&file).unwrap().contains("lead"));
    assert!(!has(&s, "lead"));
    let (mut q, file) = dirty_saved(&d, "quit.atomik");
    assert_eq!(
        q.save_then(PendingAction::Command(AppCommand::Quit)),
        vec![ShellRequest::Quit]
    );
    assert!(std::fs::read_to_string(&file).unwrap().contains("lead"));
}

#[test]
fn save_then_open_saves_and_opens_the_other_song() {
    let d = dir("save-open");
    let mut other = shell();
    add(&mut other, "other");
    let other_file = d.join("other.atomik");
    other.save_to(&other_file);
    let (mut s, file) = dirty_saved(&d, "song.atomik");
    assert_eq!(
        s.save_then(PendingAction::OpenFile(other_file.clone())),
        vec![]
    );
    assert!(std::fs::read_to_string(&file).unwrap().contains("lead"));
    assert!(has(&s, "other") && !has(&s, "lead"));
    assert_eq!(s.core().file(), Some(other_file.as_path()));
}

#[test]
fn save_then_open_command_goes_on_to_the_open_dialog() {
    let d = dir("save-open-dialog");
    let (mut s, _) = dirty_saved(&d, "song.atomik");
    assert_eq!(
        s.save_then(PendingAction::Command(AppCommand::Open)),
        vec![ShellRequest::PickOpenFile]
    );
}

#[test]
fn a_song_without_a_file_asks_where_to_save_and_carries_on_from_the_picker() {
    let d = dir("save-as");
    let mut s = shell();
    add(&mut s, "lead");
    let then = PendingAction::Command(AppCommand::New);
    let more = s.save_then(then.clone());
    assert_eq!(
        more,
        vec![ShellRequest::PickSaveFileThen { then: then.clone() }]
    );
    // Nothing happened yet: a cancelled picker ends here with the song still open.
    assert!(has(&s, "lead") && s.core().is_dirty());
    // The picker returned a path.
    let file = d.join("named.atomik");
    assert_eq!(s.save_to_then(&file, then), vec![]);
    assert!(std::fs::read_to_string(&file).unwrap().contains("lead"));
    assert!(!has(&s, "lead"));
}

#[test]
fn a_save_that_fails_leaves_the_song_open_with_a_message() {
    let d = dir("save-fails");
    let mut s = shell();
    add(&mut s, "lead");
    // The target is a folder: the write cannot succeed.
    let more = s.save_to_then(&d, PendingAction::Command(AppCommand::New));
    assert_eq!(more, vec![]);
    assert!(
        has(&s, "lead") && s.core().is_dirty(),
        "the song stays open"
    );
    assert!(s.core().last_rejection().is_some(), "the failure is shown");
}

// ------------------------------------------------------------------ Don't Save and Cancel

#[test]
fn dont_save_discards_the_changes_and_carries_on() {
    let d = dir("discard");
    for command in [AppCommand::New, AppCommand::Close] {
        let (mut s, file) = dirty_saved(&d, "song.atomik");
        assert_eq!(s.discard_then(PendingAction::Command(command)), vec![]);
        assert!(!has(&s, "lead"));
        assert!(
            !std::fs::read_to_string(&file).unwrap().contains("lead"),
            "the file keeps the last save"
        );
    }
    let (mut s, _) = dirty_saved(&d, "song.atomik");
    assert_eq!(
        s.discard_then(PendingAction::Command(AppCommand::Quit)),
        vec![ShellRequest::Quit]
    );
    let (mut s, _) = dirty_saved(&d, "song.atomik");
    assert_eq!(
        s.discard_then(PendingAction::Command(AppCommand::Open)),
        vec![ShellRequest::PickOpenFile]
    );
}

#[test]
fn cancel_changes_nothing() {
    // Cancel is no call at all: the question leaves the song as it was.
    let d = dir("cancel");
    let (mut s, file) = dirty_saved(&d, "song.atomik");
    let before = std::fs::read_to_string(&file).unwrap();
    for command in [
        AppCommand::New,
        AppCommand::Open,
        AppCommand::Close,
        AppCommand::Quit,
    ] {
        assert_eq!(s.execute(command), asks(PendingAction::Command(command)));
    }
    assert!(has(&s, "lead") && s.core().is_dirty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

// ------------------------------------------------------------------ the recent songs

#[test]
fn the_file_menu_lists_the_recent_songs_and_close() {
    let d = dir("menu");
    let mut s = shell();
    s.save_to(&d.join("first.atomik"));
    s.save_to(&d.join("second.atomik"));
    assert_eq!(recent_labels(&s), ["second", "first"]);
    let model = s.menu_model();
    let file_items: Vec<_> = model[0].items().collect();
    let close = file_items
        .iter()
        .find(|i| i.command == AppCommand::Close)
        .unwrap();
    assert_eq!(close.shortcut.as_deref(), Some("Ctrl+W"));
    let clear = file_items
        .iter()
        .find(|i| i.command == AppCommand::ClearRecent)
        .unwrap();
    assert!(clear.enabled);
    // The list is a submenu entry between Open and Close.
    let at = |f: &dyn Fn(&MenuEntry) -> bool| model[0].entries.iter().position(f).unwrap();
    let open = at(&|e| matches!(e, MenuEntry::Item(i) if i.command == AppCommand::Open));
    let sub = at(&|e| matches!(e, MenuEntry::Submenu(_)));
    let shut = at(&|e| matches!(e, MenuEntry::Item(i) if i.command == AppCommand::Close));
    assert!(open < sub && sub < shut);
}

#[test]
fn an_empty_recent_list_has_only_a_disabled_clear_menu() {
    let s = shell();
    let menu = recent_menu(&s);
    let items: Vec<_> = menu.items().collect();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].command, AppCommand::ClearRecent);
    assert!(!items[0].enabled);
}

#[test]
fn open_recent_opens_the_song_and_moves_it_first() {
    let d = dir("open-recent");
    let mut s = shell();
    add(&mut s, "one");
    s.save_to(&d.join("one.atomik"));
    s.core_mut().new_document();
    add(&mut s, "two");
    s.save_to(&d.join("two.atomik"));
    assert_eq!(recent_labels(&s), ["two", "one"]);
    // The current song is clean, so the song opens at once.
    assert_eq!(s.execute(AppCommand::OpenRecent(1)), vec![]);
    assert!(has(&s, "one") && !has(&s, "two"));
    assert_eq!(recent_labels(&s), ["one", "two"]);
}

#[test]
fn open_recent_with_unsaved_changes_asks_first() {
    let d = dir("open-recent-dirty");
    let mut s = shell();
    add(&mut s, "one");
    let one = d.join("one.atomik");
    s.save_to(&one);
    s.core_mut().new_document();
    add(&mut s, "two");
    assert_eq!(
        s.execute(AppCommand::OpenRecent(0)),
        asks(PendingAction::OpenFile(one))
    );
    assert!(has(&s, "two"), "nothing opened yet");
}

#[test]
fn a_missing_recent_song_leaves_the_list_with_a_message() {
    let d = dir("missing");
    let mut s = shell();
    let file = d.join("gone.atomik");
    s.save_to(&file);
    add(&mut s, "kept");
    s.save_to(&d.join("here.atomik"));
    std::fs::remove_file(&file).unwrap();
    assert_eq!(recent_labels(&s), ["here", "gone"]);
    assert_eq!(s.execute(AppCommand::OpenRecent(1)), vec![]);
    assert_eq!(recent_labels(&s), ["here"]);
    let message = s.core().last_rejection().unwrap_or_default();
    assert!(message.contains("no longer exists"), "{message}");
    assert!(has(&s, "kept"), "the song that was open stays");
}

#[test]
fn clear_menu_forgets_the_recent_songs() {
    let d = dir("clear");
    let mut s = shell();
    s.save_to(&d.join("a.atomik"));
    assert_eq!(recent_labels(&s).len(), 1);
    assert_eq!(s.execute(AppCommand::ClearRecent), vec![]);
    assert!(recent_labels(&s).is_empty());
    assert!(s.core().recent().is_empty());
}

#[test]
fn recent_commands_have_ids_with_the_place_as_the_argument() {
    assert_eq!(
        AppCommand::OpenRecent(3).command_id(),
        Some(("file.open_recent", Some("3")))
    );
    assert_eq!(
        AppCommand::from_command("file.open_recent", Some("3")),
        Some(AppCommand::OpenRecent(3))
    );
    assert_eq!(
        AppCommand::from_command("file.open_recent", Some("8")),
        None
    );
    assert_eq!(AppCommand::OpenRecent(8).command_id(), None);
    assert_eq!(
        AppCommand::from_command("file.close", None),
        Some(AppCommand::Close)
    );
    assert_eq!(
        crate::recent::MAX_RECENT,
        atomik_ui::keymap::RECENT_ARGS.len()
    );
}

// ------------------------------------------------------------------ start-up

/// A shell for a config folder with `workflow` as workflow.json and the songs of `recent`
/// (newest first) in recent.json; no audio, like `--no-audio`.
fn started(d: &Path, workflow: &str, recent: &[&Path]) -> Shell<NullBackend> {
    std::fs::write(d.join("workflow.json"), workflow).unwrap();
    let list = crate::recent::RecentFiles::from_json(&format!(
        "{{\"recent\": [{}]}}",
        recent
            .iter()
            .map(|p| format!("{:?}", p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    list.save(&d.join("recent.json")).unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.set_recent_path(d.join("recent.json"));
    Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(d.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    )
}

fn saved_song(d: &Path, name: &str, device: &str) -> PathBuf {
    let mut s = shell();
    add(&mut s, device);
    let file = d.join(name);
    s.save_to(&file);
    file
}

#[test]
fn start_up_reopens_the_last_song_when_the_preference_is_on() {
    let d = dir("startup-on");
    let newest = saved_song(&d, "newest.atomik", "newest");
    let older = saved_song(&d, "older.atomik", "older");
    let mut s = started(&d, r#"{"open_last_project": true}"#, &[&newest, &older]);
    assert!(s.restore_last_project());
    assert!(has(&s, "newest") && !has(&s, "older"));
    assert_eq!(s.core().file(), Some(newest.as_path()));
    assert!(!s.core().is_dirty());
}

#[test]
fn start_up_does_nothing_when_the_preference_is_off_or_the_list_is_empty() {
    let d = dir("startup-off");
    let song = saved_song(&d, "song.atomik", "song");
    let mut off = started(&d, "{}", &[&song]);
    assert!(!off.restore_last_project());
    assert!(!has(&off, "song") && off.core().file().is_none());
    let mut empty = started(&d, r#"{"open_last_project": true}"#, &[]);
    assert!(!empty.restore_last_project());
}

#[test]
fn start_up_skips_a_last_song_that_is_gone_and_says_so() {
    let d = dir("startup-gone");
    let gone = d.join("gone.atomik");
    let mut s = started(&d, r#"{"open_last_project": true}"#, &[&gone]);
    assert!(!s.restore_last_project());
    assert!(s.core().recent().is_empty(), "it left the list");
    let message = s.core().last_rejection().unwrap_or_default();
    assert!(message.contains("no longer exists"), "{message}");
}
