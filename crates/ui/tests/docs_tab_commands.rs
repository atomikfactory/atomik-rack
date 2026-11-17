//! The command tables of `docs/TAB.md` agree with the registry and with `docs/KEYBOARD.md`: every
//! command the guide lists exists, every command of the guitar tab feature is listed, and the
//! default key the guide gives is the key of the keyboard reference (and `-` means no key).

use atomik_ui::keymap::Registry;

fn read(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(path).unwrap()
}

/// One row of a command table: the id, its argument, and the text of the last cell.
struct Row {
    id: String,
    arg: Option<String>,
    last: String,
}

fn rows() -> Vec<Row> {
    let doc = read("docs/TAB.md");
    let at = doc
        .find("## Commands reference")
        .expect("the guide has a commands reference");
    doc[at..]
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            let span = cells[0].trim_matches('`');
            let (id, arg) = match span.split_once(' ') {
                Some((id, arg)) => (id.to_owned(), Some(arg.to_owned())),
                None => (span.to_owned(), None),
            };
            Row {
                id,
                arg,
                last: cells[cells.len() - 1].to_owned(),
            }
        })
        .collect()
}

/// Whether a command belongs to the guitar tab feature.
fn is_tab_command(id: &str) -> bool {
    id.starts_with("editor.tab_")
        || id.starts_with("editor.sheet_")
        || id.starts_with("editor.trainer_")
        || id.starts_with("practice.")
        || matches!(
            id,
            "view.toggle_tab_editor"
                | "view.toggle_chord_sheet"
                | "view.toggle_chord_trainer"
                | "view.toggle_fretboard"
                | "view.detach_fretboard"
                | "edit.toggle_tab_or_piano_roll"
        )
        || id.starts_with("view.fretboard_")
        || (id.starts_with("file.") && (id.contains("_tab") || id.contains("_sheet")))
}

#[test]
fn every_command_in_the_guide_exists() {
    let registry = Registry::bundled();
    let rows = rows();
    assert!(
        rows.len() > 100,
        "the tables were not found ({} rows)",
        rows.len()
    );
    for row in &rows {
        let def = registry
            .get(&row.id)
            .unwrap_or_else(|| panic!("docs/TAB.md lists {}, which is not a command", row.id));
        assert!(
            row.arg.is_none() || def.args.is_some(),
            "docs/TAB.md gives {} an argument, but it takes none",
            row.id
        );
    }
}

#[test]
fn every_tab_command_is_in_the_guide() {
    let rows = rows();
    let missing: Vec<&str> = Registry::bundled()
        .commands()
        .iter()
        .map(|c| c.id)
        .filter(|id| is_tab_command(id))
        .filter(|id| !rows.iter().any(|r| r.id == *id))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/TAB.md has no row for: {missing:?}"
    );
}

#[test]
fn the_keys_in_the_guide_are_the_keys_of_the_keyboard_reference() {
    let registry = Registry::bundled();
    let keyboard = read("docs/KEYBOARD.md");
    // The standard profile's table comes first; its keys are the third cell of a row.
    let key_of = |name: &str| -> Option<String> {
        let prefix = format!("| {name} |");
        keyboard.lines().find(|l| l.starts_with(&prefix)).map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            cells[2].to_owned()
        })
    };
    for row in rows() {
        let label = registry.get(&row.id).unwrap().label;
        let name = match &row.arg {
            Some(arg) => format!("{label} ({arg})"),
            None => label.to_owned(),
        };
        let found = key_of(&name);
        if row.last == "-" {
            assert_eq!(found, None, "docs/TAB.md says {} has no key", row.id);
        } else if row.last.starts_with('`') && row.last.ends_with('`') && !row.last.contains(' ') {
            assert_eq!(
                found.as_deref(),
                Some(row.last.as_str()),
                "the key of {} {:?} in docs/TAB.md and docs/KEYBOARD.md",
                row.id,
                row.arg
            );
        }
    }
}
