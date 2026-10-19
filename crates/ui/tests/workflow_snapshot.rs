//! Display-list goldens of the workflow widgets: the tool palette in the arrangement and
//! the piano roll (the active tool highlighted, the alternate tool while its modifier is held)
//! and the device browser in create mode. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test workflow_snapshot`.

mod support;

use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::{Os, Roles};
use atomik_ui::{DisplayList, Modifiers, RackView, Tool, UiEvent};
use std::path::PathBuf;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn arrangement_with(tool: Tool, held: Option<Modifiers>) -> String {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_tool(tool);
    if let Some(modifiers) = held {
        let _ = v.handle(&UiEvent::ModifiersChanged { modifiers }, &doc, &reg);
    }
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn roll_with(tool: Tool) -> String {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_tool(tool);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn browser_in_create_mode(kind: CreateKind) -> String {
    let (doc, reg) = (reference_doc(), registry());
    let mut v: RackView = view(&doc, &reg);
    v.open_browser_create(kind, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

#[test]
fn arrangement_toolbar_with_the_razor_matches_golden() {
    check(
        "workflow_arrangement_razor_1x.json",
        &arrangement_with(Tool::Razor, None),
    );
}

#[test]
fn arrangement_toolbar_shows_the_alternate_tool_while_alt_is_held() {
    let held = Some(Modifiers {
        alt: true,
        ..Modifiers::NONE
    });
    check(
        "workflow_arrangement_alternate_1x.json",
        &arrangement_with(Tool::Select, held),
    );
    assert_ne!(
        arrangement_with(Tool::Select, held),
        arrangement_with(Tool::Select, None),
        "the toolbar changes with the held modifier"
    );
}

#[test]
fn piano_roll_toolbar_with_the_eraser_matches_golden() {
    check("workflow_roll_eraser_1x.json", &roll_with(Tool::Eraser));
}

#[test]
fn create_browser_for_instruments_matches_golden() {
    check(
        "browser_create_instrument_1x.json",
        &browser_in_create_mode(CreateKind::Instrument),
    );
}

#[test]
fn create_browser_for_effects_matches_golden() {
    check(
        "browser_create_effect_1x.json",
        &browser_in_create_mode(CreateKind::Effect),
    );
    assert_ne!(
        browser_in_create_mode(CreateKind::Effect),
        browser_in_create_mode(CreateKind::Instrument)
    );
}

#[test]
fn the_painting_is_deterministic() {
    assert_eq!(
        arrangement_with(Tool::Razor, None),
        arrangement_with(Tool::Razor, None)
    );
    assert_eq!(roll_with(Tool::Razor), roll_with(Tool::Razor));
}

// ---------------------------------------------------------------- rack selection

#[test]
fn rack_with_a_multi_selection_matches_golden() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v: RackView = view(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_selection(&[key("synth"), key("mixer")], Some(&key("mixer")));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("rack_selection_multi_1x.json", &dl.to_json());
}

#[test]
fn rack_with_one_selected_device_matches_golden() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v: RackView = view(&doc, &reg);
    v.set_selected(Some(key("delay")));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("rack_selection_single_1x.json", &dl.to_json());
}

#[test]
fn rack_group_drag_dims_every_moving_device_and_matches_golden() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v: RackView = view(&doc, &reg);
    v.set_selection(&[key("synth"), key("delay")], Some(&key("delay")));
    let from = v.plate_center(&key("synth")).unwrap();
    let to = vello::kurbo::Point::new(from.x, v.content_height() - 4.0);
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(to), &doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("rack_group_drag_1x.json", &dl.to_json());
}
