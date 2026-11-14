//! The meter panels in the shell: Show Spectrum and Show Phase Meter open a panel on the selected
//! strip, the panel is drawn and answers presses, a press on its cross closes it, a deleted strip
//! closes it, and the right-click menu of a plugin device lists the two commands.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, ConsoleCommand, DeviceKey};
use atomik_ui::{MeterKind, Modifiers, PointerButton};

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
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    s
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
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
}

/// A shell with one instrument, its channel selected in the console, the console in focus.
fn with_channel() -> (Shell<NullBackend>, DeviceKey) {
    let mut s = shell();
    s.execute(AppCommand::ToggleConsole);
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey("lead".into())),
        })
        .expect("an instrument");
    s.frame();
    let channel = s
        .core()
        .doc()
        .console
        .channel_of(&DeviceKey("lead".into()))
        .expect("its channel")
        .channel
        .clone();
    s.core_mut()
        .console_mut()
        .set_selected_strips(std::slice::from_ref(&channel));
    s.set_focus(Pane::Console);
    (s, channel)
}

#[test]
fn the_commands_open_a_panel_on_the_selected_strip_and_the_view_shows_it() {
    let (mut s, channel) = with_channel();
    assert!(s.meter_panels_view().is_empty());
    s.run_command("mixer.show_spectrum", None);
    s.frame();
    let ids = s.core().meter_panel_ids();
    assert_eq!(ids.len(), 1);
    assert_eq!(s.core().meter_panel_target(ids[0]), Some(&channel));
    assert_eq!(s.meter_panels_view().ids(), ids);
    s.run_command("mixer.show_phase_meter", None);
    s.frame();
    assert_eq!(s.meter_panels_view().ids().len(), 2);
    // Asking again for the same kind on the same strip is the panel that is open.
    s.run_command("mixer.show_spectrum", None);
    s.frame();
    assert_eq!(s.meter_panels_view().ids().len(), 2);
}

#[test]
fn the_master_selected_in_the_console_is_the_strip_a_command_opens() {
    let (mut s, _) = with_channel();
    s.core_mut().console_mut().select_master();
    s.run_command("mixer.show_phase_meter", None);
    s.frame();
    let ids = s.core().meter_panel_ids();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        s.core().meter_panel_target(ids[0]),
        s.core().doc().console.master.as_ref()
    );
}

#[test]
fn a_press_on_the_cross_closes_the_panel_and_a_press_on_the_plot_is_the_panels_own() {
    let (mut s, _) = with_channel();
    s.run_command("mixer.show_spectrum", None);
    s.frame();
    let id = s.core().meter_panel_ids()[0];
    let plot = s.meter_panels_view().plot_rect(id).unwrap().center();
    let focus = s.focus;
    let before = s.core().meter_panel_ids();
    click(&mut s, plot);
    assert_eq!(
        s.core().meter_panel_ids(),
        before,
        "a press on the plot closes nothing"
    );
    assert_eq!(s.focus, focus, "and the pane below never saw it");
    let size_up = s.meter_panels_view().size_rect(id, true).unwrap().center();
    click(&mut s, size_up);
    assert_eq!(s.core().meter_panel_size(id), Some(8192));
    let cross = s.meter_panels_view().close_rect(id).unwrap().center();
    click(&mut s, cross);
    assert!(s.core().meter_panel_ids().is_empty());
    s.frame();
    assert!(s.meter_panels_view().is_empty());
}

#[test]
fn a_panel_is_painted_and_gone_with_its_strip() {
    let (mut s, channel) = with_channel();
    s.run_command("mixer.show_phase_meter", None);
    s.frame();
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"PHASE"), "the panel is in the picture");
    s.core_mut()
        .apply_command(Command::Console(ConsoleCommand::RemoveChannel { channel }))
        .expect("the channel goes");
    s.frame();
    assert!(s.meter_panels_view().is_empty());
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(!dl.texts().contains(&"PHASE"));
}

#[test]
fn with_nothing_selected_the_command_says_what_to_select() {
    let mut s = shell();
    s.run_command("mixer.show_spectrum", None);
    assert!(s.core().meter_panel_ids().is_empty());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("Select a console strip"))
    );
    let _ = MeterKind::Spectrum;
}

#[test]
fn the_rack_menu_of_a_plugin_device_lists_the_two_meter_commands_and_other_devices_do_not() {
    use crate::context_menus::{MenuEnable, specs};
    use atomik_ui::ContextTarget;
    use atomik_ui::context_menu::Spec;
    let commands = |e: MenuEnable| -> Vec<&'static str> {
        specs(ContextTarget::Device, &e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item { command, .. } => Some(command),
                _ => None,
            })
            .collect()
    };
    let with = commands(MenuEnable {
        plugin: true,
        ..MenuEnable::default()
    });
    assert!(with.contains(&"mixer.show_spectrum"), "{with:?}");
    assert!(with.contains(&"mixer.show_phase_meter"));
    assert!(!commands(MenuEnable::default()).contains(&"mixer.show_spectrum"));
}
