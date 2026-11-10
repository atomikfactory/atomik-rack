//! The built-in manual: the Markdown subset parser on a fixture with every supported element,
//! the table of contents and the anchors of links, the ranked search with its snippets, the
//! Manual panel (links, history, search results, scrolling, keys) and goldens of a page and of
//! a search.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test manual`.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::manual::markdown::{Block, Run, parse, plain, slug};
use atomik_ui::manual::{Link, Location, Source};
use atomik_ui::{
    DisplayList, Key, Manual, ManualAction, ManualView, Modifiers, Theme, UiAction, UiEvent,
};
use std::path::PathBuf;
use std::sync::Arc;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);
const ALT: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: true,
    meta: false,
};

const GUIDE: &str = "# Guide

Intro with **bold**, *italic*, `code` and a [link to the rack](#the-rack).

<!-- a comment that is not shown -->

## The rack

The rack holds devices. See [the keys](KEYS.md#global) and [the site](https://example.org/docs).

- First item
  that wraps onto a second line
  - A nested item
- Second item with `inline code`

1. One
2. Two

```text
a code block
  keeps its lines
```

| Name | What it does |
|---|---|
| Poly | A polyphonic synth with a long description that needs to wrap in a narrow column |
| Echo | A delay |

> A quoted note about tempo.

---

![the rack picture](rack.png)

## Tempo

Tempo is set in the transport bar. A quartz clock keeps it.

### Tap tempo

Tap the button to set the tempo.

## Tempo

A second heading with the same name.
";

const KEYS: &str = "# Keys

Every key.

## Global

Global keys such as Ctrl+S run anywhere. Back to [the rack](GUIDE.md#the-rack).

## Global

The second Global section.
";

fn manual() -> Arc<Manual> {
    Arc::new(Manual::new(&[
        Source {
            file: "GUIDE.md",
            name: "Guide",
            text: GUIDE,
        },
        Source {
            file: "KEYS.md",
            name: "Keyboard",
            text: KEYS,
        },
    ]))
}

fn view() -> ManualView {
    let mut v = ManualView::new(Theme::dark(), manual());
    v.set_bounds(BOUNDS);
    v
}

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
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn key(v: &mut ManualView, key: Key, modifiers: Modifiers) -> Vec<ManualAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::Manual(a) => Some(a),
        _ => None,
    })
    .collect()
}

fn type_text(v: &mut ManualView, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            key(v, Key::Space, Modifiers::NONE);
        } else {
            key(v, Key::Char(c), Modifiers::NONE);
        }
    }
}

fn click(v: &mut ManualView, p: Point) -> Vec<ManualAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Manual(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn texts(v: &ManualView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn loc(m: &Manual, title: &str) -> Location {
    m.find_heading(title)
        .unwrap_or_else(|| panic!("no heading {title}"))
}

// ------------------------------------------------------------------ the parser

#[test]
fn every_supported_element_parses() {
    let blocks = parse(GUIDE);
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| match b {
            Block::Heading { .. } => "heading",
            Block::Paragraph(_) => "paragraph",
            Block::Item { .. } => "item",
            Block::Code { .. } => "code",
            Block::Table { .. } => "table",
            Block::Quote(_) => "quote",
            Block::Rule => "rule",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "heading",
            "paragraph", // Guide
            "heading",
            "paragraph", // The rack
            "item",
            "item",
            "item",
            "item",
            "item", // list: First, nested, Second, One, Two
            "code",
            "table",
            "quote",
            "rule",
            "paragraph", // the picture is a paragraph
            "heading",
            "paragraph",
            "heading",
            "paragraph",
            "heading",
            "paragraph",
        ]
    );
}

#[test]
fn the_inline_styles_of_a_paragraph() {
    let blocks = parse(GUIDE);
    let Block::Paragraph(runs) = &blocks[1] else {
        panic!("a paragraph");
    };
    let find = |text: &str| {
        runs.iter()
            .find(|r| r.text == text)
            .unwrap_or_else(|| panic!("{text}"))
    };
    assert!(find("bold").bold);
    assert!(find("italic").italic);
    assert!(find("code").code);
    assert_eq!(find("link to the rack").link.as_deref(), Some("#the-rack"));
    assert_eq!(
        plain(runs),
        "Intro with bold, italic, code and a link to the rack."
    );
}

#[test]
fn lists_nest_wrap_and_number() {
    let blocks = parse(GUIDE);
    let items: Vec<(u8, Option<u32>, String)> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Item {
                depth,
                number,
                runs,
            } => Some((*depth, *number, plain(runs))),
            _ => None,
        })
        .collect();
    assert_eq!(
        items[0],
        (
            0,
            None,
            "First item that wraps onto a second line".to_owned()
        )
    );
    assert_eq!(items[1], (1, None, "A nested item".to_owned()));
    assert_eq!(items[2].2, "Second item with inline code");
    assert_eq!(items[3], (0, Some(1), "One".to_owned()));
    assert_eq!(items[4], (0, Some(2), "Two".to_owned()));
}

#[test]
fn code_tables_images_and_comments() {
    let blocks = parse(GUIDE);
    let Some(Block::Code { lang, lines }) = blocks.iter().find(|b| matches!(b, Block::Code { .. }))
    else {
        panic!("a code block");
    };
    assert_eq!(lang, "text");
    assert_eq!(lines, &["a code block", "  keeps its lines"]);
    let Some(Block::Table { head, rows }) =
        blocks.iter().find(|b| matches!(b, Block::Table { .. }))
    else {
        panic!("a table");
    };
    assert_eq!(head.len(), 2);
    assert_eq!(plain(&head[1]), "What it does");
    assert_eq!(rows.len(), 2);
    assert_eq!(plain(&rows[1][0]), "Echo");
    assert!(
        !blocks.iter().any(|b| b.plain().contains("a comment")),
        "an HTML comment is not shown"
    );
    let image = blocks
        .iter()
        .find(|b| b.plain().contains("rack picture"))
        .expect("the image paragraph");
    assert_eq!(image.plain(), "[image: the rack picture]");
}

#[test]
fn anchors_follow_the_github_rule_and_repeats_are_numbered() {
    assert_eq!(slug("The rack"), "the-rack");
    let anchors: Vec<String> = parse(GUIDE)
        .iter()
        .filter_map(|b| match b {
            Block::Heading { anchor, .. } => Some(anchor.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        anchors,
        ["guide", "the-rack", "tempo", "tap-tempo", "tempo-1"]
    );
}

#[test]
fn a_run_keeps_its_text_and_style() {
    assert_eq!(Run::plain("x").text, "x");
}

// ------------------------------------------------------------------ the manual

#[test]
fn pages_split_at_headings_of_level_one_and_two() {
    let m = manual();
    let titles: Vec<&str> = m.pages().iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Guide", "The rack", "Tempo", "Tempo", "Keys", "Global", "Global"
        ]
    );
    // The level 3 heading stays on the page of Tempo.
    assert!(
        m.pages()[2]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Heading { level: 3, .. }))
    );
    assert_eq!(m.docs()[1].first_page, 4);
    assert_eq!(m.docs()[1].pages, 3);
}

#[test]
fn the_table_of_contents_is_built_from_the_headings() {
    let m = manual();
    let toc: Vec<(u8, &str)> = m
        .toc()
        .iter()
        .map(|e| (e.level, e.title.as_str()))
        .collect();
    assert_eq!(
        toc,
        [
            (1, "Guide"),
            (2, "The rack"),
            (2, "Tempo"),
            (3, "Tap tempo"),
            (2, "Tempo"),
            (1, "Keys"),
            (2, "Global"),
            (2, "Global")
        ]
    );
    // The entry that holds a place is the current one.
    let tap = loc(&m, "Tap tempo");
    assert_eq!(m.toc_index_of(tap), Some(3));
    assert_eq!(m.toc_index_of(m.start()), Some(0));
}

#[test]
fn links_resolve_to_sections_documents_and_the_outside() {
    let m = manual();
    let rack = loc(&m, "The rack");
    assert_eq!(m.resolve(0, "#the-rack"), Link::Here(rack));
    // A link is read in the document of the page it is on.
    assert_eq!(m.resolve(5, "#global"), Link::Here(loc(&m, "Global")));
    assert_eq!(
        m.resolve(5, "GUIDE.md#the-rack"),
        Link::Here(rack),
        "a link to another document's anchor"
    );
    assert_eq!(
        m.resolve(1, "KEYS.md#global-1"),
        Link::Here(Location { page: 6, block: 0 })
    );
    assert_eq!(
        m.resolve(1, "./KEYS.md"),
        Link::Here(Location { page: 4, block: 0 }),
        "a document without an anchor opens at its top"
    );
    assert_eq!(
        m.resolve(1, "https://example.org/docs"),
        Link::Outside("https://example.org/docs".to_owned())
    );
    assert_eq!(
        m.resolve(1, "OTHER.md#x"),
        Link::Outside("OTHER.md#x".to_owned())
    );
    assert_eq!(
        m.resolve(1, "#nothing-here"),
        Link::Outside("#nothing-here".to_owned())
    );
}

// ------------------------------------------------------------------ search

#[test]
fn a_heading_match_ranks_above_a_body_match() {
    let m = manual();
    let hits = m.search("tempo");
    assert!(hits.len() >= 4, "{hits:#?}");
    // The headings come first: the exact "Tempo" ones, then "Tap tempo" (a heading match too).
    let heads: Vec<bool> = hits.iter().map(|h| h.in_heading).collect();
    let first_body = heads.iter().position(|h| !h).unwrap_or(heads.len());
    assert!(heads[..first_body].iter().all(|h| *h));
    assert!(heads[first_body..].iter().all(|h| !*h));
    assert_eq!(hits[0].title, "Tempo");
    assert_eq!(hits[1].title, "Tempo");
    assert_eq!(hits[2].title, "Tap tempo");
    // The body match is the rack section, whose text mentions tempo only in the table row.
    let body: Vec<&str> = hits[first_body..]
        .iter()
        .map(|h| h.title.as_str())
        .collect();
    assert!(body.is_empty() || !body.contains(&"Tempo"), "{body:?}");
}

#[test]
fn an_exact_heading_beats_a_longer_one_and_ties_keep_reading_order() {
    let m = manual();
    let hits = m.search("global");
    assert_eq!(hits[0].at, Location { page: 5, block: 0 });
    assert_eq!(hits[1].at, Location { page: 6, block: 0 });
    let hits = m.search("tap");
    assert_eq!(hits[0].title, "Tap tempo");
}

#[test]
fn a_body_hit_has_a_snippet_with_the_match_marked() {
    let m = manual();
    let hits = m.search("quartz");
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert!(!hit.in_heading);
    assert_eq!(hit.title, "Tempo");
    assert!(hit.snippet.contains("quartz"));
    assert_eq!(hit.snippet_hits.len(), 1);
    let chars: Vec<char> = hit.snippet.chars().collect();
    let (a, b) = hit.snippet_hits[0];
    assert_eq!(chars[a..b].iter().collect::<String>(), "quartz");
    assert!(hit.title_hits.is_empty());
}

#[test]
fn every_word_of_the_query_must_match_and_case_is_ignored() {
    let m = manual();
    assert_eq!(m.search("QUARTZ clock").len(), 1);
    assert!(m.search("quartz banana").is_empty());
    assert!(m.search("").is_empty());
    assert!(m.search("   ").is_empty());
    let hits = m.search("the RACK");
    assert_eq!(hits[0].title, "The rack");
    assert_eq!(hits[0].title_hits, [(0, 3), (4, 8)]);
}

#[test]
fn a_long_body_is_cut_to_a_window_around_the_match() {
    let filler = "word ".repeat(80);
    let text = format!("# Doc\n\n{filler}needle {filler}\n");
    let m = Manual::new(&[Source {
        file: "D.md",
        name: "Doc",
        text: &text,
    }]);
    let hit = &m.search("needle")[0];
    assert!(hit.snippet.starts_with("..."), "{}", hit.snippet);
    assert!(hit.snippet.ends_with("..."), "{}", hit.snippet);
    assert!(hit.snippet.chars().count() <= 125);
    assert_eq!(hit.snippet_hits.len(), 1);
}

#[test]
fn table_cells_and_code_lines_are_searched() {
    let m = manual();
    assert_eq!(m.search("polyphonic")[0].title, "The rack");
    assert_eq!(m.search("keeps its lines")[0].title, "The rack");
}

// ------------------------------------------------------------------ the panel

#[test]
fn the_panel_opens_at_the_start_with_the_toc_and_the_page() {
    let v = view();
    assert_eq!(v.page(), 0);
    assert_eq!(v.title(), "Guide");
    assert!(!v.can_back() && !v.can_forward());
    let t = texts(&v);
    assert!(t.contains(&"Search the manual".to_owned()), "{t:?}");
    assert!(
        t.contains(&"Tap tempo".to_owned()),
        "the table of contents: {t:?}"
    );
    assert!(v.is_text_owner());
}

#[test]
fn a_toc_row_opens_its_section_and_a_heading_is_scrolled_to() {
    let mut v = view();
    let tap = loc(v.manual(), "Tap tempo");
    let row = v.manual().toc().iter().position(|e| e.at == tap).unwrap();
    let at = v.toc_row_center(row).expect("in view");
    assert!(click(&mut v, at).is_empty());
    assert_eq!(v.title(), "Tap tempo");
    assert_eq!(v.page(), tap.page);
    assert!(v.can_back());
}

#[test]
fn a_link_inside_the_manual_goes_there_and_back_returns() {
    let mut v = view();
    let at = v.link_center("#the-rack").expect("the link is in view");
    assert!(click(&mut v, at).is_empty());
    assert_eq!(v.title(), "The rack");
    assert!(v.can_back() && !v.can_forward());
    // The Back button returns to the start, Forward comes back.
    let back = v.back_rect().center();
    assert!(click(&mut v, back).is_empty());
    assert_eq!(v.title(), "Guide");
    assert!(v.can_forward());
    let fwd = v.forward_rect().center();
    click(&mut v, fwd);
    assert_eq!(v.title(), "The rack");
}

#[test]
fn a_link_to_another_document_and_an_outside_link() {
    let mut v = view();
    v.goto_section("The rack");
    assert_eq!(
        v.link_targets(),
        ["KEYS.md#global", "https://example.org/docs"]
    );
    let at = v.link_center("KEYS.md#global").expect("in view");
    click(&mut v, at);
    assert_eq!(v.title(), "Global");
    assert_eq!(v.page(), 5);
    // The link back out of the keyboard page.
    let at = v.link_center("GUIDE.md#the-rack").expect("in view");
    click(&mut v, at);
    assert_eq!(v.title(), "The rack");
    // An address is for the app to open.
    let at = v.link_center("https://example.org/docs").expect("in view");
    assert_eq!(
        click(&mut v, at),
        [ManualAction::Open("https://example.org/docs".to_owned())]
    );
    assert_eq!(v.title(), "The rack", "the manual stays where it is");
}

#[test]
fn alt_arrows_move_through_the_history() {
    let mut v = view();
    v.goto_section("Tempo");
    v.goto_section("The rack");
    key(&mut v, Key::Left, ALT);
    assert_eq!(v.title(), "Tempo");
    key(&mut v, Key::Left, ALT);
    assert_eq!(v.title(), "Guide");
    key(&mut v, Key::Left, ALT);
    assert_eq!(v.title(), "Guide", "no further back");
    key(&mut v, Key::Right, ALT);
    key(&mut v, Key::Right, ALT);
    assert_eq!(v.title(), "The rack");
    // A new place after going back drops the forward stops.
    key(&mut v, Key::Left, ALT);
    v.goto_section("Keys");
    assert!(!v.can_forward());
}

#[test]
fn typing_searches_and_enter_opens_the_hit() {
    let mut v = view();
    type_text(&mut v, "quartz");
    assert!(v.searching());
    assert_eq!(v.hits().len(), 1);
    assert_eq!(v.selected(), Some(0));
    let t = texts(&v);
    assert!(t.contains(&"1 result".to_owned()), "{t:?}");
    assert!(t.contains(&"Tempo".to_owned()));
    // The matched word is drawn on its own, bold.
    assert!(t.contains(&"quartz".to_owned()), "{t:?}");
    assert!(key(&mut v, Key::Enter, Modifiers::NONE).is_empty());
    assert!(!v.searching(), "opening a hit shows the page");
    assert_eq!(v.title(), "Tempo");
    assert_eq!(v.query(), "");
}

#[test]
fn a_click_on_a_result_opens_it_and_escape_clears_then_closes() {
    let mut v = view();
    type_text(&mut v, "tempo");
    assert!(v.hits().len() >= 3);
    let at = v.result_center(2).expect("in view");
    click(&mut v, at);
    assert_eq!(v.title(), v.manual().heading_at(v.current()).unwrap().1);
    type_text(&mut v, "global");
    assert!(v.searching());
    assert!(key(&mut v, Key::Escape, Modifiers::NONE).is_empty());
    assert_eq!(v.query(), "");
    assert_eq!(
        key(&mut v, Key::Escape, Modifiers::NONE),
        [ManualAction::Close]
    );
}

#[test]
fn no_match_says_so_and_up_down_move_the_highlight() {
    let mut v = view();
    type_text(&mut v, "zzzz");
    assert!(texts(&v).contains(&"Nothing in the manual matches.".to_owned()));
    assert_eq!(v.selected(), None);
    assert!(key(&mut v, Key::Enter, Modifiers::NONE).is_empty());
    key(&mut v, Key::Char('a'), CTRL);
    type_text(&mut v, "tempo");
    assert_eq!(v.selected(), Some(0));
    key(&mut v, Key::Down, Modifiers::NONE);
    key(&mut v, Key::Down, Modifiers::NONE);
    assert_eq!(v.selected(), Some(2));
    key(&mut v, Key::Up, Modifiers::NONE);
    assert_eq!(v.selected(), Some(1));
}

const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    meta: false,
};

#[test]
fn a_click_outside_closes_and_the_query_has_a_limit() {
    let mut v = view();
    assert_eq!(click(&mut v, Point::new(3.0, 3.0)), [ManualAction::Close]);
    type_text(&mut v, &"a".repeat(200));
    assert_eq!(v.query().chars().count(), 80);
}

#[test]
fn a_long_page_scrolls_with_the_keys_the_wheel_and_the_bar() {
    // A page far taller than the panel.
    let mut text = String::from("# Long\n\n");
    for n in 0..120 {
        text.push_str(&format!(
            "Paragraph {n} of the long page, with some words in it.\n\n"
        ));
    }
    text.push_str("## End\n\nThe end.\n");
    let m = Arc::new(Manual::new(&[Source {
        file: "L.md",
        name: "Long",
        text: &text,
    }]));
    let mut v = ManualView::new(Theme::dark(), m);
    v.set_bounds(BOUNDS);
    assert_eq!(v.scroll(), 0.0);
    assert!(v.page_height() > 2000.0);
    assert!(v.thumb_rect().is_some());
    key(&mut v, Key::Down, Modifiers::NONE);
    assert_eq!(v.scroll(), 34.0);
    key(&mut v, Key::PageDown, Modifiers::NONE);
    let page = v.page_rect().height() - 40.0;
    assert_eq!(v.scroll(), 34.0 + page);
    v.handle(&UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
        pos: v.page_rect().center(),
        modifiers: Modifiers::NONE,
    });
    assert!(v.scroll() > 34.0 + page);
    key(&mut v, Key::End, Modifiers::NONE);
    let bottom = v.page_height() - v.page_rect().height();
    assert!((v.scroll() - bottom).abs() < 0.5, "{} {bottom}", v.scroll());
    key(&mut v, Key::Home, Modifiers::NONE);
    assert_eq!(v.scroll(), 0.0);
    // Dragging the thumb to the bottom of the track scrolls to the end.
    let thumb = v.thumb_rect().unwrap();
    v.handle(&down(thumb.center()));
    v.handle(&UiEvent::PointerMove {
        pos: Point::new(thumb.center().x, 5000.0),
        modifiers: Modifiers::NONE,
    });
    v.handle(&up(Point::new(thumb.center().x, 5000.0)));
    assert!((v.scroll() - bottom).abs() < 0.5);
    // The last section is reachable from the table of contents and scrolled into place.
    let end = v.manual().toc().len() - 1;
    let at = v.toc_row_center(end).unwrap();
    click(&mut v, at);
    assert_eq!(v.title(), "End");
}

#[test]
fn the_page_is_laid_out_inside_its_column() {
    let v = view();
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let json = dl.to_json();
    assert!(json.contains("The rack"));
    let p = v.panel();
    assert!(p.width() <= 1120.0 && p.height() <= 820.0);
    assert!(Rect::new(0.0, 0.0, 1000.0, 700.0).contains(p.origin()));
}

#[test]
fn a_page_matches_its_golden() {
    let mut v = view();
    v.goto_section("The rack");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("manual_page_1x.json", &dl.to_json());
}

#[test]
fn a_search_matches_its_golden() {
    let mut v = view();
    type_text(&mut v, "tempo");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("manual_search_1x.json", &dl.to_json());
}
