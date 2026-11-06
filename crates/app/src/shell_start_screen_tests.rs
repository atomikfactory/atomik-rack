//! The start screen in the shell: when it opens (start-up, File > Start Screen, the preference),
//! what each pick does, and that a shell nobody started by hand never shows it.

use super::*;
use crate::keymap_store::{WORKFLOW_FILE, load_workflow};
use crate::session::NullBackend;
use crate::start_screen::Target;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

fn dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-start-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A shell for config folder `d` with `workflow` as workflow.json, the songs of `recent` (newest
/// first) in recent.json and the library in `d/Library` (templates in `d/Templates`).
fn started(d: &Path, workflow: &str, recent: &[&Path]) -> Shell<NullBackend> {
    std::fs::write(d.join(WORKFLOW_FILE), workflow).unwrap();
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
    let mut s = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(d.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(d.join("Library")),
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

fn saved_song(d: &Path, name: &str, device: &str) -> PathBuf {
    let mut s = started(d, "{}", &[]);
    add(&mut s, device);
    let file = d.join(name);
    s.save_to(&file);
    file
}

fn press(s: &mut Shell<NullBackend>, key: Key) -> Vec<ShellRequest> {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    })
}

fn click_row(s: &mut Shell<NullBackend>, target: Target) -> Vec<ShellRequest> {
    let size = s.size();
    let at = s
        .start_screen()
        .expect("the start screen is open")
        .rows(size)
        .into_iter()
        .find(|(t, _)| *t == target)
        .unwrap_or_else(|| panic!("{target:?} is listed"))
        .1
        .center();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

fn painted(s: &mut Shell<NullBackend>) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    dl.to_json()
}

#[test]
fn a_shell_nobody_started_by_hand_never_shows_it() {
    // The headless and MCP sessions build a shell and never call the start-up path.
    let d = dir("never");
    let s = started(&d, "{}", &[]);
    assert!(!s.start_screen_open() && !s.modal_open());
}

#[test]
fn start_up_with_no_song_shows_it_unless_the_preference_says_not_to() {
    let d = dir("startup");
    let mut s = started(&d, "{}", &[]);
    s.start_without_project();
    assert!(s.start_screen_open() && s.modal_open());
    let off = dir("startup-off");
    let mut s = started(&off, r#"{"show_start_screen": false}"#, &[]);
    s.start_without_project();
    assert!(!s.start_screen_open());
}

#[test]
fn reopening_the_last_song_takes_the_place_of_the_start_screen() {
    let d = dir("last");
    let song = saved_song(&d, "song.atomik", "song");
    let mut s = started(&d, r#"{"open_last_project": true}"#, &[&song]);
    s.start_without_project();
    assert!(has(&s, "song") && !s.start_screen_open());
    // With the preference on but nothing to reopen, the screen shows.
    let mut empty = started(&dir("last-empty"), r#"{"open_last_project": true}"#, &[]);
    empty.start_without_project();
    assert!(empty.start_screen_open());
    // With the preference off, the screen shows even when songs are recent.
    let mut off = started(&d, "{}", &[&song]);
    off.start_without_project();
    assert!(off.start_screen_open() && !has(&off, "song"));
}

#[test]
fn the_file_menu_opens_it_and_escape_closes_it() {
    let d = dir("menu");
    let mut s = started(&d, "{}", &[]);
    assert!(
        s.menu_model()[0]
            .items()
            .any(|i| i.command == AppCommand::StartScreen)
    );
    s.execute(AppCommand::StartScreen);
    assert!(s.start_screen_open());
    // Nothing behind the box reacts to the keys while it is open.
    let before = s.core().doc().clone();
    press(&mut s, Key::Char('z'));
    assert_eq!(s.core().doc(), &before);
    press(&mut s, Key::Escape);
    assert!(!s.start_screen_open() && !s.modal_open());
}

#[test]
fn it_lists_the_templates_and_the_recent_songs_with_their_folders_and_dates() {
    let d = dir("lists");
    let song = saved_song(&d, "Night Drive.atomik", "drive");
    let mut maker = started(&d, "{}", &[]);
    add(&mut maker, "from-template");
    maker.core_mut().save_as_template("Drums").unwrap();
    let mut s = started(&d, "{}", &[&song]);
    s.execute(AppCommand::StartScreen);
    let screen = s.start_screen().unwrap();
    assert_eq!(screen.templates(), ["Drums"]);
    assert_eq!(screen.recent().len(), 1);
    let row = screen.recent()[0].clone();
    assert_eq!(row.name, "Night Drive");
    assert_eq!(row.folder, d.to_string_lossy());
    assert_eq!(row.date.len(), 10, "{}", row.date);
    assert!(row.exists);
    let text = painted(&mut s);
    assert!(text.contains("Night Drive") && text.contains("Drums"));
    assert!(text.contains(&row.date));
}

#[test]
fn a_pick_closes_it_and_does_what_the_file_menu_row_does() {
    let d = dir("picks");
    let song = saved_song(&d, "song.atomik", "song");
    let mut maker = started(&d, "{}", &[]);
    add(&mut maker, "from-template");
    maker.core_mut().save_as_template("Drums").unwrap();

    let mut s = started(&d, "{}", &[&song]);
    s.execute(AppCommand::StartScreen);
    assert!(click_row(&mut s, Target::Recent(0)).is_empty());
    assert!(!s.start_screen_open());
    assert!(has(&s, "song"));
    assert_eq!(s.core().file(), Some(song.as_path()));

    let mut s = started(&d, "{}", &[&song]);
    s.execute(AppCommand::StartScreen);
    click_row(&mut s, Target::Template(0));
    assert!(!s.start_screen_open());
    assert!(has(&s, "from-template") && s.core().file().is_none());

    let mut s = started(&d, "{}", &[&song]);
    s.execute(AppCommand::StartScreen);
    let requests = click_row(&mut s, Target::Open);
    assert_eq!(requests, [ShellRequest::PickOpenFile]);
    assert!(!s.start_screen_open());

    // A new song from a song with nothing to save replaces it at once.
    let mut s = started(&d, "{}", &[&song]);
    s.core_mut().open(&song).unwrap();
    assert!(has(&s, "song"));
    s.execute(AppCommand::StartScreen);
    assert!(click_row(&mut s, Target::New).is_empty());
    assert!(!s.start_screen_open());
    assert!(!has(&s, "song") && s.core().file().is_none());
}

#[test]
fn the_keyboard_picks_a_row_and_a_new_song_asks_about_unsaved_changes() {
    let d = dir("keys");
    let mut s = started(&d, "{}", &[]);
    add(&mut s, "scratch");
    s.execute(AppCommand::StartScreen);
    let requests = press(&mut s, Key::Enter);
    assert!(
        matches!(
            requests.as_slice(),
            [ShellRequest::ConfirmSave {
                then: PendingAction::Command(AppCommand::New)
            }]
        ),
        "{requests:?}"
    );
    assert!(!s.start_screen_open());
    assert!(has(&s, "scratch"), "nothing is lost until the answer");
}

#[test]
fn a_recent_song_that_is_gone_leaves_the_list_with_a_message() {
    let d = dir("gone");
    let gone = d.join("gone.atomik");
    let mut s = started(&d, "{}", &[&gone]);
    s.execute(AppCommand::StartScreen);
    assert!(!s.start_screen().unwrap().recent()[0].exists);
    click_row(&mut s, Target::Recent(0));
    assert!(s.core().recent().is_empty());
    let message = s.core().last_rejection().unwrap_or_default();
    assert!(message.contains("no longer exists"), "{message}");
}

#[test]
fn the_check_box_is_saved_with_the_preferences_at_once() {
    let d = dir("check");
    let mut s = started(&d, "{}", &[]);
    s.execute(AppCommand::StartScreen);
    assert!(s.start_screen().unwrap().show_at_start());
    click_row(&mut s, Target::ShowAtStart);
    assert!(s.start_screen_open(), "the box stays open");
    assert!(!s.workflow().show_start_screen);
    assert!(
        !load_workflow(&d.join(WORKFLOW_FILE))
            .value
            .show_start_screen
    );
    // The next start-up does not show it, and File > Start Screen still does.
    let mut again = started(
        &d,
        &std::fs::read_to_string(d.join(WORKFLOW_FILE)).unwrap(),
        &[],
    );
    again.start_without_project();
    assert!(!again.start_screen_open());
    again.execute(AppCommand::StartScreen);
    assert!(again.start_screen_open());
    assert!(!again.start_screen().unwrap().show_at_start());
}

#[test]
fn a_new_song_from_the_screen_uses_the_default_song_when_one_is_chosen() {
    let d = dir("default");
    let mut maker = started(&d, "{}", &[]);
    add(&mut maker, "defaulted");
    let file = d.join("Base.atomik");
    maker.save_to(&file);
    let mut s = started(&d, "{}", &[]);
    s.core_mut().open(&file).unwrap();
    s.execute(AppCommand::SetDefaultSong);
    s.execute(AppCommand::Close);
    assert!(!has(&s, "defaulted"));
    s.execute(AppCommand::StartScreen);
    assert!(
        s.start_screen()
            .unwrap()
            .lines()
            .iter()
            .any(|l| l.contains("default song"))
    );
    click_row(&mut s, Target::New);
    assert!(has(&s, "defaulted"));
}

fn click_pin(s: &mut Shell<NullBackend>, i: usize) -> Vec<ShellRequest> {
    let size = s.size();
    let screen = s.start_screen().expect("the start screen is open");
    let row = screen
        .rows(size)
        .into_iter()
        .find(|(t, _)| *t == Target::Recent(i))
        .unwrap()
        .1;
    let at = crate::start_screen::pin_rect(row).center();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

#[test]
fn a_pin_keeps_a_song_at_the_top_and_is_saved_with_the_recent_list() {
    let d = dir("pins");
    let songs: Vec<PathBuf> = ["a", "b", "c"]
        .iter()
        .map(|n| saved_song(&d, &format!("{n}.atomik"), n))
        .collect();
    let mut s = started(&d, "{}", &[&songs[0], &songs[1], &songs[2]]);
    s.execute(AppCommand::StartScreen);
    // Pin the oldest song (the third row): it moves to the top and the box shows it there.
    assert!(click_pin(&mut s, 2).is_empty());
    assert!(
        s.start_screen_open(),
        "a pin does not close the box or open the song"
    );
    assert_eq!(s.core().recent()[0], songs[2]);
    assert_eq!(s.core().pinned_count(), 1);
    let screen = s.start_screen().unwrap();
    assert_eq!(screen.recent()[0].name, "c");
    assert!(screen.recent()[0].pinned && !screen.recent()[1].pinned);
    assert_eq!(
        screen.focus(),
        Target::Recent(0),
        "the keys follow the song"
    );
    // It is on disk, and a shell started later has it.
    let file = std::fs::read_to_string(d.join("recent.json")).unwrap();
    assert!(file.contains("pinned"), "{file}");
    let later = crate::recent::RecentFiles::load(&d.join("recent.json"));
    assert_eq!(later.paths()[0], songs[2]);
    assert!(later.is_pinned(&songs[2]) && later.pinned_count() == 1);
    // A song opened now goes below the pin.
    s.core_mut().forget_recent(&songs[0]);
    s.core_mut().open(&songs[0]).unwrap();
    assert_eq!(s.core().recent()[0], songs[2]);
    // Unpinning puts it back among the others.
    s.execute(AppCommand::StartScreen);
    assert!(click_pin(&mut s, 0).is_empty());
    assert_eq!(s.core().pinned_count(), 0);
    assert!(!s.start_screen().unwrap().recent()[0].pinned);
}

#[test]
fn p_pins_the_song_under_the_keys_and_the_pin_limit_says_so() {
    let d = dir("pin-keys");
    let songs: Vec<PathBuf> = (0..6)
        .map(|i| saved_song(&d, &format!("s{i}.atomik"), &format!("d{i}")))
        .collect();
    let refs: Vec<&Path> = songs.iter().map(PathBuf::as_path).collect();
    let mut s = started(&d, "{}", &refs);
    s.execute(AppCommand::StartScreen);
    press(&mut s, Key::Right);
    for _ in 0..crate::recent::MAX_PINNED {
        press(&mut s, Key::Char('p'));
        press(&mut s, Key::Down);
    }
    assert_eq!(s.core().pinned_count(), crate::recent::MAX_PINNED);
    press(&mut s, Key::Char('p'));
    assert_eq!(s.core().pinned_count(), crate::recent::MAX_PINNED);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("At most 4 songs can be pinned")),
        "{:?}",
        s.core().last_rejection()
    );
    assert!(s.start_screen_open());
}

#[test]
fn the_card_reads_a_template_when_the_keys_or_the_pointer_reach_it() {
    let d = dir("card");
    let mut maker = started(&d, "{}", &[]);
    add(&mut maker, "from-template");
    maker
        .core_mut()
        .apply_command(Command::Song(atomik_model::SongCommand::AddTrack {
            device: DeviceKey("from-template".into()),
        }))
        .unwrap();
    maker.core_mut().save_as_template("Drums").unwrap();
    let mut s = started(&d, "{}", &[]);
    s.execute(AppCommand::StartScreen);
    assert_eq!(s.start_screen().unwrap().preview(0), None);
    press(&mut s, Key::Down);
    press(&mut s, Key::Down);
    assert_eq!(s.start_screen().unwrap().focus(), Target::Template(0));
    let preview = s
        .start_screen()
        .unwrap()
        .preview(0)
        .expect("read on demand");
    assert_eq!(
        preview.tracks.len(),
        s.core().doc().song.tracks.len().max(1)
    );
    assert!(painted(&mut s).contains("BPM"));
    // A template that cannot be read has a card that says so.
    std::fs::write(d.join("Templates").join("Broken.atomik"), "not a project").unwrap();
    s.execute(AppCommand::StartScreen);
    press(&mut s, Key::Down);
    press(&mut s, Key::Down);
    let screen = s.start_screen().unwrap();
    // The templates are listed by name: Broken comes first.
    assert_eq!(screen.templates(), ["Broken", "Drums"]);
    assert_eq!(screen.focus(), Target::Template(0));
    assert_eq!(screen.card_lines(0), ["No preview for this template"]);
}
