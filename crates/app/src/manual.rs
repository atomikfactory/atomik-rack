//! The built-in manual: the user documents of `docs/`, compiled in, in English and in French.
//!
//! The text is the same file the documentation on the project page shows, included at build
//! time with `include_str!`, so there is one source: [`docs/README.md`](../../../docs/README.md)
//! (the sections about using the app) and [`docs/KEYBOARD.md`](../../../docs/KEYBOARD.md) (the
//! keyboard reference, generated from the keymap), and their French translations in
//! `docs/fr/` (the French headings keep the anchors of the English ones, so every link and every
//! Help Mode section resolves in both). The panel, the parser and the search are in
//! `atomik_ui::manual`; this file gives them the text and says where a link that leaves the
//! manual goes ([`outside_url`]).
//!
//! The manual opens in the language of the interface ([`current`]); a document that is empty in
//! French (a translation not written yet) is shown in English. The panel has a switch for the
//! other language ([`views`]).

use atomik_ui::Manual;
use atomik_ui::i18n::{self, Lang};
use atomik_ui::manual::Source;
use std::sync::{Arc, OnceLock};

/// The documentation text, one file each.
const README: &str = include_str!("../../../docs/README.md");
const KEYBOARD: &str = include_str!("../../../docs/KEYBOARD.md");
const README_FR: &str = include_str!("../../../docs/fr/README.md");
const KEYBOARD_FR: &str = include_str!("../../../docs/fr/KEYBOARD.md");

/// The documents of the manual in English, in reading order.
pub const SOURCES: [Source<'static>; 2] = [
    Source {
        file: "README.md",
        name: "Manual",
        text: README,
    },
    Source {
        file: "KEYBOARD.md",
        name: "Keyboard",
        text: KEYBOARD,
    },
];

/// The documents of the manual in `lang`: the translation of each, or the English one when the
/// translation is empty. A document keeps its file name in every language, so the links between
/// documents are the same.
pub fn sources(lang: Lang) -> [Source<'static>; 2] {
    match lang {
        Lang::English => SOURCES,
        Lang::French => [
            Source {
                file: "README.md",
                name: "Manuel",
                text: translated(README_FR, README),
            },
            Source {
                file: "KEYBOARD.md",
                name: "Clavier",
                text: translated(KEYBOARD_FR, KEYBOARD),
            },
        ],
    }
}

/// `translation`, or `english` when the translation has no text.
fn translated(translation: &'static str, english: &'static str) -> &'static str {
    if translation.trim().is_empty() {
        english
    } else {
        translation
    }
}

/// The manual in English, parsed once.
pub fn manual() -> Arc<Manual> {
    manual_in(Lang::English)
}

/// The manual in `lang`, parsed once.
pub fn manual_in(lang: Lang) -> Arc<Manual> {
    static MANUALS: [OnceLock<Arc<Manual>>; 2] = [OnceLock::new(), OnceLock::new()];
    let at = Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0);
    MANUALS[at]
        .get_or_init(|| Arc::new(Manual::new(&sources(lang))))
        .clone()
}

/// The manual in the language of the interface.
pub fn current() -> Arc<Manual> {
    manual_in(i18n::language())
}

/// Every language the manual comes in, with its manual, for the panel's switch.
pub fn views() -> Vec<(Lang, Arc<Manual>)> {
    Lang::ALL.iter().map(|l| (*l, manual_in(*l))).collect()
}

/// The address of a file of the repository that the manual does not hold: the same single
/// request to a fixed address that Help > Documentation makes.
macro_rules! repo {
    ($tail:literal) => {
        concat!(
            "https://github.com/atomikfactory/atomik-rack/blob/main/",
            $tail
        )
    };
}

/// The files the documents link to, with where each opens in the browser.
const ONLINE: [(&str, &str); 19] = [
    ("ARCHITECTURE.md", repo!("docs/ARCHITECTURE.md")),
    ("DEVICES.md", repo!("docs/DEVICES.md")),
    ("EXTENDING.md", repo!("docs/EXTENDING.md")),
    ("TESTING.md", repo!("docs/TESTING.md")),
    ("MCP.md", repo!("docs/MCP.md")),
    ("TAB.md", repo!("docs/TAB.md")),
    ("BUILDING.md", repo!("docs/BUILDING.md")),
    ("DEPENDENCIES.md", repo!("docs/DEPENDENCIES.md")),
    ("DECISIONS.md", repo!("docs/DECISIONS.md")),
    ("RELEASING.md", repo!("docs/RELEASING.md")),
    ("RELEASE-TESTING.md", repo!("docs/RELEASE-TESTING.md")),
    ("../README.md", repo!("README.md")),
    ("../CONTRIBUTING.md", repo!("CONTRIBUTING.md")),
    ("../ROADMAP.md", repo!("ROADMAP.md")),
    ("../CHANGELOG.md", repo!("CHANGELOG.md")),
    ("../SECURITY.md", repo!("SECURITY.md")),
    ("../CODE_OF_CONDUCT.md", repo!("CODE_OF_CONDUCT.md")),
    ("../SUPPORT.md", repo!("SUPPORT.md")),
    ("../examples/README.md", repo!("examples/README.md")),
];

/// Where a link that leaves the manual opens: the page of the repository it names (without its
/// anchor). `None` for a target that is not one of the files the documents link to; the app
/// never opens an address it was not built with.
pub fn outside_url(target: &str) -> Option<&'static str> {
    let file = target.split_once('#').map_or(target, |(f, _)| f);
    let find = |file: &str| ONLINE.iter().find(|(f, _)| *f == file).map(|(_, u)| *u);
    // A French document sits one folder deeper (`docs/fr/`): its link to `../DEVICES.md` is the
    // English document's `DEVICES.md`, and `../../README.md` is `../README.md`.
    find(file).or_else(|| file.strip_prefix("../").and_then(find))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manual_is_the_two_documents_and_has_their_headings() {
        let m = manual();
        assert_eq!(m.docs().len(), 2);
        assert_eq!(m.docs()[0].file, "README.md");
        assert!(m.find_heading("The main window").is_some());
        assert!(m.find_heading("Help Mode").is_some());
        assert!(m.find_heading("Standard keys").is_some());
        // One shared copy.
        assert!(Arc::ptr_eq(&m, &manual()));
    }

    #[test]
    fn the_french_manual_has_every_anchor_of_the_english_one() {
        let (en, fr) = (manual_in(Lang::English), manual_in(Lang::French));
        assert_eq!(fr.docs().len(), en.docs().len());
        for (d, doc) in en.docs().iter().enumerate() {
            assert_eq!(fr.docs()[d].file, doc.file);
            for page in en.pages().iter().filter(|p| p.doc == d) {
                for block in &page.blocks {
                    let atomik_ui::manual::markdown::Block::Heading { anchor, .. } = block else {
                        continue;
                    };
                    assert!(
                        fr.anchor_location(d, anchor).is_some(),
                        "{} lacks #{anchor}",
                        doc.file
                    );
                }
            }
        }
        // The English name of a section (what Help Mode links to) finds the French section.
        let at = fr
            .find_heading("The main window")
            .expect("found by its anchor");
        assert_eq!(
            fr.heading_at(at).map(|h| h.1).as_deref(),
            Some("La fenêtre principale")
        );
    }

    #[test]
    fn the_french_documents_link_where_the_english_ones_do() {
        let m = manual_in(Lang::French);
        for (p, page) in m.pages().iter().enumerate() {
            let mut targets = Vec::new();
            for block in &page.blocks {
                collect_links(block, &mut targets);
            }
            for target in targets {
                if let atomik_ui::manual::Link::Outside(t) = m.resolve(p, &target)
                    && !t.contains("://")
                {
                    assert!(outside_url(&t).is_some(), "{} links to {t}", page.title);
                }
            }
        }
        assert_eq!(outside_url("../DEVICES.md#x"), outside_url("DEVICES.md"));
        assert_eq!(
            outside_url("../../ROADMAP.md"),
            outside_url("../ROADMAP.md")
        );
    }

    #[test]
    fn an_empty_translation_falls_back_to_the_english_text() {
        assert_eq!(translated("", "english"), "english");
        assert_eq!(translated("  \n", "english"), "english");
        assert_eq!(translated("français", "english"), "français");
        assert!(
            sources(Lang::French)
                .iter()
                .all(|s| !s.text.trim().is_empty())
        );
    }

    #[test]
    fn outside_links_open_a_fixed_address_or_nothing() {
        assert_eq!(
            outside_url("DEVICES.md#poly-synth"),
            Some("https://github.com/atomikfactory/atomik-rack/blob/main/docs/DEVICES.md")
        );
        assert_eq!(
            outside_url("../ROADMAP.md"),
            Some("https://github.com/atomikfactory/atomik-rack/blob/main/ROADMAP.md")
        );
        assert_eq!(outside_url("https://example.org"), None);
        assert_eq!(outside_url("nothing.md"), None);
    }

    #[test]
    fn every_link_in_the_documents_goes_somewhere() {
        // A link inside the manual resolves; a link that leaves it is one of the known files.
        let m = manual();
        for (p, page) in m.pages().iter().enumerate() {
            let mut targets = Vec::new();
            for block in &page.blocks {
                collect_links(block, &mut targets);
            }
            for target in targets {
                match m.resolve(p, &target) {
                    atomik_ui::manual::Link::Here(_) => {}
                    atomik_ui::manual::Link::Outside(t) => assert!(
                        outside_url(&t).is_some(),
                        "page {} links to {t}, which leads nowhere",
                        page.title
                    ),
                }
            }
        }
    }

    fn collect_links(block: &atomik_ui::manual::markdown::Block, out: &mut Vec<String>) {
        use atomik_ui::manual::markdown::Block;
        let mut runs: Vec<&atomik_ui::manual::markdown::Run> = Vec::new();
        match block {
            Block::Heading { runs: r, .. } | Block::Paragraph(r) | Block::Quote(r) => {
                runs.extend(r);
            }
            Block::Item { runs: r, .. } => runs.extend(r),
            Block::Table { head, rows } => {
                runs.extend(head.iter().flatten());
                runs.extend(rows.iter().flatten().flatten());
            }
            Block::Code { .. } | Block::Rule => {}
        }
        out.extend(runs.iter().filter_map(|r| r.link.clone()));
    }
}
