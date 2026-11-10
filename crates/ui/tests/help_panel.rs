//! The Help panel of Help Mode: wrapping of the measured text, the place of the "More" link and
//! the painted panel against goldens (a full card, a card without a shortcut or link, and a text
//! that is too long for its lines).

use atomik_ui::help_panel::{HELP_PANEL_HEIGHT, MAX_LINES, link_rect, paint, wrap};
use atomik_ui::kurbo::Rect;
use atomik_ui::{DisplayList, HelpCard, Theme};

fn rect() -> Rect {
    Rect::new(0.0, 700.0, 1000.0, 700.0 + HELP_PANEL_HEIGHT)
}

fn full_card() -> HelpCard {
    HelpCard::new(
        "Play",
        "Starts the song from the playhead. While the song is playing the same button stops it. \
         The lit button means the song is running.",
    )
    .with_shortcut(Some("Space".to_owned()))
    .with_more("The main window")
}

#[test]
fn the_text_wraps_at_the_width_and_never_past_the_last_line() {
    let mut dl = DisplayList::new();
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let lines = wrap(&mut dl, text, 120.0, 11.5, 400, 8);
    assert!(lines.len() > 1, "{lines:?}");
    assert_eq!(lines.join(" "), text, "no word is lost or cut");
    // Out of lines: the last one is cut with a dot and the rest is dropped.
    let short = wrap(&mut dl, text, 120.0, 11.5, 400, 2);
    assert_eq!(short.len(), 2);
    assert!(short[1].ends_with('.'), "{short:?}");
    // Nothing to say, nothing to draw.
    assert!(wrap(&mut dl, "   ", 120.0, 11.5, 400, 3).is_empty());
}

#[test]
fn a_word_wider_than_the_line_is_cut_instead_of_overflowing() {
    let mut dl = DisplayList::new();
    let lines = wrap(
        &mut dl,
        "Supercalifragilisticexpialidocious_and_more",
        60.0,
        11.5,
        400,
        3,
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with('.'));
}

#[test]
fn the_link_sits_at_the_foot_of_the_panel_and_only_a_card_with_a_section_has_one() {
    let link = link_rect(rect(), &full_card()).expect("a link");
    assert!(rect().contains(link.center()));
    assert!(link.y1 <= rect().y1 && link.y0 > rect().y0 + 50.0);
    assert!(link.x0 >= rect().x0);
    assert_eq!(link_rect(rect(), &HelpCard::new("Stop", "Stops.")), None);
    // A narrow panel keeps the link inside.
    let narrow = Rect::new(0.0, 0.0, 90.0, HELP_PANEL_HEIGHT);
    assert!(link_rect(narrow, &full_card()).unwrap().x1 <= narrow.x1);
}

#[test]
fn the_text_leaves_the_link_line_free() {
    let mut dl = DisplayList::new();
    let long = HelpCard::new("Long", "word ".repeat(200)).with_more("Devices");
    paint(&mut dl, &Theme::dark(), rect(), &long);
    let texts = dl.texts();
    // The title, the body lines (one fewer than the maximum, for the link) and the link.
    assert_eq!(texts.len(), 1 + (MAX_LINES - 1) + 1, "{texts:?}");
    assert!(texts.contains(&"More: Devices"));
}

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

fn painted(card: &HelpCard, theme: &Theme) -> String {
    let mut dl = DisplayList::new();
    paint(&mut dl, theme, rect(), card);
    dl.to_json()
}

#[test]
fn a_full_card_matches_its_golden() {
    check(
        "help_panel_card_1x.json",
        &painted(&full_card(), &Theme::dark()),
    );
}

#[test]
fn a_card_in_the_light_theme_matches_its_golden() {
    check(
        "help_panel_card_light_1x.json",
        &painted(&full_card(), &Theme::light()),
    );
}

#[test]
fn a_card_without_a_shortcut_or_a_link_matches_its_golden() {
    let card = HelpCard::new(
        "Help Mode",
        "Point at any control to read about it here. Clicks still work as usual.",
    );
    check("help_panel_idle_1x.json", &painted(&card, &Theme::dark()));
}

#[test]
fn a_text_too_long_for_its_lines_matches_its_golden() {
    let card = HelpCard::new(
        "Tempo",
        "The speed of the song in beats per minute. Drag or scroll the number to change it, hold \
         Shift for fine steps, or double-click to type a value. Tempo changes along the song \
         live in the tempo lane, and the transport bar always shows the tempo at the playhead.",
    )
    .with_more("The main window");
    check("help_panel_long_1x.json", &painted(&card, &Theme::dark()));
}
