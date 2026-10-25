//! `Shell` tests for the loop slicer: the slice editor in the lower pane, file
//! drops onto the rack panel and the editor, the Load button and the View menu.

use super::*;
use crate::session::NullBackend;
use atomik_audio_file::AudioData;
use atomik_ui::{Key, Modifiers, PointerButton};

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
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(s: &mut Shell<NullBackend>, type_id: &str, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: 0,
            type_id: type_id.into(),
            key: Some(key(k)),
        })
        .unwrap();
}

fn shell_with_slicer() -> Shell<NullBackend> {
    let mut s = shell();
    add(&mut s, "atomik.loop-slicer", "slc");
    s
}

/// A mono loop of `beats` beats at 120 BPM with a click on every beat.
fn loop_wav(name: &str, beats: usize) -> PathBuf {
    // One folder per call: tests run in parallel in one process, and two of them writing and
    // decoding the same `beat4.wav` at once made a drop read a half-written file (a flake that
    // showed up as 0 zones instead of 4 under load).
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "atomik-app-shell-slicer-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut data = vec![0.0f32; beats * 24_000];
    for beat in 0..beats {
        for i in 0..2_400usize {
            let sign = if (i / 20).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            data[beat * 24_000 + i] = 0.8 * (1.0 - i as f32 / 2_400.0) * sign;
        }
    }
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn zone_count(s: &Shell<NullBackend>) -> usize {
    s.core().doc().device(&key("slc")).unwrap().zones.len()
}

fn click(s: &mut Shell<NullBackend>, pos: Point) -> Vec<ShellRequest> {
    let mut requests = s.handle(&UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    requests.extend(s.handle(&UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    requests
}

/// A window point on the slicer's rack panel (its left ear).
fn panel_point(s: &Shell<NullBackend>) -> Point {
    let r = s.core().view().device_rect(&key("slc")).unwrap();
    let p = s
        .core()
        .view()
        .rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
    Point::new(p.x, p.y + s.layout().rack.y0)
}

#[test]
fn view_slice_editor_opens_on_the_first_slicer_and_closes_again() {
    let mut s = shell();
    s.execute(AppCommand::ToggleSliceEditor);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no loop slicer")
    );
    assert_eq!(s.lower_pane(), Pane::Arrangement);

    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleSliceEditor);
    assert_eq!(s.core().slice_editor_device(), Some(&key("slc")));
    assert_eq!(s.lower_pane(), Pane::SliceEditor);
    assert_eq!(s.focus(), Pane::SliceEditor);
    assert!(s.menu_state().slice_editor);
    let song = s.layout().song.unwrap();
    assert_eq!(
        s.core().slice_editor().bounds(),
        song,
        "the arrangement's bounds"
    );
    assert_eq!(s.hit_pane(song.center()), Some(Pane::SliceEditor));
    s.execute(AppCommand::ToggleSliceEditor);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().slice_editor);
}

#[test]
fn view_slice_editor_prefers_the_selected_slicer() {
    let mut s = shell_with_slicer();
    add(&mut s, "atomik.loop-slicer", "slc2");
    s.core_mut().select(Some(key("slc2")));
    s.execute(AppCommand::ToggleSliceEditor);
    assert_eq!(s.core().slice_editor_device(), Some(&key("slc2")));
}

#[test]
fn the_view_menu_lists_the_slice_editor() {
    let s = shell_with_slicer();
    let model = s.menu_model();
    let text = format!("{model:?}");
    assert!(text.contains("Slice Editor"), "{text}");
    assert!(text.contains("ToggleSliceEditor"), "{text}");
}

#[test]
fn the_three_editors_exclude_each_other_through_the_menu() {
    let mut s = shell_with_slicer();
    add(&mut s, "atomik.sampler", "smp");
    add(&mut s, "atomik.drum-machine", "drm");
    s.execute(AppCommand::ToggleSliceEditor);
    s.execute(AppCommand::ToggleSampleEditor);
    assert_eq!(s.lower_pane(), Pane::SampleEditor);
    assert!(s.core().slice_editor_device().is_none());
    s.execute(AppCommand::ToggleSliceEditor);
    assert_eq!(s.lower_pane(), Pane::SliceEditor);
    assert!(s.core().sample_editor_device().is_none());
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    assert!(s.core().slice_editor_device().is_none());
    assert!(!s.menu_state().slice_editor);
}

#[test]
fn the_edit_action_shows_the_editor_even_over_the_console() {
    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    let open = UiAction::OpenSliceEditor(key("slc"));
    s.core_mut().handle_action(open.clone());
    s.pane_actions(&[open]);
    assert_eq!(s.lower_pane(), Pane::SliceEditor);
    assert_eq!(s.focus(), Pane::SliceEditor);
    assert!(!s.menu_state().console);
}

#[test]
fn esc_closes_the_editor_and_a_removed_slicer_closes_it_too() {
    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleSliceEditor);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert_eq!(s.focus(), Pane::Arrangement);
    s.execute(AppCommand::ToggleSliceEditor);
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("slc") })
        .unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn pointer_events_go_to_the_editor_while_it_shows() {
    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleSliceEditor);
    // The close button works through the shell.
    let close = s.core().slice_editor().close_button_center();
    click(&mut s, close);
    assert!(s.core().slice_editor_device().is_none());
}

#[test]
fn a_drop_on_the_slicer_panel_loads_and_slices_the_loop_and_not_a_track() {
    let mut s = shell_with_slicer();
    let at = panel_point(&s);
    let paths = vec![loop_wav("beat4.wav", 4)];
    s.handle(&UiEvent::FileHover {
        pos: at,
        paths: paths.clone(),
    });
    assert_eq!(s.core().view().drop_hover(), Some(&key("slc")));
    s.handle(&UiEvent::FileDrop { pos: at, paths });
    s.core_mut().wait_background();
    assert_eq!(zone_count(&s), 4);
    assert!(
        s.core().doc().song.tracks.is_empty(),
        "no audio tracks made"
    );
}

#[test]
fn a_drop_on_the_editor_replaces_the_loop_with_the_first_file_only() {
    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleSliceEditor);
    let over = s.core().slice_editor().waveform_rect().center();
    let paths = vec![loop_wav("beat4.wav", 4), loop_wav("beat6.wav", 6)];
    s.handle(&UiEvent::FileHover {
        pos: over,
        paths: paths.clone(),
    });
    assert!(s.core().slice_editor().drop_hover().is_some());
    s.handle(&UiEvent::FileDrop { pos: over, paths });
    s.core_mut().wait_background();
    assert_eq!(zone_count(&s), 4, "the first file only");
    assert_eq!(s.core().doc().assets.len(), 1);
    assert!(s.core().slice_editor().drop_hover().is_none());
    // Over the transport bar the indicator goes away.
    s.handle(&UiEvent::FileHover {
        pos: over,
        paths: vec![loop_wav("beat4.wav", 4)],
    });
    s.handle(&UiEvent::FileHover {
        pos: s.layout().transport.center(),
        paths: vec![loop_wav("beat4.wav", 4)],
    });
    assert!(s.core().slice_editor().drop_hover().is_none());
}

#[test]
fn the_load_button_asks_for_a_dialog_and_the_picked_loop_is_sliced() {
    let mut s = shell_with_slicer();
    s.execute(AppCommand::ToggleSliceEditor);
    let p = s.core().slice_editor().empty_load_center();
    let requests = click(&mut s, p);
    assert_eq!(requests, vec![ShellRequest::PickLoadSamples]);
    s.load_samples(vec![loop_wav("beat4.wav", 4)]);
    s.core_mut().wait_background();
    assert_eq!(zone_count(&s), 4);
    // The dialog result is used once.
    s.load_samples(vec![loop_wav("beat6.wav", 6)]);
    s.core_mut().wait_background();
    assert_eq!(zone_count(&s), 4);
}
