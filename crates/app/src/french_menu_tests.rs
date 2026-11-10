//! The menus in French: every fixed label has its French text, the menus keep their shape, none
//! is wider than the screen, and the French tree and command list are goldens for review
//! (`tests/golden/fr/`, regenerate with `ATOMIK_UPDATE_GOLDEN=1`). The English menus are the
//! goldens of [`super::structure_tests`]; they must not change when French is added.

use super::tests::state;
use crate::commands::{Menu, MenuEntry, SongList, menus_with_songs};
use atomik_ui::Lang;
use atomik_ui::i18n;
use atomik_ui::keymap::{Keymap, Os, Profile};
use std::borrow::Cow;

/// The menus with every list filled, built while `lang` is the language.
fn model_in(lang: Lang) -> Vec<Menu> {
    i18n::with_language(lang, || {
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
            presets: &[],
        };
        menus_with_songs(&state(), &keymap, &recent, &templates, &list)
    })
}

/// The fixed texts of `entries` (the titles of submenus and the borrowed labels of items).
fn fixed_texts(entries: &[MenuEntry], out: &mut Vec<String>) {
    for e in entries {
        match e {
            MenuEntry::Item(i) => {
                if let Cow::Borrowed(label) = &i.label {
                    out.push((*label).to_owned());
                }
            }
            MenuEntry::Submenu(s) => {
                out.push(s.label.to_owned());
                fixed_texts(&s.entries, out);
            }
            MenuEntry::Separator => {}
        }
    }
}

/// Texts that read the same in French (a name or an abbreviation).
const SAME_IN_FRENCH: &[&str] = &["Console", "Mono", "Rack", "Players", "Plugins", "Transport"];

#[test]
fn every_fixed_menu_text_has_a_french_entry() {
    let mut missing = std::collections::BTreeSet::new();
    for menu in model_in(Lang::English) {
        let mut texts = vec![menu.title.to_owned()];
        fixed_texts(&menu.entries, &mut texts);
        for t in texts {
            // A number or a fraction (the choices of a grid) reads the same in every language.
            if !t.chars().any(|c| c.is_alphabetic()) {
                continue;
            }
            if !i18n::has(Lang::French, &t) && !SAME_IN_FRENCH.contains(&t.as_str()) {
                missing.insert(t);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "menu texts with no French entry:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// The shape of a menu: its rows, each as `-` for a separator, `>` for a submenu with its rows
/// inside and `.` for an item.
fn shape(entries: &[MenuEntry]) -> String {
    entries
        .iter()
        .map(|e| match e {
            MenuEntry::Separator => "-".to_owned(),
            MenuEntry::Item(_) => ".".to_owned(),
            MenuEntry::Submenu(s) => format!(">({})", shape(&s.entries)),
        })
        .collect()
}

#[test]
fn the_french_menus_have_the_shape_of_the_english_ones() {
    let en = model_in(Lang::English);
    let fr = model_in(Lang::French);
    assert_eq!(en.len(), fr.len());
    for (a, b) in en.iter().zip(&fr) {
        assert_eq!(shape(&a.entries), shape(&b.entries), "{}", a.title);
        let commands = |m: &Menu| {
            m.items()
                .map(|i| format!("{:?}", i.command))
                .collect::<Vec<_>>()
        };
        assert_eq!(commands(a), commands(b), "{}", a.title);
    }
}

#[test]
fn no_french_menu_is_wider_than_the_screen() {
    // The widest a list may be: a window of 1000 logical pixels leaves room for a flyout.
    const WIDEST: f64 = 560.0;
    fn walk(entries: &[MenuEntry], at: &str) {
        let width = super::list_width(entries);
        assert!(width <= WIDEST, "{at} is {width} px wide");
        for e in entries {
            if let MenuEntry::Submenu(s) = e {
                walk(&s.entries, s.label);
            }
        }
    }
    for m in &model_in(Lang::French) {
        walk(&m.entries, m.title);
        assert!(
            m.entries.len() <= crate::commands::MAX_MENU_ROWS,
            "{}",
            m.title
        );
    }
}

#[test]
fn undo_and_redo_name_the_step_in_french() {
    let fr = model_in(Lang::French);
    let edit = fr.iter().find(|m| m.title == "Édition").unwrap();
    let labels: Vec<String> = edit.items().map(|i| i.label.to_string()).collect();
    assert!(labels.iter().any(|l| l == "Annuler"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "Rétablir"), "{labels:?}");
}

fn tree(menus: &[Menu]) -> String {
    fn walk(entries: &[MenuEntry], depth: usize, out: &mut String) {
        for entry in entries {
            let pad = "  ".repeat(depth);
            match entry {
                MenuEntry::Separator => out.push_str(&format!("{pad}---\n")),
                MenuEntry::Item(i) => match &i.shortcut {
                    Some(s) => out.push_str(&format!("{pad}{} [{s}]\n", i.label)),
                    None => out.push_str(&format!("{pad}{}\n", i.label)),
                },
                MenuEntry::Submenu(s) => {
                    out.push_str(&format!("{pad}{} >\n", s.label));
                    walk(&s.entries, depth + 1, out);
                }
            }
        }
    }
    let mut out = String::new();
    for m in menus {
        out.push_str(&format!("{}\n", m.title));
        walk(&m.entries, 1, &mut out);
    }
    out
}

fn golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/fr")
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
fn the_french_menu_tree_matches_the_golden() {
    golden("menu_tree.txt", &tree(&model_in(Lang::French)));
}
