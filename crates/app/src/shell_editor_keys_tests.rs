//! The keys of the sample, slice and step editors end to end in the shell: Delete is the
//! registry's `edit.delete` (so it follows the active profile and a user rebinding), undo and
//! redo come from `edit.undo` / `edit.redo` and not from keys the views read themselves, and
//! the keys no command takes (Esc, text) still reach the views.

use super::*;
use crate::keymap_store::KEYMAP_FILE;
use crate::session::NullBackend;
use atomik_audio_file::AudioData;
use atomik_ui::keymap::script::event_for;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const OS: Os = Os::Windows;

fn dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-editor-keys-{}-{tag}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A shell on `profile`; `user` is the text of a `keymap.json` next to the settings.
fn shell_with(tag: &str, profile: Profile, user: Option<&str>) -> Shell<NullBackend> {
    let d = dir(tag);
    if let Some(text) = user {
        std::fs::write(d.join(KEYMAP_FILE), text).unwrap();
    }
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(OS),
            profile: Some(profile),
            in_window_menu: true,
            settings_path: Some(d.join("settings.json")),
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    let (k, modifiers) = event_for(OS, chord).unwrap();
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: k.clone(),
            modifiers,
            pressed,
        });
    }
}

fn add(s: &mut Shell<NullBackend>, type_id: &str, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        })
        .unwrap();
}

fn wav(name: &str, frames: usize) -> PathBuf {
    let path = dir("wav").join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn zones(s: &Shell<NullBackend>, device: &str) -> usize {
    s.core().doc().device(&key(device)).unwrap().zones.len()
}

/// A shell with the sample editor open on a sampler holding three zones.
fn sampler(tag: &str, profile: Profile, user: Option<&str>) -> Shell<NullBackend> {
    let mut s = shell_with(tag, profile, user);
    add(&mut s, "atomik.sampler", "smp");
    let paths = vec![
        wav("a_C3.wav", 2_400),
        wav("b_E3.wav", 2_400),
        wav("c_G3.wav", 2_400),
    ];
    s.core_mut().handle_action(UiAction::ImportSamples {
        paths,
        device: key("smp"),
        at_key: None,
    });
    s.core_mut().wait_background();
    assert_eq!(zones(&s, "smp"), 3);
    s.execute(AppCommand::ToggleSampleEditor);
    assert_eq!(s.focus(), Pane::SampleEditor);
    s
}

/// A shell with the slice editor open on a slicer holding a four-beat loop, slice 3 selected.
fn slicer(tag: &str, profile: Profile, user: Option<&str>) -> Shell<NullBackend> {
    let mut s = shell_with(tag, profile, user);
    add(&mut s, "atomik.loop-slicer", "slc");
    let mut data = vec![0.0f32; 4 * 24_000];
    for beat in 0..4 {
        for i in 0..2_400usize {
            data[beat * 24_000 + i] = 0.8 * (1.0 - i as f32 / 2_400.0);
        }
    }
    let path = dir("loop").join("beat4.wav");
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    s.core_mut().handle_action(UiAction::ImportSamples {
        paths: vec![path],
        device: key("slc"),
        at_key: None,
    });
    s.core_mut().wait_background();
    assert_eq!(zones(&s, "slc"), 4);
    s.execute(AppCommand::ToggleSliceEditor);
    assert_eq!(s.focus(), Pane::SliceEditor);
    s.core_mut().slice_editor_mut().set_selected_slice(Some(3));
    s
}

const MOVE_DELETE_TO_X: &str =
    r#"{"bindings": [{"command": "edit.delete", "context": "global", "chords": ["X"]}]}"#;
const MOVE_UNDO: &str =
    r#"{"bindings": [{"command": "edit.undo", "context": "global", "chords": ["Cmd+Alt+U"]}]}"#;

#[test]
fn delete_and_backspace_remove_the_selected_zone_in_both_profiles() {
    for profile in [Profile::Standard, Profile::Legacy] {
        let mut s = sampler("sample-del", profile, None);
        assert_eq!(s.core().sample_editor().selected_zone(), Some(0));
        tap(&mut s, "Delete");
        assert_eq!(zones(&s, "smp"), 2, "{profile:?}");
        tap(&mut s, "Backspace");
        assert_eq!(zones(&s, "smp"), 1, "{profile:?}");
        assert_eq!(s.focus(), Pane::SampleEditor);
    }
}

#[test]
fn delete_and_backspace_merge_the_selected_slice_in_both_profiles() {
    for profile in [Profile::Standard, Profile::Legacy] {
        let mut s = slicer("slice-del", profile, None);
        tap(&mut s, "Delete");
        assert_eq!(zones(&s, "slc"), 3, "{profile:?}");
        assert_eq!(s.core().slice_editor().selected_slice(), Some(2));
        tap(&mut s, "Backspace");
        assert_eq!(zones(&s, "slc"), 2, "{profile:?}");
        // The first slice has no marker: the key does nothing and nothing complains.
        s.core_mut().slice_editor_mut().set_selected_slice(Some(0));
        tap(&mut s, "Delete");
        assert_eq!(zones(&s, "slc"), 2);
    }
}

#[test]
fn a_user_rebinding_of_delete_moves_the_effect_in_the_sample_editor() {
    let mut s = sampler("sample-rebind", Profile::Standard, Some(MOVE_DELETE_TO_X));
    tap(&mut s, "Delete");
    tap(&mut s, "Backspace");
    assert_eq!(zones(&s, "smp"), 3, "the old keys no longer delete");
    tap(&mut s, "X");
    assert_eq!(zones(&s, "smp"), 2);
}

#[test]
fn a_user_rebinding_of_delete_moves_the_effect_in_the_slice_editor() {
    let mut s = slicer("slice-rebind", Profile::Standard, Some(MOVE_DELETE_TO_X));
    tap(&mut s, "Delete");
    tap(&mut s, "Backspace");
    assert_eq!(zones(&s, "slc"), 4, "the old keys no longer delete");
    tap(&mut s, "X");
    assert_eq!(zones(&s, "slc"), 3);
}

#[test]
fn a_delete_the_view_cannot_take_is_quiet_and_global_keys_still_work() {
    let mut s = sampler("sample-quiet", Profile::Standard, None);
    for _ in 0..3 {
        tap(&mut s, "Delete");
    }
    assert_eq!(zones(&s, "smp"), 0);
    tap(&mut s, "Delete");
    assert_eq!(zones(&s, "smp"), 0);
    assert!(!s.core().is_playing());
    // Space is global: the editor does not take it.
    tap(&mut s, "Space");
    assert!(s.core().is_playing());
    // Esc is still the view's own: it closes the editor.
    tap(&mut s, "Escape");
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn undo_and_redo_in_the_editors_are_the_registry_commands() {
    let mut s = sampler("undo-sample", Profile::Standard, None);
    tap(&mut s, "Delete");
    assert_eq!(zones(&s, "smp"), 2);
    tap(&mut s, "Cmd+Z");
    assert_eq!(zones(&s, "smp"), 3);
    tap(&mut s, "Cmd+Y");
    assert_eq!(zones(&s, "smp"), 2);
    let mut s = slicer("undo-slice", Profile::Standard, None);
    tap(&mut s, "Delete");
    assert_eq!(zones(&s, "slc"), 3);
    tap(&mut s, "Cmd+Z");
    assert_eq!(zones(&s, "slc"), 4);
    tap(&mut s, "Cmd+Shift+Z");
    assert_eq!(zones(&s, "slc"), 3);
}

#[test]
fn rebinding_undo_changes_what_the_editors_do_with_the_old_chord() {
    let mut s = sampler("rebind-undo-sample", Profile::Standard, Some(MOVE_UNDO));
    tap(&mut s, "Delete");
    assert_eq!(zones(&s, "smp"), 2);
    tap(&mut s, "Cmd+Z");
    assert_eq!(
        zones(&s, "smp"),
        2,
        "Ctrl+Z is no longer undo in the editor"
    );
    tap(&mut s, "Cmd+Alt+U");
    assert_eq!(zones(&s, "smp"), 3);
    let mut s = slicer("rebind-undo-slice", Profile::Standard, Some(MOVE_UNDO));
    tap(&mut s, "Delete");
    assert_eq!(zones(&s, "slc"), 3);
    tap(&mut s, "Cmd+Z");
    assert_eq!(zones(&s, "slc"), 3);
    tap(&mut s, "Cmd+Alt+U");
    assert_eq!(zones(&s, "slc"), 4);
}

#[test]
fn rebinding_undo_changes_the_step_editor_too() {
    let mut s = shell_with("rebind-undo-step", Profile::Standard, Some(MOVE_UNDO));
    add(&mut s, "atomik.drum-machine", "drm");
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(s.focus(), Pane::StepEditor);
    tap(&mut s, "Cmd+Z");
    assert!(s.core().doc().device(&key("drm")).is_some());
    tap(&mut s, "Cmd+Alt+U");
    assert!(
        s.core().doc().device(&key("drm")).is_none(),
        "the rebound chord undid the device add"
    );
}
