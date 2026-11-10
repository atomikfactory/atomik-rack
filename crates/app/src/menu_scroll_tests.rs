//! Scrolling, placement and nesting of the in-window menus, on the real menu model.

use super::tests::{down, mv, press, state, up};
use crate::commands::AppCommand;
use crate::commands::{Menu, MenuEntry, SongList, menus_with_songs};
use crate::menu_layout::{EDGE_MARGIN, WHEEL_STEP};
use crate::menubar::{ITEM_HEIGHT, MENU_BAR_HEIGHT, MenuBar, MenuBarOutcome};
use atomik_ui::keymap::{Keymap, Os, Profile};
use atomik_ui::kurbo::{Point, Rect, Size, Vec2};
use atomik_ui::{Key, Modifiers, UiEvent};

const INF: Size = Size::new(f64::INFINITY, f64::INFINITY);

/// A window too short for the View menu: it scrolls by more than two wheel notches.
const SHORT: Size = Size::new(1000.0, 220.0);

fn full_model() -> Vec<Menu> {
    let keymap = Keymap::bundled(Profile::Standard, Os::Windows);
    let recent: Vec<String> = (0..8).map(|i| format!("song {i}")).collect();
    let templates: Vec<String> = (0..16).map(|i| format!("template {i}")).collect();
    let songs: Vec<String> = (0..4).map(|i| format!("open song {i}")).collect();
    let vcas = vec![(1, "Drums".to_owned()), (2, "Bass".to_owned())];
    let buses = vec![(1, "Bus A".to_owned()), (2, "Bus B".to_owned())];
    let list = SongList {
        names: &songs,
        current: 0,
        vcas: &vcas,
        buses: &buses,
        presets: &[],
    };
    menus_with_songs(&state(), &keymap, &recent, &templates, &list)
}

fn menu_at(model: &[Menu], title: &str) -> usize {
    model.iter().position(|m| m.title == title).unwrap()
}

fn submenu_at(entries: &[MenuEntry], label: &str) -> usize {
    entries
        .iter()
        .position(|e| matches!(e, MenuEntry::Submenu(s) if s.label == label))
        .unwrap_or_else(|| panic!("no submenu {label}"))
}

fn wheel(p: Point, notches: f64) -> UiEvent {
    UiEvent::Wheel {
        delta: Vec2::new(0.0, notches),
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

fn opened(model: &[Menu], title: &str, window: Size) -> (MenuBar, usize) {
    let i = menu_at(model, title);
    let mut bar = MenuBar::new();
    bar.set_window(window);
    bar.handle(&down(MenuBar::title_rects(model)[i].center()), model);
    (bar, i)
}

#[test]
fn a_menu_taller_than_the_window_is_clamped_to_it_and_one_that_fits_is_not() {
    let model = full_model();
    for (w, h) in [
        (1440.0, 900.0),
        (720.0, 450.0),
        (640.0, 360.0),
        (1000.0, 300.0),
    ] {
        let window = Size::new(w, h);
        for i in 0..model.len() {
            let l = MenuBar::dropdown_in(&model, i, window, 0.0).unwrap();
            let at = format!("{} at {w}x{h}", model[i].title);
            assert!(l.panel.y0 >= MENU_BAR_HEIGHT, "{at}");
            assert!(l.panel.y1 <= h - EDGE_MARGIN + 1e-9, "{at}");
            assert!(
                l.panel.x0 >= 0.0 && l.panel.x1 <= w - EDGE_MARGIN + 1e-9,
                "{at}"
            );
            let natural = MenuBar::dropdown_in(&model, i, INF, 0.0).unwrap();
            assert_eq!(
                l.scrolls(),
                natural.panel.y1 > h - EDGE_MARGIN,
                "{at}: it scrolls only when it does not fit"
            );
        }
    }
}

#[test]
fn every_flyout_stays_in_the_window_whatever_the_size() {
    let model = full_model();
    for (w, h) in [
        (1440.0, 900.0),
        (720.0, 450.0),
        (640.0, 360.0),
        (400.0, 300.0),
    ] {
        let window = Size::new(w, h);
        for (i, m) in model.iter().enumerate() {
            let dd = MenuBar::dropdown_in(&model, i, window, 0.0).unwrap();
            for (row, e) in m.entries.iter().enumerate() {
                let MenuEntry::Submenu(s) = e else { continue };
                let (fly, entries) = MenuBar::flyout_in(&dd, &m.entries, row, window, 0.0).unwrap();
                assert_eq!(entries.len(), s.entries.len());
                let at = format!("{} > {} at {w}x{h}", m.title, s.label);
                assert!(fly.panel.y0 >= MENU_BAR_HEIGHT - 1e-9, "{at}");
                assert!(fly.panel.y1 <= h - EDGE_MARGIN + 1e-9, "{at}");
                assert!(fly.panel.x0 >= -1e-9, "{at}");
                assert!(fly.panel.x1 <= w - EDGE_MARGIN + 1e-9, "{at}");
            }
        }
    }
}

#[test]
fn the_wheel_scrolls_the_list_under_the_pointer_and_stops_at_both_ends() {
    let model = full_model();
    let (mut bar, i) = opened(&model, "View", SHORT);
    let l = MenuBar::dropdown_in(&model, i, SHORT, 0.0).unwrap();
    assert!(l.scrolls(), "the View menu is taller than a 220 px window");
    let inside = l.view.center();
    assert_eq!(bar.dropdown_scroll(), 0.0);
    // A notch toward the user scrolls down.
    assert_eq!(
        bar.handle(&wheel(inside, -2.0), &model),
        MenuBarOutcome::Consumed
    );
    assert_eq!(bar.dropdown_scroll(), 2.0 * WHEEL_STEP);
    bar.handle(&wheel(inside, 1.0), &model);
    assert_eq!(bar.dropdown_scroll(), WHEEL_STEP);
    bar.handle(&wheel(inside, 50.0), &model);
    assert_eq!(bar.dropdown_scroll(), 0.0, "never above the first row");
    bar.handle(&wheel(inside, -50.0), &model);
    assert_eq!(
        bar.dropdown_scroll(),
        l.max_offset,
        "never past the last row"
    );
    // The wheel outside the list is swallowed and moves nothing.
    assert_eq!(
        bar.handle(&wheel(Point::new(900.0, 250.0), 5.0), &model),
        MenuBarOutcome::Consumed
    );
    assert_eq!(bar.dropdown_scroll(), l.max_offset);
}

#[test]
fn a_menu_that_fits_does_not_scroll_and_its_wheel_is_still_swallowed() {
    let model = full_model();
    let window = Size::new(1000.0, 800.0);
    let (mut bar, i) = opened(&model, "Create", window);
    let l = MenuBar::dropdown_in(&model, i, window, 0.0).unwrap();
    assert!(!l.scrolls());
    assert_eq!(
        bar.handle(&wheel(l.panel.center(), -3.0), &model),
        MenuBarOutcome::Consumed
    );
    assert_eq!(bar.dropdown_scroll(), 0.0);
}

#[test]
fn hover_and_clicks_use_the_scrolled_rows() {
    let model = full_model();
    let (mut bar, i) = opened(&model, "View", SHORT);
    let l0 = MenuBar::dropdown_in(&model, i, SHORT, 0.0).unwrap();
    bar.handle(&wheel(l0.view.center(), -3.0), &model);
    let offset = bar.dropdown_scroll();
    assert!(offset > 0.0);
    let l = MenuBar::dropdown_in(&model, i, SHORT, offset).unwrap();
    // An enabled item that is whole on screen now and was out of sight before the scroll.
    let row = (0..l.rows.len())
        .find(|r| {
            matches!(&model[i].entries[*r], MenuEntry::Item(it) if it.enabled)
                && l.rows[*r].height() >= ITEM_HEIGHT - 1e-9
                && l0.rows[*r] == Rect::ZERO
        })
        .expect("a row that scrolled into view");
    let MenuEntry::Item(item) = &model[i].entries[row] else {
        unreachable!()
    };
    let p = l.rows[row].center();
    bar.handle(&mv(p), &model);
    assert_eq!(bar.highlighted_item(), Some(row));
    bar.handle(&down(p), &model);
    assert_eq!(
        bar.handle(&up(p), &model),
        MenuBarOutcome::Command(item.command)
    );
    // Where the first row was drawn there is another row now: row 0 cannot be hit.
    let (mut bar, _) = opened(&model, "View", SHORT);
    bar.handle(&wheel(l0.view.center(), -3.0), &model);
    bar.handle(&mv(l0.rows[0].center()), &model);
    assert_ne!(bar.highlighted_item(), Some(0));
}

#[test]
fn the_keyboard_keeps_the_lit_row_whole_in_view_going_down_wrapping_and_home() {
    let model = full_model();
    let (mut bar, i) = opened(&model, "View", SHORT);
    let selectable = model[i]
        .entries
        .iter()
        .filter(|e| match e {
            MenuEntry::Item(it) => it.enabled,
            MenuEntry::Submenu(_) => true,
            MenuEntry::Separator => false,
        })
        .count();
    let check = |bar: &MenuBar| {
        let lit = bar.highlighted_item().expect("a row is lit");
        let l = MenuBar::dropdown_in(&model, i, SHORT, bar.dropdown_scroll()).unwrap();
        assert!(
            l.rows[lit].height() >= ITEM_HEIGHT - 1e-9,
            "row {lit} is whole on screen at offset {}",
            bar.dropdown_scroll()
        );
    };
    for _ in 0..selectable {
        press(&mut bar, &model, Key::Down);
        check(&bar);
    }
    assert!(bar.dropdown_scroll() > 0.0, "it scrolled on the way down");
    press(&mut bar, &model, Key::Down);
    check(&bar);
    assert_eq!(
        bar.dropdown_scroll(),
        0.0,
        "wrapping to the first row shows the top"
    );
    press(&mut bar, &model, Key::Up);
    check(&bar);
    assert!(
        bar.dropdown_scroll() > 0.0,
        "Up from the first row wraps to the last"
    );
    press(&mut bar, &model, Key::Home);
    check(&bar);
    assert_eq!(bar.dropdown_scroll(), 0.0);
    press(&mut bar, &model, Key::End);
    check(&bar);
    let max = MenuBar::dropdown_in(&model, i, SHORT, 0.0)
        .unwrap()
        .max_offset;
    assert!((bar.dropdown_scroll() - max).abs() < 1e-9);
}

#[test]
fn a_pointer_resting_on_a_scroll_strip_scrolls_while_it_stays() {
    let model = full_model();
    let (mut bar, i) = opened(&model, "View", SHORT);
    let l = MenuBar::dropdown_in(&model, i, SHORT, 0.0).unwrap();
    let down_strip = l.down_strip().unwrap().center();
    bar.handle(&mv(down_strip), &model);
    assert_eq!(bar.highlighted_item(), None, "a strip lights no row");
    bar.tick(0.0, &model);
    let mut last = bar.dropdown_scroll();
    for step in 1..=5 {
        assert!(bar.tick(0.05 * f64::from(step), &model), "step {step}");
        assert!(bar.dropdown_scroll() > last);
        last = bar.dropdown_scroll();
    }
    // Off the strip it stops; on the top strip it goes back up.
    bar.handle(&mv(l.view.center()), &model);
    assert!(!bar.tick(1.0, &model));
    assert_eq!(bar.dropdown_scroll(), last);
    let up_strip = l.up_strip().unwrap().center();
    bar.handle(&mv(up_strip), &model);
    bar.tick(2.0, &model);
    assert!(bar.tick(2.1, &model));
    assert!(bar.dropdown_scroll() < last);
    // A long rest reaches the end and then nothing moves.
    bar.handle(&mv(down_strip), &model);
    for k in 0..200 {
        bar.tick(3.0 + 0.05 * f64::from(k), &model);
    }
    assert_eq!(bar.dropdown_scroll(), l.max_offset);
    assert!(!bar.tick(100.0, &model));
}

#[test]
fn a_flyout_moves_up_and_flips_to_stay_in_a_small_window() {
    let model = full_model();
    let file = menu_at(&model, "File");
    let export = submenu_at(&model[file].entries, "Export");
    // A short window: the Export list starts level with its row and would run off the bottom.
    let window = Size::new(1000.0, 600.0);
    let dd = MenuBar::dropdown_in(&model, file, window, 0.0).unwrap();
    let (fly, _) = MenuBar::flyout_in(&dd, &model[file].entries, export, window, 0.0).unwrap();
    assert!(fly.panel.y0 <= dd.drawn[export].y0, "moved up, never down");
    assert!(fly.panel.y1 <= window.height - EDGE_MARGIN + 1e-9);
    // A narrow window: it opens on the left of the dropdown.
    let narrow = Size::new(400.0, 900.0);
    let dd = MenuBar::dropdown_in(&model, file, narrow, 0.0).unwrap();
    let (fly, _) = MenuBar::flyout_in(&dd, &model[file].entries, export, narrow, 0.0).unwrap();
    assert!(fly.panel.x1 <= narrow.width - EDGE_MARGIN + 1e-9 && fly.panel.x0 >= 0.0);
    // Roomy: to the right.
    let dd = MenuBar::dropdown_in(&model, file, INF, 0.0).unwrap();
    let (fly, _) = MenuBar::flyout_in(&dd, &model[file].entries, export, INF, 0.0).unwrap();
    assert!(fly.panel.x0 >= dd.panel.x1 - 3.0);
}

#[test]
fn a_command_in_a_flyout_is_chosen_where_it_is_drawn_in_a_short_window() {
    let model = full_model();
    let window = Size::new(1000.0, 600.0);
    let (mut bar, file) = opened(&model, "File", window);
    let export = submenu_at(&model[file].entries, "Export");
    let dd = MenuBar::dropdown_in(&model, file, window, 0.0).unwrap();
    bar.handle(&mv(dd.rows[export].center()), &model);
    assert_eq!(
        bar.look().2,
        Some(export),
        "resting on the row opens the list"
    );
    let (fly, entries) =
        MenuBar::flyout_in(&dd, &model[file].entries, export, window, 0.0).unwrap();
    let patch = entries
        .iter()
        .position(|e| matches!(e, MenuEntry::Item(i) if i.command == AppCommand::ExportPatch))
        .unwrap();
    let p = fly.rows[patch].center();
    bar.handle(&mv(p), &model);
    assert_eq!(bar.look().3, Some(patch));
    bar.handle(&down(p), &model);
    assert_eq!(
        bar.handle(&up(p), &model),
        MenuBarOutcome::Command(AppCommand::ExportPatch)
    );
}

#[test]
fn a_submenu_inside_a_submenu_opens_and_its_entry_can_be_chosen() {
    let model = full_model();
    let (mut bar, view) = opened(&model, "View", INF);
    let panels = submenu_at(&model[view].entries, "Panels");
    let dd = MenuBar::dropdown_in(&model, view, INF, 0.0).unwrap();
    bar.handle(&mv(dd.rows[panels].center()), &model);
    let (fly, entries) = MenuBar::flyout_in(&dd, &model[view].entries, panels, INF, 0.0).unwrap();
    let sorting = submenu_at(entries, "Rack Sorting");
    bar.handle(&mv(fly.rows[sorting].center()), &model);
    assert_eq!(bar.look().2, Some(panels));
    let (inner, inner_entries) = MenuBar::flyout_in(&fly, entries, sorting, INF, 0.0).unwrap();
    let auto = inner_entries
        .iter()
        .position(|e| matches!(e, MenuEntry::Item(i) if i.label == "Auto-Group"))
        .unwrap();
    // Into the inner list: every list on the way stays open and its row stays lit.
    bar.handle(&mv(inner.rows[auto].center()), &model);
    assert_eq!(bar.deep_look(), [(sorting, Some(auto))]);
    assert_eq!(
        bar.look(),
        (Some(view), Some(panels), Some(panels), Some(sorting))
    );
    let p = inner.rows[auto].center();
    bar.handle(&down(p), &model);
    assert_eq!(
        bar.handle(&up(p), &model),
        MenuBarOutcome::Command(AppCommand::SetRackSort(atomik_model::RackSort::AutoGroup))
    );
    assert!(!bar.is_open());
}

#[test]
fn the_keyboard_walks_into_and_out_of_nested_submenus() {
    let model = full_model();
    let (mut bar, view) = opened(&model, "View", INF);
    let entries = &model[view].entries;
    let panels = submenu_at(entries, "Panels");
    while bar.highlighted_item() != Some(panels) {
        press(&mut bar, &model, Key::Down);
    }
    press(&mut bar, &model, Key::Right);
    assert_eq!(bar.look().2, Some(panels));
    assert_eq!(bar.look().3, Some(0), "the first entry of the list is lit");
    let MenuEntry::Submenu(panels_list) = &entries[panels] else {
        unreachable!()
    };
    let sorting = submenu_at(&panels_list.entries, "Rack Sorting");
    while bar.look().3 != Some(sorting) {
        press(&mut bar, &model, Key::Down);
    }
    press(&mut bar, &model, Key::Right);
    assert_eq!(bar.deep_look(), [(sorting, Some(0))]);
    press(&mut bar, &model, Key::Down);
    assert_eq!(
        bar.deep_look(),
        [(sorting, Some(1))],
        "moves in the inner list"
    );
    press(&mut bar, &model, Key::Left);
    assert_eq!(bar.deep_look(), [], "Left closes the inner list first");
    assert_eq!(bar.look().3, Some(sorting));
    press(&mut bar, &model, Key::Escape);
    assert_eq!(bar.look().2, None, "then the list");
    assert!(bar.is_open());
}

#[test]
fn a_flyout_taller_than_the_room_scrolls_with_the_wheel() {
    let model = full_model();
    let window = Size::new(1000.0, 300.0);
    let mixer = menu_at(&model, "Mixer");
    let (mut bar, _) = opened(&model, "Mixer", window);
    let dd = MenuBar::dropdown_in(&model, mixer, window, 0.0).unwrap();
    // The Snapshots list has 12 rows: taller than the room beside it.
    let snaps = submenu_at(&model[mixer].entries, "Snapshots");
    let (fly, _) = MenuBar::flyout_in(&dd, &model[mixer].entries, snaps, window, 0.0).unwrap();
    assert!(fly.scrolls(), "12 rows do not fit 300 px");
    bar.handle(&mv(dd.rows[snaps].center()), &model);
    bar.handle(&wheel(fly.view.center(), -2.0), &model);
    assert_eq!(bar.flyout_scroll(1), (2.0 * WHEEL_STEP).min(fly.max_offset));
    assert_eq!(bar.dropdown_scroll(), 0.0, "the dropdown stayed");
}
