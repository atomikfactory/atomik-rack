//! The level bar on a folder header: it shows for a folder that plays into a bus (the bus fader)
//! or has a VCA group following it (the group level), and nowhere else; a press sets the level,
//! a drag is one undo step, Esc puts the level back and a double-click resets it.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ConsoleCommand, Document, SongCommand};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{ArrangementView, Key, Modifiers, UiAction, UiEvent};
use support::console::*;
use support::song::*;
use support::*;

fn setup() -> (Document, Registry, ArrangementView) {
    let reg = console_registry();
    let mut d = Document::new(bus_doc());
    for k in ["bass", "lead", "keys"] {
        d.apply(
            Command::Song(SongCommand::AddTrack { device: key(k) }),
            &reg,
        )
        .unwrap();
    }
    d.apply(
        Command::Song(SongCommand::GroupTracks {
            devices: vec![key("bass"), key("lead")],
            name: "Rhythm".into(),
        }),
        &reg,
    )
    .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    v.set_bounds(Rect::new(0.0, 0.0, 1200.0, 700.0));
    v.sync(d.doc(), &reg);
    (d, reg, v)
}

fn console(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn route(d: &mut Document, reg: &Registry, v: &mut ArrangementView) {
    d.apply(
        console(ConsoleCommand::RouteFolderToBus {
            folder: 1,
            bus: key("bus"),
        }),
        reg,
    )
    .unwrap();
    v.sync(d.doc(), reg);
}

fn run(d: &Document, reg: &Registry, v: &mut ArrangementView, e: &[UiEvent]) -> Vec<UiAction> {
    e.iter().flat_map(|e| v.handle(e, d.doc(), reg)).collect()
}

fn fader_value(c: &Command) -> Option<(f32, Option<u64>)> {
    match c {
        Command::SetParam {
            key: k,
            param,
            value,
            merge,
        } if k.0 == "bus" && param == "fader" => Some((*value, *merge)),
        _ => None,
    }
}

#[test]
fn a_folder_with_no_bus_and_no_group_has_no_bar() {
    let (d, reg, mut v) = setup();
    assert_eq!(v.folder_fader_bar(1), None);
    let header = v.folder_header_center(1).unwrap();
    let low = Point::new(header.x, header.y + 11.0);
    let a = run(&d, &reg, &mut v, &[down(low), up(low)]);
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn a_routed_folder_shows_the_bus_fader_and_a_drag_sets_it_in_one_step() {
    let (mut d, reg, mut v) = setup();
    route(&mut d, &reg, &mut v);
    let bar = v.folder_fader_bar(1).expect("a bar");
    let at = |f: f64| Point::new(bar.x0 + bar.width() * f, bar.center().y);
    let a = run(&d, &reg, &mut v, &[down(at(0.2)), mv(at(0.6)), up(at(0.6))]);
    let sent: Vec<(f32, Option<u64>)> = commands(&a).iter().filter_map(fader_value).collect();
    assert_eq!(sent.len(), 2, "{a:?}");
    assert!((sent[0].0 - 0.2).abs() < 0.02 && (sent[1].0 - 0.6).abs() < 0.02);
    assert!(
        sent[0].1.is_some() && sent[0].1 == sent[1].1,
        "one merge group"
    );
    assert!(a.iter().any(|a| matches!(a, UiAction::EndMerge)));
    // What the view sent moves the bus fader, and the undo takes it back in one step.
    let before = d.doc().clone();
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    assert!((d.doc().param_value(&key("bus"), "fader", &reg).unwrap() - 0.6).abs() < 0.02);
    assert!(d.undo().is_some());
    assert_eq!(d.doc(), &before, "the drag is one undo step");
}

#[test]
fn a_double_click_puts_the_level_at_unity_and_escape_puts_it_back() {
    let (mut d, reg, mut v) = setup();
    route(&mut d, &reg, &mut v);
    let bar = v.folder_fader_bar(1).unwrap();
    let at = |f: f64| Point::new(bar.x0 + bar.width() * f, bar.center().y);
    let a = run(
        &d,
        &reg,
        &mut v,
        &[
            down_with(at(0.3), atomik_ui::PointerButton::Primary, 2),
            up(at(0.3)),
        ],
    );
    let sent: Vec<_> = commands(&a).iter().filter_map(fader_value).collect();
    assert_eq!(sent, [(0.75, None)], "{a:?}");

    let original = d.doc().param_value(&key("bus"), "fader", &reg).unwrap();
    let a = run(&d, &reg, &mut v, &[down(at(0.1)), mv(at(0.4))]);
    assert!(!commands(&a).is_empty());
    let escape = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    let a = run(&d, &reg, &mut v, &[escape]);
    let last = commands(&a)
        .iter()
        .filter_map(fader_value)
        .next_back()
        .unwrap();
    assert!((last.0 - original).abs() < 1e-4, "{last:?} vs {original}");
}

#[test]
fn a_followed_folder_shows_the_group_level() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        console(ConsoleCommand::CreateVcaForFolder {
            folder: 1,
            name: "Rhythm".into(),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    let bar = v.folder_fader_bar(1).expect("a bar");
    let a = run(
        &d,
        &reg,
        &mut v,
        &[down(Point::new(bar.x0 + bar.width() * 0.5, bar.center().y))],
    );
    let cmds = commands(&a);
    let [Command::Console(ConsoleCommand::SetVcaLevel { vca, db, merge })] = &cmds[..] else {
        panic!("{cmds:?}");
    };
    assert_eq!(*vca, 1);
    assert!((*db - -24.0).abs() <= 0.5, "{db}");
    assert!(merge.is_some());
}

#[test]
fn the_context_menu_knows_whether_the_folder_plays_into_a_bus() {
    let (mut d, reg, mut v) = setup();
    let header = v.folder_header_center(1).unwrap();
    let _ = run(&d, &reg, &mut v, &[down(header), up(header)]);
    assert!(!v.org_menu().folder_routed);
    route(&mut d, &reg, &mut v);
    assert!(v.org_menu().folder_routed);
}
