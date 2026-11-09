//! The controller wizard in the shell: Settings > Controllers > Set Up Controller..., learning
//! from a port, saving the profile and starting the controller, sharing a profile.

use super::*;
use crate::controller_wizard_view::Target;
use crate::session::NullBackend;
use crate::settings_dialog::{Choice, Tab};
use crate::surface::LoopbackSurface;
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

struct Folder(PathBuf);

impl Folder {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "atomik-shell-controller-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell(d: &Folder) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(d.0.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(d.0.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 900.0)));
    s
}

fn click_at(s: &mut Shell<NullBackend>, at: Point) -> Vec<ShellRequest> {
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

fn click(s: &mut Shell<NullBackend>, target: Target) {
    let size = s.size();
    let at = s
        .controller_wizard()
        .expect("the wizard is open")
        .center_of(size, target)
        .unwrap_or_else(|| panic!("{target:?} is on the screen"));
    click_at(s, at);
}

fn sweep(s: &mut Shell<NullBackend>, pad: &LoopbackSurface, number: u8) {
    for v in (0..=127u8).step_by(9) {
        pad.feed(&[0xB0, number, v]);
    }
    s.frame_at(1.0);
}

/// Learns one knob on a loopback port and saves.
fn learn_one(s: &mut Shell<NullBackend>, port: &str) -> LoopbackSurface {
    let pad = LoopbackSurface::new();
    s.begin_controller_wizard_on(Box::new(pad.clone()), port, vec![format!("{port} out")]);
    sweep(s, &pad, 21);
    assert_eq!(s.controller_wizard().unwrap().wizard().unwrap().count(), 1);
    click(s, Target::Finish);
    // The controller has an output port: the knob question.
    s.handle(&UiEvent::Key {
        key: Key::Char('n'),
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    click(s, Target::Save);
    pad
}

fn surfaces(s: &Shell<NullBackend>) -> Vec<SurfaceSettings> {
    s.core().sink().settings().surfaces.clone()
}

#[test]
fn the_controllers_tab_offers_the_wizard_and_a_click_opens_it() {
    let d = Folder::new("open");
    let mut s = shell(&d);
    s.open_settings(Some(Tab::Surfaces));
    let size = s.size();
    let dialog = s.dialog().unwrap();
    let options = dialog.layout(size).options;
    let labels: Vec<&str> = options.iter().map(|(_, l, _)| l.as_str()).collect();
    assert!(labels.contains(&"Set Up Controller..."), "{labels:?}");
    assert!(labels.contains(&"Import Profile..."));
    assert!(
        !labels.contains(&"Export Profile..."),
        "no profile to share yet"
    );
    let (rect, ..) = options
        .iter()
        .find(|(_, _, c)| *c == Choice::SetUpController)
        .unwrap();
    assert!(s.controller_wizard().is_none());
    click_at(&mut s, rect.center());
    assert!(s.controller_wizard().is_some() && s.modal_open());
    assert!(s.dialog().is_some(), "the settings stay open under it");
    // Escape closes the wizard first, the dialog is still there.
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(s.controller_wizard().is_none() && s.dialog().is_some());
}

#[test]
fn a_controller_is_learned_saved_and_started_as_a_surface() {
    let d = Folder::new("save");
    let mut s = shell(&d);
    learn_one(&mut s, "Desk In");
    assert!(s.controller_wizard().is_none());
    let dir = crate::controller_profile::profile_dir(Some(&d.0.join("settings.json"))).unwrap();
    let saved = crate::controller_profile::load(&dir, "desk-in").unwrap();
    assert_eq!(saved.input, "Desk In");
    assert_eq!(saved.controls.len(), 1);
    assert_eq!(
        saved.controls[0].role,
        crate::controller_profile::Role::Knob(1)
    );
    // It runs as a surface of its own, remembered by its port.
    let list = surfaces(&s);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Desk In");
    assert_eq!(list[0].driver.as_deref(), Some("profile:desk-in"));
    assert_eq!(list[0].input.as_deref(), Some("Desk In"));
    assert!(list[0].enabled);
    // And the driver of that surface is the profile.
    let driver = s.surface_driver("profile:desk-in").unwrap();
    assert_eq!(driver.name(), "profile");
}

#[test]
fn a_second_controller_is_a_second_surface() {
    let d = Folder::new("two");
    let mut s = shell(&d);
    learn_one(&mut s, "Left Pad");
    learn_one(&mut s, "Right Pad");
    let names: Vec<String> = surfaces(&s).into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["Left Pad", "Right Pad"]);
    // The same port set up again replaces its profile.
    learn_one(&mut s, "Left Pad");
    assert_eq!(surfaces(&s).len(), 2);
}

#[test]
fn a_profile_that_would_take_another_ports_name_is_renamed() {
    let d = Folder::new("clash");
    let mut s = shell(&d);
    learn_one(&mut s, "Pad");
    // "pad" and "Pad!" have the same file name but are other ports.
    learn_one(&mut s, "Pad!");
    let names: Vec<String> = surfaces(&s).into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["Pad", "Pad! 2"]);
}

#[test]
fn saving_with_the_settings_open_adds_the_surface_to_the_draft_only() {
    let d = Folder::new("draft");
    let mut s = shell(&d);
    s.open_settings(Some(Tab::Surfaces));
    learn_one(&mut s, "Desk In");
    assert!(surfaces(&s).is_empty(), "nothing applied until Apply");
    let dialog = s.dialog().unwrap();
    assert_eq!(dialog.edited_profile().as_deref(), Some("desk-in"));
    let labels: Vec<String> = dialog
        .layout(s.size())
        .options
        .into_iter()
        .map(|(_, l, _)| l)
        .collect();
    assert!(labels.contains(&"Export Profile...".to_owned()));
    assert!(labels.contains(&"Controller profile (desk-in)".to_owned()));
}

#[test]
fn unknown_and_damaged_profiles_are_reported_not_started() {
    let d = Folder::new("bad");
    let s = shell(&d);
    assert!(s.surface_driver("mcu").is_ok());
    assert!(s.surface_driver("generic").is_ok());
    assert!(s.surface_driver("profile:missing").is_err());
    assert!(s.surface_driver("nonsense").is_err());
    let dir = d.0.join("controllers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
    let e = s.surface_driver("profile:broken").err().unwrap();
    assert!(e.contains("broken"), "{e}");
}

#[test]
fn a_profile_is_exported_and_imported_to_share() {
    let d = Folder::new("share");
    let mut s = shell(&d);
    s.open_settings(Some(Tab::Surfaces));
    learn_one(&mut s, "Desk In");
    let shared = d.0.join("shared.json");
    s.export_controller_to(&shared);
    let text = std::fs::read_to_string(&shared).unwrap();
    let exported = crate::controller_profile::ControllerProfile::from_json(&text).unwrap();
    assert_eq!(exported.name, "Desk In");

    // Another machine (a fresh settings folder) reads it and gets a surface.
    let other = Folder::new("share-other");
    let mut t = shell(&other);
    t.import_controller_from(&shared);
    let list = surfaces(&t);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].driver.as_deref(), Some("profile:desk-in"));
    assert!(t.surface_driver("profile:desk-in").is_ok());
    // Importing again never overwrites: the copy is named apart.
    t.import_controller_from(&shared);
    let names: Vec<String> = surfaces(&t).into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["Desk In", "Desk In 2"]);
    // A file that is not a profile is refused and changes nothing.
    std::fs::write(d.0.join("junk.json"), "[1]").unwrap();
    t.import_controller_from(&d.0.join("junk.json"));
    assert_eq!(surfaces(&t).len(), 2);
}

#[test]
fn export_without_a_profile_to_share_says_so() {
    let d = Folder::new("noexport");
    let mut s = shell(&d);
    s.export_controller_to(&d.0.join("x.json"));
    assert!(!d.0.join("x.json").exists());
}

#[test]
fn the_wizard_swallows_the_keys_and_the_surfaces_come_back_when_it_closes() {
    let d = Folder::new("modal");
    let mut s = shell(&d);
    s.begin_controller_wizard(vec!["Desk In".to_owned()], Vec::new());
    assert!(s.modal_open());
    // A key does not reach the song while the wizard is open.
    let before = s.core().position();
    s.handle(&UiEvent::Key {
        key: Key::Space,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!(s.core().position(), before);
    assert!(!s.core().is_playing());
    // A painted frame includes the box.
    let mut dl = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    assert!(dl.to_json().contains("Set Up Controller"));
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.modal_open());
}
