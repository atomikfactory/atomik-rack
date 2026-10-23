//! Output buses in the console view: bus strips between the channels and the master, the output
//! selector of every strip, rename, the bus context menu, reordering, selection, meters and solo.

mod support;

use atomik_model::console::OutputDest;
use atomik_model::{Command, ConsoleCommand, RackDoc};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{
    ChannelMeters, ConsoleView, DisplayList, Key, Modifiers, PointerButton, UiAction, UiEvent,
};
use support::console::*;
use support::starter;
use support::*;

fn setup() -> (RackDoc, atomik_device_sdk::Registry, ConsoleView) {
    let (doc, reg) = (bus_doc(), console_registry());
    let v = console_view(&doc, &reg);
    (doc, reg, v)
}

fn click(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
) -> Vec<UiAction> {
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn click_with(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut a = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
            clicks: 1,
        },
        doc,
        reg,
    );
    a.extend(v.handle(
        &UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
        },
        doc,
        reg,
    ));
    a
}

fn click_name(v: &mut ConsoleView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, strip: &str) {
    let p = v.name_center(&key(strip)).unwrap();
    click(v, doc, reg, p);
}

fn click_name_with(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    strip: &str,
    modifiers: Modifiers,
) {
    let p = v.name_center(&key(strip)).unwrap();
    click_with(v, doc, reg, p, modifiers);
}

fn key_event(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn type_text(v: &mut ConsoleView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, text: &str) {
    for c in text.chars() {
        v.handle(&key_event(Key::Char(c)), doc, reg);
    }
}

fn console_cmds(actions: &[UiAction]) -> Vec<ConsoleCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Console(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn pick(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    text: &str,
) -> Vec<UiAction> {
    let i = v
        .menu_items()
        .iter()
        .position(|t| t == text)
        .unwrap_or_else(|| panic!("no menu row {text:?} in {:?}", v.menu_items()));
    let p = v.menu_row_center(i).unwrap();
    click(v, doc, reg, p)
}

fn open_output(v: &mut ConsoleView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, strip: &str) {
    let p = v.output_center(&key(strip)).expect("selector shown");
    assert!(click(v, doc, reg, p).is_empty(), "opening emits nothing");
    assert!(!v.menu_items().is_empty());
}

fn name_at(v: &ConsoleView, strip: &str) -> Point {
    v.name_center(&key(strip)).unwrap()
}

// ---------------------------------------------------------------- strips

#[test]
fn bus_strips_follow_the_channels_in_console_order_with_the_add_channel_slot_between() {
    let (doc, reg, mut v) = setup();
    assert_eq!(
        v.strip_keys(),
        [
            key("ch-bass"),
            key("ch-lead"),
            key("ch-keys"),
            key("bus"),
            key("bus-2")
        ]
    );
    let keys = v.strip_rect(&key("ch-keys")).unwrap();
    let bus = v.strip_rect(&key("bus")).unwrap();
    let bus2 = v.strip_rect(&key("bus-2")).unwrap();
    assert!(
        bus.x0 >= keys.x1 + keys.width(),
        "the placeholder slot sits between"
    );
    assert!(bus2.x0 > bus.x1 - 1e-9);
    assert!(bus2.x1 <= v.bounds().x1, "the master stays to the right");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    for t in [
        "Drums",
        "Synths",
        "BUS 1",
        "BUS 2",
        "CH 3",
        "MASTER",
        "ADD CHANNEL",
    ] {
        assert!(texts.contains(&t), "missing {t:?} in {texts:?}");
    }
}

#[test]
fn a_bus_strip_is_a_full_channel_strip() {
    let (_doc, _reg, v) = setup();
    for param in [
        "fader",
        "pan",
        "mute",
        "solo",
        "comp.on",
        "eq.on",
        "send1.level",
        "input.gain",
    ] {
        let at = |s: &str| v.control_center(&key(s), param).is_some();
        assert_eq!(at("bus"), at("ch-bass"), "{param}");
    }
    assert!(v.control_center(&key("bus"), "fader").is_some());
}

#[test]
fn mute_and_solo_buttons_of_a_bus_set_its_params() {
    let (doc, reg, mut v) = setup();
    let p = v.control_center(&key("bus"), "mute").unwrap();
    let a = click(&mut v, &doc, &reg, p);
    assert_eq!(
        commands(&a),
        [Command::SetParam {
            key: key("bus"),
            param: "mute".into(),
            value: 1.0,
            merge: None
        }]
    );
    let p = v.control_center(&key("bus-2"), "solo").unwrap();
    let a = click(&mut v, &doc, &reg, p);
    assert_eq!(
        commands(&a),
        [Command::SetParam {
            key: key("bus-2"),
            param: "solo".into(),
            value: 1.0,
            merge: None
        }]
    );
}

#[test]
fn the_solo_of_a_channel_keeps_the_buses_on_its_path_audible() {
    let (_doc, _reg, v) = setup();
    // ch-keys is soloed; it plays into bus-2, which plays into bus.
    assert!(!v.solo_silenced(&key("ch-keys")));
    assert!(!v.solo_silenced(&key("bus-2")));
    assert!(!v.solo_silenced(&key("bus")));
    assert!(v.solo_silenced(&key("ch-bass")));
    assert!(v.solo_silenced(&key("ch-lead")));
}

#[test]
fn a_bus_solo_counts_for_the_other_strips_and_blocks_its_own_mute() {
    let (mut doc, reg, _v) = setup();
    for d in &mut doc.devices {
        if d.key == key("ch-keys") {
            d.params.remove("solo");
        }
        if d.key == key("bus-2") {
            d.params.insert("solo".into(), 1.0);
        }
    }
    let v = console_view(&doc, &reg);
    assert!(
        !v.mute_enabled(&key("bus-2")),
        "a soloed strip's mute is off"
    );
    assert!(!v.solo_silenced(&key("ch-keys")), "feeds the soloed bus");
    assert!(
        !v.solo_silenced(&key("bus")),
        "the soloed bus plays into it"
    );
    assert!(v.solo_silenced(&key("ch-lead")));
}

#[test]
fn buses_take_meter_readings_like_channels() {
    let (_doc, _reg, mut v) = setup();
    let m = ChannelMeters {
        out_peak_l: 0.5,
        out_peak_r: 0.5,
        ..ChannelMeters::SILENT
    };
    v.set_meters(&[(key("bus"), m), (key("nope"), m)]);
    v.set_frame_time(0.0);
    v.set_frame_time(0.1);
    assert!(v.meter(&key("bus")).is_some());
    assert!(v.meter(&key("nope")).is_none());
}

#[test]
fn a_bus_shows_its_latency_readout() {
    let (doc, reg, mut v) = setup();
    v.set_latency_readouts(&[(key("bus"), 256)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"lat 256"), "{:?}", dl.texts());
}

// ---------------------------------------------------------------- output selector

#[test]
fn every_strip_shows_where_its_output_goes() {
    let (_doc, _reg, v) = setup();
    let text = |s: &str| v.output_text(&key(s)).unwrap();
    assert_eq!(text("ch-bass"), "Audio I/O 3+4");
    assert_eq!(text("ch-lead"), "Bus Drums");
    assert_eq!(text("ch-keys"), "Bus Synths");
    assert_eq!(text("bus-2"), "Bus Drums");
    assert_eq!(text("bus"), "Master");
}

#[test]
fn a_hand_cabled_or_unrouted_output_reads_as_such() {
    let (mut doc, reg, _v) = setup();
    doc.cables.retain(|c| c.from.device != key("ch-lead"));
    let v = console_view(&doc, &reg);
    assert_eq!(v.output_text(&key("ch-lead")).unwrap(), "none");
    doc.cables
        .push(cable(("ch-lead", "out_l"), ("delay", "in_l")));
    let v = console_view(&doc, &reg);
    assert_eq!(v.output_text(&key("ch-lead")).unwrap(), "Custom");
}

#[test]
fn the_selector_lists_master_buses_audio_io_pairs_and_new_bus() {
    let (doc, reg, mut v) = setup();
    open_output(&mut v, &doc, &reg, "ch-lead");
    let items = v.menu_items();
    assert_eq!(
        items,
        [
            "Master",
            "Drums",
            "Synths",
            "Audio I/O 1+2 (the master)",
            "Audio I/O 3+4 (in use by Bass)",
            "Audio I/O 5+6 and up: raise Outputs in Audio settings",
            "New bus...",
        ]
    );
    let enabled: Vec<bool> = (0..items.len()).map(|i| v.menu_enabled(i)).collect();
    assert_eq!(enabled, [true, true, true, false, false, false, true]);
}

#[test]
fn a_bus_selector_leaves_out_the_buses_that_would_loop() {
    let (doc, reg, mut v) = setup();
    // bus-2 plays into bus: bus cannot go to bus-2 (and not to itself).
    open_output(&mut v, &doc, &reg, "bus");
    let items = v.menu_items();
    assert_eq!(items[0], "Master");
    assert!(
        !items.iter().any(|i| i == "Drums" || i == "Synths"),
        "{items:?}"
    );
    // bus-2 may go to bus (it does) but not to itself.
    let mut v = console_view(&doc, &reg);
    open_output(&mut v, &doc, &reg, "bus-2");
    let items = v.menu_items();
    assert!(items.contains(&"Drums".to_string()));
    assert!(!items.contains(&"Synths".to_string()));
}

#[test]
fn a_strip_keeps_its_own_audio_io_pair_selectable() {
    let (doc, reg, mut v) = setup();
    open_output(&mut v, &doc, &reg, "ch-bass");
    let items = v.menu_items();
    let i = items
        .iter()
        .position(|t| t == "Audio I/O 3+4")
        .expect("own pair");
    assert!(v.menu_enabled(i));
}

#[test]
fn picking_a_destination_emits_set_output() {
    let (doc, reg, mut v) = setup();
    open_output(&mut v, &doc, &reg, "ch-lead");
    let a = pick(&mut v, &doc, &reg, "Master");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetOutput {
            strip: key("ch-lead"),
            dest: OutputDest::Master
        }]
    );
    assert!(v.menu_items().is_empty(), "the menu closed");
    open_output(&mut v, &doc, &reg, "ch-lead");
    let a = pick(&mut v, &doc, &reg, "Synths");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetOutput {
            strip: key("ch-lead"),
            dest: OutputDest::Bus(key("bus-2"))
        }]
    );
    // A free pair (the project has 4 output channels, 1+2 is the master's, 3+4 is ch-bass's).
    let mut doc2 = doc.clone();
    doc2.output_channels = 6;
    let mut v = console_view(&doc2, &reg);
    open_output(&mut v, &doc2, &reg, "ch-lead");
    let a = pick(&mut v, &doc2, &reg, "Audio I/O 5+6");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetOutput {
            strip: key("ch-lead"),
            dest: OutputDest::AudioIo(3)
        }]
    );
}

#[test]
fn a_greyed_row_does_nothing_and_closes_the_menu() {
    let (doc, reg, mut v) = setup();
    open_output(&mut v, &doc, &reg, "ch-lead");
    let a = pick(&mut v, &doc, &reg, "Audio I/O 3+4 (in use by Bass)");
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn new_bus_routes_the_strip_to_a_bus_with_the_default_name() {
    let (doc, reg, mut v) = setup();
    open_output(&mut v, &doc, &reg, "ch-lead");
    let a = pick(&mut v, &doc, &reg, "New bus...");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RouteToNewBus {
            strips: vec![key("ch-lead")],
            name: doc.console.default_bus_name(),
        }]
    );
    assert_eq!(doc.console.default_bus_name(), "Bus 1");
}

#[test]
fn the_selector_menu_flips_above_the_pinned_row() {
    let (doc, reg, mut v) = setup();
    let p = v.output_center(&key("ch-lead")).unwrap();
    click(&mut v, &doc, &reg, p);
    let first = v.menu_row_center(0).unwrap();
    let last = v.menu_row_center(v.menu_items().len() - 1).unwrap();
    assert!(
        first.y < p.y && last.y < p.y,
        "opens upwards near the bottom"
    );
    assert!(first.y >= v.bounds().y0);
}

// ---------------------------------------------------------------- rename

#[test]
fn double_clicking_a_bus_name_edits_it_and_enter_renames() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "bus");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    v.handle(&up(p), &doc, &reg);
    assert!(v.is_text_owner());
    assert_eq!(v.rename_text(), Some("Drums"));
    for _ in 0..5 {
        v.handle(&key_event(Key::Backspace), &doc, &reg);
    }
    type_text(&mut v, &doc, &reg, "Kit");
    v.handle(&key_event(Key::Space), &doc, &reg);
    type_text(&mut v, &doc, &reg, "A");
    assert_eq!(v.rename_text(), Some("Kit A"));
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RenameBus {
            bus: key("bus"),
            name: "Kit A".into()
        }]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn escape_cancels_a_rename_and_an_unchanged_name_emits_nothing() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "bus");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    type_text(&mut v, &doc, &reg, "x");
    let a = v.handle(&key_event(Key::Escape), &doc, &reg);
    assert!(a.is_empty());
    assert!(!v.is_text_owner());
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert!(a.is_empty(), "same name: {a:?}");
}

#[test]
fn a_rename_is_capped_at_the_bus_name_length_and_clicking_elsewhere_commits_it() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "bus-2");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    type_text(&mut v, &doc, &reg, &"x".repeat(60));
    assert_eq!(v.rename_text().unwrap().chars().count(), 32);
    let elsewhere = v.control_center(&key("ch-bass"), "fader").unwrap();
    let a = v.handle(&down(elsewhere), &doc, &reg);
    assert_eq!(console_cmds(&a).len(), 1, "{a:?}");
    assert!(!v.is_text_owner());
}

#[test]
fn double_clicking_a_channel_name_renames_the_device_the_strip_shows() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "ch-bass");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    v.handle(&up(p), &doc, &reg);
    assert!(v.is_text_owner());
    assert_eq!(v.rename_text(), Some("Bass"));
    for _ in 0..4 {
        v.handle(&key_event(Key::Backspace), &doc, &reg);
    }
    type_text(&mut v, &doc, &reg, "Sub");
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert!(console_cmds(&a).is_empty());
    assert_eq!(
        commands(&a),
        [Command::Rename {
            key: key("bass"),
            name: "Sub".into()
        }]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn an_empty_or_escaped_channel_rename_emits_nothing() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "ch-bass");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    for _ in 0..4 {
        v.handle(&key_event(Key::Backspace), &doc, &reg);
    }
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert!(a.is_empty(), "empty name cancels: {a:?}");
    assert!(!v.is_text_owner());
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    type_text(&mut v, &doc, &reg, "x");
    let a = v.handle(&key_event(Key::Escape), &doc, &reg);
    assert!(a.is_empty());
    assert!(!v.is_text_owner());
}

#[test]
fn an_empty_bus_rename_cancels_too() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "bus");
    v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    for _ in 0..5 {
        v.handle(&key_event(Key::Backspace), &doc, &reg);
    }
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
}

// ---------------------------------------------------------------- context menu and delete

#[test]
fn the_bus_context_menu_offers_rename_and_delete() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "bus");
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert!(
        a.is_empty(),
        "a bus is not removed by the click itself: {a:?}"
    );
    assert_eq!(v.menu_items(), ["Rename", "Delete bus"]);
    let a = pick(&mut v, &doc, &reg, "Delete bus");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::DeleteBus { bus: key("bus") }]
    );
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert!(a.is_empty());
    let a = pick(&mut v, &doc, &reg, "Rename");
    assert!(a.is_empty());
    assert!(v.is_text_owner());
    assert_eq!(v.rename_text(), Some("Drums"));
}

#[test]
fn right_click_on_a_channel_name_does_nothing() {
    let (doc, reg, mut v) = setup();
    let p = name_at(&v, "ch-lead");
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
    assert!(v.menu_items().is_empty());
}

// ---------------------------------------------------------------- reordering

fn drag_name(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    strip: &str,
    to_x: f64,
) -> Vec<UiAction> {
    let p = name_at(v, strip);
    v.handle(&down(p), doc, reg);
    v.handle(&mv(Point::new(to_x, p.y)), doc, reg);
    v.handle(&up(Point::new(to_x, p.y)), doc, reg)
}

#[test]
fn dragging_a_bus_name_reorders_the_buses_only() {
    let (doc, reg, mut v) = setup();
    let first = v.strip_rect(&key("bus")).unwrap();
    // bus-2 dropped on the left half of the first bus.
    let a = drag_name(&mut v, &doc, &reg, "bus-2", first.x0 + 10.0);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::ReorderBus {
            bus: key("bus-2"),
            to: 0
        }]
    );
    // bus dropped far right: after bus-2.
    let right = v.bounds().x1 - 400.0;
    let a = drag_name(&mut v, &doc, &reg, "bus", right);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::ReorderBus {
            bus: key("bus"),
            to: 1
        }]
    );
    // Dropped among the channels, a bus stays among the buses.
    let a = drag_name(&mut v, &doc, &reg, "bus-2", 20.0);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::ReorderBus {
            bus: key("bus-2"),
            to: 0
        }]
    );
}

#[test]
fn a_channel_dragged_over_the_buses_stays_among_the_channels() {
    let (doc, reg, mut v) = setup();
    let far = v.strip_rect(&key("bus-2")).unwrap().x1 + 30.0;
    let a = drag_name(&mut v, &doc, &reg, "ch-bass", far);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveChannel {
            channel: key("ch-bass"),
            to: 2
        }]
    );
}

// ---------------------------------------------------------------- selection

#[test]
fn a_click_on_a_name_plate_selects_the_strip_and_command_click_toggles() {
    let (doc, reg, mut v) = setup();
    assert!(v.selected_strips().is_empty());
    click_name(&mut v, &doc, &reg, "ch-lead");
    assert_eq!(v.selected_strips(), [key("ch-lead")]);
    click_name(&mut v, &doc, &reg, "bus");
    assert_eq!(v.selected_strips(), [key("bus")], "a plain click replaces");
    click_name_with(&mut v, &doc, &reg, "ch-bass", Modifiers::CTRL);
    assert_eq!(v.selected_strips(), [key("ch-bass"), key("bus")]);
    click_name_with(&mut v, &doc, &reg, "bus", Modifiers::CTRL);
    assert_eq!(v.selected_strips(), [key("ch-bass")]);
}

#[test]
fn the_primary_strip_is_the_last_clicked_one_while_it_stays_selected() {
    let (doc, reg, mut v) = setup();
    assert_eq!(v.primary_strip(), None);
    click_name(&mut v, &doc, &reg, "ch-lead");
    assert_eq!(v.primary_strip(), Some(key("ch-lead")));
    click_name_with(&mut v, &doc, &reg, "ch-bass", Modifiers::CTRL);
    assert_eq!(v.primary_strip(), Some(key("ch-bass")));
    click_name_with(&mut v, &doc, &reg, "ch-bass", Modifiers::CTRL);
    assert_eq!(v.primary_strip(), None, "toggled off");
}

#[test]
fn shift_click_selects_the_range_from_the_last_click() {
    let (doc, reg, mut v) = setup();
    click_name(&mut v, &doc, &reg, "ch-bass");
    click_name_with(&mut v, &doc, &reg, "ch-keys", Modifiers::SHIFT);
    assert_eq!(
        v.selected_strips(),
        [key("ch-bass"), key("ch-lead"), key("ch-keys")]
    );
}

#[test]
fn a_drag_moves_a_strip_without_selecting_it_and_a_removed_strip_leaves_the_selection() {
    let (doc, reg, mut v) = setup();
    drag_name(&mut v, &doc, &reg, "ch-bass", 400.0);
    assert!(v.selected_strips().is_empty());
    click_name(&mut v, &doc, &reg, "bus-2");
    let mut doc2 = doc.clone();
    doc2.console.buses.retain(|b| b.bus != key("bus-2"));
    v.sync(&doc2, &console_registry());
    assert!(v.selected_strips().is_empty());
}

#[test]
fn selected_strips_are_painted_and_the_host_can_set_them() {
    let (doc, reg, mut v) = setup();
    let mut before = DisplayList::new();
    v.paint(&mut before, &doc, &reg);
    v.set_selected_strips(&[key("bus"), key("nope")]);
    assert_eq!(v.selected_strips(), [key("bus")]);
    let mut after = DisplayList::new();
    v.paint(&mut after, &doc, &reg);
    assert_ne!(before.to_json(), after.to_json());
}

// ---------------------------------------------------------------- the master column

#[test]
fn faders_of_the_strips_and_the_master_line_up_with_the_selector_row() {
    let (_doc, _reg, v) = setup();
    let ch = v.strip_layout().unwrap();
    let master = v.master_layout().unwrap();
    let out = ch.output.expect("a strip has a selector row");
    assert!(out.y1 <= ch.name.y0 && out.y0 > ch.pinned_top);
    let fader = |l: &[atomik_ui::console_layout::PlacedCtrl]| {
        l.iter().find(|c| c.param == "fader").unwrap().rect
    };
    let (a, b) = (fader(&ch.controls), fader(&master.controls));
    assert!((a.y0 - b.y0).abs() < 1e-9 && (a.y1 - b.y1).abs() < 1e-9);
    assert!(master.output_row.y1 <= master.name.y0);
}

#[test]
fn the_master_selector_row_reads_the_main_outputs() {
    let (doc, reg, mut v) = setup();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"MAIN 1+2"), "{:?}", dl.texts());
    let _ = Vec2::ZERO;
}

#[test]
fn a_rejected_command_is_shown_in_the_toolbar_until_the_next_click() {
    let (doc, reg, mut v) = setup();
    assert_eq!(v.notice(), None);
    v.notify_rejected("a bus is already called \"Drums\"; bus names are unique");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(
        dl.texts().iter().any(|t| t.contains("already called")),
        "{:?}",
        dl.texts()
    );
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    v.handle(&down(p), &doc, &reg);
    assert_eq!(v.notice(), None);
}

// ---------------------------------------------------------------- golden

#[test]
fn console_with_buses_matches_golden() {
    let (doc, reg) = (bus_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    v.set_latency_readouts(&[(key("bus"), 128)]);
    let m = ChannelMeters {
        out_peak_l: 0.5,
        out_peak_r: 0.9,
        out_rms_l: 0.2,
        out_rms_r: 0.4,
        gr: 0.8,
    };
    v.set_meters(&[(key("bus"), m), (key("master"), m)]);
    v.set_frame_time(1.0);
    v.set_selected_strips(&[key("bus-2")]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    starter::check_golden("console_buses_1x.json", &dl.to_json());
}
