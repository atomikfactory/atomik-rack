//! `Shell` tests for the sampler: the sample editor in the lower pane, file drops
//! onto the rack panel and the editor, and the Load button.

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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn shell_with_sampler() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: 0,
            type_id: "atomik.sampler".into(),
            key: Some(key("smp")),
        })
        .unwrap();
    s
}

fn wav(name: &str, frames: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-sampler-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn zone_roots(s: &Shell<NullBackend>) -> Vec<u8> {
    s.core()
        .doc()
        .device(&key("smp"))
        .unwrap()
        .zones
        .iter()
        .map(|z| z.root_key)
        .collect()
}

/// A window point on the sampler's rack panel (its left ear).
fn panel_point(s: &Shell<NullBackend>) -> Point {
    let r = s.core().view().device_rect(&key("smp")).unwrap();
    let p = s
        .core()
        .view()
        .rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
    Point::new(p.x, p.y + s.layout().rack.y0)
}

#[test]
fn view_sample_editor_opens_on_the_first_sampler_and_closes_again() {
    let mut s = shell();
    // Without a sampler it says so.
    s.execute(AppCommand::ToggleSampleEditor);
    assert!(s.core().last_rejection().unwrap().contains("no sampler"));
    assert_eq!(s.lower_pane(), Pane::Arrangement);

    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    assert_eq!(s.core().sample_editor_device(), Some(&key("smp")));
    assert_eq!(s.lower_pane(), Pane::SampleEditor);
    assert_eq!(s.focus(), Pane::SampleEditor);
    assert!(s.menu_state().sample_editor);
    let song = s.layout().song.unwrap();
    assert_eq!(
        s.core().sample_editor().bounds(),
        song,
        "same bounds as the arrangement"
    );
    assert_eq!(s.hit_pane(song.center()), Some(Pane::SampleEditor));
    s.execute(AppCommand::ToggleSampleEditor);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().sample_editor);
}

#[test]
fn the_edit_action_shows_the_editor_even_over_the_console() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    let open = UiAction::OpenSampleEditor(key("smp"));
    s.core_mut().handle_action(open.clone());
    s.pane_actions(&[open]);
    assert_eq!(s.lower_pane(), Pane::SampleEditor);
    assert_eq!(s.focus(), Pane::SampleEditor);
    assert!(!s.menu_state().console);
}

#[test]
fn esc_closes_the_editor_and_a_removed_sampler_closes_it_too() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    s.execute(AppCommand::ToggleSampleEditor);
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("smp") })
        .unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn pointer_events_go_to_the_editor_while_it_shows() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    let close = s.core().sample_editor().close_button_center();
    for event in [
        UiEvent::PointerDown {
            pos: close,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: close,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    ] {
        s.handle(&event);
    }
    assert!(
        s.core().sample_editor_device().is_none(),
        "the close button works"
    );
}

#[test]
fn a_drop_on_the_sampler_panel_imports_zones_and_not_tracks() {
    let mut s = shell_with_sampler();
    let at = panel_point(&s);
    let paths = vec![wav("tone_C3.wav", 2_400), wav("tone_E3.wav", 2_400)];
    s.handle(&UiEvent::FileHover {
        pos: at,
        paths: paths.clone(),
    });
    assert_eq!(s.core().view().drop_hover(), Some(&key("smp")));
    s.handle(&UiEvent::FileDrop { pos: at, paths });
    assert!(s.core().view().drop_hover().is_none());
    s.core_mut().wait_background();
    assert_eq!(zone_roots(&s), [48, 52]);
    assert!(
        s.core().doc().song.tracks.is_empty(),
        "no audio tracks made"
    );
}

#[test]
fn a_drop_on_a_rack_device_that_is_not_a_sampler_still_makes_tracks() {
    let mut s = shell_with_sampler();
    let nowhere = Point::new(1190.0, s.layout().rack.y0 + 2.0);
    s.handle(&UiEvent::FileDrop {
        pos: nowhere,
        paths: vec![wav("loop.wav", 2_400)],
    });
    s.core_mut().wait_background();
    assert!(zone_roots(&s).is_empty());
    assert_eq!(s.core().doc().song.tracks.len(), 1, "an audio track");
}

#[test]
fn hovering_the_editor_shows_its_indicator_and_leaving_clears_it() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    let over_map = s.core().sample_editor().map_rect().center();
    let paths = vec![wav("x_C3.wav", 100)];
    s.handle(&UiEvent::FileHover {
        pos: over_map,
        paths: paths.clone(),
    });
    assert!(matches!(
        s.core().sample_editor().drop_hover(),
        Some(Some(_))
    ));
    // Over the transport bar the editor's indicator goes away.
    s.handle(&UiEvent::FileHover {
        pos: s.layout().transport.center(),
        paths: paths.clone(),
    });
    assert!(s.core().sample_editor().drop_hover().is_none());
    s.handle(&UiEvent::FileHover {
        pos: over_map,
        paths,
    });
    s.handle(&UiEvent::FileHoverCancel);
    assert!(s.core().sample_editor().drop_hover().is_none());
}

#[test]
fn a_drop_on_the_key_map_puts_a_single_file_at_that_key() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    let ed = s.core().sample_editor();
    let at = Point::new(ed.key_center_x(80), ed.map_rect().center().y);
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![wav("plain.wav", 2_400)],
    });
    s.core_mut().wait_background();
    assert_eq!(zone_roots(&s), [80]);
    // Elsewhere on the editor (no key): the default C4.
    let elsewhere = s.core().sample_editor().waveform_rect().center();
    s.handle(&UiEvent::FileDrop {
        pos: elsewhere,
        paths: vec![wav("second.wav", 1_200)],
    });
    s.core_mut().wait_background();
    assert_eq!(zone_roots(&s), [80, 60]);
}

#[test]
fn the_load_button_asks_for_a_dialog_and_the_picked_files_become_zones() {
    let mut s = shell_with_sampler();
    s.execute(AppCommand::ToggleSampleEditor);
    let p = s.core().sample_editor().load_button_center();
    let press = |pos| UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let release = |pos| UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    };
    // The button acts on the press (as the editor's other buttons do).
    let mut requests = s.handle(&press(p));
    requests.extend(s.handle(&release(p)));
    assert_eq!(requests, vec![ShellRequest::PickLoadSamples]);
    s.load_samples(vec![wav("picked_G3.wav", 2_400)]);
    s.core_mut().wait_background();
    assert_eq!(zone_roots(&s), [55]);
    // The dialog result is used once.
    s.load_samples(vec![wav("stray.wav", 100)]);
    s.core_mut().wait_background();
    assert_eq!(zone_roots(&s), [55]);
}
