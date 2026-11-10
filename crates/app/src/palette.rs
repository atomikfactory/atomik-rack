//! What the command palette lists and remembers.
//!
//! * [`commands`] turns the live menus and the command registry into the palette's entries and
//!   says how to run each: the menu items (with their live label, shortcut and enabled state, and
//!   their menu path) and every other command of the registry that needs no argument (listed
//!   under its group). The commands that only work with an argument a menu or a gesture
//!   supplies (the `_pick` ones) and the commands of the Piano Keys strip are left out.
//! * The last [`MAX_RECENT`] commands run from the palette are kept in `palette.json` next to the
//!   settings ([`load`], [`remember`]). A missing or unreadable file is an empty list, and the
//!   file is read again before each write, so two song windows do not undo each other.

use crate::commands::{AppCommand, Menu, MenuEntry};
use atomik_ui::PaletteEntry;
use atomik_ui::keymap::{Group, Keymap, Registry};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The file the recent commands are kept in.
pub const FILE: &str = "palette.json";
/// How many recent commands are kept.
pub const MAX_RECENT: usize = 8;

/// The recent-commands file next to a settings file.
pub fn path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(FILE)
}

/// How the shell runs a palette entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runner {
    /// A menu item: the shell runs its command like a click in the menu.
    Menu(AppCommand),
    /// A command of the registry without a menu item: the shell runs it like its key.
    Registry(&'static str),
    /// A heading of the manual: the shell opens the manual there.
    Manual(atomik_ui::manual::Location),
}

/// The entries of the palette and, for each, how to run it (same order).
pub fn commands(
    menus: &[Menu],
    registry: &Registry,
    keymap: &Keymap,
) -> (Vec<PaletteEntry>, Vec<Runner>) {
    let mut entries: Vec<PaletteEntry> = Vec::new();
    let mut runners: Vec<Runner> = Vec::new();
    for menu in menus {
        walk(
            &menu.entries,
            menu.title.to_owned(),
            &mut entries,
            &mut runners,
        );
    }
    for def in registry.commands() {
        let skip = def.args.is_some() || def.id.ends_with("_pick") || def.group == Group::Keys;
        if skip || entries.iter().any(|e| e.key == def.id) {
            continue;
        }
        entries.push(PaletteEntry {
            key: def.id.to_owned(),
            id: def.id.to_owned(),
            name: plain(&atomik_ui::i18n::tr(def.label)),
            path: atomik_ui::i18n::t(def.group.title()).to_owned(),
            shortcut: keymap.shortcut_text(def.id, None),
            enabled: true,
        });
        runners.push(Runner::Registry(def.id));
    }
    (entries, runners)
}

/// Adds a "Manual: <heading>" entry for every heading of the manual a person would look for:
/// down to level three in the first document, level two in the others.
pub fn manual_entries(
    manual: &atomik_ui::Manual,
    entries: &mut Vec<PaletteEntry>,
    runners: &mut Vec<Runner>,
) {
    for (title, at, doc) in manual.headings(3) {
        let Some((level, _, anchor)) = manual.heading_at(at) else {
            continue;
        };
        if doc > 0 && level > 2 {
            continue;
        }
        let file = &manual.docs()[doc].file;
        let key = format!("manual:{file}#{anchor}");
        if entries.iter().any(|e| e.key == key) {
            continue;
        }
        let name = if doc == 0 {
            format!("Manual: {title}")
        } else {
            format!("Manual: {}: {title}", manual.docs()[doc].name)
        };
        entries.push(PaletteEntry {
            key,
            id: "help.manual".to_owned(),
            name,
            path: "Help > Manual".to_owned(),
            shortcut: None,
            enabled: true,
        });
        runners.push(Runner::Manual(at));
    }
}

fn walk(
    entries: &[MenuEntry],
    path: String,
    out: &mut Vec<PaletteEntry>,
    runners: &mut Vec<Runner>,
) {
    for entry in entries {
        match entry {
            MenuEntry::Separator => {}
            MenuEntry::Submenu(sub) => walk(
                &sub.entries,
                format!("{path} > {}", sub.label),
                out,
                runners,
            ),
            MenuEntry::Item(item) => {
                let Some((id, arg)) = item.command.command_id() else {
                    continue;
                };
                let key = match arg {
                    Some(a) => format!("{id}|{a}"),
                    None => id.to_owned(),
                };
                if out.iter().any(|e| e.key == key) {
                    continue;
                }
                out.push(PaletteEntry {
                    key,
                    id: id.to_owned(),
                    name: plain(&item.label),
                    path: path.clone(),
                    shortcut: item.shortcut.clone(),
                    enabled: item.enabled,
                });
                runners.push(Runner::Menu(item.command));
            }
        }
    }
}

/// A label as the palette shows it: without the ellipsis that marks a dialog in a menu.
fn plain(label: &str) -> String {
    label
        .trim_end_matches(['\u{2026}', '.'])
        .trim_end()
        .to_owned()
}

/// The recent commands (their keys, newest first). Empty when the file is missing or unusable.
pub fn load(file: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(Value::Array(list)) = root.get("recent") else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for key in list.iter().filter_map(Value::as_str) {
        if !key.is_empty() && !out.iter().any(|k| k == key) && out.len() < MAX_RECENT {
            out.push(key.to_owned());
        }
    }
    out
}

/// Puts `key` first in the recent commands of `file` and saves them. Returns the new list and
/// the result of the write; when the write failed the list is still the new one, so use it.
pub fn remember(file: &Path, key: &str) -> (Vec<String>, std::io::Result<()>) {
    let mut list = load(file);
    list.retain(|k| k != key);
    list.insert(0, key.to_owned());
    list.truncate(MAX_RECENT);
    let text = serde_json::to_string_pretty(&json!({ "recent": list })).unwrap_or_default();
    let written = crate::recovery::atomic_write(file, text.as_bytes());
    (list, written)
}

#[cfg(test)]
#[path = "palette_tests.rs"]
mod tests;
