//! The tempo lane in the shell: the View command, the saved view state and the context-menu
//! items of the tempo lane and the ruler.

use super::*;
use crate::session::NullBackend;
use atomik_ui::ContextTarget;
use atomik_ui::context_menu::Spec;
use atomik_ui::keymap::Os;

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

#[test]
fn the_tempo_lane_is_off_by_default_and_the_command_toggles_it_and_the_saved_state() {
    let mut s = shell();
    assert!(!s.core().arrangement().tempo_lane_visible());
    assert!(!s.ui_state().show_tempo_lane);
    assert_eq!(
        AppCommand::from_command("view.toggle_tempo_lane", None),
        Some(AppCommand::ToggleTempoLane)
    );
    s.execute(AppCommand::ToggleTempoLane);
    assert!(s.core().arrangement().tempo_lane_visible());
    assert!(s.ui_state().show_tempo_lane);
    s.execute(AppCommand::ToggleTempoLane);
    assert!(!s.core().arrangement().tempo_lane_visible());
    assert!(!s.ui_state().show_tempo_lane);
}

#[test]
fn the_context_menus_list_their_commands() {
    let e = context_menus::MenuEnable {
        tempo_can_ramp: true,
        tempo_ramped: true,
        tempo_can_delete: false,
        meter_can_delete: true,
        ..context_menus::MenuEnable::default()
    };
    let items = |t: ContextTarget| -> Vec<(&'static str, Option<&'static str>, bool)> {
        context_menus::specs(t, &e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item {
                    command,
                    arg,
                    enabled,
                    ..
                } => Some((command, arg, enabled)),
                Spec::Separator | Spec::Submenu { .. } => None,
            })
            .collect()
    };
    assert_eq!(
        items(ContextTarget::TempoPoint),
        vec![
            ("sequencer.tempo_toggle_ramp", None, true),
            ("sequencer.tempo_set_value", None, true),
            ("sequencer.tempo_delete_point", None, false),
        ]
    );
    assert_eq!(
        items(ContextTarget::TempoLane),
        vec![("sequencer.tempo_add_point", None, true)]
    );
    let ruler = items(ContextTarget::RulerMeter);
    assert_eq!(ruler[0], ("sequencer.locator_left_here", None, true));
    assert_eq!(ruler[1], ("sequencer.locator_right_here", None, true));
    assert_eq!(ruler[2], ("sequencer.marker_end_here", None, true));
    assert_eq!(ruler[3], ("sequencer.insert_bars", None, true));
    assert_eq!(ruler[4], ("sequencer.remove_bars", None, true));
    let insert = &ruler[5..];
    let args: Vec<_> = insert.iter().map(|i| i.1.unwrap()).collect();
    assert_eq!(args, ["2/4", "3/4", "4/4", "5/4", "6/8", "7/8", "12/8"]);
    assert!(
        insert
            .iter()
            .all(|i| i.0 == "sequencer.meter_insert" && i.2)
    );
    let change = items(ContextTarget::MeterChange);
    assert_eq!(change.len(), 8);
    assert_eq!(change[7], ("sequencer.meter_delete", None, true));
    assert!(change[..7].iter().all(|i| i.0 == "sequencer.meter_set"));
    // The ramp item is a check item, checked for a point that ramps.
    let checked = context_menus::specs(ContextTarget::TempoPoint, &e)
        .into_iter()
        .find_map(|s| match s {
            Spec::Item { checked, .. } => checked,
            Spec::Separator | Spec::Submenu { .. } => None,
        });
    assert_eq!(checked, Some(true));
}
