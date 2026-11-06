//! The start screen box: its rows, the mouse and the keyboard, and what each pick asks for.

use super::*;
use atomik_ui::kurbo::Vec2;
use atomik_ui::{DisplayList, Modifiers};
use std::path::PathBuf;

const VIEW: Size = Size::new(1000.0, 800.0);

fn songs(n: usize) -> Vec<RecentRow> {
    (0..n)
        .map(|i| {
            let path = PathBuf::from(format!("/songs/set{i}/Song {i}.atomik"));
            RecentRow::new(&path, Some(1_700_000_000 + i as u64 * 86_400))
        })
        .collect()
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("Template {i}")).collect()
}

fn screen() -> StartScreen {
    StartScreen::new(names(3), songs(4), false, true)
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn down(p: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn centre(s: &StartScreen, target: Target) -> Point {
    s.rows(VIEW)
        .into_iter()
        .find(|(t, _)| *t == target)
        .unwrap_or_else(|| panic!("{target:?} is shown"))
        .1
        .center()
}

#[test]
fn a_recent_row_has_the_name_the_folder_and_the_day_of_the_file() {
    let row = RecentRow::new(
        &PathBuf::from("/songs/demo/Night Drive.atomik"),
        Some(1_700_000_000),
    );
    assert_eq!(row.name, "Night Drive");
    assert_eq!(row.folder, PathBuf::from("/songs/demo").to_string_lossy());
    assert_eq!(row.date, "2023-11-14");
    assert!(row.exists);
    let gone = RecentRow::new(&PathBuf::from("/songs/demo/Gone.atomik"), None);
    assert!(!gone.exists && gone.date.is_empty());
}

#[test]
fn it_lists_new_open_the_templates_and_the_recent_songs() {
    let s = screen();
    let targets: Vec<Target> = s.rows(VIEW).into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        targets,
        [
            Target::New,
            Target::Open,
            Target::Template(0),
            Target::Template(1),
            Target::Template(2),
            Target::Recent(0),
            Target::Recent(1),
            Target::Recent(2),
            Target::Recent(3),
            Target::ShowAtStart,
            Target::Close,
        ]
    );
    let lines = s.lines().join("\n");
    assert!(lines.contains("Template 1") && lines.contains("Song 3"));
    let long = StartScreen::new(names(20), songs(20), true, true);
    assert_eq!(long.templates().len(), VISIBLE_TEMPLATES);
    assert_eq!(long.recent().len(), VISIBLE_RECENT);
    assert!(
        long.lines()
            .contains(&"New Song (default song)".to_string())
    );
}

#[test]
fn empty_lists_say_so() {
    let s = StartScreen::new(Vec::new(), Vec::new(), false, true);
    let lines = s.lines();
    assert!(lines.contains(&"No templates yet".to_string()));
    assert!(lines.contains(&"No songs opened yet".to_string()));
    let mut list = DisplayList::new();
    s.paint(&mut list, &Theme::dark(), VIEW);
    let json = list.to_json();
    assert!(json.contains("No templates yet") && json.contains("No songs opened yet"));
}

#[test]
fn a_click_on_a_row_runs_the_file_menu_command_of_that_row() {
    for (target, command) in [
        (Target::New, AppCommand::New),
        (Target::Open, AppCommand::Open),
        (Target::Template(1), AppCommand::NewFromTemplate(1)),
        (Target::Recent(2), AppCommand::OpenRecent(2)),
    ] {
        let mut s = screen();
        let at = centre(&s, target);
        assert_eq!(s.handle(&down(at), VIEW), StartOutcome::Run(command));
    }
}

#[test]
fn close_escape_and_a_click_outside_close_it() {
    let mut s = screen();
    assert_eq!(s.handle(&key(Key::Escape), VIEW), StartOutcome::Close);
    let at = centre(&s, Target::Close);
    assert_eq!(s.handle(&down(at), VIEW), StartOutcome::Close);
    assert_eq!(
        s.handle(&down(Point::new(2.0, 2.0)), VIEW),
        StartOutcome::Close
    );
    let inside = s.rect(VIEW).center();
    // A click on empty room inside the box does nothing.
    let blank = Point::new(inside.x, s.rect(VIEW).y0 + 10.0);
    assert_eq!(s.handle(&down(blank), VIEW), StartOutcome::Open);
}

#[test]
fn the_check_box_flips_the_preference() {
    let mut s = screen();
    assert!(s.show_at_start());
    let at = centre(&s, Target::ShowAtStart);
    assert_eq!(s.handle(&down(at), VIEW), StartOutcome::ShowAtStart(false));
    assert!(!s.show_at_start());
    assert_eq!(s.focus(), Target::ShowAtStart);
    assert_eq!(
        s.handle(&key(Key::Space), VIEW),
        StartOutcome::ShowAtStart(true)
    );
}

#[test]
fn the_arrow_keys_move_inside_a_column_and_across_and_enter_picks() {
    let mut s = screen();
    assert_eq!(s.focus(), Target::New);
    assert_eq!(
        s.handle(&key(Key::Enter), VIEW),
        StartOutcome::Run(AppCommand::New)
    );
    s.handle(&key(Key::Up), VIEW);
    assert_eq!(s.focus(), Target::New, "the first row stays");
    s.handle(&key(Key::Down), VIEW);
    assert_eq!(s.focus(), Target::Open);
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    assert_eq!(s.focus(), Target::Template(1));
    for _ in 0..9 {
        s.handle(&key(Key::Down), VIEW);
    }
    assert_eq!(s.focus(), Target::Template(2), "the last template stays");
    // Right goes to the recent row at the nearest height; Left comes back.
    s.handle(&key(Key::Right), VIEW);
    assert!(matches!(s.focus(), Target::Recent(_)), "{:?}", s.focus());
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    assert_eq!(s.focus(), Target::Recent(3));
    assert_eq!(
        s.handle(&key(Key::Enter), VIEW),
        StartOutcome::Run(AppCommand::OpenRecent(3))
    );
    s.handle(&key(Key::Left), VIEW);
    assert!(matches!(
        s.focus(),
        Target::Template(_) | Target::Open | Target::New
    ));
    s.handle(&key(Key::Home), VIEW);
    assert_eq!(s.focus(), Target::New);
    s.handle(&key(Key::End), VIEW);
    assert_eq!(s.focus(), Target::Template(2));
}

#[test]
fn tab_visits_every_row_and_wraps_and_shift_tab_goes_back() {
    let mut s = screen();
    let all: Vec<Target> = s.rows(VIEW).into_iter().map(|(t, _)| t).collect();
    let mut seen = vec![s.focus()];
    for _ in 1..all.len() {
        s.handle(&key(Key::Tab), VIEW);
        seen.push(s.focus());
    }
    assert_eq!(seen, all);
    s.handle(&key(Key::Tab), VIEW);
    assert_eq!(s.focus(), Target::New, "it wraps");
    let shift = UiEvent::Key {
        key: Key::Tab,
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
        pressed: true,
    };
    s.handle(&shift, VIEW);
    assert_eq!(s.focus(), Target::Close, "Shift+Tab wraps backwards");
    s.handle(&key(Key::Up), VIEW);
    assert_eq!(s.focus(), Target::Template(2), "Up returns to the lists");
    // Close has the keys: Enter closes.
    let at = centre(&s, Target::Close);
    s.handle(&down(at), VIEW);
    assert_eq!(s.focus(), Target::Close);
    assert_eq!(s.handle(&key(Key::Enter), VIEW), StartOutcome::Close);
}

#[test]
fn with_nothing_recent_the_arrows_stay_in_the_left_column() {
    let mut s = StartScreen::new(names(2), Vec::new(), false, true);
    s.handle(&key(Key::Right), VIEW);
    assert_eq!(s.focus(), Target::New);
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Right), VIEW);
    assert_eq!(s.focus(), Target::Open);
}

#[test]
fn the_box_and_its_rows_stay_inside_the_window_even_a_small_one() {
    for size in [
        VIEW,
        Size::new(640.0, 480.0),
        Size::new(400.0, 300.0),
        Size::new(300.0, 260.0),
    ] {
        let s = StartScreen::new(names(20), songs(20), true, true);
        let r = s.rect(size);
        assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= size.width && r.y1 <= size.height);
        for (target, row) in s.rows(size) {
            assert!(
                r.contains(row.center()) && row.y1 <= r.y1,
                "{target:?} {row:?} in {r:?} at {size:?}"
            );
        }
        // The check box and Close are always there, and no two rows overlap.
        let rows = s.rows(size);
        assert!(rows.iter().any(|(t, _)| *t == Target::Close));
        for (i, (_, a)) in rows.iter().enumerate() {
            for (_, b) in &rows[i + 1..] {
                assert!(
                    a.intersect(*b).area() < 1.0,
                    "{a:?} overlaps {b:?} at {size:?}"
                );
            }
        }
        // Keys never land on a row that is not shown.
        let mut s = s;
        for _ in 0..30 {
            s.handle(&key(Key::Down), size);
            assert!(s.rows(size).iter().any(|(t, _)| *t == s.focus()));
        }
    }
}

#[test]
fn the_pointer_row_has_a_tooltip_that_says_what_it_does() {
    let s = StartScreen::new(names(1), songs(1), true, true);
    let help = |t: Target| s.help_at(VIEW, centre(&s, t)).expect("a tooltip").text;
    assert!(help(Target::New).contains("default song"));
    assert!(help(Target::Recent(0)).contains("Open"));
    assert!(help(Target::ShowAtStart).contains("starts"));
    assert!(s.help_at(VIEW, Point::new(1.0, 1.0)).is_none());
}

fn golden(name: &str, theme: &Theme) {
    let mut list = DisplayList::new();
    let mut s = StartScreen::new(
        vec![
            "Band Rehearsal".into(),
            "Beat Sketch".into(),
            "Vocal Take".into(),
        ],
        vec![
            RecentRow::new(
                &PathBuf::from("/songs/demo/Night Drive.atomik"),
                Some(1_700_000_000),
            ),
            RecentRow::new(&PathBuf::from("/songs/demo/Old Idea.atomik"), None),
        ],
        false,
        true,
    );
    s.handle(&key(Key::Down), VIEW);
    s.paint(&mut list, theme, VIEW);
    check_golden(name, &list.to_json());
}

fn check_golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
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
fn golden_start_screen_dark() {
    golden("start_screen", &Theme::dark());
}

#[test]
fn golden_start_screen_light() {
    golden("start_screen_light", &Theme::light());
}

fn row_rect(s: &StartScreen, target: Target) -> Rect {
    s.rows(VIEW)
        .into_iter()
        .find(|(t, _)| *t == target)
        .unwrap_or_else(|| panic!("{target:?} is shown"))
        .1
}

fn preview(tracks: usize) -> TemplatePreview {
    TemplatePreview {
        tracks: (0..tracks).map(|i| format!("Track {i}")).collect(),
        tempo_bpm: 128.0,
        meter: (3, 4),
    }
}

#[test]
fn the_pin_button_toggles_the_pin_and_does_not_open_the_song() {
    let mut s = screen();
    let pin = pin_rect(row_rect(&s, Target::Recent(2))).center();
    assert_eq!(s.row_at(VIEW, pin), Some(Target::Pin(2)), "the pin wins");
    // The rest of the row still opens the song.
    let name = row_rect(&s, Target::Recent(2)).center() - Vec2::new(40.0, 0.0);
    assert_eq!(s.row_at(VIEW, name), Some(Target::Recent(2)));
    assert_eq!(s.handle(&down(pin), VIEW), StartOutcome::TogglePin(2));
    assert_eq!(
        s.focus(),
        Target::Recent(2),
        "the keys stay on the song, not on the button"
    );
    assert_eq!(
        s.handle(&down(name), VIEW),
        StartOutcome::Run(AppCommand::OpenRecent(2))
    );
}

#[test]
fn p_pins_the_song_the_keys_are_on_and_nothing_else() {
    let mut s = screen();
    assert_eq!(s.handle(&key(Key::Char('p')), VIEW), StartOutcome::Open);
    s.handle(&key(Key::Right), VIEW);
    let Target::Recent(i) = s.focus() else {
        panic!("{:?}", s.focus());
    };
    assert_eq!(
        s.handle(&key(Key::Char('P')), VIEW),
        StartOutcome::TogglePin(i)
    );
    let ctrl = UiEvent::Key {
        key: Key::Char('p'),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
        pressed: true,
    };
    assert_eq!(s.handle(&ctrl, VIEW), StartOutcome::Open, "Ctrl+P is not P");
}

#[test]
fn a_pin_is_never_a_tab_stop_and_has_a_tooltip() {
    let s = screen();
    assert!(
        s.rows(VIEW)
            .iter()
            .all(|(t, _)| !matches!(t, Target::Pin(_)))
    );
    let pin = pin_rect(row_rect(&s, Target::Recent(0))).center();
    assert!(
        s.help_at(VIEW, pin)
            .unwrap()
            .text
            .starts_with("Pin this song")
    );
    let mut rows = songs(2);
    rows[0] = rows[0].clone().pinned(true);
    let s = StartScreen::new(names(1), rows, false, true);
    let pin = pin_rect(row_rect(&s, Target::Recent(0))).center();
    assert!(
        s.help_at(VIEW, pin)
            .unwrap()
            .text
            .starts_with("Unpin this song")
    );
    assert_eq!(Target::Pin(0).command(), None);
}

#[test]
fn new_recent_rows_keep_the_keys_on_the_song_that_moved() {
    let mut s = screen();
    s.handle(&key(Key::Right), VIEW);
    let mut rows = songs(4);
    rows.swap(0, 3);
    rows[0] = rows[0].clone().pinned(true);
    s.set_recent(rows, Some(0));
    assert_eq!(s.focus(), Target::Recent(0));
    assert!(s.recent()[0].pinned && !s.recent()[1].pinned);
    // A list that lost the row the keys were on falls back to New.
    s.set_recent(Vec::new(), None);
    assert_eq!(s.focus(), Target::New);
}

#[test]
fn the_card_follows_the_template_under_the_pointer_or_the_keys_and_asks_once() {
    let mut s = screen();
    assert_eq!(s.shown_template(), None);
    assert_eq!(s.preview_wanted(), None);
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    assert_eq!(s.focus(), Target::Template(0));
    assert_eq!(s.shown_template(), Some(0));
    assert_eq!(s.preview_wanted(), Some(0));
    assert_eq!(s.card_lines(0), ["Reading the template..."]);
    s.set_preview(0, Some(preview(2)));
    assert_eq!(s.preview_wanted(), None, "it is read once");
    // The pointer wins over the keys.
    let over = centre(&s, Target::Template(2));
    s.handle(
        &UiEvent::PointerMove {
            pos: over,
            modifiers: Modifiers::NONE,
        },
        VIEW,
    );
    assert_eq!(s.shown_template(), Some(2));
    assert_eq!(s.preview_wanted(), Some(2));
    s.set_preview(2, None);
    assert_eq!(s.card_lines(2), ["No preview for this template"]);
    assert_eq!(s.preview_wanted(), None);
    s.handle(&UiEvent::PointerLeave, VIEW);
    assert_eq!(s.shown_template(), Some(0));
}

#[test]
fn the_card_lists_tempo_meter_and_tracks_and_counts_the_rest() {
    let mut s = StartScreen::new(names(2), songs(1), false, true);
    s.set_preview(0, Some(preview(1)));
    assert_eq!(s.card_lines(0), ["128 BPM, 3/4, 1 track", "Track 0"]);
    let mut odd = preview(11);
    odd.tempo_bpm = 93.5;
    odd.meter = (4, 4);
    s.set_preview(1, Some(odd));
    let lines = s.card_lines(1);
    assert_eq!(lines[0], "93.50 BPM, 4/4, 11 tracks");
    assert_eq!(lines.len(), 1 + 8 + 1);
    assert_eq!(lines.last().unwrap(), "and 3 more");
    s.set_preview(0, Some(preview(0)));
    assert_eq!(s.card_lines(0), ["128 BPM, 3/4, no tracks"]);
}

#[test]
fn the_card_stays_inside_the_box_and_hides_in_a_narrow_window() {
    let mut s = screen();
    s.handle(&key(Key::End), VIEW);
    s.set_preview(2, Some(preview(11)));
    let mut list = DisplayList::new();
    s.paint(&mut list, &Theme::dark(), VIEW);
    let text = list.to_json();
    assert!(text.contains("Track 7") && text.contains("and 3 more"));
    // Narrow: one column, no card.
    let narrow = Size::new(420.0, 600.0);
    let mut list = DisplayList::new();
    s.paint(&mut list, &Theme::dark(), narrow);
    assert!(!list.to_json().contains("Track 0"));
}

fn golden_pinned(name: &str, theme: &Theme) {
    let mut list = DisplayList::new();
    let mut s = StartScreen::new(
        vec!["Band Rehearsal".into(), "Beat Sketch".into()],
        vec![
            RecentRow::new(
                &PathBuf::from("/songs/demo/Night Drive.atomik"),
                Some(1_700_000_000),
            )
            .pinned(true),
            RecentRow::new(
                &PathBuf::from("/songs/demo/Old Idea.atomik"),
                Some(1_690_000_000),
            ),
        ],
        false,
        true,
    );
    s.set_preview(
        0,
        Some(TemplatePreview {
            tracks: vec!["Drums".into(), "Bass".into(), "Keys".into()],
            tempo_bpm: 120.0,
            meter: (4, 4),
        }),
    );
    s.handle(&key(Key::Down), VIEW);
    s.handle(&key(Key::Down), VIEW);
    s.paint(&mut list, theme, VIEW);
    check_golden(name, &list.to_json());
}

#[test]
fn golden_start_screen_pinned_and_card_dark() {
    golden_pinned("start_screen_pinned", &Theme::dark());
}

#[test]
fn golden_start_screen_pinned_and_card_light() {
    golden_pinned("start_screen_pinned_light", &Theme::light());
}
