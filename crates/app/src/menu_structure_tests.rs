//! The shape of the menus: how long each is, that every command is still reachable after the
//! menus were regrouped, and the menu tree as a golden for review.

use super::tests::state;
use crate::commands::{AppCommand, Menu, MenuEntry, SongList, menus_with_songs};
use atomik_ui::keymap::{Keymap, Os, Profile};

/// The most rows (separators and submenu entries included) a top-level menu may have: a longer
/// one is split into submenus.
const MAX_ROWS: usize = 30;

/// The menus with every list filled: recent songs, templates, open songs, groups and buses.
fn full_model() -> Vec<Menu> {
    let keymap = Keymap::bundled(Profile::Standard, Os::Windows);
    let recent: Vec<String> = (0..8).map(|i| format!("song {i}")).collect();
    let templates: Vec<String> = (0..16).map(|i| format!("template {i}")).collect();
    let songs: Vec<String> = (0..4).map(|i| format!("open song {i}")).collect();
    let vcas = vec![(1, "Drums".to_owned()), (2, "Bass".to_owned())];
    let buses = vec![(1, "Bus A".to_owned()), (2, "Bus B".to_owned())];
    let list = SongList {
        names: &songs,
        current: 0,
        vcas: &vcas,
        buses: &buses,
    };
    menus_with_songs(&state(), &keymap, &recent, &templates, &list)
}

/// One line per item, `command | label`, sorted: the menus' content without their place.
fn commands_text(menus: &[Menu]) -> String {
    let mut lines: Vec<String> = menus
        .iter()
        .flat_map(Menu::items)
        .map(|i| format!("{:?} | {}", i.command, i.label))
        .collect();
    lines.sort();
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn write_tree(entries: &[MenuEntry], depth: usize, out: &mut String) {
    for entry in entries {
        let pad = "  ".repeat(depth);
        match entry {
            MenuEntry::Separator => out.push_str(&format!("{pad}---\n")),
            MenuEntry::Item(i) => out.push_str(&format!("{pad}{}\n", i.label)),
            MenuEntry::Submenu(s) => {
                out.push_str(&format!("{pad}{} >\n", s.label));
                write_tree(&s.entries, depth + 1, out);
            }
        }
    }
}

/// The menus as an indented tree: titles, submenus and labels.
fn tree_text(menus: &[Menu]) -> String {
    let mut out = String::new();
    for m in menus {
        out.push_str(&format!("{}\n", m.title));
        write_tree(&m.entries, 1, &mut out);
    }
    out
}

fn golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
        path.display()
    );
}

#[test]
fn every_command_is_still_in_the_menus_exactly_as_before() {
    golden("menu_commands.txt", &commands_text(&full_model()));
}

#[test]
fn the_menu_tree_matches_the_golden() {
    golden("menu_tree.txt", &tree_text(&full_model()));
}

#[test]
fn a_command_is_in_one_menu_only_apart_from_the_options_menu() {
    // The Options menu repeats commands of other menus on purpose; no other menu does.
    let model = full_model();
    let mut seen: Vec<(String, String)> = Vec::new();
    for m in model.iter().filter(|m| m.title != "Options") {
        for i in m.items() {
            let at = (format!("{:?}", i.command), i.label.to_string());
            assert!(
                !seen.contains(&at),
                "{} appears twice (second time in {})",
                i.label,
                m.title
            );
            seen.push(at);
        }
    }
}

#[test]
fn no_top_level_menu_is_longer_than_the_limit() {
    let plain = {
        let keymap = Keymap::bundled(Profile::Standard, Os::Windows);
        crate::commands::menus(&state(), &keymap)
    };
    for model in [plain, full_model()] {
        for m in &model {
            assert!(
                m.entries.len() <= MAX_ROWS,
                "{} has {} rows, the limit is {MAX_ROWS}",
                m.title,
                m.entries.len()
            );
        }
    }
    assert_eq!(MAX_ROWS, crate::commands::MAX_MENU_ROWS);
}

#[test]
fn analyze_follows_mixer_and_holds_what_was_moved_there() {
    let model = full_model();
    let titles: Vec<_> = model.iter().map(|m| m.title).collect();
    assert_eq!(
        titles,
        [
            "File",
            "Edit",
            "View",
            "Transport",
            "Mixer",
            "Analyze",
            "Create",
            "Track",
            "Audio",
            "Options",
            "Window",
            "Help"
        ]
    );
    let moved = [
        AppCommand::SoundAnalyst,
        AppCommand::ReferencePanel,
        AppCommand::ToggleReference,
        AppCommand::AddReference,
        AppCommand::CheckMix,
        AppCommand::MaskingMap,
        AppCommand::KickBassCheck,
        AppCommand::BassRead,
        AppCommand::FindRepetition,
        AppCommand::DeliveryTargets,
        AppCommand::LensMono,
    ];
    for m in &model {
        for c in moved {
            let here = m.items().any(|i| i.command == c);
            assert_eq!(here, m.title == "Analyze", "{c:?} in {}", m.title);
        }
    }
}

#[test]
fn submenu_titles_are_plain_words() {
    fn walk(entries: &[MenuEntry], at: &str) {
        for e in entries {
            if let MenuEntry::Submenu(s) = e {
                assert!(!s.label.is_empty() && s.label.trim() == s.label, "{at}");
                assert!(
                    s.label
                        .chars()
                        .all(|c| c.is_alphanumeric() || " -/".contains(c)),
                    "{at} > {}",
                    s.label
                );
                walk(&s.entries, s.label);
            }
        }
    }
    for m in &full_model() {
        walk(&m.entries, m.title);
    }
}
