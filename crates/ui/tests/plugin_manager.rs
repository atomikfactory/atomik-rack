//! The plugin manager view: the table with its sort, the search and the four filters, the choice
//! of rows, the favourite star, the tag field and chips, the action buttons and what each asks the
//! app for, and goldens of the panel (a table, and a scan running with nothing chosen).
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test plugin_manager`.

mod support;

use atomik_device_sdk::PluginFormat;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::plugin_data::{PluginEntryStatus, ScanProgressInfo};
use atomik_ui::{
    DisplayList, Key, Modifiers, PluginManagerAction, PluginManagerData, PluginManagerRow,
    PluginManagerView, StatusFilter, Theme, UiAction, UiEvent,
};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 700.0);

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

#[allow(clippy::too_many_arguments)]
fn row(
    id: Option<&str>,
    name: &str,
    format: PluginFormat,
    category: &str,
    status: PluginEntryStatus,
    scanned: u64,
    favourite: bool,
    tags: &[&str],
) -> PluginManagerRow {
    PluginManagerRow {
        type_id: id.map(str::to_owned),
        name: name.to_owned(),
        vendor: "Example Vendor".to_owned(),
        format,
        category: category.to_owned(),
        status,
        path: format!("/plugins/{}", name.to_lowercase().replace(' ', "-")),
        scanned_secs: scanned,
        scanned: if scanned == 0 {
            String::new()
        } else {
            format!("2026-11-0{scanned} 10:00")
        },
        favourite,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
    }
}

fn data() -> PluginManagerData {
    use PluginFormat::{Clap, Vst3};
    PluginManagerData {
        rows: vec![
            row(
                Some("vst3.synth"),
                "Example Synth",
                Vst3,
                "Instrument",
                PluginEntryStatus::Ok,
                3,
                true,
                &["pads", "Warm"],
            ),
            row(
                Some("clap.verb"),
                "Example Verb",
                Clap,
                "Effect",
                PluginEntryStatus::Quarantined,
                2,
                false,
                &["warm"],
            ),
            row(
                Some("clap.echo"),
                "Example Echo",
                Clap,
                "Effect",
                PluginEntryStatus::Ok,
                1,
                false,
                &[],
            ),
            row(
                Some("vst3.bell"),
                "Example Bell",
                Vst3,
                "Instrument",
                PluginEntryStatus::Disabled,
                4,
                false,
                &[],
            ),
            row(
                Some("clap.broken"),
                "Example Broken",
                Clap,
                "Effect",
                PluginEntryStatus::Failed("it needs a library that is missing".into()),
                5,
                false,
                &[],
            ),
            row(
                None,
                "example-bad.vst3",
                Vst3,
                "",
                PluginEntryStatus::Failed("the scanner crashed".into()),
                6,
                false,
                &[],
            ),
        ],
        scan: None,
        scanner_available: true,
        plugins_on: true,
    }
}

fn view() -> PluginManagerView {
    let mut v = PluginManagerView::new(Theme::dark(), data());
    v.set_bounds(BOUNDS);
    v
}

fn plugin_actions(out: Vec<UiAction>) -> Vec<PluginManagerAction> {
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::PluginManager(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn click_with_fn(
    v: &mut PluginManagerView,
    p: Point,
    modifiers: Modifiers,
) -> Vec<PluginManagerAction> {
    let mut out = v.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers,
        clicks: 1,
    });
    out.extend(v.handle(&up(p)));
    plugin_actions(out)
}

fn click_fn(v: &mut PluginManagerView, p: Point) -> Vec<PluginManagerAction> {
    click_with_fn(v, p, Modifiers::NONE)
}

/// A click at a point worked out from the view first (two borrows of the view cannot overlap in
/// one call, so the point is taken before the view is lent out).
macro_rules! click {
    ($v:ident, $p:expr) => {{
        let at = $p;
        click_fn(&mut $v, at)
    }};
}

macro_rules! click_with {
    ($v:ident, $p:expr, $m:expr) => {{
        let at = $p;
        click_with_fn(&mut $v, at, $m)
    }};
}

fn press(v: &mut PluginManagerView, key: Key, modifiers: Modifiers) -> Vec<PluginManagerAction> {
    plugin_actions(v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }))
}

fn type_text(v: &mut PluginManagerView, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            press(v, Key::Space, Modifiers::NONE);
        } else {
            press(v, Key::Char(c), Modifiers::NONE);
        }
    }
}

fn names(v: &PluginManagerView) -> Vec<String> {
    v.shown().iter().map(|r| r.name.clone()).collect()
}

fn center(r: Rect) -> Point {
    r.center()
}

#[test]
fn the_table_starts_sorted_by_name() {
    let v = view();
    assert_eq!(
        names(&v),
        [
            "Example Bell",
            "Example Broken",
            "Example Echo",
            "Example Synth",
            "Example Verb",
            "example-bad.vst3",
        ],
        "case does not matter and a failed file is a row too"
    );
    assert_eq!(v.sorted_by(), ("NAME", true));
}

#[test]
fn a_click_on_a_head_sorts_and_a_second_click_reverses() {
    let mut v = view();
    click!(v, v.head_center("SCANNED").unwrap());
    assert_eq!(v.sorted_by(), ("SCANNED", true));
    assert_eq!(names(&v)[0], "Example Echo", "the oldest scan first");
    click!(v, v.head_center("SCANNED").unwrap());
    assert_eq!(v.sorted_by(), ("SCANNED", false));
    assert_eq!(names(&v)[0], "example-bad.vst3", "the newest first");
    click!(v, v.head_center("STATUS").unwrap());
    assert_eq!(
        names(&v)[0],
        "Example Bell",
        "Disabled sorts before the rest"
    );
    click!(v, v.head_center("FORMAT").unwrap());
    assert_eq!(v.shown()[0].format, PluginFormat::Clap);
    click!(v, v.head_center("").unwrap());
    assert_eq!(v.sorted_by().0, "FAVOURITE");
    assert!(v.shown()[0].favourite, "the starred plugin comes first");
}

#[test]
fn the_search_matches_name_vendor_path_and_tags() {
    let mut v = view();
    for (query, want) in [
        ("synth", vec!["Example Synth"]),
        ("ECHO", vec!["Example Echo"]),
        ("pads", vec!["Example Synth"]),
        ("warm", vec!["Example Synth", "Example Verb"]),
        ("/plugins/example-bell", vec!["Example Bell"]),
        ("nothing like it", vec![]),
    ] {
        v.set_query(query);
        assert_eq!(names(&v), want, "{query:?}");
    }
    // Typing goes to the search field, which has the keys.
    v.set_query("");
    type_text(&mut v, "ver");
    assert_eq!(v.query(), "ver");
    assert_eq!(names(&v), ["Example Verb"]);
}

#[test]
fn the_format_status_category_and_tag_filters_step_on_each_click() {
    let mut v = view();
    click!(v, center(v.format_rect()));
    assert_eq!(v.format_filter(), Some(PluginFormat::Vst3));
    assert_eq!(
        names(&v),
        ["Example Bell", "Example Synth", "example-bad.vst3"]
    );
    click!(v, center(v.format_rect()));
    assert_eq!(v.format_filter(), Some(PluginFormat::Clap));
    click!(v, center(v.format_rect()));
    assert_eq!(v.format_filter(), None);
    assert_eq!(names(&v).len(), 6);

    // Status steps through ALL, OK, FAILED, UNSUPPORTED, QUARANTINED, DISABLED.
    let mut seen = vec![v.status_filter()];
    for _ in 0..5 {
        click!(v, center(v.status_rect()));
        seen.push(v.status_filter());
    }
    assert_eq!(
        seen,
        [
            StatusFilter::All,
            StatusFilter::Ok,
            StatusFilter::Failed,
            StatusFilter::Unsupported,
            StatusFilter::Quarantined,
            StatusFilter::Disabled,
        ]
    );
    assert_eq!(names(&v), ["Example Bell"], "disabled");
    click!(v, center(v.status_rect()));
    assert_eq!(v.status_filter(), StatusFilter::All);
    click!(v, center(v.status_rect()));
    assert_eq!(names(&v), ["Example Echo", "Example Synth"], "ok");
    click!(v, center(v.status_rect()));
    assert_eq!(
        names(&v),
        ["Example Broken", "example-bad.vst3"],
        "a failed plugin and a failed file"
    );
    click!(v, center(v.status_rect()));
    click!(v, center(v.status_rect()));
    click!(v, center(v.status_rect()));
    click!(v, center(v.status_rect()));
    assert_eq!(v.status_filter(), StatusFilter::All);

    click!(v, center(v.category_rect()));
    assert_eq!(v.category_filter(), Some("Effect"));
    assert_eq!(
        names(&v),
        ["Example Broken", "Example Echo", "Example Verb"]
    );
    click!(v, center(v.category_rect()));
    assert_eq!(v.category_filter(), Some("Instrument"));
    click!(v, center(v.category_rect()));
    assert_eq!(v.category_filter(), None);

    click!(v, center(v.tag_filter_rect()));
    assert_eq!(v.tag_filter(), Some("pads"));
    assert_eq!(names(&v), ["Example Synth"]);
    click!(v, center(v.tag_filter_rect()));
    assert_eq!(v.tag_filter(), Some("Warm"));
    assert_eq!(
        names(&v),
        ["Example Synth", "Example Verb"],
        "ignoring case"
    );
    click!(v, center(v.tag_filter_rect()));
    assert_eq!(v.tag_filter(), None);

    // The filters work together.
    click!(v, center(v.format_rect()));
    click!(v, center(v.tag_filter_rect()));
    click!(v, center(v.tag_filter_rect()));
    assert_eq!(names(&v), ["Example Synth"]);
}

#[test]
fn a_click_chooses_a_row_ctrl_toggles_and_shift_chooses_a_range() {
    let mut v = view();
    click!(v, v.row_center(1).unwrap());
    assert_eq!(v.selected_keys(), ["clap.broken"]);
    click_with!(v, v.row_center(3).unwrap(), Modifiers::CTRL);
    assert_eq!(v.selected_keys().len(), 2);
    click_with!(v, v.row_center(1).unwrap(), Modifiers::CTRL);
    assert_eq!(v.selected_keys(), ["vst3.synth"], "ctrl again takes it off");
    click!(v, v.row_center(0).unwrap());
    click_with!(v, v.row_center(3).unwrap(), Modifiers::SHIFT);
    assert_eq!(v.selected_keys().len(), 4);
    click_with!(v, v.row_center(2).unwrap(), Modifiers::SHIFT);
    assert_eq!(
        v.selected_keys().len(),
        3,
        "the range runs from the first choice"
    );
    // The arrow keys move the choice when the list has the keys.
    click!(v, v.row_center(0).unwrap());
    press(&mut v, Key::Down, Modifiers::NONE);
    assert_eq!(v.selected_keys(), ["clap.broken"]);
    press(&mut v, Key::Down, Modifiers::SHIFT);
    assert_eq!(v.selected_keys().len(), 2);
    press(&mut v, Key::Char('a'), Modifiers::CTRL);
    assert_eq!(v.selected_keys().len(), 6);
}

#[test]
fn the_star_toggles_a_favourite_and_a_failed_file_has_none() {
    let mut v = view();
    // The synth is the fourth row and starred: the click asks to clear it.
    assert_eq!(
        click!(v, v.star_center(3).unwrap()),
        [PluginManagerAction::Favourite {
            type_id: "vst3.synth".into(),
            on: false
        }]
    );
    assert_eq!(
        click!(v, v.star_center(2).unwrap()),
        [PluginManagerAction::Favourite {
            type_id: "clap.echo".into(),
            on: true
        }]
    );
    assert!(
        click!(v, v.star_center(5).unwrap()).is_empty(),
        "a file that failed to scan has no id to star"
    );
    // The space bar does the same for the row the keys are on.
    click!(v, v.row_center(2).unwrap());
    assert_eq!(
        press(&mut v, Key::Space, Modifiers::NONE),
        [PluginManagerAction::Favourite {
            type_id: "clap.echo".into(),
            on: true
        }]
    );
}

#[test]
fn disable_enable_and_try_again_ask_for_the_chosen_plugins() {
    let mut v = view();
    assert!(
        click!(v, center(v.disable_rect())).is_empty(),
        "nothing is chosen"
    );
    click!(v, v.row_center(2).unwrap());
    assert_eq!(
        click!(v, center(v.disable_rect())),
        [PluginManagerAction::Disable {
            type_ids: vec!["clap.echo".into()],
            disabled: true
        }]
    );
    // A disabled plugin offers ENABLE.
    click!(v, v.row_center(0).unwrap());
    assert_eq!(
        click!(v, center(v.disable_rect())),
        [PluginManagerAction::Disable {
            type_ids: vec!["vst3.bell".into()],
            disabled: false
        }]
    );
    // TRY AGAIN clears a quarantine and scans a failed file again.
    click!(v, v.row_center(4).unwrap());
    click_with!(v, v.row_center(5).unwrap(), Modifiers::CTRL);
    click_with!(v, v.row_center(2).unwrap(), Modifiers::CTRL);
    assert_eq!(
        click!(v, center(v.try_again_rect())),
        [
            PluginManagerAction::TryAgain(vec!["clap.verb".into()]),
            PluginManagerAction::RescanFiles(vec!["/plugins/example-bad.vst3".into()]),
        ]
    );
    // Nothing to try again for a working plugin.
    click!(v, v.row_center(2).unwrap());
    assert!(click!(v, center(v.try_again_rect())).is_empty());
}

#[test]
fn rescan_show_in_folder_and_cancel_ask_for_the_right_files() {
    let mut v = view();
    click!(v, v.row_center(3).unwrap());
    click_with!(v, v.row_center(4).unwrap(), Modifiers::CTRL);
    assert_eq!(
        click!(v, center(v.rescan_selected_rect())),
        [PluginManagerAction::RescanFiles(vec![
            "/plugins/example-synth".into(),
            "/plugins/example-verb".into()
        ])]
    );
    assert_eq!(
        click!(v, center(v.folder_rect())),
        [PluginManagerAction::ShowInFolder(
            "/plugins/example-synth".into()
        )]
    );
    assert_eq!(
        click!(v, center(v.rescan_all_rect())),
        [PluginManagerAction::RescanAll]
    );
    // While a scan runs the same button cancels it and the progress is shown.
    let mut running = data();
    running.scan = Some(ScanProgressInfo { done: 3, total: 9 });
    v.set_data(running);
    assert_eq!(
        click!(v, center(v.rescan_all_rect())),
        [PluginManagerAction::CancelScan]
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().contains(&"Scanning 3 of 9"));
    assert!(dl.texts().contains(&"CANCEL"));
}

#[test]
fn tags_are_added_to_the_chosen_plugins_and_a_chip_takes_one_off() {
    let mut v = view();
    v.set_tag_text("drone");
    assert!(
        click!(v, center(v.add_tag_rect())).is_empty(),
        "nothing is chosen to tag"
    );
    click!(v, v.row_center(3).unwrap());
    click_with!(v, v.row_center(2).unwrap(), Modifiers::CTRL);
    assert_eq!(
        click!(v, center(v.add_tag_rect())),
        [PluginManagerAction::AddTag {
            type_ids: vec!["clap.echo".into(), "vst3.synth".into()],
            tag: "drone".into()
        }]
    );
    assert_eq!(v.tag_text(), "", "the field empties");
    // Enter in the tag field does the same.
    click!(v, center(v.tag_rect()));
    type_text(&mut v, "bright");
    assert_eq!(
        press(&mut v, Key::Enter, Modifiers::NONE),
        [PluginManagerAction::AddTag {
            type_ids: vec!["clap.echo".into(), "vst3.synth".into()],
            tag: "bright".into()
        }]
    );
    // The chips list the tags of the first chosen plugin (the table's order).
    click!(v, v.row_center(3).unwrap());
    let chips = v.tag_chips();
    assert_eq!(
        chips.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        ["pads", "Warm"]
    );
    assert_eq!(
        click!(v, center(chips[1].1)),
        [PluginManagerAction::RemoveTag {
            type_id: "vst3.synth".into(),
            tag: "Warm".into()
        }]
    );
}

#[test]
fn new_data_keeps_the_choice_and_resets_a_filter_whose_value_is_gone() {
    let mut v = view();
    click!(v, v.row_center(3).unwrap());
    click!(v, center(v.tag_filter_rect()));
    click!(v, center(v.tag_filter_rect()));
    click!(v, center(v.tag_filter_rect()));
    click!(v, center(v.tag_filter_rect()));
    assert_eq!(v.tag_filter(), Some("pads"));
    let mut next = data();
    next.rows
        .retain(|r| r.type_id.as_deref() != Some("vst3.synth"));
    for r in &mut next.rows {
        r.tags.clear();
    }
    v.set_data(next);
    assert_eq!(v.tag_filter(), None, "no plugin has a tag any more");
    assert!(v.selected_keys().is_empty(), "the chosen plugin is gone");
    assert_eq!(v.shown().len(), 5);
}

#[test]
fn the_panel_says_why_it_is_empty() {
    let mut off = data();
    off.rows.clear();
    off.plugins_on = false;
    let mut v = PluginManagerView::new(Theme::dark(), off);
    v.set_bounds(BOUNDS);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().iter().any(|t| t.contains("turn them on")));
    let mut none = data();
    none.rows.clear();
    v.set_data(none);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().iter().any(|t| t.contains("RESCAN ALL looks")));
    v.set_data(data());
    v.set_query("zzz");
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.texts().iter().any(|t| t.contains("No plugin matches")));
}

#[test]
fn the_rows_scroll_with_the_wheel_and_the_keys() {
    let mut big = data();
    for n in 0..40 {
        big.rows.push(row(
            Some(&format!("clap.extra{n:02}")),
            &format!("Extra {n:02}"),
            PluginFormat::Clap,
            "Effect",
            PluginEntryStatus::Ok,
            1,
            false,
            &[],
        ));
    }
    let mut v = PluginManagerView::new(Theme::dark(), big);
    v.set_bounds(BOUNDS);
    let first = v.row_center(0).unwrap();
    assert!(
        v.row_center(30).is_none(),
        "a row below the table is not shown"
    );
    v.handle(&UiEvent::Wheel {
        pos: first,
        delta: atomik_ui::kurbo::Vec2::new(0.0, -3.0),
        modifiers: Modifiers::NONE,
    });
    v.handle(&UiEvent::Wheel {
        pos: first,
        delta: atomik_ui::kurbo::Vec2::new(0.0, -3.0),
        modifiers: Modifiers::NONE,
    });
    assert!(v.row_center(0).is_none(), "the first row scrolled out");
    // End takes the cursor to the last row and shows it.
    click!(v, v.row_center(14).unwrap_or(first));
    press(&mut v, Key::End, Modifiers::NONE);
    assert_eq!(v.selected_keys(), ["clap.extra39"]);
    assert!(v.row_center(45).is_some());
}

#[test]
fn every_tooltip_is_short() {
    let v = view();
    let spots = [
        v.close_rect(),
        v.search_rect(),
        v.format_rect(),
        v.status_rect(),
        v.category_rect(),
        v.tag_filter_rect(),
        v.add_tag_rect(),
        v.disable_rect(),
        v.try_again_rect(),
        v.folder_rect(),
        v.rescan_selected_rect(),
        v.rescan_all_rect(),
    ];
    for r in spots {
        let help = v.help_at(r.center()).expect("a tooltip");
        assert!(help.text.len() <= 90, "{}", help.text);
    }
    let head = v
        .help_at(v.head_center("NAME").unwrap())
        .expect("a head tip");
    assert!(head.text.len() <= 90);
    // The reason of a failure is the tooltip of its status cell.
    let tip = v.help_at(v.status_center(1).unwrap()).expect("the reason");
    assert!(tip.text.contains("it needs a library that is missing"));
}

#[test]
fn the_panel_matches_its_golden_and_follows_the_theme_and_scale() {
    let mut v = view();
    click!(v, v.row_center(3).unwrap());
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("plugin_manager_table_1x.json", &dl.to_json());

    let mut scanning = data();
    scanning.scan = Some(ScanProgressInfo {
        done: 12,
        total: 40,
    });
    let mut w = PluginManagerView::new(Theme::dark(), scanning);
    w.set_bounds(BOUNDS);
    let mut dl = DisplayList::new();
    w.paint(&mut dl);
    check("plugin_manager_scan_1x.json", &dl.to_json());

    let mut before = DisplayList::new();
    v.paint(&mut before);
    v.set_theme(Theme::light());
    v.set_scale(2.0);
    let mut after = DisplayList::new();
    v.paint(&mut after);
    assert_ne!(before.to_json(), after.to_json());
    assert!(v.needs_repaint());
}
