//! Templates, the default song, song information, project archives and the scratch folder in the
//! shell: the File menu, what each command asks the platform for, and what happens after each
//! answer. The shell is built as Windows, so Ctrl is the command key on every host.

use super::*;
use crate::commands::{MenuEntry, Submenu};
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey, SongInfo};
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-shell-templates-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn templates(&self) -> PathBuf {
        self.path("Templates")
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell whose library is `<tmp>/Library` (so its templates folder is `<tmp>/Templates`) and
/// whose preferences are saved next to `<tmp>/settings.json`.
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

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        s.handle(&UiEvent::Key {
            key: Key::Char(c),
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
}

fn press(s: &mut Shell<NullBackend>, key: Key) -> Vec<ShellRequest> {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
}

fn template_menu(s: &Shell<NullBackend>) -> Submenu {
    s.menu_model()[0]
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu(sub) if sub.label == "New from Template" => Some(sub.clone()),
            _ => None,
        })
        .expect("File has a New from Template submenu")
}

fn template_labels(s: &Shell<NullBackend>) -> Vec<String> {
    template_menu(s)
        .items()
        .filter(|i| matches!(i.command, AppCommand::NewFromTemplate(_)))
        .map(|i| i.label.to_string())
        .collect()
}

fn item(s: &Shell<NullBackend>, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.command == command)
        .unwrap_or_else(|| panic!("{command:?} is in a menu"))
        .clone()
}

/// A shell with a saved template `name` holding a synth called `from-template`.
fn with_template(t: &Tmp, name: &str) -> Shell<NullBackend> {
    let mut s = shell(t);
    add(&mut s, "from-template");
    s.core_mut().save_as_template(name).unwrap();
    s.core_mut().new_document();
    s
}

// ------------------------------------------------------------------ the menu

#[test]
fn the_file_menu_lists_the_templates_and_follows_the_folder() {
    let t = Tmp::new("menu");
    let mut s = shell(&t);
    assert!(template_labels(&s).is_empty());
    let items: Vec<String> = template_menu(&s)
        .items()
        .map(|i| i.label.to_string())
        .collect();
    assert_eq!(
        items,
        [
            "Save as Template",
            "Show Template Folder",
            "Use Song as Default Song",
            "Reset Default Song"
        ]
    );
    assert!(!item(&s, AppCommand::ResetDefaultSong).enabled);
    // A project put into the folder shows after the next look at it.
    std::fs::create_dir_all(t.templates()).unwrap();
    std::fs::write(t.templates().join("Drums.atomik"), "{}").unwrap();
    std::fs::write(t.templates().join("Bass.atomik"), "{}").unwrap();
    s.frame_at(10.0);
    assert_eq!(template_labels(&s), ["Bass", "Drums"]);
    std::fs::remove_file(t.templates().join("Bass.atomik")).unwrap();
    s.frame_at(10.5);
    assert_eq!(
        template_labels(&s),
        ["Bass", "Drums"],
        "not within a second"
    );
    s.frame_at(12.0);
    assert_eq!(template_labels(&s), ["Drums"]);
}

#[test]
fn the_new_menu_rows_sit_after_open_recent_and_the_first_rows_do_not_move() {
    let t = Tmp::new("rows");
    let s = shell(&t);
    let labels: Vec<String> = s.menu_model()[0]
        .entries
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item(i) => Some(i.label.to_string()),
            MenuEntry::Submenu(s) => Some(s.label.to_string()),
            MenuEntry::Separator => None,
        })
        .collect();
    assert_eq!(
        labels[..6],
        [
            "New",
            "Open\u{2026}",
            "Open Recent",
            "New from Template",
            "Close",
            "Save"
        ]
    );
    for want in [
        "Song Information\u{2026}",
        "Export Project Archive\u{2026}",
        "Import Project Archive\u{2026}",
        "Cancel Project Archive",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want}");
    }
    assert!(!item(&s, AppCommand::CancelArchive).enabled);
}

// ------------------------------------------------------------------ New from Template

#[test]
fn a_template_opens_untitled_and_save_asks_for_a_name() {
    let t = Tmp::new("open");
    let mut s = with_template(&t, "Mine");
    s.frame_at(5.0);
    assert_eq!(template_labels(&s), ["Mine"]);
    assert_eq!(s.execute(AppCommand::NewFromTemplate(0)), vec![]);
    assert!(has(&s, "from-template"));
    assert_eq!(s.core().file(), None);
    assert!(!s.core().is_dirty());
    assert_eq!(s.title(), "Atomik Rack \u{2014} Untitled");
    assert_eq!(
        s.execute(AppCommand::Save),
        vec![ShellRequest::PickSaveFile]
    );
    // An entry that is not in the list does nothing.
    let doc = s.core().doc().clone();
    s.execute(AppCommand::NewFromTemplate(9));
    assert_eq!(s.core().doc(), &doc);
}

#[test]
fn a_template_that_vanished_is_said_and_dropped_from_the_list() {
    let t = Tmp::new("vanish");
    let mut s = with_template(&t, "Gone");
    s.frame_at(5.0);
    std::fs::remove_file(t.templates().join("Gone.atomik")).unwrap();
    s.execute(AppCommand::NewFromTemplate(0));
    assert!(template_labels(&s).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("no longer exists"))
    );
}

#[test]
fn a_template_asks_about_unsaved_changes_first() {
    let t = Tmp::new("ask");
    let mut s = with_template(&t, "Mine");
    s.frame_at(5.0);
    add(&mut s, "unsaved");
    let command = AppCommand::NewFromTemplate(0);
    assert_eq!(
        s.execute(command),
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::Command(command)
        }]
    );
    assert!(has(&s, "unsaved"), "asking changes nothing");
    // Don't Save carries on and opens the template.
    s.discard_then(PendingAction::Command(command));
    assert!(has(&s, "from-template") && !has(&s, "unsaved"));
}

#[test]
fn save_as_template_adds_a_copy_without_touching_the_song() {
    let t = Tmp::new("save");
    let mut s = shell(&t);
    add(&mut s, "lead");
    s.set_song_info_for_test("Cool Tune");
    let doc = s.core().doc().clone();
    assert_eq!(s.execute(AppCommand::SaveAsTemplate), vec![]);
    assert_eq!(s.core().doc(), &doc);
    assert!(s.core().is_dirty(), "the song is as unsaved as it was");
    assert!(t.templates().join("Cool Tune.atomik").is_file());
    assert_eq!(template_labels(&s), ["Cool Tune"]);
    // A second one takes a free name.
    s.execute(AppCommand::SaveAsTemplate);
    assert!(t.templates().join("Cool Tune 2.atomik").is_file());
}

#[test]
fn show_template_folder_makes_it_and_asks_the_platform_to_show_it() {
    let t = Tmp::new("show");
    let mut s = shell(&t);
    assert!(!t.templates().exists());
    assert_eq!(
        s.execute(AppCommand::ShowTemplateFolder),
        vec![ShellRequest::ShowFolder(t.templates())]
    );
    assert!(t.templates().is_dir());
}

// ------------------------------------------------------------------ the default song

fn workflow_json(t: &Tmp) -> String {
    std::fs::read_to_string(t.path("workflow.json")).unwrap_or_default()
}

#[test]
fn the_default_song_replaces_the_starter_project_for_new_but_not_for_close() {
    let t = Tmp::new("default");
    let mut s = shell(&t);
    add(&mut s, "in-default");
    s.execute(AppCommand::SetDefaultSong);
    let path = s.workflow().default_song.clone().expect("a default song");
    assert!(path.exists());
    assert!(workflow_json(&t).contains("default_song"));
    assert!(item(&s, AppCommand::ResetDefaultSong).enabled);
    // File > New opens a copy, untitled, and not dirty.
    s.core_mut().save_as(&t.path("work.atomik")).unwrap();
    s.execute(AppCommand::New);
    assert!(has(&s, "in-default"));
    assert_eq!(s.core().file(), None);
    assert!(!s.core().is_dirty());
    // Close ends the song and starts the plain starter project.
    s.execute(AppCommand::Close);
    assert!(!has(&s, "in-default"));
    s.execute(AppCommand::New);
    assert!(has(&s, "in-default"));
    // The default song is also what a start with no project opens.
    s.core_mut().new_document();
    assert!(s.start_with_default_song());
    assert!(has(&s, "in-default"));
    // Reset goes back to the starter project and is saved.
    s.execute(AppCommand::ResetDefaultSong);
    assert_eq!(s.workflow().default_song, None);
    assert!(!workflow_json(&t).contains("default_song"));
    s.execute(AppCommand::New);
    assert!(!has(&s, "in-default"));
    assert!(!s.start_with_default_song());
    assert!(!item(&s, AppCommand::ResetDefaultSong).enabled);
}

#[test]
fn a_default_song_that_is_gone_falls_back_to_the_starter_project_with_a_message() {
    let t = Tmp::new("gone");
    let mut s = shell(&t);
    add(&mut s, "in-default");
    s.execute(AppCommand::SetDefaultSong);
    let path = s.workflow().default_song.clone().unwrap();
    std::fs::remove_file(&path).ok();
    std::fs::remove_dir_all(path.parent().unwrap().join("Default Song")).ok();
    s.confirmed(AppCommand::New);
    assert!(!has(&s, "in-default"));
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("is gone"))
    );
    assert!(!s.start_with_default_song());
}

#[test]
fn the_default_song_survives_a_restart() {
    let t = Tmp::new("restart");
    {
        let mut s = shell(&t);
        add(&mut s, "in-default");
        s.execute(AppCommand::SetDefaultSong);
    }
    let mut again = shell(&t);
    assert!(again.workflow().default_song.is_some());
    assert!(again.start_with_default_song());
    assert!(has(&again, "in-default"));
}

// ------------------------------------------------------------------ song information

impl Shell<NullBackend> {
    /// Test helper: sets the song title through the core.
    fn set_song_info_for_test(&mut self, title: &str) {
        self.core_mut()
            .set_song_info(SongInfo {
                title: title.into(),
                ..SongInfo::default()
            })
            .unwrap();
    }
}

#[test]
fn song_information_is_typed_applied_and_shown_in_the_window_title() {
    let t = Tmp::new("info");
    let mut s = shell(&t);
    assert_eq!(s.execute(AppCommand::SongInfo), vec![]);
    assert!(s.song_info_open());
    assert!(s.modal_open());
    type_text(&mut s, "Night Drive");
    press(&mut s, Key::Tab);
    type_text(&mut s, "Example Author");
    press(&mut s, Key::Tab);
    type_text(&mut s, "me@example.org");
    // Nothing is applied before APPLY.
    assert!(s.core().song_info().is_empty());
    let apply = s.song_info_view().unwrap().apply_rect().center();
    click(&mut s, apply);
    assert!(!s.song_info_open());
    assert_eq!(s.core().song_info().title, "Night Drive");
    assert_eq!(s.core().song_info().author, "Example Author");
    assert_eq!(s.core().song_info().email, "me@example.org");
    assert_eq!(s.title(), "Atomik Rack \u{2014} Night Drive *");
    // One undo step.
    assert!(s.core_mut().undo());
    assert!(s.core().song_info().is_empty());
}

#[test]
fn escape_and_close_leave_the_information_alone_and_enter_applies() {
    let t = Tmp::new("info-esc");
    let mut s = shell(&t);
    s.execute(AppCommand::SongInfo);
    type_text(&mut s, "Draft");
    press(&mut s, Key::Escape);
    assert!(!s.song_info_open());
    assert!(s.core().song_info().is_empty());
    s.execute(AppCommand::SongInfo);
    type_text(&mut s, "Draft");
    let close = s.song_info_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.song_info_open() && s.core().song_info().is_empty());
    // Enter in a one-line text applies.
    s.execute(AppCommand::SongInfo);
    type_text(&mut s, "Final");
    press(&mut s, Key::Enter);
    assert!(!s.song_info_open());
    assert_eq!(s.core().song_info().title, "Final");
    // The panel opens with what the song has.
    s.execute(AppCommand::SongInfo);
    assert_eq!(s.song_info_view().unwrap().info().title, "Final");
    // Applying what is there changes nothing and is no undo step.
    let apply = s.song_info_view().unwrap().apply_rect().center();
    click(&mut s, apply);
    assert!(s.core_mut().undo());
    assert!(s.core().song_info().is_empty(), "one step only");
}

#[test]
fn typing_in_the_panel_never_reaches_the_shortcuts() {
    let t = Tmp::new("info-keys");
    let mut s = shell(&t);
    s.execute(AppCommand::SongInfo);
    // `t` is the speaker tool and `f` follows the playhead in the panes behind it.
    type_text(&mut s, "tf !1");
    assert_eq!(s.song_info_view().unwrap().info().title, "tf !1");
    assert!(!s.core().is_dirty());
}

// ------------------------------------------------------------------ project archives

#[test]
fn the_archive_commands_ask_the_platform_for_their_files() {
    let t = Tmp::new("archive-requests");
    let mut s = shell(&t);
    assert_eq!(
        s.execute(AppCommand::ExportArchive),
        vec![ShellRequest::PickExportArchive {
            name: "Untitled.atomikz".into()
        }]
    );
    s.set_song_info_for_test("Night Drive");
    assert_eq!(
        s.execute(AppCommand::ExportArchive),
        vec![ShellRequest::PickExportArchive {
            name: "Night Drive.atomikz".into()
        }]
    );
    assert_eq!(
        s.execute(AppCommand::ImportArchive),
        vec![ShellRequest::PickImportArchive]
    );
    s.execute(AppCommand::CancelArchive);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("No project archive"))
    );
}

#[test]
fn an_exported_archive_imports_and_opens_through_the_shell() {
    let t = Tmp::new("archive-flow");
    let mut s = shell(&t);
    add(&mut s, "in-archive");
    s.set_song_info_for_test("Packed");
    // The extension is added when the dialog's name has none.
    s.export_archive_to(&t.path("packed"));
    assert!(item(&s, AppCommand::CancelArchive).enabled || t.path("packed.atomikz").exists());
    s.core_mut().wait_background();
    s.frame();
    assert!(t.path("packed.atomikz").is_file());
    assert!(!item(&s, AppCommand::CancelArchive).enabled);
    // Importing unpacks beside it and opens the project, once the song is not in the way.
    s.core_mut().new_document();
    s.import_archive_from(&t.path("packed.atomikz"));
    s.core_mut().wait_background();
    s.frame();
    assert!(has(&s, "in-archive"));
    assert_eq!(s.core().song_info().title, "Packed");
    assert_eq!(
        s.core().file(),
        Some(t.path("packed").join("packed.atomik").as_path())
    );
}

#[test]
fn an_imported_archive_asks_before_it_replaces_unsaved_changes() {
    let t = Tmp::new("archive-ask");
    let mut s = shell(&t);
    add(&mut s, "in-archive");
    s.export_archive_to(&t.path("packed.atomikz"));
    s.core_mut().wait_background();
    s.core_mut().new_document();
    add(&mut s, "unsaved");
    s.import_archive_from(&t.path("packed.atomikz"));
    s.core_mut().wait_background();
    s.frame();
    assert!(
        has(&s, "unsaved"),
        "the song is not replaced behind the user's back"
    );
    assert!(
        t.path("packed").join("packed.atomik").is_file(),
        "the archive is unpacked"
    );
    let asked = s.take_requests();
    assert!(
        asked.iter().any(|r| matches!(
            r,
            ShellRequest::ConfirmSave {
                then: PendingAction::OpenFile(_)
            }
        )),
        "{asked:?}"
    );
}

// ------------------------------------------------------------------ relink and scratch

#[test]
fn the_relink_panel_asks_for_the_project_folder() {
    let t = Tmp::new("relink");
    let mut s = shell(&t);
    // A project with a missing audio file.
    let text = {
        let mut c = AppCore::new(crate::sink::RecordingSink::new());
        let dir = t.path("src");
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("a.wav");
        atomik_audio_file::write_wav_f32(
            &wav,
            &atomik_audio_file::AudioData::new(48_000, vec![vec![0.5; 4_800]]).unwrap(),
        )
        .unwrap();
        c.import_audio(vec![wav], atomik_ui::ImportTarget::NewTracks, 0);
        c.wait_background();
        c.save_as(&t.path("home").join("song.atomik")).unwrap();
        std::fs::read_to_string(t.path("home").join("song").join("song.atomik")).unwrap()
    };
    std::fs::create_dir_all(t.path("moved")).unwrap();
    std::fs::write(t.path("moved").join("song.atomik"), text).unwrap();
    s.open_file(&t.path("moved").join("song.atomik"));
    s.core_mut().wait_background();
    assert_eq!(s.core().missing_assets().len(), 1);
    s.execute(AppCommand::RelinkAudio);
    let button = s.relink_view().expect("the panel").project_rect().center();
    assert_eq!(
        click(&mut s, button),
        vec![ShellRequest::PickRelinkProjectFolder]
    );
    s.relink_project_folder(t.path("home").join("song"));
    s.core_mut().wait_background();
    s.frame();
    assert!(s.core().missing_assets().is_empty());
}

#[test]
fn the_scratch_folder_is_a_preference_that_the_core_follows() {
    let t = Tmp::new("scratch");
    let mut s = shell(&t);
    assert_eq!(s.core().scratch_folder(), None);
    // Without the settings dialog it applies and is saved at once.
    s.set_scratch_folder(t.path("scratch"));
    assert_eq!(s.core().scratch_folder(), Some(t.path("scratch").as_path()));
    assert!(workflow_json(&t).contains("scratch_folder"));
    // It comes back after a restart.
    let again = shell(&t);
    assert_eq!(
        again.core().scratch_folder(),
        Some(t.path("scratch").as_path())
    );
}

#[test]
fn the_workflow_tab_chooses_and_resets_the_scratch_folder() {
    use crate::settings_dialog::Tab;
    let t = Tmp::new("scratch-tab");
    let mut s = shell(&t);
    s.execute(AppCommand::AudioSettings);
    let tab = s
        .dialog()
        .unwrap()
        .tab_strip(Size::new(1000.0, 800.0))
        .iter()
        .find(|(_, tab)| *tab == Tab::Workflow)
        .map(|(r, _)| r.center())
        .unwrap();
    click(&mut s, tab);
    let layout = s
        .dialog()
        .unwrap()
        .workflow_layout(Size::new(1000.0, 800.0))
        .unwrap();
    assert_eq!(
        click(&mut s, layout.scratch.choose.center()),
        vec![ShellRequest::PickScratchFolder]
    );
    // The picked folder goes into the draft, and counts when Apply is pressed.
    s.set_scratch_folder(t.path("picked"));
    assert_eq!(s.core().scratch_folder(), None, "not before Apply");
    let size = Size::new(1000.0, 800.0);
    let layout = s.dialog().unwrap().workflow_layout(size).unwrap();
    assert!(
        layout.scratch.path.contains("picked"),
        "{}",
        layout.scratch.path
    );
    let apply = s.dialog().unwrap().buttons(size).0.center();
    click(&mut s, apply);
    assert_eq!(s.core().scratch_folder(), Some(t.path("picked").as_path()));
    // Default goes back to the temporary folder.
    s.execute(AppCommand::AudioSettings);
    click(&mut s, tab);
    let layout = s.dialog().unwrap().workflow_layout(size).unwrap();
    click(&mut s, layout.scratch.default.center());
    let apply = s.dialog().unwrap().buttons(size).0.center();
    click(&mut s, apply);
    assert_eq!(s.core().scratch_folder(), None);
}
