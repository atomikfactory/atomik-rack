//! The built-in manual: the user documents of `docs/`, compiled in.
//!
//! The text is the same file the documentation on the project page shows, included at build
//! time with `include_str!`, so there is one source: [`docs/README.md`](../../../docs/README.md)
//! (the sections about using the app) and [`docs/KEYBOARD.md`](../../../docs/KEYBOARD.md) (the
//! keyboard reference, generated from the keymap). The panel, the parser and the search are in
//! `atomik_ui::manual`; this file gives them the text and says where a link that leaves the
//! manual goes ([`outside_url`]).

use atomik_ui::Manual;
use atomik_ui::manual::Source;
use std::sync::{Arc, OnceLock};

/// The documentation text, one file each.
const README: &str = include_str!("../../../docs/README.md");
const KEYBOARD: &str = include_str!("../../../docs/KEYBOARD.md");

/// The documents of the manual, in reading order.
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

/// The manual, parsed once.
pub fn manual() -> Arc<Manual> {
    static MANUAL: OnceLock<Arc<Manual>> = OnceLock::new();
    MANUAL
        .get_or_init(|| Arc::new(Manual::new(&SOURCES)))
        .clone()
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
const ONLINE: [(&str, &str); 17] = [
    ("ARCHITECTURE.md", repo!("docs/ARCHITECTURE.md")),
    ("DEVICES.md", repo!("docs/DEVICES.md")),
    ("EXTENDING.md", repo!("docs/EXTENDING.md")),
    ("TESTING.md", repo!("docs/TESTING.md")),
    ("MCP.md", repo!("docs/MCP.md")),
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
    ONLINE.iter().find(|(f, _)| *f == file).map(|(_, u)| *u)
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
