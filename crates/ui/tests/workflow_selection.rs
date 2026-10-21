//! Rack multi-selection: the selection set with a primary
//! device, the toggle and range roles per OS, select all, the neighbour keys, delete and
//! duplicate as workflow commands, the group drag and the context-menu requests.

mod support;

use atomik_model::{Command, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::{ContextTarget, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::container::{layered, path};
use support::*;
use vello::kurbo::Point;

fn mods(shift: bool, ctrl: bool, meta: bool) -> Modifiers {
    Modifiers {
        shift,
        ctrl,
        alt: false,
        meta,
    }
}

fn press(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    at: Point,
    button: PointerButton,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button,
            modifiers,
            clicks: 1,
        },
        doc,
        reg,
    );
    out.extend(v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button,
            modifiers,
        },
        doc,
        reg,
    ));
    out
}

fn click(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    device: &str,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let at = v.plate_center(&key(device)).unwrap();
    press(v, doc, reg, at, PointerButton::Primary, modifiers)
}

fn names(v: &RackView) -> Vec<String> {
    v.selection().into_iter().map(|k| k.0).collect()
}

fn setup() -> (atomik_model::RackDoc, atomik_device_sdk::Registry, RackView) {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    (doc, reg, v)
}

// ------------------------------------------------------------------ gestures

#[test]
fn a_plain_click_selects_one_device_and_ctrl_click_toggles_on_windows() {
    let (doc, reg, mut v) = setup();
    let a = click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    assert_eq!(a, vec![UiAction::Select(key("synth"))]);
    assert_eq!(names(&v), ["synth"]);
    let a = click(&mut v, &doc, &reg, "mixer", Modifiers::CTRL);
    assert_eq!(
        a,
        vec![UiAction::SelectMany {
            primary: key("mixer"),
            keys: vec![key("synth"), key("mixer")]
        }]
    );
    assert_eq!(names(&v), ["synth", "mixer"]);
    assert_eq!(
        v.selected(),
        Some(&key("mixer")),
        "the last clicked is primary"
    );
    // Toggling the primary off hands the primary to the one that is left.
    let a = click(&mut v, &doc, &reg, "mixer", Modifiers::CTRL);
    assert_eq!(names(&v), ["synth"]);
    assert_eq!(v.selected(), Some(&key("synth")));
    assert_eq!(a, vec![UiAction::Select(key("synth"))]);
}

#[test]
fn shift_click_selects_the_range_from_the_anchor_in_rack_order() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    let a = click(&mut v, &doc, &reg, "mixer", Modifiers::SHIFT);
    assert_eq!(names(&v), ["synth", "delay", "mixer"]);
    assert!(matches!(
        a.as_slice(),
        [UiAction::SelectMany { primary, keys }]
            if *primary == key("mixer") && keys.len() == 3
    ));
    // A second shift-click re-ranges from the same anchor.
    click(&mut v, &doc, &reg, "delay", Modifiers::SHIFT);
    assert_eq!(names(&v), ["synth", "delay"]);
    // The range goes upwards too, and a plain click starts over.
    click(&mut v, &doc, &reg, "mixer", Modifiers::NONE);
    click(&mut v, &doc, &reg, "synth", Modifiers::SHIFT);
    assert_eq!(names(&v), ["synth", "delay", "mixer"]);
    click(&mut v, &doc, &reg, "delay", Modifiers::NONE);
    assert_eq!(names(&v), ["delay"]);
}

#[test]
fn on_macos_command_toggles_and_control_does_not() {
    let (doc, reg, mut v) = setup();
    v.set_roles(Roles::defaults(Os::Mac));
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "mixer", mods(false, false, true));
    assert_eq!(names(&v), ["synth", "mixer"]);
    // Control is not the toggle role on macOS: it is a plain click there.
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    assert_eq!(names(&v), ["delay"]);
}

#[test]
fn a_click_on_the_empty_rack_clears_the_selection() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "mixer", Modifiers::CTRL);
    let below = Point::new(500.0, v.content_height() + 60.0);
    let _ = press(
        &mut v,
        &doc,
        &reg,
        below,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert!(v.selection().is_empty());
    assert_eq!(v.selected(), None);
}

#[test]
fn clicking_a_member_without_moving_collapses_to_it_on_release() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    assert_eq!(names(&v), ["synth", "delay"]);
    // Press on a member: the selection stays (it may start a group drag)...
    let at = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        &doc,
        &reg,
    );
    assert_eq!(names(&v), ["synth", "delay"]);
    // ...and a release in place makes it the only selection.
    let a = v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert_eq!(names(&v), ["synth"]);
    assert!(commands(&a).is_empty(), "no move: {a:?}");
}

#[test]
fn the_interface_panel_is_never_selected_and_gone_devices_leave_the_selection() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    let mut smaller = doc.clone();
    smaller.devices.retain(|d| d.key != key("delay"));
    smaller
        .cables
        .retain(|c| c.from.device != key("delay") && c.to.device != key("delay"));
    v.sync(&smaller, &reg);
    assert_eq!(names(&v), ["synth"]);
    assert_eq!(v.selected(), Some(&key("synth")));
}

// ------------------------------------------------------------------ commands

#[test]
fn select_all_selects_every_device_in_rack_order_and_primary_is_the_last() {
    let (doc, reg, mut v) = setup();
    let out = v.command("edit.select_all", None, &doc, &reg);
    assert_eq!(names(&v), ["synth", "delay", "mixer"]);
    assert_eq!(
        out,
        CommandOutcome::Handled(vec![UiAction::SelectMany {
            primary: key("mixer"),
            keys: vec![key("synth"), key("delay"), key("mixer")]
        }])
    );
}

#[test]
fn delete_removes_the_whole_selection_in_one_command() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "mixer", Modifiers::NONE);
    click(&mut v, &doc, &reg, "synth", Modifiers::CTRL);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DeleteDevices {
                keys: vec![key("synth"), key("mixer")]
            }
        ))]),
        "rack order, one command, bridging while auto-routing is on"
    );
    // Without auto-routing the removal is plain.
    v.set_auto_route(false);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::RemoveDevices {
                keys: vec![key("synth"), key("mixer")]
            }
        ))]),
    );
    // A single device keeps the plain command.
    click(&mut v, &doc, &reg, "delay", Modifiers::NONE);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::RemoveDevice {
            key: key("delay")
        })])
    );
}

#[test]
fn duplicate_copies_the_selection_with_its_cables() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateDevices {
                keys: vec![key("synth")]
            }
        ))])
    );
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateDevices {
                keys: vec![key("synth"), key("delay")]
            }
        ))])
    );
}

#[test]
fn the_edit_commands_decline_with_nothing_selected() {
    let (doc, reg, mut v) = setup();
    for id in ["edit.delete", "edit.duplicate", "rack.disconnect"] {
        assert_eq!(
            v.command(id, None, &doc, &reg),
            CommandOutcome::Declined,
            "{id}"
        );
    }
}

#[test]
fn up_and_down_move_the_selection_and_shift_extends_it() {
    let (doc, reg, mut v) = setup();
    // Nothing selected: Down picks the first device, Up the last.
    let out = v.command("rack.select_next", None, &doc, &reg);
    assert!(out.is_taken());
    assert_eq!(names(&v), ["synth"]);
    v.command("rack.select_next", None, &doc, &reg);
    assert_eq!(names(&v), ["delay"]);
    v.command("rack.select_next", None, &doc, &reg);
    v.command("rack.select_next", None, &doc, &reg);
    assert_eq!(names(&v), ["mixer"], "stops at the end");
    v.command("rack.select_prev", None, &doc, &reg);
    assert_eq!(names(&v), ["delay"]);
    // Extending grows from the anchor, and shrinking back goes the other way.
    v.command("rack.extend_next", None, &doc, &reg);
    assert_eq!(names(&v), ["delay", "mixer"]);
    assert_eq!(v.selected(), Some(&key("mixer")));
    v.command("rack.extend_prev", None, &doc, &reg);
    assert_eq!(names(&v), ["delay"]);
    v.command("rack.extend_prev", None, &doc, &reg);
    assert_eq!(names(&v), ["synth", "delay"]);
    // Plain navigation after an extension leaves one device.
    v.command("rack.select_next", None, &doc, &reg);
    assert_eq!(names(&v), ["delay"]);
}

#[test]
fn the_neighbour_keys_step_a_focused_control_instead() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    let knob = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let _ = press(
        &mut v,
        &doc,
        &reg,
        knob,
        PointerButton::Primary,
        Modifiers::NONE,
    );
    assert!(v.focused_control().is_some());
    assert_eq!(
        v.command("rack.select_next", None, &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn disconnect_removes_the_cables_of_one_device_and_of_a_group() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "delay", Modifiers::NONE);
    assert_eq!(
        v.command("rack.disconnect", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DisconnectDevice { key: key("delay") }
        ))])
    );
    click(&mut v, &doc, &reg, "mixer", Modifiers::CTRL);
    assert_eq!(
        v.command("rack.disconnect", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::DisconnectDevices {
                keys: vec![key("delay"), key("mixer")]
            }
        ))]),
        "several devices are one command"
    );
}

// ------------------------------------------------------------------ group drag

fn drag(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    from: Point,
    to: Point,
) -> Vec<UiAction> {
    let mut out = v.handle(&down(from), doc, reg);
    out.extend(v.handle(&mv(to), doc, reg));
    out.extend(v.handle(&up(to), doc, reg));
    out
}

#[test]
fn dragging_a_selected_device_moves_the_group_in_one_command() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    let from = v.plate_center(&key("delay")).unwrap();
    let below = Point::new(from.x, v.content_height() - 4.0);
    let a = drag(&mut v, &doc, &reg, from, below);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![key("synth"), key("delay")],
            slot: atomik_model::RackSlot {
                column: 0,
                index: 1
            }
        })]
    );
}

#[test]
fn a_group_drag_that_changes_nothing_sends_no_move() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    let from = v.plate_center(&key("synth")).unwrap();
    // Dropped back between the group and the mixer: where it already is.
    let to = Point::new(from.x, v.plate_center(&key("delay")).unwrap().y + 20.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    assert_eq!(names(&v), ["synth", "delay"], "a real drag keeps the group");
}

#[test]
fn a_single_device_moves_with_a_slot_command_of_one_key() {
    let (doc, reg, mut v) = setup();
    let from = v.plate_center(&key("synth")).unwrap();
    let below = Point::new(from.x, v.content_height() - 4.0);
    let a = drag(&mut v, &doc, &reg, from, below);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Workflow(WorkflowCommand::MoveDevicesToSlot { keys, .. })] if keys.len() == 1
        ),
        "{a:?}"
    );
}

// ------------------------------------------------------------------ context menus

#[test]
fn right_click_on_a_device_selects_it_and_asks_for_the_device_menu() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    let at = v.plate_center(&key("mixer")).unwrap();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert_eq!(names(&v), ["mixer"]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Device,
        at
    }));
}

#[test]
fn right_click_on_a_member_keeps_the_whole_selection() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "delay", Modifiers::CTRL);
    let at = v.plate_center(&key("synth")).unwrap();
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert_eq!(names(&v), ["synth", "delay"]);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Device,
        at
    }));
}

#[test]
fn right_click_on_the_empty_rack_asks_for_the_rack_menu() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    let at = Point::new(500.0, v.content_height() + 60.0);
    let a = press(
        &mut v,
        &doc,
        &reg,
        at,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Rack,
        at
    }));
    assert_eq!(
        names(&v),
        ["synth"],
        "a right click leaves the selection alone"
    );
}

#[test]
fn right_click_on_a_control_keeps_the_control_behaviour() {
    let (doc, reg, mut v) = setup();
    let wave = v.control_center(&key("synth"), "osc2.wave").unwrap();
    let a = press(
        &mut v,
        &doc,
        &reg,
        wave,
        PointerButton::Secondary,
        Modifiers::NONE,
    );
    assert!(
        !a.iter().any(|x| matches!(x, UiAction::ContextMenu { .. })),
        "{a:?}"
    );
}

// ------------------------------------------------------------------ inside a container

#[test]
fn inside_a_container_the_selection_uses_path_keys() {
    let d = layered();
    let reg = registry();
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_scope(&path(&["layer"]));
    v.sync(&doc, &reg);
    let out = v.command("edit.select_all", None, &doc, &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("{out:?}")
    };
    let [UiAction::SelectMany { primary, keys }] = actions.as_slice() else {
        panic!("{actions:?}")
    };
    assert_eq!(primary.0, "layer/reverb");
    let all: Vec<&str> = keys.iter().map(|k| k.0.as_str()).collect();
    assert_eq!(
        all,
        ["layer/synth", "layer/pad", "layer/delay", "layer/reverb"]
    );
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            WorkflowCommand::RemoveDevices { keys: keys.clone() }
        ))])
    );
    assert_eq!(v.scope_path().unwrap().0, "layer");
}

// ------------------------------------------------------------------ additions

fn escape(v: &mut RackView, doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) {
    let _ = v.handle(
        &UiEvent::Key {
            key: atomik_ui::Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        doc,
        reg,
    );
}

#[test]
fn escape_clears_the_selection_when_nothing_else_is_pending() {
    let (doc, reg, mut v) = setup();
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "mixer", Modifiers::CTRL);
    assert_eq!(names(&v), ["synth", "mixer"]);
    escape(&mut v, &doc, &reg);
    assert!(v.selection().is_empty());
    assert_eq!(v.selected(), None);
}

#[test]
fn the_roles_decide_the_toggle_and_range_chords_on_every_os() {
    let (doc, reg, mut v) = setup();
    // Linux behaves as Windows: Ctrl toggles, Shift ranges, Meta is a plain click.
    v.set_roles(Roles::defaults(Os::Linux));
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "mixer", mods(false, true, false));
    assert_eq!(names(&v), ["synth", "mixer"]);
    click(&mut v, &doc, &reg, "mixer", Modifiers::NONE);
    click(&mut v, &doc, &reg, "synth", mods(true, false, false));
    assert_eq!(names(&v), ["synth", "delay", "mixer"]);
    click(&mut v, &doc, &reg, "delay", mods(false, false, true));
    assert_eq!(names(&v), ["delay"], "Meta is not the toggle role on Linux");
    // Windows: the same, Meta (the Windows key) is not a toggle.
    v.set_roles(Roles::defaults(Os::Windows));
    click(&mut v, &doc, &reg, "synth", mods(false, false, true));
    assert_eq!(names(&v), ["synth"]);
    // macOS: Shift ranges as everywhere.
    v.set_roles(Roles::defaults(Os::Mac));
    click(&mut v, &doc, &reg, "synth", Modifiers::NONE);
    click(&mut v, &doc, &reg, "mixer", mods(true, false, false));
    assert_eq!(names(&v), ["synth", "delay", "mixer"]);
    // Command-click on a member toggles it out, keeping the others.
    click(&mut v, &doc, &reg, "delay", mods(false, false, true));
    assert_eq!(names(&v), ["synth", "mixer"]);
}

#[test]
fn the_extend_keys_do_nothing_without_a_selection() {
    let (doc, reg, mut v) = setup();
    for id in ["rack.extend_next", "rack.extend_prev"] {
        assert_eq!(
            v.command(id, None, &doc, &reg),
            CommandOutcome::Declined,
            "{id} has nothing to extend"
        );
    }
    assert!(v.selection().is_empty());
    // Plain Down from nothing selects the first device, Up the last.
    v.command("rack.select_next", None, &doc, &reg);
    assert_eq!(names(&v), ["synth"]);
    let mut v = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.command("rack.select_prev", None, &doc, &reg);
    assert_eq!(names(&v), ["mixer"]);
}

#[test]
fn a_lone_container_duplicates_as_a_container_and_a_device_without_cables_cannot_disconnect() {
    let d = layered();
    let reg = registry();
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    click(&mut v, &doc, &reg, "layer", Modifiers::NONE);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Container(
            atomik_model::ContainerCommand::DuplicateContainer {
                container: key("layer")
            }
        ))])
    );
    // The container has no cables in this document: the command says so and emits nothing.
    assert!(matches!(
        v.command("rack.disconnect", None, &doc, &reg),
        CommandOutcome::NotApplicable(m) if m.contains("no cables")
    ));
    // A group of the container and the synth duplicates as devices.
    click(&mut v, &doc, &reg, "lead", Modifiers::CTRL);
    let CommandOutcome::Handled(a) = v.command("edit.duplicate", None, &doc, &reg) else {
        panic!("not handled")
    };
    assert!(matches!(
        a.as_slice(),
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateDevices { .. }
        ))]
    ));
}

#[test]
fn escape_in_a_container_clears_the_selection_first_and_a_second_escape_goes_up() {
    let d = layered();
    let reg = registry();
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_scope(&path(&["layer"]));
    v.sync(&doc, &reg);
    let press_escape = |v: &mut RackView| {
        v.handle(
            &UiEvent::Key {
                key: atomik_ui::Key::Escape,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            &doc,
            &reg,
        )
    };
    v.command("edit.select_all", None, &doc, &reg);
    assert!(v.selection_len() > 1);
    assert!(press_escape(&mut v).is_empty());
    assert!(v.selection().is_empty());
    assert_eq!(v.scope_path().unwrap().0, "layer", "the first Escape stays");
    assert_eq!(
        press_escape(&mut v),
        vec![UiAction::ExitContainer { levels: 1 }]
    );
}
