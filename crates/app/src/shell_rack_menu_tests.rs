//! The rack's Create submenu with the device types, the Replace Device menu item, and a row of
//! the device browser dragged onto a track header, through a headless `Shell`.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, CreateOptions, CreateSource, CreateTarget, DeviceKey};
use atomik_ui::context_menu::Entry;
use atomik_ui::{ContextTarget, Modifiers, PointerButton};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";

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
    s.handle(&UiEvent::Resize(Size::new(1400.0, 2800.0)));
    s.core_mut().view_mut().set_navigator_visible(false);
    s.set_focus(Pane::Rack);
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A synth with its track and channel, and a delay after it on the channel.
fn with_synth_and_delay(s: &mut Shell<NullBackend>) {
    let create = |source: &str, target: CreateTarget, k: &str, instrument: bool| {
        let source = CreateSource::Type(source.into());
        let options = CreateOptions::default();
        Command::Workflow(if instrument {
            atomik_model::WorkflowCommand::CreateInstrument {
                index: 0,
                source,
                key: Some(key(k)),
                options,
            }
        } else {
            atomik_model::WorkflowCommand::CreateEffect {
                source,
                target,
                key: Some(key(k)),
                options,
            }
        })
    };
    let core = s.core_mut();
    core.apply_command(create(SYNTH, CreateTarget::Nothing, "synth", true))
        .unwrap();
    core.apply_command(create(
        DELAY,
        CreateTarget::Selected(key("synth")),
        "delay",
        false,
    ))
    .unwrap();
}

fn open_device_menu(s: &mut Shell<NullBackend>, device: &str) {
    s.core_mut().select(Some(key(device)));
    s.open_context_menu(Pane::Rack, ContextTarget::Device, Point::new(300.0, 300.0));
}

fn menu_args(s: &Shell<NullBackend>, command: &str) -> Vec<String> {
    s.context_menu()
        .expect("a menu")
        .all_items()
        .into_iter()
        .filter(|i| i.command == command)
        .filter_map(|i| i.arg.clone())
        .collect()
}

fn type_of(s: &Shell<NullBackend>, device: &str) -> String {
    s.core().doc().device(&key(device)).unwrap().type_id.clone()
}

fn saved(s: &Shell<NullBackend>) -> String {
    atomik_model::save_project(s.core().doc())
}

// ----------------------------------------------------------------------------- Create

#[test]
fn the_create_submenu_lists_the_families_and_then_every_device_type_by_family() {
    let mut s = shell();
    s.open_context_menu(Pane::Rack, ContextTarget::Rack, Point::new(300.0, 300.0));
    let menu = s.context_menu().expect("the rack menu");
    let create = menu
        .entries()
        .iter()
        .find_map(|e| match e {
            Entry::Submenu(sub) if sub.label == "Create" => Some(sub),
            _ => None,
        })
        .expect("a Create submenu");
    let labels: Vec<&str> = create
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some(i.command.as_str()),
            Entry::Submenu(s) => Some(s.label.as_str()),
            Entry::Separator => None,
        })
        .collect();
    assert_eq!(
        labels,
        [
            "create.instrument",
            "create.effect",
            "create.player",
            "Instruments",
            "Effects",
            "Utilities",
            "Modulators",
            "Players",
            "Containers"
        ]
    );
    let args = menu_args(&s, "create.device");
    for t in [SYNTH, DELAY, "atomik.arpeggiator", "atomik.container"] {
        assert!(args.iter().any(|a| a == t), "{t} is listed");
    }
    // No console device, no duplicate.
    assert!(!args.iter().any(|a| a == "atomik.channel"));
    let mut sorted = args.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), args.len());
    // The device menu has the same submenu, and the rack menu no Replace Device.
    assert!(menu_args(&s, "rack.replace_device").is_empty());
}

#[test]
fn choosing_a_device_type_creates_it_as_the_browser_does_in_one_undo_step() {
    let mut s = shell();
    let start = saved(&s);
    s.run_command("create.device", Some(SYNTH));
    let doc = s.core().doc();
    let synth = doc
        .devices
        .iter()
        .find(|d| d.type_id == SYNTH)
        .expect("the instrument")
        .key
        .clone();
    assert!(
        doc.console.channel_of(&synth).is_some(),
        "routed like Enter"
    );
    assert!(doc.song.track(&synth).is_some(), "with its track");
    // An effect goes after the selected device.
    s.core_mut().select(Some(synth.clone()));
    s.run_command("create.device", Some(DELAY));
    assert!(s.core().doc().devices.iter().any(|d| d.type_id == DELAY));
    assert!(s.core_mut().undo());
    assert!(!s.core().doc().devices.iter().any(|d| d.type_id == DELAY));
    assert!(s.core_mut().undo());
    assert_eq!(saved(&s), start, "one undo per device");
    // An unknown type is a message and no edit.
    s.run_command("create.device", Some("atomik.nothing"));
    assert!(!s.core().document().can_redo() || saved(&s) == start);
}

// ---------------------------------------------------------------------------- Replace

#[test]
fn the_device_menu_lists_only_the_types_the_device_can_become() {
    let mut s = shell();
    with_synth_and_delay(&mut s);
    open_device_menu(&mut s, "delay");
    let args = menu_args(&s, "rack.replace_device");
    assert!(args.iter().any(|a| a == "atomik.reverb"));
    assert!(!args.iter().any(|a| a == DELAY), "not its own type");
    assert!(!args.iter().any(|a| a == SYNTH), "not another family");
    assert!(!args.iter().any(|a| a == "atomik.arpeggiator"));
    // An instrument becomes another instrument.
    open_device_menu(&mut s, "synth");
    let args = menu_args(&s, "rack.replace_device");
    assert!(args.iter().any(|a| a == "atomik.mono-bass"));
    assert!(!args.iter().any(|a| a == "atomik.reverb"));
    // Two devices selected: nothing to replace, the row is greyed.
    s.core_mut()
        .view_mut()
        .set_selection(&[key("synth"), key("delay")], Some(&key("synth")));
    s.open_context_menu(Pane::Rack, ContextTarget::Device, Point::new(300.0, 300.0));
    assert!(menu_args(&s, "rack.replace_device").is_empty());
    let greyed = s
        .context_menu()
        .unwrap()
        .all_items()
        .into_iter()
        .any(|i| i.command == "rack.replace_device" && !i.enabled);
    assert!(greyed);
}

#[test]
fn replace_device_keeps_the_cables_and_the_shared_values_in_one_undo_step() {
    let mut s = shell();
    with_synth_and_delay(&mut s);
    let set = |s: &mut Shell<NullBackend>, param: &str, value: f32| {
        s.core_mut()
            .apply_command(atomik_model::Command::SetParam {
                key: key("delay"),
                param: param.into(),
                value,
                merge: None,
            })
            .unwrap();
    };
    set(&mut s, "mix", 0.45);
    let before = saved(&s);
    let cables = s.core().doc().cables.clone();
    s.core_mut().select(Some(key("delay")));
    s.run_command("rack.replace_device", Some("atomik.reverb"));
    assert_eq!(type_of(&s, "delay"), "atomik.reverb");
    assert_eq!(s.core().doc().cables, cables, "the stereo cables fit");
    assert_eq!(
        s.core()
            .doc()
            .device(&key("delay"))
            .unwrap()
            .params
            .get("mix"),
        Some(&0.45),
        "mix exists on both"
    );
    assert!(s.core_mut().undo());
    assert_eq!(saved(&s), before, "one undo puts the delay back");
    // Another family is refused with a message and changes nothing.
    s.run_command("rack.replace_device", Some(SYNTH));
    assert_eq!(saved(&s), before);
    assert!(s.core().last_rejection().is_some());
}

// ------------------------------------------------------- a browser row onto a track header

fn drag_row_to(s: &mut Shell<NullBackend>, type_id: &str, to: Point) -> Vec<Point> {
    s.core_mut().view_mut().set_browser_visible(true);
    let origin = s.layout().rack.origin();
    let row = {
        let view = s.core().view();
        let i = view
            .browser_rows(s.core().registry())
            .iter()
            .position(|t| *t == type_id)
            .expect("row");
        view.browser_row_center(i).expect("browser open")
    };
    let from = Point::new(row.x + origin.x, row.y + origin.y);
    let mouse = |s: &mut Shell<NullBackend>, e: UiEvent| {
        s.handle(&e);
    };
    mouse(
        s,
        UiEvent::PointerDown {
            pos: from,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
    );
    mouse(
        s,
        UiEvent::PointerMove {
            pos: Point::new(from.x + 60.0, from.y + 60.0),
            modifiers: Modifiers::NONE,
        },
    );
    mouse(
        s,
        UiEvent::PointerMove {
            pos: to,
            modifiers: Modifiers::NONE,
        },
    );
    vec![from, to]
}

fn release(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn a_browser_row_dropped_on_a_track_header_replaces_the_tracks_device_in_one_step() {
    let mut s = shell();
    with_synth_and_delay(&mut s);
    let before = saved(&s);
    let header = s
        .core()
        .arrangement()
        .track_header_center(&key("synth"))
        .expect("the track header is shown");
    let points = drag_row_to(&mut s, "atomik.mono-bass", header);
    assert_eq!(
        s.core().arrangement().header_drop_target(),
        Some(&key("synth")),
        "the header is outlined while the row hovers over it"
    );
    assert_eq!(
        s.drag_cursor(),
        Some(s.core().view().cursor()),
        "not the not-allowed cursor of another pane"
    );
    release(&mut s, points[1]);
    assert_eq!(type_of(&s, "synth"), "atomik.mono-bass");
    assert_eq!(s.core().arrangement().header_drop_target(), None);
    assert!(s.core().doc().song.track(&key("synth")).is_some());
    assert!(s.core_mut().undo());
    assert_eq!(saved(&s), before, "one undo step");
}

#[test]
fn a_row_of_another_family_or_released_elsewhere_replaces_nothing() {
    let mut s = shell();
    with_synth_and_delay(&mut s);
    let before = saved(&s);
    let header = s
        .core()
        .arrangement()
        .track_header_center(&key("synth"))
        .expect("header");
    // An effect cannot replace an instrument: no outline, no edit.
    let points = drag_row_to(&mut s, "atomik.reverb", header);
    assert_eq!(s.core().arrangement().header_drop_target(), None);
    release(&mut s, points[1]);
    assert_eq!(saved(&s), before);
    // A fitting row that moves away from the header again drops its outline.
    let points = drag_row_to(&mut s, "atomik.mono-bass", header);
    assert!(s.core().arrangement().header_drop_target().is_some());
    let away = Point::new(points[0].x, points[0].y + 40.0);
    s.handle(&UiEvent::PointerMove {
        pos: away,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.core().arrangement().header_drop_target(), None);
    release(&mut s, away);
    assert_eq!(type_of(&s, "synth"), SYNTH);
}
