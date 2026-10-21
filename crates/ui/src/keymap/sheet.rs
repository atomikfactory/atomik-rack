//! The printable cheat sheet (`atomik --print-keymap`): a markdown listing of the
//! commands and their chords per profile and OS, generated from the registry and the keymaps,
//! never typed. `docs/KEYBOARD.md` is its output for every profile and OS; a test keeps the
//! committed file equal to it.

use super::{Group, Keymap, Os, Profile, Registry, UserKeymap};
use std::collections::BTreeSet;
use std::fmt::Write as _;

type Key = (String, Option<String>, super::Context);

/// The OS as the sheet names it.
pub fn os_title(os: Os) -> &'static str {
    match os {
        Os::Windows => "Windows",
        Os::Linux => "Linux",
        Os::Mac => "macOS",
    }
}

/// The profile as the sheet names it.
pub fn profile_title(profile: Profile) -> &'static str {
    match profile {
        Profile::Standard => "Standard keys",
        Profile::Legacy => "Atomik legacy keys",
    }
}

fn keymaps(profile: Profile, oses: &[Os], user: Option<&UserKeymap>) -> Vec<Keymap> {
    oses.iter()
        .map(|os| match user {
            Some(u) => u.build(profile, *os).keymap,
            None => Keymap::bundled(profile, *os),
        })
        .collect()
}

fn keys_of(maps: &[Keymap]) -> BTreeSet<Key> {
    maps.iter()
        .flat_map(|m| m.bindings())
        .map(|b| (b.command.clone(), b.arg.clone(), b.context))
        .collect()
}

fn cell(map: &Keymap, key: &Key) -> String {
    let chords: Vec<String> = map
        .bindings()
        .iter()
        .filter(|b| b.command == key.0 && b.arg == key.1 && b.context == key.2)
        .flat_map(|b| b.chords.iter())
        .map(|c| format!("`{}`", c.menu_text(map.os())))
        .collect();
    if chords.is_empty() {
        "-".to_string()
    } else {
        chords.join(", ")
    }
}

fn label(key: &Key) -> String {
    let registry = Registry::bundled();
    let base = registry.get(&key.0).map_or(key.0.as_str(), |d| d.label);
    match &key.1 {
        Some(a) => format!("{base} ({a})"),
        None => base.to_string(),
    }
}

fn group_of(key: &Key) -> Group {
    Registry::bundled()
        .get(&key.0)
        .map_or(Group::Global, |d| d.group)
}

fn sorted(keys: BTreeSet<Key>) -> Vec<Key> {
    let mut v: Vec<Key> = keys.into_iter().collect();
    v.sort_by(|a, b| {
        (group_of(a), label(a).to_lowercase(), a.2).cmp(&(
            group_of(b),
            label(b).to_lowercase(),
            b.2,
        ))
    });
    v
}

fn header(out: &mut String, first: &str, oses: &[Os]) {
    let _ = write!(out, "| {first} | Where |");
    for os in oses {
        let _ = write!(out, " {} |", os_title(*os));
    }
    out.push_str("\n|---|---|");
    for _ in oses {
        out.push_str("---|");
    }
    out.push('\n');
}

/// One profile: a table per group with a column per OS, then the commands without a key.
pub fn profile_section(profile: Profile, oses: &[Os], user: Option<&UserKeymap>) -> String {
    let maps = keymaps(profile, oses, user);
    let keys = sorted(keys_of(&maps));
    let mut out = String::new();
    let _ = writeln!(out, "## {}\n", profile_title(profile));
    for group in Group::ALL {
        let rows: Vec<&Key> = keys.iter().filter(|k| group_of(k) == group).collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(out, "### {}\n", group.title());
        header(&mut out, "Command", oses);
        for key in rows {
            let _ = write!(out, "| {} | {} |", label(key), key.2.name());
            for map in &maps {
                let _ = write!(out, " {} |", cell(map, key));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    let bound: BTreeSet<&str> = keys.iter().map(|k| k.0.as_str()).collect();
    let unbound: Vec<&str> = Registry::bundled()
        .commands()
        .iter()
        .filter(|d| !bound.contains(d.id))
        .map(|d| d.label)
        .collect();
    if !unbound.is_empty() {
        let _ = writeln!(
            out,
            "Commands without a key in this profile (menus and Settings still reach them): {}.\n",
            unbound.join(", ")
        );
    }
    out
}

/// All the chords of a command in every context, contexts with the same chords grouped
/// (`` `L` (rack, sequencer) ``); no context is named when the command is only global.
fn cell_all(map: &Keymap, command: &str, arg: &Option<String>) -> String {
    let mut groups: Vec<(String, Vec<&'static str>)> = Vec::new();
    for ctx in super::Context::ALL {
        let chords: Vec<String> = map
            .bindings()
            .iter()
            .filter(|b| b.command == command && b.arg == *arg && b.context == ctx)
            .flat_map(|b| b.chords.iter())
            .map(|c| format!("`{}`", c.menu_text(map.os())))
            .collect();
        if chords.is_empty() {
            continue;
        }
        let text = chords.join(", ");
        match groups.iter_mut().find(|(t, _)| *t == text) {
            Some((_, ctxs)) => ctxs.push(ctx.name()),
            None => groups.push((text, vec![ctx.name()])),
        }
    }
    if groups.is_empty() {
        return "-".to_string();
    }
    let only_global = groups.iter().all(|(_, c)| c == &["global"]);
    groups
        .into_iter()
        .map(|(t, c)| {
            if only_global {
                t
            } else {
                format!("{t} ({})", c.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The commands whose chords differ between two profiles on one OS.
fn changes(from: Profile, to: Profile, os: Os) -> String {
    let (a, b) = (Keymap::bundled(from, os), Keymap::bundled(to, os));
    let mut keys: Vec<(String, Option<String>)> = keys_of(&[a.clone(), b.clone()])
        .into_iter()
        .map(|(c, arg, _)| (c, arg))
        .collect();
    keys.sort();
    keys.dedup();
    keys.sort_by(|x, y| {
        let gx = group_of(&(x.0.clone(), x.1.clone(), super::Context::Global));
        let gy = group_of(&(y.0.clone(), y.1.clone(), super::Context::Global));
        let lx = label(&(x.0.clone(), x.1.clone(), super::Context::Global)).to_lowercase();
        let ly = label(&(y.0.clone(), y.1.clone(), super::Context::Global)).to_lowercase();
        (gx, lx).cmp(&(gy, ly))
    });
    let mut out = String::new();
    let _ = writeln!(
        out,
        "| Command | {} | {} |\n|---|---|---|",
        profile_title(from),
        profile_title(to)
    );
    for (command, arg) in keys {
        let (x, y) = (cell_all(&a, &command, &arg), cell_all(&b, &command, &arg));
        if x != y {
            let name = label(&(command, arg, super::Context::Global));
            let _ = writeln!(out, "| {name} | {x} | {y} |");
        }
    }
    out.push('\n');
    out
}

/// The sheet for `profiles` on `oses`; `user` (a `--keymap` file) is applied on top when given.
/// With both profiles the sheet also lists what the standard profile changed.
pub fn markdown(profiles: &[Profile], oses: &[Os], user: Option<&UserKeymap>) -> String {
    let mut out = String::new();
    out.push_str("# Atomik Rack keyboard reference\n\n");
    out.push_str(
        "<!-- Generated by `atomik --print-keymap`; do not edit. A test keeps this file equal to the output. -->\n\n",
    );
    out.push_str(
        "Chords are shown as the menus show them. `Ctrl` is the Control key; on macOS the primary \
         modifier is `Cmd` (Command) and Alt is `Opt` (Option), and Control and Command are never \
         interchanged. `Numpad` keys are told apart from the main keys. Letters are matched by the \
         character they type, so a layout that moves a letter moves the shortcut with it; any \
         binding can be changed in Settings, Keyboard (saved as `keymap.json` next to \
         `settings.json`, only your changes). While a text field has focus only chords with the \
         primary modifier, function keys and Esc run commands. Where a binding in a pane overlaps \
         a global one, the pane wins inside that pane. The file browser panel (View, File \
         Browser, `F4`) is the `files` context: while it has the focus, its keys (arrows, Page \
         Up and Down, Home and End, Enter, Space, Esc, Backspace, `F`, the search and select-all \
         chords) are the `files.*` commands, which the shell hands to the panel as those keys, \
         so they win over the global bindings of the same chords (Space previews instead of \
         playing, Backspace goes up a folder instead of deleting); every other chord, `F4` and \
         `F5` included, still runs as usual. While the panel's search or tag field has focus, \
         only Esc, function keys and the chords with the primary modifier run commands.\n\n",
    );
    if profiles.contains(&Profile::Legacy)
        && profiles.contains(&Profile::Standard)
        && let Some(os) = oses.first()
    {
        let _ = writeln!(
            out,
            "## Moved keys: standard keys against the legacy keys ({})\n",
            os_title(*os)
        );
        out.push_str(&changes(Profile::Legacy, Profile::Standard, *os));
    }
    for profile in profiles {
        out.push_str(&profile_section(*profile, oses, user));
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_is_deterministic_and_names_every_os_and_profile() {
        let a = markdown(&Profile::ALL, &Os::ALL, None);
        assert_eq!(a, markdown(&Profile::ALL, &Os::ALL, None));
        for t in [
            "Windows",
            "Linux",
            "macOS",
            "Standard keys",
            "Atomik legacy keys",
        ] {
            assert!(a.contains(t), "{t}");
        }
        assert!(a.ends_with("|\n") || a.ends_with(".\n") || a.ends_with('.'));
        assert!(!a.contains("\n\n\n"));
    }

    #[test]
    fn it_lists_the_chords_as_each_os_shows_them() {
        let s = markdown(&[Profile::Standard], &Os::ALL, None);
        let row = s
            .lines()
            .find(|l| l.starts_with("| Import Audio |"))
            .unwrap();
        assert!(
            row.contains("`Ctrl+Shift+I`") && row.contains("`Cmd+Opt+I`"),
            "{row}"
        );
        let one = markdown(&[Profile::Legacy], &[Os::Mac], None);
        assert!(one.contains("| Undo | global | `Cmd+Z` |"), "{one}");
        assert!(!one.contains("Windows"));
    }

    #[test]
    fn only_both_profiles_get_a_changes_table_and_it_shows_the_moved_keys() {
        let both = markdown(&Profile::ALL, &[Os::Windows], None);
        assert!(both.contains("## Moved keys: standard keys against the legacy keys (Windows)"));
        let row = both
            .lines()
            .find(|l| l.starts_with("| Create Instrument |"));
        assert!(row.is_some_and(|r| r.contains("`Ctrl+I`")));
        assert!(!markdown(&[Profile::Standard], &[Os::Windows], None).contains("Moved keys"));
    }

    #[test]
    fn a_user_file_changes_the_sheet() {
        let user = UserKeymap::from_json(
            r#"{"bindings": [{"command": "edit.duplicate", "context": "global", "chords": ["Cmd+Alt+D"]}]}"#,
        )
        .unwrap()
        .0;
        let s = markdown(&[Profile::Standard], &[Os::Windows], Some(&user));
        assert!(s.contains("`Ctrl+Alt+D`"));
        assert!(!markdown(&[Profile::Standard], &[Os::Windows], None).contains("`Ctrl+Alt+D`"));
    }

    #[test]
    fn unbound_commands_are_listed_once_at_the_end_of_a_profile() {
        let s = profile_section(Profile::Standard, &[Os::Windows], None);
        assert!(s.contains("Commands without a key in this profile"));
    }
}
