//! FileBrowserView behaviour: list, selection, preview, keyboard, search, tree, menus, drag,
//! info requests and large-list performance. Driven through `handle` and the display list.

use atomik_library::{
    AudioFormat, AudioInfo, FileKind, FileRow, ItemInfo, LibrarySection, device_uri,
};
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::{
    BrowseAction, DisplayList, FileBrowserView, Key, Modifiers, PlacesData, PointerButton, Theme,
    TreeChild, UiAction, UiEvent, action::Place,
};
use std::path::{Path, PathBuf};

// Geometry of a 300 x 600 panel at the origin with tree_fraction 0.4 (see layout.rs):
// header 0..34, tree 34..260.4, splitter 260.4..266.4, path 266.4..288.4,
// column header 288.4..308.4, list rows from 308.4 (22 px each); tree rows 20 px each.
const LIST_Y: f64 = 308.4;
const TREE_Y: f64 = 34.0;

fn row_pt(i: usize) -> Point {
    Point::new(80.0, LIST_Y + 22.0 * i as f64 + 11.0)
}

/// Rows of the library block (heading and five sections) above the Favourites entry.
const LIBRARY_ROWS: usize = 6;

/// The point of tree item `i` counted from Favourites, the first entry below the library block.
fn tree_pt(i: usize, x: f64) -> Point {
    lib_pt(LIBRARY_ROWS + i, x)
}

/// The point of tree item `i` counted from the top of the tree (the library heading is 0).
fn lib_pt(i: usize, x: f64) -> Point {
    Point::new(x, TREE_Y + 20.0 * i as f64 + 10.0)
}

fn info() -> AudioInfo {
    AudioInfo {
        sample_rate: 44100,
        channels: 2,
        frames: Some(44100 * 3),
    }
}

fn audio(name: &str) -> FileRow {
    FileRow {
        path: PathBuf::from(format!("/lib/{name}")),
        name: name.to_string(),
        kind: FileKind::Audio(AudioFormat::Wav),
        size: 2048,
        info: Some(info()),
        item: None,
        favorite: false,
        tags: Vec::new(),
        author: String::new(),
        categories: Vec::new(),
    }
}

fn folder(name: &str) -> FileRow {
    FileRow {
        path: PathBuf::from(format!("/lib/{name}")),
        name: name.to_string(),
        kind: FileKind::Folder,
        size: 0,
        info: None,
        item: None,
        favorite: false,
        tags: Vec::new(),
        author: String::new(),
        categories: Vec::new(),
    }
}

fn p(name: &str) -> PathBuf {
    PathBuf::from(format!("/lib/{name}"))
}

/// Sorted order: 0 drums, 1 hat, 2 kick 1, 3 kick 2, 4 kick 10, 5 snare.
fn sample_rows() -> Vec<FileRow> {
    vec![
        audio("snare.wav"),
        audio("kick 10.wav"),
        folder("drums"),
        audio("kick 2.wav"),
        audio("hat.wav"),
        audio("kick 1.wav"),
    ]
}

fn view() -> FileBrowserView {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_location(Place::Folder(PathBuf::from("/lib")));
    v.set_rows(sample_rows());
    v
}

fn places() -> PlacesData {
    PlacesData {
        favorites: 3,
        tags: vec![("808".into(), 4), ("snare".into(), 2)],
        lists: Vec::new(),
        bookmarks: vec![PathBuf::from("/music/drums")],
        roots: vec![PathBuf::from("/"), PathBuf::from("/home/me")],
    }
}

fn down_with(pos: Point, modifiers: Modifiers, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers,
        clicks,
    }
}

fn down(pos: Point) -> UiEvent {
    down_with(pos, Modifiers::NONE, 1)
}

fn up(pos: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn mv(pos: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos,
        modifiers: Modifiers::NONE,
    }
}

fn right(pos: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn key_with(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

fn key(k: Key) -> UiEvent {
    key_with(k, Modifiers::NONE)
}

fn ch(c: char) -> UiEvent {
    key(Key::Char(c))
}

fn wheel(pos: Point, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        delta: Vec2::new(0.0, dy),
        pos,
        modifiers: Modifiers::NONE,
    }
}

fn browse(actions: Vec<UiAction>) -> Vec<BrowseAction> {
    actions
        .into_iter()
        .map(|a| match a {
            UiAction::Browse(b) => b,
            other => panic!("unexpected action {other:?}"),
        })
        .collect()
}

fn run(v: &mut FileBrowserView, events: &[UiEvent]) -> Vec<BrowseAction> {
    let mut all = Vec::new();
    for e in events {
        all.extend(browse(v.handle(e)));
    }
    all
}

fn typed(v: &mut FileBrowserView, text: &str) {
    for c in text.chars() {
        run(v, &[if c == ' ' { key(Key::Space) } else { ch(c) }]);
    }
}

fn paint(v: &mut FileBrowserView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl
}

/// Clears the initial RequestInfo traffic.
fn settled(mut v: FileBrowserView) -> FileBrowserView {
    v.handle(&UiEvent::Resize(Size::new(300.0, 600.0)));
    v
}

// ----- list, selection, preview -----------------------------------------------------------

#[test]
fn click_on_an_audio_row_selects_and_previews_on_press() {
    let mut v = settled(view());
    assert_eq!(
        run(&mut v, &[down(row_pt(1))]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
    // Folders do not preview.
    assert!(run(&mut v, &[up(row_pt(1)), down(row_pt(0)), up(row_pt(0))]).is_empty());
}

#[test]
fn rows_are_naturally_sorted_with_folders_first() {
    let mut v = settled(view());
    let order: Vec<BrowseAction> = (2..5)
        .flat_map(|i| run(&mut v, &[down(row_pt(i)), up(row_pt(i))]))
        .collect();
    assert_eq!(
        order,
        vec![
            BrowseAction::Preview(p("kick 1.wav")),
            BrowseAction::Preview(p("kick 2.wav")),
            BrowseAction::Preview(p("kick 10.wav")),
        ]
    );
}

#[test]
fn double_click_on_a_folder_shows_it() {
    let mut v = settled(view());
    let acts = run(
        &mut v,
        &[
            down(row_pt(0)),
            up(row_pt(0)),
            down_with(row_pt(0), Modifiers::NONE, 2),
        ],
    );
    assert_eq!(acts, vec![BrowseAction::Show(Place::Folder(p("drums")))]);
}

#[test]
fn auto_preview_off_selects_without_sound_but_space_still_plays() {
    let mut v = settled(view());
    v.set_auto_preview(false);
    assert!(run(&mut v, &[down(row_pt(1)), up(row_pt(1))]).is_empty());
    assert_eq!(
        run(&mut v, &[key(Key::Space)]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
    assert!(run(&mut v, &[key(Key::Down)]).is_empty());
}

#[test]
fn ctrl_click_toggles_and_shift_click_selects_a_range() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    let acts = run(
        &mut v,
        &[down_with(row_pt(3), Modifiers::CTRL, 1), up(row_pt(3))],
    );
    assert!(acts.is_empty(), "no preview while multi-selecting");
    assert_eq!(
        run(&mut v, &[ch('f')]),
        vec![BrowseAction::ToggleFavorite(vec![
            p("hat.wav"),
            p("kick 2.wav")
        ])]
    );
    run(
        &mut v,
        &[down_with(row_pt(5), Modifiers::SHIFT, 1), up(row_pt(5))],
    );
    assert_eq!(
        run(&mut v, &[ch('F')]),
        vec![BrowseAction::ToggleFavorite(vec![
            p("kick 2.wav"),
            p("kick 10.wav"),
            p("snare.wav")
        ])]
    );
    // Ctrl-click again removes a row.
    run(
        &mut v,
        &[down_with(row_pt(4), Modifiers::CTRL, 1), up(row_pt(4))],
    );
    assert_eq!(
        run(&mut v, &[ch('f')]),
        vec![BrowseAction::ToggleFavorite(vec![
            p("kick 2.wav"),
            p("snare.wav")
        ])]
    );
}

#[test]
fn star_click_toggles_favourite_without_selecting() {
    let mut v = settled(view());
    let star = Point::new(11.0, row_pt(2).y);
    assert_eq!(
        run(&mut v, &[down(star), up(star)]),
        vec![BrowseAction::ToggleFavorite(vec![p("kick 1.wav")])]
    );
}

#[test]
fn header_click_sorts_and_toggles_descending() {
    let mut v = settled(view());
    let name = Point::new(60.0, 298.0);
    run(&mut v, &[down(name), up(name)]);
    // Descending: folders first, then snare, kick 10, kick 2, kick 1, hat.
    assert_eq!(
        run(&mut v, &[down(row_pt(1)), up(row_pt(1))]),
        vec![BrowseAction::Preview(p("snare.wav"))]
    );
    run(&mut v, &[down(name), up(name)]);
    assert_eq!(
        run(&mut v, &[down(row_pt(1)), up(row_pt(1))]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
}

#[test]
fn clicking_the_duration_header_sorts_by_duration() {
    let mut rows = sample_rows();
    for r in &mut rows {
        if r.name == "hat.wav" {
            r.info = Some(AudioInfo {
                frames: Some(44100 * 10),
                ..info()
            });
        }
    }
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_rows(rows);
    let dur = Point::new(300.0 - 5.0 - 7.0 - 4.0 - 20.0, 298.0);
    run(&mut v, &[down(dur), up(dur)]);
    // Ascending: the 10 s hat is last.
    assert_eq!(
        run(&mut v, &[down(row_pt(5)), up(row_pt(5))]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
}

// ----- keyboard -------------------------------------------------------------------------------

#[test]
fn up_down_move_the_selection_and_auto_preview() {
    let mut v = settled(view());
    assert_eq!(
        run(&mut v, &[key(Key::Down)]),
        vec![],
        "first Down lands on a folder (row 0)"
    );
    assert_eq!(
        run(&mut v, &[key(Key::Down)]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
    assert_eq!(
        run(&mut v, &[key(Key::Down), key(Key::Up)]),
        vec![
            BrowseAction::Preview(p("kick 1.wav")),
            BrowseAction::Preview(p("hat.wav"))
        ]
    );
    // Clamped at the ends.
    run(&mut v, &[key(Key::End)]);
    assert!(
        run(&mut v, &[key(Key::Down)])
            .iter()
            .all(|a| matches!(a, BrowseAction::Preview(path) if *path == p("snare.wav")))
    );
}

#[test]
fn space_toggles_preview_of_the_selected_file() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    assert_eq!(
        run(&mut v, &[key(Key::Space)]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
    v.set_preview(Some(p("hat.wav")), 0.2);
    assert_eq!(
        run(&mut v, &[key(Key::Space)]),
        vec![BrowseAction::StopPreview]
    );
    v.set_preview(Some(p("snare.wav")), 0.2);
    assert_eq!(
        run(&mut v, &[key(Key::Space)]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
}

#[test]
fn escape_stops_the_preview_and_enter_acts_on_the_cursor() {
    let mut v = settled(view());
    assert_eq!(
        run(&mut v, &[key(Key::Escape)]),
        vec![BrowseAction::StopPreview]
    );
    run(&mut v, &[down(row_pt(0)), up(row_pt(0))]);
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::Show(Place::Folder(p("drums")))]
    );
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::Preview(p("hat.wav"))]
    );
}

#[test]
fn select_path_selects_the_row_alone_and_scrolls_it_into_view() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(0)), up(row_pt(0))]);
    assert!(v.select_path(&p("snare.wav")));
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::Preview(p("snare.wav"))],
        "the cursor moved to the row"
    );
    assert!(!v.select_path(&p("nope.wav")), "no such row");
    // A long list: the selected row far down is scrolled to.
    let rows: Vec<FileRow> = (0..200).map(|i| audio(&format!("s{i:03}.wav"))).collect();
    v.set_rows(rows);
    assert!(!paint(&mut v).texts().contains(&"s199.wav"));
    assert!(v.select_path(&p("s199.wav")));
    assert!(paint(&mut v).texts().contains(&"s199.wav"));
    assert!(v.select_path(&p("s000.wav")));
    assert!(paint(&mut v).texts().contains(&"s000.wav"));
    assert!(v.needs_repaint() || !paint(&mut v).texts().is_empty());
}

#[test]
fn backspace_and_alt_up_go_to_the_parent() {
    let mut v = settled(view());
    assert_eq!(
        run(&mut v, &[key(Key::Backspace)]),
        vec![BrowseAction::Parent]
    );
    assert_eq!(
        run(
            &mut v,
            &[key_with(
                Key::Up,
                Modifiers {
                    alt: true,
                    ..Modifiers::NONE
                }
            )]
        ),
        vec![BrowseAction::Parent]
    );
}

#[test]
fn keys_are_ignored_without_focus() {
    let mut v = settled(view());
    v.set_focused(false);
    assert!(run(&mut v, &[key(Key::Backspace), key(Key::Space), ch('f')]).is_empty());
}

#[test]
fn path_bar_segments_navigate() {
    let mut v = settled(view());
    v.set_location(Place::Folder(PathBuf::from("/music/drums/kicks")));
    let mut dl = paint(&mut v);
    assert!(dl.texts().contains(&"kicks"));
    // Segments: "/", "music", "drums", "kicks"; "music" starts after "/" (8+14 wide) + sep.
    let y = 277.0;
    let acts = run(
        &mut v,
        &[
            down(Point::new(8.0 + 12.0 + 12.0 + 6.0, y)),
            up(Point::new(50.0, y)),
        ],
    );
    assert_eq!(
        acts,
        vec![BrowseAction::Show(Place::Folder(PathBuf::from("/music")))]
    );
    dl = paint(&mut v);
    assert!(!dl.texts().is_empty());
}

// ----- scrolling --------------------------------------------------------------------------------

fn many(n: usize) -> Vec<FileRow> {
    (0..n).map(|i| audio(&format!("s{i:06}.wav"))).collect()
}

#[test]
fn wheel_scrolls_three_rows_per_notch_and_clamps() {
    let mut v = view();
    v.set_rows(many(100));
    v.handle(&UiEvent::Resize(Size::new(300.0, 600.0)));
    run(&mut v, &[wheel(row_pt(2), -1.0)]);
    assert_eq!(
        run(&mut v, &[down(row_pt(0)), up(row_pt(0))]),
        vec![BrowseAction::Preview(p("s000003.wav"))]
    );
    run(&mut v, &[wheel(row_pt(2), 50.0)]);
    assert_eq!(
        run(&mut v, &[down(row_pt(0)), up(row_pt(0))]),
        vec![BrowseAction::Preview(p("s000000.wav"))]
    );
    run(&mut v, &[wheel(row_pt(2), -500.0)]);
    let last = run(
        &mut v,
        &[down(Point::new(80.0, 599.0)), up(Point::new(80.0, 599.0))],
    );
    assert_eq!(last, vec![BrowseAction::Preview(p("s000099.wav"))]);
}

#[test]
fn scroll_bar_thumb_drags_the_list() {
    let mut v = view();
    v.set_rows(many(1000));
    let thumb = Point::new(300.0 - 5.0 - 3.0, LIST_Y + 4.0);
    assert!(run(&mut v, &[down(thumb)]).is_empty());
    assert!(v.is_dragging());
    run(&mut v, &[mv(Point::new(thumb.x, thumb.y + 100.0))]);
    run(&mut v, &[up(Point::new(thumb.x, thumb.y + 100.0))]);
    assert!(!v.is_dragging());
    let acts = run(&mut v, &[down(row_pt(0)), up(row_pt(0))]);
    let BrowseAction::Preview(path) = &acts[0] else {
        panic!("{acts:?}")
    };
    let n: usize = path.to_string_lossy()[6..12].parse().unwrap();
    assert!(n > 100, "dragged well past the top: {n}");
}

#[test]
fn keyboard_navigation_scrolls_the_cursor_into_view() {
    let mut v = view();
    v.set_rows(many(1000));
    run(&mut v, &[key(Key::End)]);
    let dl = paint(&mut v);
    assert!(dl.texts().contains(&"s000999.wav"));
    run(&mut v, &[key(Key::Home)]);
    let dl = paint(&mut v);
    assert!(dl.texts().contains(&"s000000.wav"));
}

// ----- painting --------------------------------------------------------------------------------

#[test]
fn paint_op_count_is_independent_of_the_row_count() {
    let mut small = view();
    small.set_rows(many(50));
    let mut big = view();
    big.set_rows(many(100_000));
    let (a, b) = (paint(&mut small).ops().len(), paint(&mut big).ops().len());
    assert!(b < 1500, "bounded paint: {b} ops");
    assert!(a.abs_diff(b) < 150, "{a} vs {b}");
    run(&mut big, &[key(Key::End)]);
    assert!(paint(&mut big).ops().len() < 1500);
}

#[test]
fn needs_repaint_follows_changes() {
    let mut v = view();
    paint(&mut v);
    assert!(!v.needs_repaint());
    v.set_preview(Some(p("hat.wav")), 0.5);
    assert!(v.needs_repaint());
    paint(&mut v);
    v.set_preview(Some(p("hat.wav")), 0.5);
    assert!(!v.needs_repaint());
    v.handle(&mv(row_pt(1)));
    assert!(v.needs_repaint());
    paint(&mut v);
    v.handle(&mv(Point::new(row_pt(1).x + 1.0, row_pt(1).y)));
    assert!(!v.needs_repaint(), "same row: nothing changed");
}

#[test]
fn rows_show_duration_tags_and_playing_progress() {
    let mut v = view();
    let mut rows = sample_rows();
    rows[0].tags = vec!["808".into(), "punchy".into()];
    rows[3].info = None;
    v.set_rows(rows);
    v.set_preview(Some(p("hat.wav")), 0.5);
    let dl = paint(&mut v);
    let t = dl.texts();
    assert!(t.contains(&"3.0s"));
    assert!(t.contains(&"\u{2014}"), "unknown duration");
    assert!(t.contains(&"hat.wav"));
}

#[test]
fn wide_panels_show_rate_and_size_columns() {
    let mut v = view();
    v.set_bounds(Rect::new(0.0, 0.0, 500.0, 600.0));
    let t: Vec<String> = paint(&mut v)
        .texts()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(t.iter().any(|s| s == "Rate") && t.iter().any(|s| s == "Size"));
    assert!(t.iter().any(|s| s == "44.1k"));
    assert!(t.iter().any(|s| s == "2 KB"));
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    let t: Vec<String> = paint(&mut v)
        .texts()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(!t.iter().any(|s| s == "Rate" || s == "Size"));
}

#[test]
fn patching_fills_in_info_and_marks() {
    let mut v = view();
    let mut rows = sample_rows();
    for r in &mut rows {
        r.info = None;
    }
    v.set_rows(rows);
    assert!(!paint(&mut v).texts().contains(&"3.0s"));
    v.patch_info(&[(p("hat.wav"), info())]);
    assert!(paint(&mut v).texts().contains(&"3.0s"));
    v.patch_marks(&[(p("hat.wav"), true, vec!["808".into()])]);
    let t: Vec<String> = paint(&mut v)
        .texts()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(t.iter().any(|s| s == "808"), "chip: {t:?}");
}

#[test]
fn append_rows_keeps_selection_and_scroll() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    v.append_rows(vec![audio("aaa.wav")]);
    // "aaa.wav" sorts before "hat.wav": the selected row moved down by one.
    assert_eq!(
        run(&mut v, &[ch('f')]),
        vec![BrowseAction::ToggleFavorite(vec![p("hat.wav")])]
    );
}

// ----- search ----------------------------------------------------------------------------------

#[test]
fn search_filters_per_keystroke_and_owns_the_keyboard() {
    let mut v = settled(view());
    assert!(!v.is_text_owner());
    run(
        &mut v,
        &[down(Point::new(60.0, 17.0)), up(Point::new(60.0, 17.0))],
    );
    assert!(v.is_text_owner());
    typed(&mut v, "kick");
    // Letters typed in the field are not shortcuts (f would favourite).
    typed(&mut v, " 1");
    assert!(paint(&mut v).texts().contains(&"kick 1.wav"));
    assert!(!paint(&mut v).texts().contains(&"snare.wav"));
    run(&mut v, &[key(Key::Backspace), key(Key::Backspace)]);
    run(&mut v, &[key(Key::Enter)]);
    assert!(!v.is_text_owner());
    // Three kicks remain, first row is kick 1.
    assert_eq!(
        run(&mut v, &[down(row_pt(0)), up(row_pt(0))]),
        vec![BrowseAction::Preview(p("kick 1.wav"))]
    );
    assert!(paint(&mut v).texts().contains(&"kick 10.wav"));
    assert!(!paint(&mut v).texts().contains(&"hat.wav"));
}

#[test]
fn search_understands_tags_and_favourites() {
    let mut rows = sample_rows();
    rows[0].tags = vec!["808".into()];
    rows[4].favorite = true;
    let mut v = settled(view());
    v.set_rows(rows);
    v.focus_search();
    typed(&mut v, "#808");
    let t = paint(&mut v);
    assert!(t.texts().contains(&"snare.wav") && !t.texts().contains(&"hat.wav"));
    run(&mut v, &[key(Key::Char('a'))]);
    run(&mut v, &[key_with(Key::Char('a'), Modifiers::CTRL)]);
    typed(&mut v, "fav");
    let t = paint(&mut v);
    assert!(t.texts().contains(&"kick 10.wav") || t.texts().contains(&"hat.wav"));
    assert!(!t.texts().contains(&"snare.wav"));
}

#[test]
fn ctrl_backspace_deletes_a_word_and_ctrl_a_replaces() {
    let mut v = settled(view());
    v.focus_search();
    typed(&mut v, "snare kick");
    run(&mut v, &[key_with(Key::Backspace, Modifiers::CTRL)]);
    let t = paint(&mut v);
    assert!(t.texts().contains(&"snare |"), "{:?}", t.texts());
    run(&mut v, &[key_with(Key::Char('a'), Modifiers::CTRL)]);
    typed(&mut v, "hat");
    let t = paint(&mut v);
    assert!(t.texts().contains(&"hat|"));
}

#[test]
fn escape_clears_and_unfocuses_the_search_before_stopping_the_preview() {
    let mut v = settled(view());
    v.focus_search();
    typed(&mut v, "kick");
    assert!(run(&mut v, &[key(Key::Escape)]).is_empty());
    assert!(!v.is_text_owner());
    assert!(paint(&mut v).texts().contains(&"hat.wav"), "query cleared");
    assert_eq!(
        run(&mut v, &[key(Key::Escape)]),
        vec![BrowseAction::StopPreview]
    );
}

#[test]
fn down_in_the_search_moves_to_the_list() {
    let mut v = settled(view());
    v.focus_search();
    typed(&mut v, "kick");
    assert_eq!(
        run(&mut v, &[key(Key::Down)]),
        vec![BrowseAction::Preview(p("kick 1.wav"))]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn toolbar_toggles_emit_and_report_state() {
    let mut v = settled(view());
    assert!(!v.recursive() && v.auto_preview());
    // recursive: x 125..159 at width 300 (content 295 - 8 - 24 - 4 - 34), y 6..28.
    let rec = Point::new(240.0, 17.0);
    assert_eq!(
        run(&mut v, &[down(rec), up(rec)]),
        vec![BrowseAction::SetRecursive(true)]
    );
    assert!(v.recursive());
    let auto = Point::new(275.0, 17.0);
    assert_eq!(
        run(&mut v, &[down(auto), up(auto)]),
        vec![BrowseAction::SetAutoPreview(false)]
    );
    assert!(!v.auto_preview());
}

// ----- resize and splitter ----------------------------------------------------------------------

#[test]
fn dragging_the_right_edge_resizes_and_reports_on_release() {
    let mut v = settled(view());
    let edge = Point::new(298.0, 200.0);
    assert!(run(&mut v, &[down(edge)]).is_empty());
    assert!(v.is_dragging());
    assert!(run(&mut v, &[mv(Point::new(400.0, 210.0))]).is_empty());
    assert!((v.width() - 400.0).abs() < 1e-9);
    assert_eq!(
        run(&mut v, &[up(Point::new(400.0, 210.0))]),
        vec![BrowseAction::LayoutChanged]
    );
    assert!(!v.is_dragging());
    run(
        &mut v,
        &[down(Point::new(398.0, 200.0)), mv(Point::new(50.0, 200.0))],
    );
    assert!((v.width() - FileBrowserView::MIN_WIDTH).abs() < 1e-9);
    run(&mut v, &[up(Point::new(50.0, 200.0))]);
}

#[test]
fn set_width_clamps_to_the_minimum() {
    let mut v = view();
    v.set_width(50.0);
    assert!((v.width() - FileBrowserView::MIN_WIDTH).abs() < 1e-9);
    v.set_width(333.0);
    assert!((v.width() - 333.0).abs() < 1e-9);
}

#[test]
fn dragging_the_splitter_changes_the_tree_fraction() {
    let mut v = settled(view());
    let split = Point::new(100.0, 263.0);
    run(&mut v, &[down(split)]);
    assert!(v.is_dragging());
    run(&mut v, &[mv(Point::new(100.0, 34.0 + 566.0 * 0.7 + 3.0))]);
    assert!(
        (v.tree_fraction() - 0.7).abs() < 0.01,
        "{}",
        v.tree_fraction()
    );
    assert_eq!(
        run(&mut v, &[up(Point::new(100.0, 400.0))]),
        vec![BrowseAction::LayoutChanged]
    );
    v.set_tree_fraction(5.0);
    assert!((v.tree_fraction() - 0.9).abs() < 1e-9);
}

// ----- tree ----------------------------------------------------------------------------------------

fn tree_view() -> FileBrowserView {
    let mut v = settled(view());
    v.set_places(places());
    v
}

// Items (counted by `tree_pt` from Favourites): 0 Favourites, 1 Tags, 2 808, 3 snare,
// 4 BOOKMARKS, 5 drums, 6 LOCATIONS, 7 "/", 8 me. Above them: 0 LIBRARY, 1 Devices, 2 Patches,
// 3 Containers, 4 Clips, 5 Projects (`lib_pt`).

/// A taller panel, so the whole tree is visible.
fn tall_tree_view() -> FileBrowserView {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 800.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_location(Place::Folder(PathBuf::from("/lib")));
    v.set_rows(sample_rows());
    v.handle(&UiEvent::Resize(Size::new(300.0, 800.0)));
    v.set_places(places());
    v
}

#[test]
fn tree_entries_show_places() {
    let mut v = tall_tree_view();
    assert_eq!(
        run(&mut v, &[down(tree_pt(0, 60.0)), up(tree_pt(0, 60.0))]),
        vec![BrowseAction::Show(Place::Favorites)]
    );
    assert_eq!(
        run(&mut v, &[down(tree_pt(2, 60.0)), up(tree_pt(2, 60.0))]),
        vec![BrowseAction::Show(Place::Tag("808".into()))]
    );
    assert_eq!(
        run(&mut v, &[down(tree_pt(5, 60.0)), up(tree_pt(5, 60.0))]),
        vec![BrowseAction::Show(Place::Folder(PathBuf::from(
            "/music/drums"
        )))]
    );
    // Headings do nothing.
    assert!(run(&mut v, &[down(tree_pt(4, 60.0)), up(tree_pt(4, 60.0))]).is_empty());
    let t = paint(&mut v);
    assert!(t.texts().contains(&"Favourites") && t.texts().contains(&"808"));
}

#[test]
fn tags_entry_collapses_and_expands() {
    let mut v = tree_view();
    run(&mut v, &[down(tree_pt(1, 60.0)), up(tree_pt(1, 60.0))]);
    assert!(!paint(&mut v).texts().contains(&"808"));
    run(&mut v, &[down(tree_pt(1, 15.0)), up(tree_pt(1, 15.0))]);
    assert!(paint(&mut v).texts().contains(&"808"));
}

#[test]
fn expanding_a_folder_requests_its_children_once() {
    let mut v = tall_tree_view();
    let arrow = tree_pt(5, 15.0);
    assert_eq!(
        run(&mut v, &[down(arrow), up(arrow)]),
        vec![BrowseAction::ListChildren(PathBuf::from("/music/drums"))]
    );
    assert!(
        paint(&mut v)
            .texts()
            .iter()
            .any(|t| t.contains("drums ...")),
        "loading"
    );
    // Collapse and expand again before the answer: no second request.
    assert!(run(&mut v, &[down(arrow), up(arrow), down(arrow), up(arrow)]).is_empty());
    v.set_children(
        Path::new("/music/drums"),
        vec![
            TreeChild {
                name: "kicks".into(),
                path: PathBuf::from("/music/drums/kicks"),
            },
            TreeChild {
                name: "snares".into(),
                path: PathBuf::from("/music/drums/snares"),
            },
        ],
    );
    let t = paint(&mut v);
    assert!(t.texts().contains(&"kicks") && t.texts().contains(&"snares"));
    // Clicking the child's label shows it.
    assert_eq!(
        run(&mut v, &[down(tree_pt(6, 80.0)), up(tree_pt(6, 80.0))]),
        vec![BrowseAction::Show(Place::Folder(PathBuf::from(
            "/music/drums/kicks"
        )))]
    );
}

#[test]
fn a_folder_without_children_becomes_a_leaf() {
    let mut v = tall_tree_view();
    let arrow = tree_pt(5, 15.0);
    run(&mut v, &[down(arrow), up(arrow)]);
    v.set_children(Path::new("/music/drums"), Vec::new());
    // The arrow zone no longer expands: it shows the folder.
    assert_eq!(
        run(&mut v, &[down(arrow), up(arrow)]),
        vec![BrowseAction::Show(Place::Folder(PathBuf::from(
            "/music/drums"
        )))]
    );
}

#[test]
fn tree_keyboard_navigates_expands_and_shows() {
    let mut v = tree_view();
    run(&mut v, &[down(tree_pt(0, 60.0)), up(tree_pt(0, 60.0))]);
    assert!(run(&mut v, &[key(Key::Down), key(Key::Down)]).is_empty());
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::Show(Place::Tag("808".into()))]
    );
    // Down skips the heading to the bookmark; Right expands it (children requested).
    run(&mut v, &[key(Key::Down), key(Key::Down)]);
    assert_eq!(
        run(&mut v, &[key(Key::Right)]),
        vec![BrowseAction::ListChildren(PathBuf::from("/music/drums"))]
    );
    v.set_children(
        Path::new("/music/drums"),
        vec![TreeChild {
            name: "kicks".into(),
            path: PathBuf::from("/music/drums/kicks"),
        }],
    );
    // Right on an expanded node enters it; Left goes back to the parent, then collapses.
    run(&mut v, &[key(Key::Right)]);
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::Show(Place::Folder(PathBuf::from(
            "/music/drums/kicks"
        )))]
    );
    run(&mut v, &[key(Key::Left)]);
    assert!(!paint(&mut v).texts().is_empty());
    run(&mut v, &[key(Key::Left)]);
    assert!(!paint(&mut v).texts().contains(&"kicks"));
    // Up from the first entry stays put.
    run(&mut v, &[key(Key::Home)]);
}

// ----- context menus ---------------------------------------------------------------------------

/// The centre of item `k` (rows of 22 px, after the 4 px padding) of a menu opened at `at`.
fn menu_item(at: Point, k: usize) -> Point {
    Point::new(at.x + 20.0, at.y + 4.0 + 22.0 * k as f64 + 11.0)
}

fn menu_view() -> FileBrowserView {
    let mut v = tree_view();
    let mut rows = sample_rows();
    rows[0].tags = vec!["808".into()]; // snare (sorted last)
    v.set_rows(rows);
    v
}

#[test]
fn list_menu_toggles_existing_tags_on_the_selection() {
    let mut v = menu_view();
    // Row 1 (hat) has no tags: choosing "808" adds it.
    let at = row_pt(1);
    assert!(run(&mut v, &[right(at)]).is_empty());
    let texts = paint(&mut v);
    assert!(texts.texts().contains(&"New tag..."));
    assert!(texts.texts().contains(&"Toggle favourite"));
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 0))]),
        vec![BrowseAction::AddTag {
            paths: vec![p("hat.wav")],
            tag: "808".into()
        }]
    );
    // The menu is closed afterwards.
    assert!(!paint(&mut v).texts().contains(&"New tag..."));
}

#[test]
fn tag_item_is_checked_when_all_selected_rows_have_it_and_removes_it() {
    let mut rows = sample_rows();
    for r in &mut rows {
        if r.name.starts_with("kick") {
            r.tags = vec!["808".into()];
        }
    }
    let mut v = tree_view();
    v.set_rows(rows);
    // Select kick 1..kick 2 (rows 2 and 3), right-click inside the selection.
    run(&mut v, &[down(row_pt(2)), up(row_pt(2))]);
    run(
        &mut v,
        &[down_with(row_pt(3), Modifiers::SHIFT, 1), up(row_pt(3))],
    );
    let at = row_pt(3);
    run(&mut v, &[right(at)]);
    assert!(paint(&mut v).texts().contains(&"\u{2713}"));
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 0))]),
        vec![BrowseAction::RemoveTag {
            paths: vec![p("kick 1.wav"), p("kick 2.wav")],
            tag: "808".into()
        }]
    );
    // Adding "snare" (not on any) adds it to both.
    run(&mut v, &[right(at)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 1))]),
        vec![BrowseAction::AddTag {
            paths: vec![p("kick 1.wav"), p("kick 2.wav")],
            tag: "snare".into()
        }]
    );
}

#[test]
fn right_click_selects_an_unselected_row_first() {
    let mut v = menu_view();
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    let at = row_pt(5);
    run(&mut v, &[right(at)]);
    // Toggle favourite sits after 2 tags, a separator and "New tag...".
    let fav = Point::new(at.x + 20.0, at.y + 4.0 + 44.0 + 8.0 + 22.0 + 11.0);
    // The menu may have been moved up to fit; find it through the painted texts instead.
    let _ = fav;
    let acts = run(
        &mut v,
        &[
            key(Key::Down),
            key(Key::Down),
            key(Key::Down),
            key(Key::Down),
            key(Key::Enter),
        ],
    );
    assert_eq!(
        acts,
        vec![BrowseAction::ToggleFavorite(vec![p("snare.wav")])]
    );
}

#[test]
fn new_tag_entry_commits_with_enter_and_cancels_with_escape() {
    let mut v = menu_view();
    let at = row_pt(1);
    run(&mut v, &[right(at)]);
    // New tag... follows 2 tags and a separator.
    let new_tag = Point::new(at.x + 20.0, at.y + 4.0 + 44.0 + 8.0 + 11.0);
    assert!(run(&mut v, &[down(new_tag)]).is_empty());
    assert!(v.is_text_owner());
    typed(&mut v, "909 hits");
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::AddTag {
            paths: vec![p("hat.wav")],
            tag: "909 hits".into()
        }]
    );
    assert!(!v.is_text_owner());
    run(&mut v, &[right(at), down(new_tag)]);
    typed(&mut v, "zzz");
    assert!(run(&mut v, &[key(Key::Escape)]).is_empty());
    assert!(!v.is_text_owner());
    // An empty tag adds nothing.
    run(&mut v, &[right(at), down(new_tag)]);
    assert!(run(&mut v, &[key(Key::Enter)]).is_empty());
}

#[test]
fn folder_rows_offer_bookmarking() {
    let mut v = menu_view();
    let at = row_pt(0);
    run(&mut v, &[right(at)]);
    // Tags are disabled for folders; items: 808, snare, sep, New tag, Toggle, sep, Bookmark.
    let bm = Point::new(at.x + 20.0, at.y + 4.0 + 44.0 + 8.0 + 44.0 + 8.0 + 11.0);
    assert_eq!(
        run(&mut v, &[down(bm)]),
        vec![BrowseAction::AddPlace(p("drums"))]
    );
}

#[test]
fn menu_closes_on_outside_click_and_escape() {
    let mut v = menu_view();
    run(&mut v, &[right(row_pt(1))]);
    assert!(run(&mut v, &[down(Point::new(5.0, 5.0))]).is_empty());
    assert!(!paint(&mut v).texts().contains(&"New tag..."));
    run(&mut v, &[right(row_pt(1))]);
    assert!(run(&mut v, &[key(Key::Escape)]).is_empty());
    assert!(!paint(&mut v).texts().contains(&"New tag..."));
}

#[test]
fn tag_menu_renames_and_deletes() {
    let mut v = tree_view();
    let at = tree_pt(2, 60.0);
    run(&mut v, &[right(at)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 1))]),
        vec![BrowseAction::DeleteTag("808".into())]
    );
    run(&mut v, &[right(at)]);
    assert!(run(&mut v, &[down(menu_item(at, 0))]).is_empty());
    assert!(v.is_text_owner());
    typed(&mut v, "909");
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::RenameTag {
            from: "808".into(),
            to: "909".into()
        }]
    );
    // Unchanged name: nothing.
    run(&mut v, &[right(at), down(menu_item(at, 0))]);
    assert!(run(&mut v, &[key(Key::Enter)]).is_empty());
}

#[test]
fn bookmark_menus_in_the_tree() {
    let mut v = tall_tree_view();
    let bookmark = tree_pt(5, 60.0);
    run(&mut v, &[right(bookmark)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(bookmark, 0))]),
        vec![BrowseAction::RemovePlace(PathBuf::from("/music/drums"))]
    );
    let root = tree_pt(8, 60.0);
    run(&mut v, &[right(root)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(root, 0))]),
        vec![BrowseAction::AddPlace(PathBuf::from("/home/me"))]
    );
    // No menu on headings or Favourites.
    run(&mut v, &[right(tree_pt(4, 60.0))]);
    assert!(!paint(&mut v).texts().contains(&"Bookmark"));
}

// ----- drag ---------------------------------------------------------------------------------------------

#[test]
fn moving_past_the_threshold_starts_a_file_drag_once() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1))]);
    assert!(v.is_dragging());
    assert!(run(&mut v, &[mv(Point::new(row_pt(1).x + 3.0, row_pt(1).y))]).is_empty());
    assert!(v.is_dragging());
    assert_eq!(
        run(&mut v, &[mv(Point::new(row_pt(1).x + 12.0, row_pt(1).y))]),
        vec![BrowseAction::DragFiles(vec![p("hat.wav")])]
    );
    assert!(!v.is_dragging());
    assert!(
        run(
            &mut v,
            &[mv(Point::new(200.0, 400.0)), up(Point::new(200.0, 400.0))]
        )
        .is_empty()
    );
}

#[test]
fn dragging_a_selected_row_drags_the_whole_audio_selection() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(0)), up(row_pt(0))]);
    run(
        &mut v,
        &[down_with(row_pt(4), Modifiers::SHIFT, 1), up(row_pt(4))],
    );
    run(&mut v, &[down(row_pt(2))]);
    let acts = run(&mut v, &[mv(Point::new(row_pt(2).x, row_pt(2).y + 20.0))]);
    assert_eq!(
        acts,
        vec![BrowseAction::DragFiles(vec![
            p("hat.wav"),
            p("kick 1.wav"),
            p("kick 2.wav"),
            p("kick 10.wav")
        ])],
        "folders are not dragged"
    );
}

#[test]
fn a_plain_click_on_a_multi_selection_collapses_it_on_release() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    run(
        &mut v,
        &[down_with(row_pt(3), Modifiers::SHIFT, 1), up(row_pt(3))],
    );
    assert!(
        run(&mut v, &[down(row_pt(2))]).is_empty(),
        "no preview until release"
    );
    assert_eq!(
        run(&mut v, &[up(row_pt(2))]),
        vec![BrowseAction::Preview(p("kick 1.wav"))]
    );
    assert_eq!(
        run(&mut v, &[ch('f')]),
        vec![BrowseAction::ToggleFavorite(vec![p("kick 1.wav")])]
    );
}

#[test]
fn clicking_a_folder_or_the_header_does_not_hold_the_pointer() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(0))]);
    assert!(!v.is_dragging());
    run(&mut v, &[down(Point::new(60.0, 298.0))]);
    assert!(!v.is_dragging());
}

// ----- info requests ----------------------------------------------------------------------------

fn bare(n: usize) -> Vec<FileRow> {
    (0..n)
        .map(|i| {
            let mut r = audio(&format!("s{i:06}.wav"));
            r.info = None;
            r
        })
        .collect()
}

#[test]
fn visible_rows_without_info_are_requested_once() {
    let mut v = view();
    v.set_rows(bare(100));
    let acts = run(&mut v, &[mv(Point::new(1.0, 1.0))]);
    let [BrowseAction::RequestInfo(paths)] = &acts[..] else {
        panic!("{acts:?}")
    };
    // 292 px of list: 14 rows (13.27 rows, the last one partial).
    assert_eq!(paths.len(), 14);
    assert_eq!(paths[0], p("s000000.wav"));
    assert!(
        run(&mut v, &[mv(Point::new(2.0, 2.0))]).is_empty(),
        "deduped"
    );
    // Scrolling reveals more rows: only new ones are asked for.
    let acts = run(&mut v, &[wheel(row_pt(2), -3.0)]);
    let [BrowseAction::RequestInfo(paths)] = &acts[..] else {
        panic!("{acts:?}")
    };
    assert_eq!(paths.first().unwrap(), &p("s000014.wav"));
    assert!(paths.len() <= 10);
    // New rows reset the memory.
    v.set_rows(bare(100));
    assert_eq!(
        run(&mut v, &[UiEvent::Resize(Size::new(300.0, 600.0))]).len(),
        1
    );
}

#[test]
fn rows_with_info_and_folders_are_not_requested() {
    let mut v = view();
    assert!(run(&mut v, &[mv(Point::new(1.0, 1.0))]).is_empty());
    let mut rows = bare(3);
    rows.push(folder("zzz"));
    v.set_rows(rows);
    let acts = run(&mut v, &[mv(Point::new(1.0, 1.0))]);
    let [BrowseAction::RequestInfo(paths)] = &acts[..] else {
        panic!("{acts:?}")
    };
    assert_eq!(paths.len(), 3);
    v.patch_info(&[(p("s000001.wav"), info())]);
    v.set_rows(bare(3));
    assert_eq!(run(&mut v, &[mv(Point::new(3.0, 3.0))]).len(), 1);
}

#[test]
fn filtering_requests_the_rows_that_became_visible() {
    let mut v = view();
    v.set_rows(bare(100));
    run(&mut v, &[mv(Point::new(1.0, 1.0))]);
    v.focus_search();
    typed(&mut v, "s0000");
    let acts = run(&mut v, &[mv(Point::new(1.0, 1.0))]);
    // Everything visible was already asked for, or is now: no duplicates ever.
    for a in &acts {
        if let BrowseAction::RequestInfo(paths) = a {
            let mut seen = paths.clone();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), paths.len());
        }
    }
}

// ----- large lists ---------------------------------------------------------------------------------

#[test]
fn a_hundred_thousand_rows_filter_sort_and_select() {
    let mut v = view();
    v.set_rows(many(100_000));
    v.focus_search();
    typed(&mut v, "s0999");
    assert!(paint(&mut v).texts().contains(&"s099900.wav"));
    run(&mut v, &[key(Key::Escape)]);
    run(&mut v, &[key(Key::End)]);
    assert!(paint(&mut v).texts().contains(&"s099999.wav"));
}

#[test]
#[ignore = "timing: run with --release"]
#[allow(
    clippy::disallowed_methods,
    reason = "release timing probe, never part of production code"
)]
fn filtering_and_sorting_a_hundred_thousand_rows_is_fast() {
    use std::time::Instant;
    let mut v = view();
    v.set_rows(many(100_000));
    v.focus_search();
    typed(&mut v, "s00");
    let t = Instant::now();
    typed(&mut v, "1");
    let filter = t.elapsed();
    let hdr = Point::new(60.0, 298.0);
    let t = Instant::now();
    v.handle(&down(hdr));
    let sort = t.elapsed();
    println!("filter {filter:?}, sort {sort:?}");
    assert!(filter.as_millis() <= 8, "filter {filter:?}");
    assert!(sort.as_millis() <= 30, "sort {sort:?}");
}

// ----- library sections and items ------------------------------------------------------------------

fn item_row(name: &str, kind: FileKind, detail: &str) -> FileRow {
    FileRow {
        kind,
        size: 900,
        info: None,
        item: Some(ItemInfo {
            detail: detail.into(),
            category: String::new(),
            ..Default::default()
        }),
        ..audio(name)
    }
}

fn device_row(type_id: &str, name: &str, category: &str) -> FileRow {
    FileRow {
        path: device_uri(type_id),
        name: name.to_string(),
        kind: FileKind::Device,
        size: 0,
        info: None,
        item: Some(ItemInfo {
            detail: category.into(),
            category: category.into(),
            ..Default::default()
        }),
        favorite: false,
        tags: Vec::new(),
        author: String::new(),
        categories: Vec::new(),
    }
}

/// A panel listing `rows` (sorted by name) in a library section, the library folder set.
fn library_view(section: LibrarySection, rows: Vec<FileRow>) -> FileBrowserView {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_library_folder(Some(PathBuf::from("/lib")));
    v.set_location(Place::Library(section));
    v.set_rows(rows);
    settled(v)
}

/// Sorted order: 0 Box, 1 Lead, 2 Riff, 3 Song.
fn patch_rows() -> Vec<FileRow> {
    vec![
        item_row(
            "Lead.atomikpatch",
            FileKind::Patch,
            "2 devices: Poly, Delay",
        ),
        item_row(
            "Box.atomikcontainer",
            FileKind::Container,
            "Container, 3 inside",
        ),
        item_row("Riff.atomikclip", FileKind::Clip, "2 bars, 8 notes"),
        item_row("Song.atomik", FileKind::Project, "120 BPM, 4 tracks"),
    ]
}

fn click(v: &mut FileBrowserView, row: usize) -> Vec<BrowseAction> {
    run(v, &[down(row_pt(row)), up(row_pt(row))])
}

fn click_at(v: &mut FileBrowserView, at: Point) -> Vec<BrowseAction> {
    run(v, &[down(at), up(at)])
}

#[test]
fn tree_starts_with_the_library_sections_and_shows_them() {
    let mut v = settled(view());
    let dl = paint(&mut v);
    for label in ["Devices", "Patches", "Containers", "Clips", "Projects"] {
        assert!(dl.texts().contains(&label), "{label}");
    }
    for (i, section) in LibrarySection::ALL.into_iter().enumerate() {
        assert_eq!(
            click_at(&mut v, lib_pt(1 + i, 60.0)),
            vec![BrowseAction::Show(Place::Library(section))],
            "{section:?}"
        );
    }
    // The heading does nothing.
    assert!(click_at(&mut v, lib_pt(0, 60.0)).is_empty());
}

#[test]
fn a_library_section_is_highlighted_as_the_location() {
    let mut v = settled(view());
    let plain = paint(&mut v).to_json();
    v.set_location(Place::Library(LibrarySection::Clips));
    assert_ne!(paint(&mut v).to_json(), plain);
    assert_eq!(v.location(), Some(&Place::Library(LibrarySection::Clips)));
}

#[test]
fn an_empty_library_folder_section_shows_the_hint() {
    let hint = "Nothing saved yet: right-click a device or clip and choose Save to Library";
    for section in [
        LibrarySection::Patches,
        LibrarySection::Containers,
        LibrarySection::Clips,
        LibrarySection::Projects,
    ] {
        let mut v = library_view(section, Vec::new());
        assert!(paint(&mut v).texts().contains(&hint), "{section:?}");
    }
    // A plain folder says "Nothing here", and so do the devices.
    let mut v = library_view(LibrarySection::Devices, Vec::new());
    assert!(!paint(&mut v).texts().contains(&hint));
    let mut v = settled(view());
    v.set_rows(Vec::new());
    assert!(!paint(&mut v).texts().contains(&hint));
    // A search without a match is not "nothing saved".
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    run(&mut v, &[key_with(Key::Char('f'), Modifiers::CTRL)]);
    typed(&mut v, "zzz");
    assert!(!paint(&mut v).texts().contains(&hint));
}

#[test]
fn the_path_bar_names_a_library_section() {
    let mut v = library_view(LibrarySection::Containers, patch_rows());
    assert!(paint(&mut v).texts().contains(&"Containers"));
}

#[test]
fn enter_on_an_item_row_loads_it() {
    let mut rows = patch_rows();
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = library_view(LibrarySection::Patches, rows);
    // Sorted: Box, Lead, Poly Synth, Riff, Song.
    for (row, path) in [
        (0, p("Box.atomikcontainer")),
        (1, p("Lead.atomikpatch")),
        (2, device_uri("atomik.poly")),
        (3, p("Riff.atomikclip")),
        (4, p("Song.atomik")),
    ] {
        click(&mut v, row);
        assert_eq!(
            run(&mut v, &[key(Key::Enter)]),
            vec![BrowseAction::Load(path.clone())],
            "{path:?}"
        );
    }
}

#[test]
fn double_click_on_an_item_row_loads_it_and_a_single_click_only_selects() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    assert!(
        click(&mut v, 1).is_empty(),
        "auto-preview is on: still silent"
    );
    let acts = run(
        &mut v,
        &[down_with(row_pt(1), Modifiers::NONE, 2), up(row_pt(1))],
    );
    assert_eq!(acts, vec![BrowseAction::Load(p("Lead.atomikpatch"))]);
}

#[test]
fn space_and_arrow_keys_never_preview_an_item() {
    let mut rows = patch_rows();
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = library_view(LibrarySection::Patches, rows);
    click(&mut v, 0);
    assert!(run(&mut v, &[key(Key::Space)]).is_empty());
    assert!(run(&mut v, &[key(Key::Down), key(Key::Down), key(Key::Down)]).is_empty());
    assert!(run(&mut v, &[key(Key::Space)]).is_empty());
}

#[test]
fn dragging_an_item_row_drags_the_item_and_a_device_row_its_uri() {
    let mut rows = patch_rows();
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = library_view(LibrarySection::Patches, rows);
    run(&mut v, &[down(row_pt(1))]);
    let far = Point::new(row_pt(1).x + 40.0, row_pt(1).y);
    assert_eq!(
        run(&mut v, &[mv(far)]),
        vec![BrowseAction::DragFiles(vec![p("Lead.atomikpatch")])]
    );
    run(&mut v, &[up(far)]);
    run(&mut v, &[down(row_pt(2))]);
    let far = Point::new(row_pt(2).x + 40.0, row_pt(2).y);
    assert_eq!(
        run(&mut v, &[mv(far)]),
        vec![BrowseAction::DragFiles(vec![device_uri("atomik.poly")])]
    );
}

#[test]
fn f_toggles_the_favourite_of_an_item() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    click(&mut v, 1);
    assert_eq!(
        run(&mut v, &[ch('f')]),
        vec![BrowseAction::ToggleFavorite(vec![p("Lead.atomikpatch")])]
    );
}

#[test]
fn item_rows_show_their_detail_and_devices_their_category() {
    let mut rows = patch_rows();
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = library_view(LibrarySection::Patches, rows);
    let dl = paint(&mut v);
    let t = dl.texts();
    assert!(t.contains(&"2 devices: Poly, Delay"));
    assert!(t.contains(&"2 bars, 8 notes"));
    assert!(t.contains(&"INSTRUMENT"));
    assert!(!t.contains(&"\u{2014}"), "no duration text for items");
}

#[test]
fn visible_item_rows_without_detail_are_requested_once_and_devices_never() {
    let mut rows = patch_rows();
    for r in &mut rows {
        r.item = None;
    }
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    v.set_location(Place::Library(LibrarySection::Patches));
    v.set_rows(rows);
    let acts = run(&mut v, &[UiEvent::Resize(Size::new(300.0, 600.0))]);
    let [BrowseAction::RequestInfo(paths)] = acts.as_slice() else {
        panic!("{acts:?}");
    };
    assert_eq!(paths.len(), 4);
    assert!(!paths.contains(&device_uri("atomik.poly")));
    assert!(run(&mut v, &[UiEvent::Resize(Size::new(300.0, 600.0))]).is_empty());
    // The answer fills the detail column and nothing is asked again.
    v.patch_item_info(&[(
        p("Lead.atomikpatch"),
        ItemInfo {
            detail: "2 devices: Poly, Delay".into(),
            category: String::new(),
            ..Default::default()
        },
    )]);
    assert!(paint(&mut v).texts().contains(&"2 devices: Poly, Delay"));
    assert!(run(&mut v, &[UiEvent::Resize(Size::new(300.0, 600.0))]).is_empty());
}

#[test]
fn the_search_matches_the_category_of_a_device_row() {
    let mut v = library_view(
        LibrarySection::Devices,
        vec![
            device_row("atomik.poly", "Poly Synth", "INSTRUMENT"),
            device_row("atomik.delay", "Delay", "EFFECT"),
        ],
    );
    run(&mut v, &[key_with(Key::Char('f'), Modifiers::CTRL)]);
    typed(&mut v, "effect");
    let dl = paint(&mut v);
    assert!(dl.texts().contains(&"Delay") && !dl.texts().contains(&"Poly Synth"));
}

/// Which of the item entries a right click on list row `row` adds to the painted texts.
fn menu_labels(v: &mut FileBrowserView, row: usize) -> Vec<String> {
    run(v, &[right(row_pt(row))]);
    let texts: Vec<String> = paint(v).texts().iter().map(|s| (*s).to_string()).collect();
    ["Load", "Open as Project", "Rename..."]
        .into_iter()
        .filter(|l| texts.iter().any(|t| t == l))
        .map(str::to_string)
        .collect()
}

#[test]
fn list_menu_offers_load_open_as_project_and_rename_by_kind() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    let all = ["Load", "Open as Project", "Rename..."];
    assert_eq!(menu_labels(&mut v, 0), all, "container");
    run(&mut v, &[key(Key::Escape)]);
    assert_eq!(menu_labels(&mut v, 1), all, "patch");
    run(&mut v, &[key(Key::Escape)]);
    assert_eq!(menu_labels(&mut v, 2), ["Load", "Rename..."], "clip");
    run(&mut v, &[key(Key::Escape)]);
    assert_eq!(menu_labels(&mut v, 3), all, "project");
}

#[test]
fn rename_is_only_offered_inside_the_library_folder() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    v.set_library_folder(Some(PathBuf::from("/elsewhere")));
    assert_eq!(menu_labels(&mut v, 1), ["Load", "Open as Project"]);
    run(&mut v, &[key(Key::Escape)]);
    v.set_library_folder(None);
    assert_eq!(menu_labels(&mut v, 1), ["Load", "Open as Project"]);
    run(&mut v, &[key(Key::Escape)]);
    // A file below the library folder counts as inside.
    v.set_library_folder(Some(PathBuf::from("/")));
    assert_eq!(
        menu_labels(&mut v, 1),
        ["Load", "Open as Project", "Rename..."]
    );
}

#[test]
fn device_rows_offer_only_load() {
    let mut v = library_view(
        LibrarySection::Devices,
        vec![device_row("atomik.poly", "Poly Synth", "INSTRUMENT")],
    );
    assert_eq!(menu_labels(&mut v, 0), ["Load"]);
    assert_eq!(
        run(&mut v, &[down(menu_item(row_pt(0), 0))]),
        vec![BrowseAction::Load(device_uri("atomik.poly"))]
    );
}

#[test]
fn menu_load_and_open_as_project_emit_their_actions() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    let at = row_pt(1);
    run(&mut v, &[right(at)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 0))]),
        vec![BrowseAction::Load(p("Lead.atomikpatch"))]
    );
    run(&mut v, &[right(at)]);
    assert_eq!(
        run(&mut v, &[down(menu_item(at, 1))]),
        vec![BrowseAction::OpenProject(p("Lead.atomikpatch"))]
    );
}

#[test]
fn rename_entry_starts_with_the_stem_and_commits_the_new_name() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    let at = row_pt(1);
    run(&mut v, &[right(at)]);
    assert!(run(&mut v, &[down(menu_item(at, 2))]).is_empty());
    assert!(v.is_text_owner());
    assert!(
        paint(&mut v).texts().contains(&"Lead"),
        "prefilled with the stem"
    );
    // The text is selected: typing replaces it.
    typed(&mut v, "Pad");
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::RenameItem {
            path: p("Lead.atomikpatch"),
            to: "Pad".into()
        }]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn rename_with_the_same_empty_or_cancelled_name_emits_nothing() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    let at = row_pt(1);
    run(&mut v, &[right(at), down(menu_item(at, 2))]);
    assert!(run(&mut v, &[key(Key::Enter)]).is_empty(), "unchanged");
    run(&mut v, &[right(at), down(menu_item(at, 2))]);
    run(&mut v, &[key(Key::Backspace)]);
    assert!(run(&mut v, &[key(Key::Enter)]).is_empty(), "empty");
    run(&mut v, &[right(at), down(menu_item(at, 2))]);
    typed(&mut v, "zzz");
    assert!(run(&mut v, &[key(Key::Escape)]).is_empty());
    assert!(!v.is_text_owner());
}

#[test]
fn f2_starts_the_rename_of_the_cursor_item_inside_the_library_only() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    click(&mut v, 1);
    assert!(run(&mut v, &[key(Key::F(2))]).is_empty());
    assert!(v.is_text_owner());
    assert!(paint(&mut v).texts().contains(&"Rename"), "path bar label");
    typed(&mut v, "Pad");
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::RenameItem {
            path: p("Lead.atomikpatch"),
            to: "Pad".into()
        }]
    );
    // Outside the library folder, on an audio file or on a device: nothing.
    v.set_library_folder(Some(PathBuf::from("/elsewhere")));
    click(&mut v, 1);
    run(&mut v, &[key(Key::F(2))]);
    assert!(!v.is_text_owner());
    let mut v = settled(view());
    v.set_library_folder(Some(PathBuf::from("/lib")));
    click(&mut v, 1);
    run(&mut v, &[key(Key::F(2))]);
    assert!(!v.is_text_owner());
    let mut v = library_view(
        LibrarySection::Devices,
        vec![device_row("atomik.poly", "Poly Synth", "INSTRUMENT")],
    );
    click(&mut v, 0);
    run(&mut v, &[key(Key::F(2))]);
    assert!(!v.is_text_owner());
}

#[test]
fn set_library_folder_cancels_a_rename_that_is_no_longer_allowed() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    click(&mut v, 1);
    run(&mut v, &[key(Key::F(2))]);
    assert!(v.is_text_owner());
    v.set_library_folder(None);
    assert!(!v.is_text_owner());
}

#[test]
fn begin_rename_selects_the_row_and_starts_the_entry_inside_the_library_only() {
    let mut v = library_view(LibrarySection::Patches, patch_rows());
    assert!(v.begin_rename(&p("Lead.atomikpatch")));
    assert!(v.is_text_owner());
    typed(&mut v, "Pad");
    assert_eq!(
        run(&mut v, &[key(Key::Enter)]),
        vec![BrowseAction::RenameItem {
            path: p("Lead.atomikpatch"),
            to: "Pad".into()
        }]
    );
    assert!(!v.begin_rename(&p("Nothing.atomikpatch")), "no such row");
    v.set_library_folder(Some(PathBuf::from("/elsewhere")));
    assert!(
        !v.begin_rename(&p("Lead.atomikpatch")),
        "outside the library"
    );
    assert!(!v.is_text_owner());
}

// ----- goldens -------------------------------------------------------------------------------------

fn check_golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
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
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn golden_rows() -> Vec<FileRow> {
    let mut rows = sample_rows();
    for r in &mut rows {
        match r.name.as_str() {
            "kick 1.wav" => {
                r.tags = vec!["808".into(), "punchy".into()];
                r.favorite = true;
            }
            "kick 2.wav" => r.tags = vec!["808".into()],
            "snare.wav" => r.info = None,
            _ => {}
        }
    }
    rows
}

fn golden_view(width: f64) -> FileBrowserView {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, width, 600.0));
    v.set_tree_fraction(0.4);
    v.set_focused(true);
    v.set_places(places());
    v.set_location(Place::Folder(PathBuf::from("/lib")));
    v.set_rows(golden_rows());
    v
}

#[test]
fn golden_empty() {
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    check_golden("file_browser_empty_1x.json", &paint(&mut v).to_json());
}

#[test]
fn golden_tree_and_list() {
    let mut v = golden_view(300.0);
    v.set_children(
        Path::new("/music/drums"),
        vec![TreeChild {
            name: "kicks".into(),
            path: PathBuf::from("/music/drums/kicks"),
        }],
    );
    // One wheel notch scrolls the tree three rows: the drums bookmark sits at index 11 - 3.
    run(&mut v, &[wheel(lib_pt(0, 60.0), -1.0)]);
    let arrow = lib_pt(LIBRARY_ROWS + 5 - 3, 15.0);
    run(&mut v, &[down(arrow), up(arrow)]);
    run(&mut v, &[down(row_pt(3)), up(row_pt(3))]);
    v.set_preview(Some(p("hat.wav")), 0.4);
    check_golden("file_browser_tree_list_1x.json", &paint(&mut v).to_json());
}

#[test]
fn golden_narrow() {
    let mut v = golden_view(FileBrowserView::MIN_WIDTH);
    run(
        &mut v,
        &[
            down(Point::new(60.0, row_pt(2).y)),
            up(Point::new(60.0, row_pt(2).y)),
        ],
    );
    check_golden("file_browser_narrow_1x.json", &paint(&mut v).to_json());
}

#[test]
fn golden_menu() {
    let mut v = golden_view(300.0);
    run(&mut v, &[right(row_pt(2))]);
    run(&mut v, &[mv(menu_item(row_pt(2), 0))]);
    check_golden("file_browser_menu_1x.json", &paint(&mut v).to_json());
}

#[test]
fn golden_library_items() {
    let mut rows = patch_rows();
    rows.push(device_row("atomik.poly", "Poly Synth", "INSTRUMENT"));
    let mut v = library_view(LibrarySection::Patches, rows);
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    check_golden("file_browser_library_1x.json", &paint(&mut v).to_json());
}

#[test]
fn take_info_requests_polls_visible_rows_without_an_event() {
    let mut v = FileBrowserView::new(atomik_ui::Theme::dark());
    v.set_bounds(vello::kurbo::Rect::new(0.0, 0.0, 300.0, 800.0));
    let row = atomik_library::FileRow {
        path: std::path::PathBuf::from("/s/a.wav"),
        name: "a.wav".into(),
        kind: atomik_library::FileKind::Audio(atomik_library::AudioFormat::Wav),
        size: 1,
        info: None,
        item: None,
        favorite: false,
        tags: Vec::new(),
        author: String::new(),
        categories: Vec::new(),
    };
    v.set_rows(vec![row]);
    assert_eq!(
        v.take_info_requests(),
        vec![std::path::PathBuf::from("/s/a.wav")]
    );
    assert!(
        v.take_info_requests().is_empty(),
        "each path is requested once"
    );
}

#[test]
fn ctrl_a_in_the_list_selects_every_file() {
    let mut v = settled(view());
    run(&mut v, &[down(row_pt(1)), up(row_pt(1))]);
    run(&mut v, &[key_with(Key::Char('a'), Modifiers::CTRL)]);
    let files = sample_rows()
        .into_iter()
        .filter(|r| matches!(r.kind, FileKind::Audio(_)))
        .count();
    match run(&mut v, &[ch('f')]).as_slice() {
        [BrowseAction::ToggleFavorite(paths)] => assert_eq!(paths.len(), files),
        other => panic!("expected one ToggleFavorite, got {other:?}"),
    }
}
