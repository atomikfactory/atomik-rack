//! `Shell` tests for patch browsing: the commands that aim the file browser at a device, Return
//! and Escape from any pane, the previous and next patch keys, the pop-up on the patch name and
//! the folders kept in `ui-state.json`.

use super::*;
use crate::session::NullBackend;
use atomik_model::save_project;
use atomik_ui::keymap::Os;
use atomik_ui::{Modifiers, PatchOp};
use std::path::PathBuf;

const SYNTH: &str = "atomik.poly-synth";

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-shell-patch-{tag}-{}",
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

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn shell_in(dir: &Tmp, ui_state: bool) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: ui_state.then(|| dir.0.join("ui-state.json")),
            library_path: None,
            library_root: Some(dir.0.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn patch_file(dir: &Tmp, name: &str) -> PathBuf {
    dir.0.join(format!("Library/Patches/{name}.atomikpatch"))
}

fn set(s: &mut Shell<NullBackend>, k: &str, cutoff: f32) {
    s.core_mut()
        .apply_command(Command::SetParam {
            key: key(k),
            param: "filter.cutoff".to_string(),
            value: cutoff,
            merge: None,
        })
        .unwrap();
}

fn cutoff(s: &Shell<NullBackend>, k: &str) -> Option<f32> {
    s.core()
        .doc()
        .device(&key(k))
        .unwrap()
        .params
        .get("filter.cutoff")
        .copied()
}

/// Three synths (`src`, `d`, `e`) and the patches `P1..P3` (cutoff 100, 200, 300), `d` selected
/// and the rack focused.
fn setup(dir: &Tmp, ui_state: bool) -> Shell<NullBackend> {
    let mut s = shell_in(dir, ui_state);
    for k in ["src", "d", "e"] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.to_string(),
                key: Some(key(k)),
            })
            .unwrap();
    }
    for i in 1..=3 {
        set(&mut s, "src", 100.0 * i as f32);
        s.core_mut()
            .save_device_patch(&key("src"), &patch_file(dir, &format!("P{i}")));
    }
    s.core_mut().view_mut().set_selection(&[key("d")], None);
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    s
}

fn press(s: &mut Shell<NullBackend>, k: Key) {
    s.handle(&UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

#[test]
fn browse_aims_the_file_browser_and_return_accepts_one_undo_step() {
    let dir = Tmp::new("return");
    let mut s = setup(&dir, false);
    let original = save_project(s.core().doc());
    s.run_command("rack.browse_patch", None);
    assert_eq!(s.core().patch_browse_device(), Some(&key("d")));
    assert!(s.ui_state().file_browser_visible, "the panel opens");
    assert_eq!(s.core().file_browser().patch_target(), Some(SYNTH));
    for _ in 0..3 {
        s.run_command("rack.patch_next", None);
    }
    assert_eq!(cutoff(&s, "d"), Some(300.0));
    press(&mut s, Key::Enter);
    assert!(s.core().patch_browse_device().is_none());
    assert_eq!(cutoff(&s, "d"), Some(300.0));
    assert!(s.core_mut().undo());
    assert_eq!(
        save_project(s.core().doc()),
        original,
        "the whole browse was one step"
    );
}

#[test]
fn escape_from_the_rack_puts_the_device_back() {
    let dir = Tmp::new("escape");
    let mut s = setup(&dir, false);
    let original = save_project(s.core().doc());
    s.run_command("rack.browse_patch", None);
    s.run_command("rack.patch_next", None);
    s.run_command("rack.patch_next", None);
    assert_eq!(cutoff(&s, "d"), Some(200.0));
    press(&mut s, Key::Escape);
    assert!(s.core().patch_browse_device().is_none());
    assert_eq!(save_project(s.core().doc()), original);
    assert_eq!(s.core().file_browser().patch_target(), None);
}

#[test]
fn the_patch_keys_are_bound_in_the_rack_and_the_menu_lists_the_commands() {
    let dir = Tmp::new("keys");
    let s = shell_in(&dir, false);
    let chords = |id: &str| {
        s.keymap()
            .chords_of(id, None)
            .into_iter()
            .map(|(ctx, c)| (ctx, c.menu_text(Os::Windows)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        chords("rack.patch_next"),
        vec![(atomik_ui::keymap::Context::Rack, "Ctrl+Right".to_string())]
    );
    assert_eq!(
        chords("rack.patch_prev"),
        vec![(atomik_ui::keymap::Context::Rack, "Ctrl+Left".to_string())]
    );
    assert_eq!(
        chords("rack.browse_patch"),
        vec![(atomik_ui::keymap::Context::Rack, "Ctrl+Shift+B".to_string())]
    );
}

#[test]
fn a_right_click_on_the_name_opens_the_pop_up_and_a_choice_applies_it() {
    let dir = Tmp::new("popup");
    let mut s = setup(&dir, false);
    // Layout of the rack first, so the strip has a place.
    s.core_mut().after_view_change();
    s.pane_actions(&[UiAction::DevicePatch {
        device: key("d"),
        op: PatchOp::Siblings,
    }]);
    assert!(s.context_menu().is_some(), "the pop-up opens");
    s.context_menu = None;
    let path = patch_file(&dir, "P2");
    s.run_command("rack.patch_pick", Some(&path.to_string_lossy()));
    assert_eq!(cutoff(&s, "d"), Some(200.0));
    assert_eq!(s.core().doc().device(&key("d")).unwrap().patch_name, "P2");
}

#[test]
fn the_folder_browsed_for_a_type_is_kept_in_the_ui_state() {
    let dir = Tmp::new("ui-state");
    let mut s = setup(&dir, true);
    s.run_command("rack.browse_patch", None);
    s.run_command("rack.patch_next", None);
    press(&mut s, Key::Enter);
    let saved = std::fs::read_to_string(dir.0.join("ui-state.json")).unwrap();
    assert!(
        saved.contains("patch_folders") && saved.contains(SYNTH),
        "{saved}"
    );
    assert!(s.ui_state().patch_folders.contains_key(SYNTH));

    // A new session reads it back.
    let again = shell_in(&dir, true);
    assert_eq!(
        again.core().patch_folders().get(SYNTH),
        s.core().patch_folders().get(SYNTH)
    );
    assert!(again.core().patch_folders().contains_key(SYNTH));
}
