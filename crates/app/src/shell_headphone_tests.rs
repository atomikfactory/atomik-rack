//! `Shell` and the Headphone Correction panel: the IMPORT button and the empty graph ask for the
//! file dialog of the device they belong to, the REMOVE button removes its data in one undo step,
//! and the panel's meter indices are the device's.

use super::*;
use crate::app_core::default_registry;
use crate::session::NullBackend;
use atomik_model::empty_doc;

const TYPE: &str = "atomik.headphone-correction";
const RESPONSE: &str = "freq, dB\n20, 2\n1000, 0\n20000, -4\n";

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let core = AppCore::with_registry(session, default_registry(), empty_doc(), None);
    let mut s = Shell::new(
        core,
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
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn add(s: &mut Shell<NullBackend>, type_id: &str, k: &str) -> DeviceKey {
    let key = DeviceKey(k.to_string());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.to_string(),
            key: Some(key.clone()),
        })
        .unwrap();
    key
}

#[test]
fn the_panel_buttons_become_the_dialog_request_and_the_removal() {
    let mut s = shell();
    let hp = add(&mut s, TYPE, "hp");
    // IMPORT (or a click on the empty graph) asks for the file dialog of that device.
    s.pane_actions(&[UiAction::HeadphoneImport(hp.clone())]);
    assert_eq!(
        s.handle(&UiEvent::PointerLeave),
        vec![ShellRequest::PickImportHeadphone { device: hp.clone() }]
    );
    assert!(s.handle(&UiEvent::PointerLeave).is_empty(), "once");
    // The dialog's file goes into that device, as one undo step.
    assert!(
        s.core_mut()
            .import_headphone_text(&hp, RESPONSE, "cans.csv")
    );
    assert_eq!(s.core().headphone_data_of(&hp), Some(true));
    // REMOVE takes it out again, and undo brings it back.
    s.pane_actions(&[UiAction::HeadphoneRemove(hp.clone())]);
    assert_eq!(s.core().headphone_data_of(&hp), Some(false));
    s.core_mut().undo();
    assert_eq!(s.core().headphone_data_of(&hp), Some(true));
}

#[test]
fn the_buttons_of_a_device_that_is_not_a_headphone_correction_do_nothing() {
    let mut s = shell();
    let other = add(&mut s, "atomik.poly-synth", "synth");
    s.pane_actions(&[
        UiAction::HeadphoneImport(other.clone()),
        UiAction::HeadphoneRemove(other),
    ]);
    assert!(s.handle(&UiEvent::PointerLeave).is_empty());
}

#[test]
fn the_panels_meters_are_the_devices() {
    use atomik_devices::headphone_correction as dev;
    assert_eq!(atomik_ui::headphone_panel::METER_LOADED, dev::meter::LOADED);
    assert_eq!(atomik_ui::headphone_panel::METER_PREAMP, dev::meter::PREAMP);
    assert_eq!(atomik_ui::headphone_panel::METER_BOOST, dev::meter::BOOST);
    assert_eq!(atomik_ui::headphone_panel::METER_BUSY, dev::meter::BUSY);
    assert_eq!(
        atomik_ui::fx::FxKind::HeadphoneCorrection.meter_count(),
        dev::METERS
    );
}
