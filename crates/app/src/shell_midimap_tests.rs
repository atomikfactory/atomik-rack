//! MIDI learn through the shell: the Mixer menu items, the status line, the right-click menu of
//! a control, the MIDI Mappings panel, preset files, the per-frame control path and the control
//! surface settings. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_core::ChannelMessage;
use atomik_model::{
    Command, DeviceKey, MapMode, MidiMapCommand, MidiSource, MidiTarget, SourceKind,
};
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
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.into())
}

fn add_synth(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key(k)),
        })
        .unwrap();
}

fn mixer_item(s: &Shell<NullBackend>, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()
        .into_iter()
        .find(|m| m.title == "Mixer")
        .and_then(|m| m.items().find(|i| i.command == command).cloned())
        .expect("in the Mixer menu")
}

fn mouse(s: &mut Shell<NullBackend>, p: Point, button: PointerButton) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
    });
}

fn value(s: &Shell<NullBackend>, device: &str, p: &str) -> f32 {
    s.core()
        .doc()
        .param_value(&key(device), p, s.core().registry())
        .unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-midimap-shell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn cc(n: u8, v: u8) -> ChannelMessage {
    ChannelMessage::cc(0, n, v)
}

fn learn_cc(s: &mut Shell<NullBackend>, target: MidiTarget, n: u8) {
    s.core_mut()
        .apply_command(Command::MidiMap(MidiMapCommand::Learn {
            source: MidiSource::any_port(Some(0), SourceKind::Cc(n)),
            target,
        }))
        .unwrap();
}

fn param(device: &str, p: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: p.into(),
    }
}

#[test]
fn the_mixer_menu_has_the_midi_items_and_they_enable_with_the_mappings() {
    let mut s = shell();
    let learn = mixer_item(&s, AppCommand::MidiLearn);
    assert_eq!(learn.label, "MIDI Learn");
    assert_eq!(learn.checked, Some(false));
    assert_eq!(learn.shortcut, None, "no default key");
    assert!(mixer_item(&s, AppCommand::MidiMappings).enabled);
    assert!(mixer_item(&s, AppCommand::ImportMidiPreset).enabled);
    assert!(!mixer_item(&s, AppCommand::ExportMidiPreset).enabled);
    assert!(!mixer_item(&s, AppCommand::ClearMidiMaps).enabled);
    for (c, id) in [
        (AppCommand::MidiLearn, "midi.learn"),
        (AppCommand::MidiMappings, "midi.mappings"),
        (AppCommand::ExportMidiPreset, "midi.export_preset"),
        (AppCommand::ImportMidiPreset, "midi.import_preset"),
        (AppCommand::ClearMidiMaps, "midi.clear_maps"),
    ] {
        assert_eq!(c.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(c));
    }
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("a", "filter.cutoff"), 74);
    assert!(mixer_item(&s, AppCommand::ExportMidiPreset).enabled);
    assert!(mixer_item(&s, AppCommand::ClearMidiMaps).enabled);
    // Switching learn on checks the item and says so in the status bar.
    s.execute(AppCommand::MidiLearn);
    assert_eq!(mixer_item(&s, AppCommand::MidiLearn).checked, Some(true));
    s.poll_status();
    assert!(
        s.status_text().starts_with("MIDI LEARN"),
        "{}",
        s.status_text()
    );
    s.execute(AppCommand::MidiLearn);
    s.poll_status();
    assert!(!s.status_text().contains("MIDI LEARN"));
    // Clear removes the mapping in one step; there is nothing left to clear.
    s.execute(AppCommand::ClearMidiMaps);
    assert!(s.core().doc().midi_maps.is_empty());
    assert!(!mixer_item(&s, AppCommand::ClearMidiMaps).enabled);
}

#[test]
fn a_right_click_on_a_rack_knob_offers_learn_and_forget_midi() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    let origin = s.layout().rack.origin();
    let at = s
        .core()
        .view()
        .control_center(&key("a"), "filter.cutoff")
        .unwrap();
    let window = Point::new(at.x + origin.x, at.y + origin.y);
    mouse(&mut s, window, PointerButton::Secondary);
    let menu = s.context_menu().expect("a menu");
    assert!(menu.item_center("midi.learn_control").is_some());
    assert!(menu.item_center("midi.forget_control").is_some());
    // Forget is greyed (nothing is mapped yet): clicking it does nothing and the menu stays.
    let forget = menu.item_center("midi.forget_control").unwrap();
    mouse(&mut s, forget, PointerButton::Primary);
    assert!(s.core().midi_armed().is_none());
    // Choose Learn MIDI: the control is armed.
    let mut s = shell();
    add_synth(&mut s, "a");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    mouse(&mut s, window, PointerButton::Secondary);
    let learn = s
        .context_menu()
        .unwrap()
        .item_center("midi.learn_control")
        .unwrap();
    mouse(&mut s, learn, PointerButton::Primary);
    assert!(s.context_menu().is_none());
    assert_eq!(s.core().midi_armed(), Some(&param("a", "filter.cutoff")));
    s.poll_status();
    assert!(
        s.status_text().contains("MIDI LEARN"),
        "{}",
        s.status_text()
    );
    // Move a hardware control: through the frame the shell runs.
    s.core().inject_midi("Pad", ChannelMessage::cc(0, 74, 100));
    s.frame_at(1.0);
    assert_eq!(s.core().doc().midi_maps.len(), 1);
    // Now Forget MIDI is enabled and removes it.
    mouse(&mut s, window, PointerButton::Secondary);
    let forget = s
        .context_menu()
        .unwrap()
        .item_center("midi.forget_control")
        .unwrap();
    mouse(&mut s, forget, PointerButton::Primary);
    assert!(s.core().doc().midi_maps.is_empty());
}

#[test]
fn a_mapped_control_moves_the_param_when_the_frame_runs_and_a_pause_ends_the_gesture() {
    let mut s = shell();
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("channel", "fader"), 7);
    let id = s.core().doc().midi_maps[0].id;
    let mut m = s.core().doc().midi_map(id).unwrap().clone();
    m.options.pickup = false;
    assert!(s.core_mut().edit_midi_map(m));
    s.core().inject_midi("Pad", cc(7, 127));
    s.frame_at(1.0);
    assert!((value(&s, "channel", "fader") - 1.0).abs() < 1e-4);
    s.core().inject_midi("Pad", cc(7, 0));
    s.frame_at(1.1);
    assert!(value(&s, "channel", "fader").abs() < 1e-4);
    // The listening is by the frame, not by an event: nothing queued, nothing happens.
    s.frame_at(2.0);
    assert!(value(&s, "channel", "fader").abs() < 1e-4);
}

#[test]
fn the_midi_mappings_panel_lists_edits_and_deletes() {
    let mut s = shell();
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("channel", "fader"), 7);
    learn_cc(&mut s, param("a", "filter.cutoff"), 74);
    assert!(!s.midi_maps_open());
    s.execute(AppCommand::MidiMappings);
    assert!(s.midi_maps_open());
    let view = s.midi_maps_view().unwrap();
    assert_eq!(view.rows().len(), 2);
    assert!(
        view.rows()[0].target.contains("Fader"),
        "{}",
        view.rows()[0].target
    );
    assert!(!view.rows()[0].missing);
    // A click on the mode of the first row cycles it: one undo step.
    let at = view.control_center(0, "mode").unwrap();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert!(matches!(
        s.core().doc().midi_maps[0].options.mode,
        MapMode::Relative(_)
    ));
    assert!(s.core_mut().undo());
    assert_eq!(s.core().doc().midi_maps[0].options.mode, MapMode::Absolute);
    s.frame();
    // The cross deletes the second mapping.
    let at = s
        .midi_maps_view()
        .unwrap()
        .control_center(1, "remove")
        .unwrap();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert_eq!(s.core().doc().midi_maps.len(), 1);
    assert_eq!(
        s.midi_maps_view().unwrap().rows().len(),
        1,
        "the panel follows"
    );
    // A deleted target is shown as missing.
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("a") })
        .unwrap();
    // Esc closes it.
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.midi_maps_open());
}

#[test]
fn learn_on_a_panel_row_closes_the_panel_and_arms_the_target() {
    let mut s = shell();
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("a", "filter.cutoff"), 74);
    s.execute(AppCommand::MidiMappings);
    let at = s
        .midi_maps_view()
        .unwrap()
        .control_center(0, "learn")
        .unwrap();
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert!(!s.midi_maps_open());
    assert_eq!(s.core().midi_armed(), Some(&param("a", "filter.cutoff")));
    s.core().inject_midi("Pad", ChannelMessage::cc(3, 90, 1));
    s.frame_at(1.0);
    assert_eq!(
        s.core().doc().midi_maps[0].source,
        MidiSource::any_port(Some(3), SourceKind::Cc(90))
    );
}

#[test]
fn the_panels_header_asks_for_the_preset_dialogs() {
    let mut s = shell();
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("a", "filter.cutoff"), 74);
    s.execute(AppCommand::MidiMappings);
    let view = s.midi_maps_view().unwrap();
    let (export, import) = (view.export_rect().center(), view.import_rect().center());
    let click = |s: &mut Shell<NullBackend>, p: Point| {
        s.handle(&UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        })
    };
    assert_eq!(
        click(&mut s, export),
        vec![ShellRequest::PickExportMidiPreset]
    );
    assert_eq!(
        click(&mut s, import),
        vec![ShellRequest::PickImportMidiPreset]
    );
    // The menu commands ask the same, and Export says so when there is nothing.
    assert_eq!(
        s.execute(AppCommand::ExportMidiPreset),
        vec![ShellRequest::PickExportMidiPreset]
    );
    s.core_mut().clear_midi_maps();
    assert!(s.execute(AppCommand::ExportMidiPreset).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no MIDI mappings")
    );
}

#[test]
fn preset_files_round_trip_through_the_shell_and_a_second_project() {
    let mut s = shell();
    add_synth(&mut s, "a");
    learn_cc(&mut s, param("a", "filter.cutoff"), 74);
    let path = temp("desk.json");
    s.export_midi_preset_to(&path);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("atomik-midi-preset"));
    s.core_mut().clear_midi_maps();
    s.import_midi_preset_from(&path);
    assert_eq!(s.core().doc().midi_maps.len(), 1);
    assert_eq!(
        s.core().doc().midi_maps[0].target,
        param("a", "filter.cutoff")
    );
    // The preset folder sits beside the settings file; none without one.
    assert!(s.midi_preset_dir().is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_surface_settings_open_a_surface_or_say_why_not() {
    let mut s = shell();
    let with = |driver: Option<&str>, input: Option<&str>| AudioSettings {
        surface: crate::settings::SurfaceSettings {
            driver: driver.map(Into::into),
            input: input.map(Into::into),
            output: None,
        },
        ..AudioSettings::default()
    };
    // No driver: no surface.
    s.apply_settings(with(None, None));
    assert!(s.core().surface().is_none());
    // An unknown driver is said so.
    s.apply_settings(with(Some("bogus"), Some("Whatever")));
    assert!(s.core().surface().is_none());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("unknown control surface")
    );
    // A driver whose port is not there is not set up, and the problem is shown.
    s.apply_settings(with(Some("mcu"), Some("No Such Surface Port")));
    assert!(s.core().surface().is_none());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .starts_with("surface input")
    );
    // Applying the same settings again does not try again (no second message).
    s.core_mut().report("cleared");
    s.apply_settings(with(Some("mcu"), Some("No Such Surface Port")));
    assert_eq!(s.core().last_rejection(), Some("cleared"));
}
