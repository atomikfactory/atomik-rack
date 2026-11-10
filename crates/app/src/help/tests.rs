//! The long help text: the lookup by id, the fallback to the tooltip, the parameter cards, the
//! coverage of the device types, and the consistency of the data files with the views that use
//! their ids.

use super::*;
use crate::app_core::default_registry;
use atomik_ui::keymap::{Keymap, Os, Profile};
use atomik_ui::kurbo::Rect;
use std::path::{Path, PathBuf};

fn keymap() -> Keymap {
    Keymap::bundled(Profile::Standard, Os::Windows)
}

fn help(text: &'static str) -> Help {
    Help::new(Rect::new(0.0, 0.0, 10.0, 10.0), text)
}

#[test]
fn the_shipped_files_parse_and_name_real_manual_sections() {
    let book = Book::parse(CONTROLS, DEVICES).expect("both files are valid JSON");
    assert!(book.control_ids().count() > 100);
    for id in book.control_ids() {
        let entry = book.control(id).unwrap();
        assert!(!entry.title.trim().is_empty(), "{id} has a title");
        assert!(!entry.text.trim().is_empty(), "{id} has a text");
        if let Some(section) = &entry.more {
            assert!(section_url(section).is_some(), "{id}: no section {section}");
            assert!(
                crate::manual::manual().find_heading(section).is_some(),
                "{id}: the manual has no section {section}"
            );
        }
    }
    // The shipped copy is the one the program reads.
    assert_eq!(
        super::book().control_ids().count(),
        book.control_ids().count()
    );
}

#[test]
fn every_entry_is_a_few_plain_sentences() {
    for id in super::book().control_ids() {
        let entry = super::book().control(id).unwrap();
        let sentences = entry.text.matches(['.', '!', '?']).count();
        assert!(
            (1..=4).contains(&sentences),
            "{id}: {sentences} sentences, wanted 1 to 4"
        );
        assert!(entry.title.chars().count() <= 40, "{id}: title too long");
        assert!(entry.text.chars().count() <= 330, "{id}: text too long");
    }
}

#[test]
fn a_control_id_finds_its_entry_with_the_live_shortcut() {
    let found = resolve(
        &help("Play, or stop while playing")
            .with_command("transport.play_stop")
            .id("transport.play"),
        &keymap(),
        &default_registry(),
    );
    assert_eq!(found.card.title, "Play");
    assert!(found.card.text.starts_with("Starts the song"));
    assert_eq!(found.card.shortcut.as_deref(), Some("Space"));
    assert_eq!(found.card.more.as_deref(), Some("The main window"));
    assert_eq!(found.url, None, "the link opens the built-in manual");
    assert_eq!(found.manual.as_deref(), Some("The main window"));
    // A control with no command has no shortcut.
    let tempo = resolve(
        &help("Tempo in BPM").id("transport.tempo"),
        &keymap(),
        &default_registry(),
    );
    assert_eq!(tempo.card.title, "Tempo");
    assert_eq!(tempo.card.shortcut, None);
}

#[test]
fn a_control_without_an_entry_falls_back_to_its_tooltip() {
    let registry = default_registry();
    let plain = resolve(&help("Go to this folder"), &keymap(), &registry);
    assert_eq!(plain.card.title, "Go to this folder");
    assert_eq!(plain.card.text, "");
    assert_eq!(plain.url, None);
    // The text before the first colon is the name, the rest its line.
    let split = resolve(
        &help("Mute this lane: the parameter keeps its value").id("no.such.id"),
        &keymap(),
        &registry,
    );
    assert_eq!(split.card.title, "Mute this lane");
    assert_eq!(split.card.text, "The parameter keeps its value.");
    assert_eq!(split.card.more, None);
    // The command's shortcut still shows.
    let bound = resolve(
        &help("Stop").with_command("transport.stop"),
        &keymap(),
        &registry,
    );
    assert_eq!(bound.card.title, "Stop");
    assert_eq!(
        bound.card.shortcut,
        keymap().shortcut_text("transport.stop", None)
    );
    // A tooltip of several lines (the status line) keeps its first line.
    let status = resolve(
        &help("Engine load 3 %\nHeaviest devices:"),
        &keymap(),
        &registry,
    );
    assert_eq!(status.card.title, "Engine load 3 %");
}

#[test]
fn a_parameter_card_has_its_name_its_range_and_the_device_about_line() {
    let registry = default_registry();
    let desc = registry.descriptor("atomik.compressor").unwrap();
    let ratio = desc
        .params
        .iter()
        .find(|p| p.unit == Unit::None && !p.discrete)
        .expect("a plain number");
    let found = resolve(
        &help("label").id(format!("param:atomik.compressor:{}", ratio.key)),
        &keymap(),
        &registry,
    );
    assert_eq!(found.card.title, ratio.name);
    assert!(found.card.text.contains("From "), "{}", found.card.text);
    assert!(
        found.card.text.contains(&format!("{}: ", desc.name)),
        "{}",
        found.card.text
    );
    assert!(
        found.card.text.contains("compressor"),
        "{}",
        found.card.text
    );
    assert_eq!(found.card.more.as_deref(), Some("Devices"));
    // A toggle says how it is operated and that it is on or off.
    let bypass = desc.params.iter().find(|p| p.key == "bypass").unwrap();
    let toggle = resolve(
        &help("Bypass: click to switch on or off")
            .id(format!("param:atomik.compressor:{}", bypass.key)),
        &keymap(),
        &registry,
    );
    assert!(toggle.card.text.starts_with("Click to switch on or off."));
    assert!(toggle.card.text.contains("either on or off"));
    // An id of a parameter that does not exist falls back to the tooltip.
    let missing = resolve(
        &help("Whatever: it is a knob").id("param:atomik.compressor:no.such.key"),
        &keymap(),
        &registry,
    );
    assert_eq!(missing.card.title, "Whatever");
}

#[test]
fn the_range_text_says_choices_toggles_and_numbers() {
    let registry = default_registry();
    let mut seen = [false; 3];
    for id in registry.type_ids() {
        for p in registry.descriptor(id).unwrap().params {
            let text = range_text(p);
            match p.unit {
                Unit::Toggle => {
                    assert_eq!(text, "It is either on or off.");
                    seen[0] = true;
                }
                Unit::Choice(names) => {
                    assert!(text.contains(names[0]), "{text}");
                    seen[1] = true;
                }
                _ => {
                    assert!(text.starts_with("From ") && text.ends_with('.'), "{text}");
                    seen[2] = true;
                }
            }
        }
    }
    assert_eq!(seen, [true; 3]);
}

#[test]
fn every_device_type_has_an_about_line_and_none_is_left_over() {
    let registry = default_registry();
    let types: Vec<&str> = registry.type_ids().collect();
    assert!(types.len() >= 60);
    for id in &types {
        let about = super::book()
            .about(id)
            .unwrap_or_else(|| panic!("{id} has no about line"));
        assert!(about.chars().count() >= 40, "{id}: about line too short");
        assert!(about.chars().count() <= 260, "{id}: about line too long");
    }
    for id in super::book().device_ids() {
        assert!(types.contains(&id), "{id} is no device type");
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The control ids the views of the interface crate name: the string literals with one of the
/// prefixes of the help ids. The second list holds those that are not command ids (a command id
/// may also be a control id, as `transport.play` is, and then is looked up like any other).
fn ids_in_the_views() -> (Vec<String>, Vec<String>) {
    const PREFIXES: [&str; 7] = [
        "transport.",
        "arrangement.",
        "console.",
        "browser.",
        "roll.",
        "launcher.",
        "rack.",
    ];
    let commands = atomik_ui::keymap::Registry::bundled();
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src"),
        &mut files,
    );
    let (mut all, mut plain) = (Vec::new(), Vec::new());
    for file in files {
        let name = file.to_string_lossy().replace('\\', "/");
        if name.contains("_tests.rs") || name.contains("/keymap/") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for part in text.split('"').skip(1).step_by(2) {
            let ok = PREFIXES.iter().any(|p| part.starts_with(p))
                && !part.ends_with('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_');
            if !ok {
                continue;
            }
            if !all.iter().any(|i| i == part) {
                all.push(part.to_owned());
            }
            if commands.get(part).is_none() && !plain.iter().any(|i| i == part) {
                plain.push(part.to_owned());
            }
        }
    }
    all.sort();
    plain.sort();
    (all, plain)
}

#[test]
fn every_id_a_view_gives_has_an_entry_and_no_entry_is_left_over() {
    let (all, plain) = ids_in_the_views();
    assert!(all.len() > 100, "the views name {} ids", all.len());
    let missing: Vec<&String> = plain
        .iter()
        .filter(|id| super::book().control(id).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "controls.json has no entry for the control ids {missing:?}"
    );
    let stale: Vec<&str> = super::book()
        .control_ids()
        // The two ids the shell itself gives are not in the views of the interface crate.
        .filter(|id| !matches!(*id, "help.idle" | "status.load") && !all.iter().any(|i| i == id))
        .collect();
    assert!(stale.is_empty(), "no view uses the entries {stale:?}");
}

#[test]
fn every_anchor_the_help_text_links_to_exists_in_the_manual() {
    let manual = crate::manual::manual();
    for section in sections() {
        let at = manual
            .find_heading(section)
            .unwrap_or_else(|| panic!("the manual has no section {section}"));
        let (_, _, anchor) = manual.heading_at(at).expect("a heading");
        // The address of the section in the browser names the same anchor as the manual's.
        if let Some((_, tail)) = section_url(section).and_then(|u| u.split_once("README.md#")) {
            assert_eq!(tail, anchor, "{section}");
        }
        assert_eq!(
            manual.resolve(0, &format!("README.md#{anchor}")),
            atomik_ui::manual::Link::Here(at),
            "{section}"
        );
    }
}

#[test]
fn the_more_link_of_a_card_opens_the_manual_not_the_browser() {
    let found = resolve(
        &help("Play").id("transport.play"),
        &keymap(),
        &default_registry(),
    );
    assert_eq!(found.url, None);
    assert_eq!(found.manual.as_deref(), Some("The main window"));
    let device = resolve(
        &help("Bypass: click").id("param:atomik.compressor:bypass"),
        &keymap(),
        &default_registry(),
    );
    assert_eq!(device.manual.as_deref(), Some("Devices"));
}
