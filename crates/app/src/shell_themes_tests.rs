//! The themes in a headless `Shell`: Next Theme and the View menu, the Themes tab (use, duplicate,
//! rename, delete, import, export), the theme editor with its live preview and its file, the
//! files that are skipped, and old settings files. Every folder is a temporary one of this run,
//! removed at the end; the user's own settings folder is never touched.

use super::*;
use crate::keymap_store::{WORKFLOW_FILE, load_workflow};
use crate::session::NullBackend;
use crate::settings_dialog::Tab;
use crate::themes::FOLDER;
use crate::themes_tab::{Button, ThemeAction};
use atomik_ui::theme_doc::{TOKENS, ThemeDoc, rgba_of};
use atomik_ui::{Modifiers, PointerButton, Theme, ThemeChoice};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("atomik-thm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn themes(&self) -> PathBuf {
        self.0.join(FOLDER)
    }

    fn shell(&self) -> Shell<NullBackend> {
        shell_in(Some(&self.0))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell_in(dir: Option<&Path>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: dir.map(|d| d.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 900.0)));
    s
}

fn click(s: &mut Shell<NullBackend>, at: Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

fn key(s: &mut Shell<NullBackend>, key: Key, modifiers: Modifiers) {
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    });
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        key(s, Key::Char(c), Modifiers::NONE);
    }
}

fn ctrl() -> Modifiers {
    Modifiers::CTRL
}

fn open_themes_tab(s: &mut Shell<NullBackend>) {
    s.execute(AppCommand::ThemeManager);
    assert!(s.dialog_open());
    assert_eq!(s.dialog().unwrap().tab(), Tab::Themes);
}

fn tab_row(s: &Shell<NullBackend>, id: &str) -> Point {
    let d = s.dialog().unwrap();
    let l = d.themes_layout(s.size()).unwrap();
    let i = d
        .themes_tab()
        .unwrap()
        .rows()
        .iter()
        .position(|r| r.id == id)
        .unwrap_or_else(|| panic!("{id} is listed"));
    l.rows[i].center()
}

fn tab_button(s: &Shell<NullBackend>, b: Button) -> Point {
    s.dialog()
        .unwrap()
        .themes_layout(s.size())
        .unwrap()
        .buttons
        .iter()
        .find(|(x, _)| *x == b)
        .unwrap()
        .1
        .center()
}

fn files(dir: &Dir) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.themes())
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn painted(s: &mut Shell<NullBackend>) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    dl.to_json()
}

#[test]
fn next_theme_walks_the_list_applies_live_and_saves_the_choice() {
    let dir = Dir::new("next");
    let mut s = dir.shell();
    assert_eq!(s.theme(), Theme::dark());
    assert_eq!(s.active_theme_id(), "dark");
    s.execute(AppCommand::NextTheme);
    assert_eq!(s.core().view().theme(), &Theme::light());
    s.execute(AppCommand::NextTheme);
    assert_eq!(s.core().view().theme(), &Theme::synthwave());
    assert_eq!(s.active_theme_id(), "synthwave");
    assert_eq!(
        load_workflow(&dir.0.join(WORKFLOW_FILE)).value.theme_id,
        "synthwave"
    );
    // A click on the song clears the message of the moved keys, which a later start no longer
    // shows.
    click(&mut s, Point::new(600.0, 330.0));
    let synth = painted(&mut s);
    // A shell started with it paints exactly what the switched one paints.
    let mut fresh = dir.shell();
    assert_eq!(fresh.theme(), Theme::synthwave());
    let other = painted(&mut fresh);
    let at = synth.bytes().zip(other.bytes()).position(|(a, b)| a != b);
    assert!(
        synth == other,
        "differs at {at:?}: {:?} against {:?}",
        at.map(|i| &synth[i.saturating_sub(120)..(i + 120).min(synth.len())]),
        at.map(|i| &other[i.saturating_sub(120)..(i + 120).min(other.len())])
    );
    s.execute(AppCommand::NextTheme);
    assert_eq!(s.core().view().theme(), &Theme::dark());
    assert_eq!(load_workflow(&dir.0.join(WORKFLOW_FILE)).value.theme_id, "");
    // The View menu's choices still work, and clear the theme id.
    s.execute(AppCommand::NextTheme);
    s.execute(AppCommand::NextTheme);
    s.execute(AppCommand::SetTheme(ThemeChoice::Light));
    assert_eq!(s.theme(), Theme::light());
    assert_eq!(s.active_theme_id(), "light");
    // Not one byte of the project changed on the way.
    assert!(!s.core().is_dirty());
}

#[test]
fn the_view_menu_lists_the_theme_commands() {
    let dir = Dir::new("menu");
    let s = dir.shell();
    let menus = s.menu_model();
    let view = menus.iter().find(|m| m.title == "View").unwrap();
    let text = format!("{view:?}");
    assert!(text.contains("Next Theme"), "{text}");
    assert!(text.contains("Theme Manager"), "{text}");
    for c in [AppCommand::NextTheme, AppCommand::ThemeManager] {
        let (id, arg) = c.command_id().unwrap();
        assert_eq!(AppCommand::from_command(id, arg), Some(c));
    }
}

#[test]
fn a_theme_chosen_in_the_themes_tab_applies_at_once_and_survives_apply() {
    let dir = Dir::new("tab");
    let mut s = dir.shell();
    open_themes_tab(&mut s);
    let p = tab_row(&s, "synthwave");
    click(&mut s, p);
    assert_eq!(
        s.core().view().theme(),
        &Theme::synthwave(),
        "live, dialog open"
    );
    assert_eq!(
        load_workflow(&dir.0.join(WORKFLOW_FILE)).value.theme_id,
        "synthwave"
    );
    // Apply keeps it (the drafts followed the click).
    key(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.dialog_open());
    assert_eq!(s.theme(), Theme::synthwave());
    assert_eq!(s.active_theme_id(), "synthwave");
    // Light from the list is the Light choice.
    open_themes_tab(&mut s);
    let p = tab_row(&s, "light");
    click(&mut s, p);
    assert_eq!(s.theme(), Theme::light());
    key(&mut s, Key::Enter, Modifiers::NONE);
    let saved = load_workflow(&dir.0.join(WORKFLOW_FILE)).value;
    assert_eq!(
        (saved.theme, saved.theme_id.as_str()),
        (ThemeChoice::Light, "")
    );
}

#[test]
fn duplicate_rename_and_delete_work_on_files_and_only_on_user_themes() {
    let dir = Dir::new("files");
    let mut s = dir.shell();
    open_themes_tab(&mut s);
    // Duplicate the built-in synthwave theme.
    let p = tab_row(&s, "synthwave");
    click(&mut s, p);
    let p = tab_button(&s, Button::Duplicate);
    click(&mut s, p);
    assert_eq!(files(&dir), ["synthwave-copy.json"]);
    let tab = s.dialog().unwrap().themes_tab().unwrap();
    assert_eq!(
        tab.selected().unwrap().id,
        "user:synthwave-copy",
        "the copy is selected"
    );
    assert_eq!(tab.rows().len(), 4);

    // Rename it: type in the row, Enter.
    let p = tab_button(&s, Button::Rename);
    click(&mut s, p);
    key(&mut s, Key::Char('a'), ctrl());
    type_text(&mut s, "Night drive");
    key(&mut s, Key::Enter, Modifiers::NONE);
    let tab = s.dialog().unwrap().themes_tab().unwrap();
    assert_eq!(tab.selected().unwrap().name, "Night drive");
    // A built-in name is taken.
    let p = tab_button(&s, Button::Rename);
    click(&mut s, p);
    key(&mut s, Key::Char('a'), ctrl());
    type_text(&mut s, "dark");
    key(&mut s, Key::Enter, Modifiers::NONE);
    let tab = s.dialog().unwrap().themes_tab().unwrap();
    assert_eq!(tab.selected().unwrap().name, "Night drive");

    // The built-in themes cannot be deleted.
    let p = tab_row(&s, "dark");
    click(&mut s, p);
    let p = tab_button(&s, Button::Delete);
    click(&mut s, p);
    assert!(!s.dialog().unwrap().themes_tab().unwrap().is_confirming());
    assert_eq!(s.themes().entries().len(), 4);

    // Use the user theme, then delete it: the choice falls back.
    let p = tab_row(&s, "user:synthwave-copy");
    click(&mut s, p);
    assert_eq!(s.active_theme_id(), "user:synthwave-copy");
    let p = tab_button(&s, Button::Delete);
    click(&mut s, p);
    assert!(s.dialog().unwrap().themes_tab().unwrap().is_confirming());
    let p = tab_button(&s, Button::Delete);
    click(&mut s, p);
    assert!(files(&dir).is_empty());
    assert_eq!(s.active_theme_id(), "dark");
    assert_eq!(s.theme(), Theme::dark());
    assert_eq!(s.themes().entries().len(), 3);
    // Apply does not bring the deleted theme back.
    key(&mut s, Key::Enter, Modifiers::NONE);
    assert_eq!(s.active_theme_id(), "dark");
    assert_eq!(load_workflow(&dir.0.join(WORKFLOW_FILE)).value.theme_id, "");
}

#[test]
fn files_that_are_not_themes_are_skipped_with_one_message_and_the_rest_load() {
    let dir = Dir::new("skip");
    std::fs::create_dir_all(dir.themes()).unwrap();
    std::fs::write(dir.themes().join("broken.json"), "{ nope").unwrap();
    std::fs::write(
        dir.themes().join("later.json"),
        r#"{"version": 7, "name": "Later"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.themes().join("fine.json"),
        r##"{"version": 1, "name": "Fine", "colors": {"text": "#010203"}}"##,
    )
    .unwrap();
    let s = dir.shell();
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("broken.json") && message.contains("later.json"),
        "{message}"
    );
    assert!(!message.contains("fine.json"));
    assert_eq!(s.themes().entries().len(), 4);
    // The files are left alone.
    assert_eq!(files(&dir), ["broken.json", "fine.json", "later.json"]);
}

#[test]
fn a_theme_that_is_gone_falls_back_to_the_choice() {
    let dir = Dir::new("gone");
    std::fs::write(
        dir.0.join(WORKFLOW_FILE),
        r#"{"theme": "light", "theme_id": "user:vanished"}"#,
    )
    .unwrap();
    let s = dir.shell();
    assert_eq!(s.theme(), Theme::light());
    assert_eq!(s.active_theme_id(), "light");
    // An old file with no theme id at all.
    std::fs::write(dir.0.join(WORKFLOW_FILE), r#"{"theme": "dark"}"#).unwrap();
    assert_eq!(dir.shell().theme(), Theme::dark());
}

#[test]
fn export_and_import_go_through_files_and_the_checks() {
    let dir = Dir::new("io");
    let mut s = dir.shell();
    open_themes_tab(&mut s);
    let p = tab_row(&s, "synthwave");
    click(&mut s, p);
    let p = tab_button(&s, Button::Export);
    let requests = click(&mut s, p);
    assert_eq!(
        requests,
        [ShellRequest::PickExportTheme("synthwave.json".into())]
    );
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let file = outside.join("neon.json");
    s.export_theme_to(&file);
    let doc = ThemeDoc::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(doc.resolve(), Theme::synthwave());

    let p = tab_button(&s, Button::Import);
    assert_eq!(click(&mut s, p), [ShellRequest::PickImportTheme]);
    s.import_theme_from(&file);
    assert_eq!(files(&dir), ["synthwave-2.json"]);
    assert_eq!(s.themes().entries().len(), 4);
    // A file that is not a theme is refused, with the reason, and nothing is copied.
    std::fs::write(outside.join("bad.json"), "[]").unwrap();
    s.import_theme_from(&outside.join("bad.json"));
    assert_eq!(files(&dir), ["synthwave-2.json"]);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .starts_with("Not imported")
    );

    let p = tab_button(&s, Button::OpenFolder);
    let requests = click(&mut s, p);
    assert_eq!(requests, [ShellRequest::OpenFolder(dir.themes())]);
}

#[test]
fn without_a_settings_folder_only_the_builtin_themes_exist() {
    let mut s = shell_in(None);
    assert_eq!(s.themes().entries().len(), 3);
    open_themes_tab(&mut s);
    let p = tab_row(&s, "dark");
    click(&mut s, p);
    let p = tab_button(&s, Button::Duplicate);
    click(&mut s, p);
    assert_eq!(s.themes().entries().len(), 3);
    let p = tab_button(&s, Button::OpenFolder);
    assert!(click(&mut s, p).is_empty());
}

fn index_of(key: &str) -> usize {
    TOKENS.iter().position(|t| t.key == key).unwrap()
}

/// Opens the editor on a fresh copy of Synthwave.
fn editing(dir: &Dir) -> Shell<NullBackend> {
    let mut s = dir.shell();
    open_themes_tab(&mut s);
    let p = tab_row(&s, "synthwave");
    click(&mut s, p);
    let p = tab_button(&s, Button::Duplicate);
    click(&mut s, p);
    let p = tab_button(&s, Button::Edit);
    click(&mut s, p);
    assert!(s.theme_editor_open());
    s
}

#[test]
fn the_editor_previews_in_the_whole_interface_and_writes_the_file_when_it_closes() {
    let dir = Dir::new("edit");
    let mut s = editing(&dir);
    // The theme in use is Synthwave (the click that began the copy); the copy shows at once.
    assert_eq!(s.active_theme_id(), "synthwave");
    assert_eq!(s.core().view().theme(), &Theme::synthwave());
    let i = index_of("background");
    let hex = s.theme_editor_view().unwrap().hex_rect(i).unwrap().center();
    click(&mut s, hex);
    key(&mut s, Key::Char('a'), ctrl());
    type_text(&mut s, "#102030");
    assert_eq!(
        rgba_of(s.core().view().theme().background),
        [0x10, 0x20, 0x30, 255]
    );
    assert_eq!(rgba_of(s.theme().background), [0x10, 0x20, 0x30, 255]);
    // Not written yet.
    let on_disk = ThemeDoc::parse(
        &std::fs::read_to_string(dir.themes().join("synthwave-copy.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk.colour("background"), Some([0x14, 0x0c, 0x28, 255]));
    // Ctrl+Z in the editor takes the change back, live.
    key(&mut s, Key::Char('z'), ctrl());
    assert_eq!(
        rgba_of(s.core().view().theme().background),
        [0x14, 0x0c, 0x28, 255]
    );
    key(&mut s, Key::Char('a'), ctrl());
    type_text(&mut s, "#102030");
    // Closing writes the file and goes back to the theme in use.
    key(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.theme_editor_open());
    assert_eq!(s.core().view().theme(), &Theme::synthwave());
    let on_disk = ThemeDoc::parse(
        &std::fs::read_to_string(dir.themes().join("synthwave-copy.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk.colour("background"), Some([0x10, 0x20, 0x30, 255]));
    // The theme is in the list with its new colours, and a restart has it.
    let e = s.themes().get("user:synthwave-copy").unwrap();
    assert_eq!(rgba_of(e.theme().background), [0x10, 0x20, 0x30, 255]);
    assert!(s.dialog_open(), "the settings dialog is still there");
    let mut s2 = dir.shell();
    assert_eq!(s2.active_theme_id(), "synthwave");
    s2.execute(AppCommand::NextTheme);
    assert_eq!(s2.active_theme_id(), "user:synthwave-copy");
    assert_eq!(rgba_of(s2.theme().background), [0x10, 0x20, 0x30, 255]);
}

#[test]
fn a_swatch_opens_the_colour_picker_and_its_colour_goes_to_the_token() {
    let dir = Dir::new("pick");
    let mut s = editing(&dir);
    let i = index_of("background");
    let at = s
        .theme_editor_view()
        .unwrap()
        .swatch_rect(i)
        .unwrap()
        .center();
    click(&mut s, at);
    assert!(s.color_picker_open());
    assert_eq!(s.color_picker_view().unwrap().color(), [0x14, 0x0c, 0x28]);
    {
        let (view, _) = s.color_picker.as_mut().unwrap();
        view.set_color([1, 2, 3]);
    }
    let apply = s.color_picker_view().unwrap().apply_rect().center();
    click(&mut s, apply);
    assert!(!s.color_picker_open());
    assert_eq!(s.theme_editor_view().unwrap().color(i), [1, 2, 3, 255]);
    assert_eq!(rgba_of(s.core().view().theme().background), [1, 2, 3, 255]);
    // Esc in the picker closes only the picker.
    click(&mut s, at);
    assert!(s.color_picker_open());
    key(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.color_picker_open() && s.theme_editor_open());
    // The project is untouched by all of it.
    assert!(!s.core().is_dirty());
}

#[test]
fn a_built_in_theme_cannot_be_edited_and_a_theme_painted_in_the_editor_matches_its_view() {
    let dir = Dir::new("builtin-edit");
    let mut s = dir.shell();
    open_themes_tab(&mut s);
    let p = tab_row(&s, "synthwave");
    click(&mut s, p);
    let p = tab_button(&s, Button::Edit);
    click(&mut s, p);
    assert!(!s.theme_editor_open());
    // A copy edited and not changed leaves its file as it was.
    let p = tab_button(&s, Button::Duplicate);
    click(&mut s, p);
    let before = std::fs::read_to_string(dir.themes().join("synthwave-copy.json")).unwrap();
    let p = tab_button(&s, Button::Edit);
    click(&mut s, p);
    assert!(s.theme_editor_open());
    key(&mut s, Key::Escape, Modifiers::NONE);
    assert_eq!(
        std::fs::read_to_string(dir.themes().join("synthwave-copy.json")).unwrap(),
        before
    );
    assert_eq!(ThemeAction::OpenFolder, ThemeAction::OpenFolder);
}
