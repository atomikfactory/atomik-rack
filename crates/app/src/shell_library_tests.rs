//! The library folder in the shell: where it comes from (the option, the Workflow tab, the
//! default), the Choose and Default buttons of the dialog, and what the file browser is told.
//! Every test uses its own temporary config directory.

use super::*;
use crate::keymap_store::WORKFLOW_FILE;
use crate::session::NullBackend;
use crate::settings_dialog::Tab;
use std::path::PathBuf;

fn config(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-library-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_with(dir: &Path, library_root: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

const VIEW: Size = Size::new(1000.0, 800.0);

fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
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

fn open_workflow(s: &mut Shell<NullBackend>) {
    s.execute(AppCommand::AudioSettings);
    let r = s
        .dialog()
        .unwrap()
        .layout(VIEW)
        .tabs
        .iter()
        .find(|(_, t)| *t == Tab::Workflow)
        .unwrap()
        .0;
    click(s, r.center());
}

fn press_apply(s: &mut Shell<NullBackend>) {
    let p = s.dialog().unwrap().buttons(VIEW).0.center();
    click(s, p);
}

#[test]
fn library_root_without_choice_or_default_is_none() {
    let s = shell_with(&config("none"), None);
    assert_eq!(s.library_root(), None);
    assert_eq!(s.core().library_root(), None);
}

#[test]
fn library_root_order_is_option_then_workflow_then_default() {
    let dir = config("order");
    let default = PathBuf::from("D:/Docs/Atomik Rack/Library");
    let chosen = PathBuf::from("D:/Sounds/Library");
    let forced = PathBuf::from("D:/Tests/Library");
    let mut s = shell_with(&dir, None);
    s.set_library_default(Some(default.clone()));
    assert_eq!(s.library_root(), Some(default.clone()));
    assert_eq!(s.core().library_root(), Some(default.clone()));
    // A saved choice beats the default.
    std::fs::write(
        dir.join(WORKFLOW_FILE),
        format!("{{\"library_folder\": {:?}}}", chosen.to_string_lossy()),
    )
    .unwrap();
    let mut t = shell_with(&dir, None);
    t.set_library_default(Some(default));
    assert_eq!(t.library_root(), Some(chosen.clone()));
    // The option beats both.
    let mut u = shell_with(&dir, Some(forced.clone()));
    u.set_library_default(Some(PathBuf::from("D:/Other")));
    assert_eq!(u.library_root(), Some(forced.clone()));
    assert_eq!(u.core().library_root(), Some(forced));
}

#[test]
fn library_dialog_choose_asks_the_platform_and_apply_saves_the_folder() {
    let dir = config("choose");
    let mut s = shell_with(&dir, None);
    open_workflow(&mut s);
    let l = s.dialog().unwrap().workflow_layout(VIEW).unwrap();
    assert_eq!(
        click(&mut s, l.library.choose.center()),
        vec![ShellRequest::PickLibraryFolder]
    );
    let picked = PathBuf::from("D:/Sounds/Library");
    s.set_library_folder(picked.clone());
    assert_eq!(s.library_root(), None, "nothing applies before Apply");
    press_apply(&mut s);
    assert_eq!(s.library_root(), Some(picked.clone()));
    assert_eq!(s.core().library_root(), Some(picked.clone()));
    assert_eq!(s.workflow().library_folder, Some(picked.clone()));
    let saved = std::fs::read_to_string(dir.join(WORKFLOW_FILE)).unwrap();
    assert!(saved.contains("library_folder"), "{saved}");
    // Default goes back.
    open_workflow(&mut s);
    let l = s.dialog().unwrap().workflow_layout(VIEW).unwrap();
    click(&mut s, l.library.default.center());
    press_apply(&mut s);
    assert_eq!(s.library_root(), None);
    assert!(
        !std::fs::read_to_string(dir.join(WORKFLOW_FILE))
            .unwrap()
            .contains("library_folder")
    );
}

#[test]
fn library_folder_picked_without_a_dialog_applies_and_saves_at_once() {
    let dir = config("direct");
    let mut s = shell_with(&dir, None);
    let picked = PathBuf::from("D:/Sounds/Library");
    s.set_library_folder(picked.clone());
    assert_eq!(s.library_root(), Some(picked));
    assert!(dir.join(WORKFLOW_FILE).exists());
}
