//! The French catalogue: its entries are well formed, name the same values as the English text,
//! stay inside a length budget, and cover every command of the registry. A text with no entry
//! shows in English (the fallback), so nothing here can make the interface fail; these tests
//! keep the French honest.

use atomik_ui::Lang;
use atomik_ui::i18n::{self, placeholders};
use atomik_ui::keymap::{Group, Registry};
use std::collections::BTreeSet;

const SOURCE: &str = include_str!("../locales/fr.txt");

/// The entries as written in the file, in file order.
fn raw_entries() -> Vec<(String, String)> {
    SOURCE
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (k, v) = l
                .split_once(" => ")
                .unwrap_or_else(|| panic!("no ` => ` in: {l}"));
            (k.trim().to_owned(), v.trim().to_owned())
        })
        .collect()
}

#[test]
fn no_text_has_two_entries() {
    let mut seen = BTreeSet::new();
    for (k, _) in raw_entries() {
        assert!(seen.insert(k.clone()), "two entries for: {k}");
    }
    assert!(
        seen.len() > 1500,
        "the catalogue holds {} entries",
        seen.len()
    );
}

#[test]
fn every_entry_has_a_french_text_that_is_not_blank_or_padded() {
    for (k, v) in raw_entries() {
        assert!(!v.is_empty(), "{k}");
        assert_eq!(v, v.trim(), "{k}");
        assert!(!k.contains('\t') && !v.contains('\t'), "{k}");
    }
}

#[test]
fn a_translation_names_the_values_the_english_text_names() {
    for (k, v) in raw_entries() {
        let mut a = placeholders(&k);
        let mut b = placeholders(&v);
        a.sort();
        b.sort();
        assert_eq!(a, b, "values of {k:?} and {v:?}");
        // Two values side by side cannot be told apart in a text of the program.
        assert!(!k.contains("}{"), "{k}");
    }
}

#[test]
fn the_french_stays_inside_its_length_budget() {
    // French is longer than English: twice the text and a few characters are allowed; a shorter
    // French is always fine. The entries over it are listed.
    let mut over = Vec::new();
    for (k, v) in raw_entries() {
        let budget = k.chars().count() * 2 + 8;
        if v.chars().count() > budget {
            over.push(format!(
                "{k} ({}) => {v} ({})",
                k.chars().count(),
                v.chars().count()
            ));
        }
    }
    assert!(over.is_empty(), "over the budget:\n{}", over.join("\n"));
}

/// The entries under the `# Tooltips` heading of the file are the one-line help of buttons.
fn tooltip_entries() -> Vec<(String, String)> {
    let mut on = false;
    let mut out = Vec::new();
    for l in SOURCE.lines() {
        if l.starts_with('#') {
            on = l.starts_with("# Tooltips");
            continue;
        }
        if on && let Some((k, v)) = l.split_once(" => ") {
            out.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    out
}

#[test]
fn a_french_tooltip_is_one_line_of_at_most_ninety_characters() {
    let tips = tooltip_entries();
    assert!(tips.len() > 50, "{} tooltips", tips.len());
    let long: Vec<String> = tips
        .iter()
        .filter(|(_, v)| v.chars().count() > 90)
        .map(|(k, v)| format!("{} chars: {v} (from: {k})", v.chars().count()))
        .collect();
    assert!(
        long.is_empty(),
        "tooltips over 90 characters:
{}",
        long.join(
            "
"
        )
    );
}

#[test]
fn a_short_label_stays_short() {
    // The labels of buttons and tabs have room for a few more characters than the English, no
    // more: a word of up to 10 letters stays within 18.
    let grown: Vec<String> = raw_entries()
        .into_iter()
        .filter(|(k, v)| {
            k.chars().count() <= 10
                && !k.contains(' ')
                && !k.contains('{')
                && v.chars().count() > 18
        })
        .map(|(k, v)| format!("{k} => {v}"))
        .collect();
    assert!(
        grown.is_empty(),
        "short labels that grew:
{}",
        grown.join(
            "
"
        )
    );
}

/// Command labels that read the same in French.
const SAME: &[&str] = &[
    "Console",
    "Mono",
    "Rack",
    "Players",
    "Plugins",
    "Clip Editor",
    "Transport",
    "Piano Roll",
    "Warp",
    "Sync",
    "Macros...",
    "Versions",
    "Documentation",
    "Arrangement",
];

#[test]
fn every_command_of_the_registry_has_a_french_label() {
    let mut missing = Vec::new();
    for d in Registry::bundled().commands() {
        if !i18n::has(Lang::French, d.label) && !SAME.contains(&d.label) {
            missing.push(format!("{} ({})", d.label, d.id));
        }
    }
    assert!(
        missing.is_empty(),
        "{} commands with no French label:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn every_group_title_has_a_french_text() {
    for g in Group::ALL {
        let t = g.title();
        assert!(
            i18n::has(Lang::French, t) || SAME.contains(&t),
            "no French for the group {t}"
        );
    }
}

#[test]
fn english_shows_the_texts_of_the_program_unchanged() {
    i18n::with_language(Lang::English, || {
        for d in Registry::bundled().commands() {
            assert_eq!(i18n::t(d.label), d.label);
        }
        assert_eq!(i18n::tr("Save").as_ref(), "Save");
    });
}

#[test]
fn a_text_that_names_values_matches_any_text_of_its_shape() {
    i18n::with_language(Lang::French, || {
        assert_eq!(i18n::tr("Froze Poly Synth").as_ref(), "Poly Synth gelé");
        assert_eq!(
            i18n::tr("Output 3: connected").as_ref(),
            "Sortie 3 : connectée"
        );
        // A text of no known shape stays as it is.
        assert_eq!(
            i18n::tr("Kick drum heavy 01").as_ref(),
            "Kick drum heavy 01"
        );
    });
}

#[test]
fn the_ellipsis_may_be_written_as_three_dots() {
    i18n::with_language(Lang::French, || {
        assert_eq!(i18n::tr("Open...").as_ref(), "Ouvrir...");
        assert_eq!(i18n::tr("Open…").as_ref(), "Ouvrir…");
    });
}

#[test]
fn key_names_are_written_the_french_way() {
    i18n::with_language(Lang::French, || {
        assert_eq!(i18n::key_name("Shift"), "Maj");
        assert_eq!(i18n::key_name("Enter"), "Entrée");
        assert_eq!(i18n::key_name("Esc"), "Échap");
        assert_eq!(i18n::key_name("Ctrl"), "Ctrl");
        assert_eq!(i18n::key_name("K"), "K");
    });
    assert_eq!(i18n::key_name("Shift"), "Shift");
}

#[test]
fn a_shortcut_is_written_in_the_language() {
    use atomik_ui::keymap::{Chord, Os};
    let chord = Chord::parse("Cmd+Shift+Return").unwrap();
    assert_eq!(chord.menu_text(Os::Windows), "Ctrl+Shift+Enter");
    i18n::with_language(Lang::French, || {
        assert_eq!(chord.menu_text(Os::Windows), "Ctrl+Maj+Entrée");
    });
}
