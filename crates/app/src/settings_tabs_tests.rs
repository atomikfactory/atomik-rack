//! The Keyboard and Workflow tabs of the settings dialog,
//! headless: search, learn, clear, restore, conflicts, profile, roles, export and import,
//! workflow toggles, and the display-list goldens `settings_keyboard_*` and
//! `settings_workflow_*` (regenerate with `ATOMIK_UPDATE_GOLDEN=1`).

use super::*;
use crate::keyboard_tab::Item;
use crate::keymap_store::WorkflowPrefs;
use atomik_ui::keymap::{
    Chord, Context, KeymapEditor, Mods, Os, Profile, Role, Row, RowKey, Source, UserKeymap,
};
use atomik_ui::{DisplayList, Modifiers};

const VIEW: Size = Size::new(1000.0, 800.0);

fn dialog_for(os: Os, profile: Profile, user: UserKeymap) -> SettingsDialog {
    SettingsDialog::new(AudioSettings::default(), DeviceLists::default()).with_preferences(
        KeymapEditor::new(os, profile, user),
        WorkflowPrefs::default(),
    )
}

fn dialog() -> SettingsDialog {
    dialog_for(Os::Windows, Profile::Standard, UserKeymap::default())
}

fn down(d: &mut SettingsDialog, p: Point) -> DialogOutcome {
    let out = d.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        VIEW,
    );
    d.handle(
        &UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
        VIEW,
    );
    out
}

fn press(d: &mut SettingsDialog, chord: &str, os: Os) -> DialogOutcome {
    let (key, modifiers) = atomik_ui::keymap::script::event_for(os, chord).unwrap();
    d.handle(
        &UiEvent::Key {
            key,
            modifiers,
            pressed: true,
        },
        VIEW,
    )
}

fn type_text(d: &mut SettingsDialog, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { Key::Char(c) };
        d.handle(
            &UiEvent::Key {
                key,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            VIEW,
        );
    }
}

fn goto(d: &mut SettingsDialog, tab: Tab) {
    let r = d.tab_strip(VIEW).iter().find(|(_, t)| *t == tab).unwrap().0;
    down(d, r.center());
    assert_eq!(d.tab(), tab);
}

fn kb(d: &SettingsDialog) -> KeyboardLayout {
    d.keyboard_layout(VIEW).unwrap()
}

/// The chord chip of a row (by command id and slot index): the search brings the row into view.
fn chip(d: &mut SettingsDialog, command: &str, slot: usize) -> Point {
    d.keyboard_search(command);
    let l = kb(d);
    let row = l
        .items
        .iter()
        .find_map(|i| match i {
            Item::Row { row, chips, .. } if row.key.command == command => Some(chips),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no row {command}"));
    row[slot].rect.center()
}

fn row_chords(d: &SettingsDialog, command: &str) -> Vec<String> {
    d.keyboard()
        .unwrap()
        .editor()
        .rows("")
        .into_iter()
        .find(|r| r.key.command == command)
        .unwrap()
        .chords
        .iter()
        .map(|c| c.to_string())
        .collect()
}

fn scroll_to_end(d: &mut SettingsDialog) {
    let l = kb(d);
    d.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -100_000.0),
            pos: l.list.center(),
            modifiers: Modifiers::NONE,
        },
        VIEW,
    );
}

fn button(l: &KeyboardLayout, which: &str) -> Point {
    let c = l.conflict.as_ref().expect("a conflict is shown");
    match which {
        "swap" => c.swap.center(),
        "unassign" => c.unassign.center(),
        _ => c.cancel.center(),
    }
}

// ------------------------------------------------------------------ tabs

#[test]
fn the_tab_strip_exists_only_with_preferences_and_switches_the_tab() {
    let plain = SettingsDialog::new(AudioSettings::default(), DeviceLists::default());
    assert!(
        plain.layout(VIEW).tabs.is_empty(),
        "the audio-only dialog is as before"
    );
    assert!(plain.keyboard_layout(VIEW).is_none());
    let mut d = dialog();
    let tabs: Vec<Tab> = d.layout(VIEW).tabs.iter().map(|(_, t)| *t).collect();
    assert_eq!(
        tabs,
        [Tab::Audio, Tab::Keyboard, Tab::Workflow, Tab::Surfaces]
    );
    assert_eq!(d.tab(), Tab::Audio);
    goto(&mut d, Tab::Keyboard);
    goto(&mut d, Tab::Workflow);
    goto(&mut d, Tab::Audio);
}

#[test]
fn the_audio_tab_is_the_same_with_and_without_preferences() {
    let plain = SettingsDialog::new(AudioSettings::default(), DeviceLists::default());
    let with = dialog();
    let (a, b) = (plain.layout(VIEW), with.layout(VIEW));
    assert_eq!(a.panel, b.panel);
    assert_eq!(a.options, b.options);
    assert_eq!(a.apply, b.apply);
    assert_eq!(a.content, b.content);
    let mut la = DisplayList::new();
    let mut lb = DisplayList::new();
    plain.paint(&mut la, &Theme::dark(), VIEW);
    with.paint(&mut lb, &Theme::dark(), VIEW);
    let strip: Vec<&str> = lb.texts();
    for t in ["Keyboard", "Workflow"] {
        assert!(strip.contains(&t), "{t}");
    }
    for t in la.texts() {
        assert!(lb.texts().contains(&t), "{t} still painted");
    }
}

// ------------------------------------------------------------------ keyboard

#[test]
fn every_command_is_listed_by_area_with_its_chords_for_this_os() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let l = kb(&d);
    let groups: Vec<&str> = l
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Group { title, .. } => Some(title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        groups,
        [
            "Global",
            "Transport",
            "View",
            "Edit",
            "Rack",
            "Sequencer",
            "Editor",
            "Mixer",
            "Browser",
            "Files",
            "Piano Keys",
            "Create",
            "Tools"
        ]
    );
    let rows: Vec<&Row> = l
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Row { row, .. } => Some(row),
            _ => None,
        })
        .collect();
    assert!(rows.len() > 90);
    let import = rows
        .iter()
        .find(|r| r.key.command == "file.import_audio")
        .unwrap();
    assert_eq!(import.chords[0].menu_text(Os::Windows), "Ctrl+Shift+I");
    // The same list on macOS shows the Mac chord text.
    let mut m = dialog_for(Os::Mac, Profile::Standard, UserKeymap::default());
    goto(&mut m, Tab::Keyboard);
    let mut list = DisplayList::new();
    m.paint(&mut list, &Theme::dark(), VIEW);
    assert!(list.texts().contains(&"Cmd+Opt+I"), "{:?}", list.texts());
}

#[test]
fn searching_split_finds_the_command_and_a_nonsense_query_finds_nothing() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    type_text(&mut d, "split");
    let found: Vec<String> = kb(&d)
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Row { row, .. } => Some(row.key.command.clone()),
            _ => None,
        })
        .collect();
    assert!(
        found.contains(&"sequencer.split_at_playhead".to_string()),
        "{found:?}"
    );
    assert!(found.len() < 10);
    // Esc clears the search first, then closes the dialog.
    assert_eq!(press(&mut d, "Escape", Os::Windows), DialogOutcome::Open);
    assert!(kb(&d).items.len() > 90);
    type_text(&mut d, "zzzzzz");
    assert!(
        !kb(&d).items.iter().any(|i| matches!(i, Item::Row { .. })),
        "nothing matches"
    );
    assert_eq!(press(&mut d, "Escape", Os::Windows), DialogOutcome::Open);
    assert_eq!(press(&mut d, "Escape", Os::Windows), DialogOutcome::Cancel);
}

#[test]
fn rebinding_by_pressing_the_chord_changes_only_that_binding() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    assert!(d.keyboard().unwrap().is_learning());
    assert!(
        d.capturing_keys(),
        "the shell passes raw keys while learning"
    );
    press(&mut d, "Cmd+Shift+D", Os::Windows);
    assert!(!d.keyboard().unwrap().is_learning());
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+Shift+D"]);
    let (user, _) = d.preferences().unwrap();
    assert_eq!(
        user.bindings.len(),
        1,
        "only the changed binding is in the diff"
    );
    let json = user.to_json();
    assert!(
        json.contains("edit.duplicate") && json.contains("Cmd+Shift+D"),
        "{json}"
    );
    assert!(
        d.keyboard()
            .unwrap()
            .message()
            .unwrap()
            .contains("Duplicate")
    );
}

#[test]
fn a_numpad_chord_can_be_learned() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    d.handle(
        &UiEvent::Key {
            key: Key::Numpad(atomik_ui::NumpadKey::Digit(6)),
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        VIEW,
    );
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Numpad6"]);
}

#[test]
fn escape_aborts_learning_and_backspace_clears_the_slot() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    assert_eq!(
        press(&mut d, "Escape", Os::Windows),
        DialogOutcome::Open,
        "Esc aborts, not cancels"
    );
    assert!(!d.keyboard().unwrap().is_learning());
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D"]);
    down(&mut d, p);
    press(&mut d, "Backspace", Os::Windows);
    assert!(row_chords(&d, "edit.duplicate").is_empty());
    // An unbound command offers a chip to add one.
    let again = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, again);
    press(&mut d, "Cmd+Alt+J", Os::Windows);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+Alt+J"]);
}

#[test]
fn modifier_only_presses_do_not_end_learning() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    d.handle(
        &UiEvent::Key {
            key: Key::Other,
            modifiers: Modifiers::CTRL,
            pressed: true,
        },
        VIEW,
    );
    assert!(d.keyboard().unwrap().is_learning());
}

#[test]
fn a_second_chord_is_added_with_the_empty_chip() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let add = chip(&mut d, "edit.duplicate", 1);
    down(&mut d, add);
    press(&mut d, "Cmd+Alt+D", Os::Windows);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D", "Cmd+Alt+D"]);
}

#[test]
fn rebinding_onto_a_used_chord_shows_the_conflict_naming_both_commands() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    press(&mut d, "Cmd+C", Os::Windows);
    let l = kb(&d);
    let c = l.conflict.as_ref().expect("conflict shown");
    assert!(
        c.text.contains("Duplicate") && c.text.contains("Copy"),
        "{}",
        c.text
    );
    assert!(c.text.contains("Ctrl+C"), "{}", c.text);
    assert_eq!(
        row_chords(&d, "edit.duplicate"),
        ["Cmd+D"],
        "nothing changed yet"
    );
    let mut list = DisplayList::new();
    d.paint(&mut list, &Theme::dark(), VIEW);
    for t in ["Swap", "Cancel"] {
        assert!(list.texts().contains(&t), "{t}");
    }
    assert!(list.texts().iter().any(|t| t.starts_with("Unassign")));
}

#[test]
fn the_conflict_can_be_settled_by_swap_unassign_or_cancel() {
    let conflict = |d: &mut SettingsDialog| {
        let p = chip(d, "edit.duplicate", 0);
        down(d, p);
        press(d, "Cmd+C", Os::Windows);
    };
    // Swap.
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    conflict(&mut d);
    let b = button(&kb(&d), "swap");
    down(&mut d, b);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+C"]);
    assert_eq!(row_chords(&d, "edit.copy"), ["Cmd+D"]);
    assert!(kb(&d).conflict.is_none());
    // Unassign the other.
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    conflict(&mut d);
    let b = button(&kb(&d), "unassign");
    down(&mut d, b);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+C"]);
    assert!(row_chords(&d, "edit.copy").is_empty());
    // Cancel (button and Esc).
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    conflict(&mut d);
    let b = button(&kb(&d), "cancel");
    down(&mut d, b);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D"]);
    conflict(&mut d);
    assert_eq!(press(&mut d, "Escape", Os::Windows), DialogOutcome::Open);
    assert!(kb(&d).conflict.is_none());
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D"]);
}

#[test]
fn a_reserved_chord_is_refused_with_a_message() {
    let mut d = dialog_for(Os::Mac, Profile::Standard, UserKeymap::default());
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    press(&mut d, "Cmd+Q", Os::Mac);
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D"]);
    assert!(
        d.keyboard()
            .unwrap()
            .message()
            .unwrap()
            .contains("reserved")
    );
}

#[test]
fn shadowing_a_global_binding_is_a_note_in_the_message_not_a_conflict() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "tool.razor", 0);
    down(&mut d, p);
    press(&mut d, "Space", Os::Windows);
    assert!(kb(&d).conflict.is_none());
    assert_eq!(row_chords(&d, "tool.razor"), ["Space"]);
    assert!(d.keyboard().unwrap().message().unwrap().contains("Play"));
}

#[test]
fn reset_restores_one_command_and_restore_all_every_one() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    for (cmd, chord) in [("edit.duplicate", "Cmd+Alt+D"), ("edit.undo", "Cmd+Alt+U")] {
        let p = chip(&mut d, cmd, 0);
        down(&mut d, p);
        press(&mut d, chord, Os::Windows);
    }
    // The changed rows are marked as the user's and offer a reset.
    d.keyboard_search("edit.duplicate");
    let l = kb(&d);
    let reset = l
        .items
        .iter()
        .find_map(|i| match i {
            Item::Row { row, reset, .. } if row.key.command == "edit.duplicate" => {
                assert_eq!(row.source, Source::User);
                *reset
            }
            _ => None,
        })
        .expect("a reset button");
    down(&mut d, reset.center());
    assert_eq!(row_chords(&d, "edit.duplicate"), ["Cmd+D"]);
    assert_eq!(row_chords(&d, "edit.undo"), ["Cmd+Alt+U"]);
    let all = kb(&d).restore_all;
    down(&mut d, all.center());
    assert_eq!(row_chords(&d, "edit.undo"), ["Cmd+Z"]);
    assert!(d.preferences().unwrap().0.is_empty());
}

#[test]
fn the_profile_is_chosen_with_a_chip_and_kept() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    assert_eq!(d.keyboard().unwrap().editor().profile(), Profile::Standard);
    let legacy = kb(&d)
        .profiles
        .iter()
        .find(|(_, p, _)| *p == Profile::Legacy)
        .unwrap()
        .0;
    down(&mut d, legacy.center());
    assert_eq!(d.keyboard().unwrap().editor().profile(), Profile::Legacy);
    assert_eq!(row_chords(&d, "file.import_audio"), ["Cmd+I"]);
    assert_eq!(d.preferences().unwrap().0.profile, Some(Profile::Legacy));
}

#[test]
fn roles_are_shown_and_rebound_by_holding_modifiers() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    scroll_to_end(&mut d);
    let l = kb(&d);
    let (chip_rect, text) = l
        .items
        .iter()
        .find_map(|i| match i {
            Item::Role {
                role, chip, text, ..
            } if *role == Role::AlternateTool => Some((*chip, text.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(text, "Alt");
    down(&mut d, chip_rect.center());
    assert!(d.capturing_keys());
    let hold = |d: &mut SettingsDialog, m: Modifiers| {
        d.handle(&UiEvent::ModifiersChanged { modifiers: m }, VIEW);
    };
    hold(
        &mut d,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    hold(
        &mut d,
        Modifiers {
            shift: true,
            alt: true,
            ..Modifiers::NONE
        },
    );
    hold(&mut d, Modifiers::NONE);
    assert!(!d.capturing_keys());
    assert_eq!(
        d.keyboard()
            .unwrap()
            .editor()
            .roles()
            .binding(Role::AlternateTool)
            .unwrap()
            .mods,
        Mods::parse("Alt+Shift").unwrap()
    );
    assert_eq!(
        d.preferences()
            .unwrap()
            .0
            .roles
            .get("alternate_tool")
            .unwrap(),
        "Alt+Shift"
    );
}

#[test]
fn a_role_clash_is_refused_with_a_message() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    scroll_to_end(&mut d);
    let l = kb(&d);
    let rect = l
        .items
        .iter()
        .find_map(|i| match i {
            Item::Role { role, chip, .. } if *role == Role::CopyDrag => Some(*chip),
            _ => None,
        })
        .unwrap();
    down(&mut d, rect.center());
    let hold = |d: &mut SettingsDialog, m: Modifiers| {
        d.handle(&UiEvent::ModifiersChanged { modifiers: m }, VIEW);
    };
    hold(
        &mut d,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    hold(&mut d, Modifiers::NONE);
    assert!(
        d.keyboard()
            .unwrap()
            .message()
            .unwrap()
            .contains("Constrain")
    );
    assert!(d.preferences().unwrap().0.roles.is_empty());
}

#[test]
fn a_loaded_conflict_is_shown_as_a_message() {
    let user = UserKeymap::from_json(
        r#"{"bindings": [{"command": "edit.cut", "context": "global", "chords": ["Cmd+C"]}]}"#,
    )
    .unwrap()
    .0;
    let mut d = dialog_for(Os::Windows, Profile::Standard, user);
    goto(&mut d, Tab::Keyboard);
    assert!(
        d.keyboard().unwrap().message().unwrap().contains("Ctrl+C")
            || d.keyboard().unwrap().message().unwrap().contains("Cmd+C")
    );
}

#[test]
fn export_and_import_are_requests_for_the_shell() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let l = kb(&d);
    assert_eq!(down(&mut d, l.export.center()), DialogOutcome::ExportKeymap);
    assert_eq!(down(&mut d, l.import.center()), DialogOutcome::ImportKeymap);
    // An imported file replaces the draft.
    let user = UserKeymap::from_json(
        r#"{"profile": "legacy", "bindings": [{"command": "edit.undo", "context": "global", "chords": ["Cmd+Alt+U"]}]}"#,
    )
    .unwrap()
    .0;
    d.replace_user(user);
    assert_eq!(row_chords(&d, "edit.undo"), ["Cmd+Alt+U"]);
    assert_eq!(d.keyboard().unwrap().editor().profile(), Profile::Legacy);
}

#[test]
fn apply_returns_the_audio_settings_and_the_drafts_cancel_discards() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    press(&mut d, "Cmd+Alt+D", Os::Windows);
    let out = press(&mut d, "Return", Os::Windows);
    assert!(matches!(out, DialogOutcome::Apply(_)));
    let (user, wf) = d.preferences().unwrap();
    assert_eq!(user.bindings.len(), 1);
    assert_eq!(wf, WorkflowPrefs::default());
}

#[test]
fn the_buttons_of_the_keyboard_tab_apply_and_cancel() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let (apply, cancel) = d.buttons(VIEW);
    assert!(matches!(
        down(&mut d, apply.center()),
        DialogOutcome::Apply(_)
    ));
    assert_eq!(down(&mut d, cancel.center()), DialogOutcome::Cancel);
}

#[test]
fn the_list_scrolls_with_the_wheel_and_stays_inside_its_bounds() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let l = kb(&d);
    assert!(l.content_height > l.list.height());
    let wheel = |dy| UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
        pos: l.list.center(),
        modifiers: Modifiers::NONE,
    };
    d.handle(&wheel(-5.0), VIEW);
    let moved = kb(&d);
    assert!(moved.items[0].top() < l.items[0].top());
    d.handle(&wheel(-10_000.0), VIEW);
    let end = kb(&d);
    assert!(end.items.last().unwrap().bottom() <= end.list.y1 + 0.5);
    d.handle(&wheel(10_000.0), VIEW);
    assert_eq!(kb(&d).items[0].top(), l.items[0].top());
}

#[test]
fn nothing_overlaps_and_everything_is_inside_the_panel() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let l = kb(&d);
    let panel = d.panel(VIEW);
    assert!(panel.y1 <= VIEW.height && panel.x1 <= VIEW.width);
    for r in [l.search, l.restore_all, l.export, l.import, l.list] {
        assert!(panel.contains(r.center()));
    }
    assert!(l.restore_all.intersect(l.export).area() <= 0.0);
    assert!(l.export.intersect(l.import).area() <= 0.0);
    let (apply, cancel) = d.buttons(VIEW);
    assert!(l.import.intersect(cancel).area() <= 0.0);
    assert!(apply.intersect(cancel).area() <= 0.0);
}

// ------------------------------------------------------------------ workflow

#[test]
fn the_workflow_tab_toggles_the_preferences_and_apply_returns_them() {
    let mut d = dialog();
    goto(&mut d, Tab::Workflow);
    let l = d.workflow_layout(VIEW).unwrap();
    assert_eq!(l.toggles.len(), 6);
    let labels: Vec<&str> = l.toggles.iter().map(|t| t.label).collect();
    assert_eq!(
        labels,
        [
            "Auto-route new devices",
            "Create keeps the browser open",
            "Follow song while playing",
            "Automation follows the selection",
            "Show tooltips",
            "Open last project at start-up"
        ]
    );
    assert!(!l.toggles[3].on, "following the selection starts off");
    assert!(l.toggles[4].on, "tooltips start on");
    assert!(l.toggles[0].on && !l.toggles[1].on);
    down(&mut d, l.toggles[0].rect.center());
    down(&mut d, l.toggles[1].rect.center());
    let (_, wf) = d.preferences().unwrap();
    assert!(!wf.auto_route && wf.create_keeps_browser_open && !wf.follow_selection);
    let l = d.workflow_layout(VIEW).unwrap();
    down(&mut d, l.toggles[3].rect.center());
    assert!(d.preferences().unwrap().1.follow_selection);
    let l = d.workflow_layout(VIEW).unwrap();
    down(&mut d, l.toggles[4].rect.center());
    assert!(!d.preferences().unwrap().1.show_tooltips);
    assert!(!l.toggles[5].on, "reopening the last song starts off");
    let l = d.workflow_layout(VIEW).unwrap();
    down(&mut d, l.toggles[5].rect.center());
    assert!(d.preferences().unwrap().1.open_last_project);
    let again = d.workflow_layout(VIEW).unwrap();
    assert!(!again.toggles[0].on && again.toggles[1].on);
    assert!(matches!(
        press(&mut d, "Return", Os::Windows),
        DialogOutcome::Apply(_)
    ));
    // Cancel keeps nothing: the shell drops the dialog.
    let mut c = dialog();
    goto(&mut c, Tab::Workflow);
    assert_eq!(press(&mut c, "Escape", Os::Windows), DialogOutcome::Cancel);
}

#[test]
fn the_workflow_tab_steps_the_record_mode_and_the_keep_events_policy() {
    use crate::record_over::{KeepEvents, RecordMode};
    let mut d = dialog();
    goto(&mut d, Tab::Workflow);
    let l = d.workflow_layout(VIEW).unwrap();
    assert_eq!(l.cycles.len(), 2);
    assert_eq!(l.cycles[0].value, "Replace (takes)");
    assert_eq!(l.cycles[1].value, "Keep");
    let (_, apply) = d.buttons(VIEW);
    assert!(
        l.cycles[1].rect.y1 <= apply.y0,
        "the rows sit above the Apply button"
    );
    down(&mut d, l.cycles[0].button.center());
    down(&mut d, l.cycles[1].button.center());
    down(&mut d, l.cycles[1].button.center());
    let (_, wf) = d.preferences().unwrap();
    assert_eq!(wf.record_mode, RecordMode::Layer);
    assert_eq!(wf.keep_events, KeepEvents::NewClip);
    let l = d.workflow_layout(VIEW).unwrap();
    assert_eq!(l.cycles[0].value, "Layer");
    assert_eq!(l.cycles[1].value, "New clip");
}

#[test]
fn the_workflow_tab_library_row_chooses_and_resets_the_folder() {
    let mut d = dialog();
    goto(&mut d, Tab::Workflow);
    let l = d.workflow_layout(VIEW).unwrap();
    assert!(l.library.path.contains("default"), "{}", l.library.path);
    // Choose asks the platform for a folder; the dialog stays open.
    assert_eq!(
        down(&mut d, l.library.choose.center()),
        DialogOutcome::PickLibraryFolder
    );
    assert_eq!(d.preferences().unwrap().1.library_folder, None);
    // The picked folder goes into the draft and shows.
    d.set_library_folder(Some(PathBuf::from("D:/Sounds/Library")));
    let l = d.workflow_layout(VIEW).unwrap();
    assert!(l.library.path.contains("Library"), "{}", l.library.path);
    assert_eq!(
        d.preferences().unwrap().1.library_folder,
        Some(PathBuf::from("D:/Sounds/Library"))
    );
    // Default clears it.
    assert_eq!(
        down(&mut d, l.library.default.center()),
        DialogOutcome::Open
    );
    assert_eq!(d.preferences().unwrap().1.library_folder, None);
    // The row sits inside the panel and above the Apply and Cancel buttons.
    let panel = d.panel(VIEW);
    let (apply, _) = d.buttons(VIEW);
    assert!(l.library.rect.y1 <= apply.y0 && l.library.rect.y0 >= panel.y0);
}

#[test]
fn other_workflow_fields_survive_the_dialog() {
    let prefs = WorkflowPrefs::from_json(r#"{"auto_route": false, "layout_note": 7}"#).unwrap();
    let d = SettingsDialog::new(AudioSettings::default(), DeviceLists::default()).with_preferences(
        KeymapEditor::new(Os::Windows, Profile::Standard, UserKeymap::default()),
        prefs,
    );
    let (_, wf) = d.preferences().unwrap();
    assert!(wf.extra.contains_key("layout_note"));
    assert!(!wf.auto_route);
}

// ------------------------------------------------------------------ goldens

fn golden(name: &str, d: &SettingsDialog) {
    let mut list = DisplayList::new();
    d.paint(&mut list, &Theme::dark(), VIEW);
    let actual = list.to_json();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
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
fn golden_keyboard_list() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    golden("settings_keyboard_list", &d);
}

#[test]
fn golden_keyboard_search_and_learn() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    type_text(&mut d, "dup");
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    golden("settings_keyboard_learn", &d);
}

#[test]
fn golden_keyboard_conflict() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let p = chip(&mut d, "edit.duplicate", 0);
    down(&mut d, p);
    press(&mut d, "Cmd+C", Os::Windows);
    golden("settings_keyboard_conflict", &d);
}

#[test]
fn golden_keyboard_mac_with_user_changes() {
    let user = UserKeymap::from_json(
        r#"{"profile": "legacy", "bindings": [{"command": "edit.duplicate", "context": "global", "chords": ["Cmd+Shift+D"]}]}"#,
    )
    .unwrap()
    .0;
    let mut d = dialog_for(Os::Mac, Profile::Standard, user);
    goto(&mut d, Tab::Keyboard);
    golden("settings_keyboard_mac_user", &d);
}

#[test]
fn golden_workflow() {
    let mut d = dialog();
    goto(&mut d, Tab::Workflow);
    golden("settings_workflow", &d);
    let l = d.workflow_layout(VIEW).unwrap();
    down(&mut d, l.toggles[0].rect.center());
    golden("settings_workflow_auto_route_off", &d);
}

#[test]
fn the_row_keys_are_the_registry_keys() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    let key = RowKey {
        command: "edit.undo".into(),
        arg: None,
        context: Context::Global,
    };
    let rows = d.keyboard().unwrap().editor().rows("undo");
    assert!(rows.iter().any(|r| r.key == key));
    let chord = Chord::parse("Cmd+Z").unwrap();
    assert_eq!(rows.iter().find(|r| r.key == key).unwrap().chords, [chord]);
}

// ------------------------------------------------------------------ surfaces

fn surface_lists() -> DeviceLists {
    DeviceLists {
        midi_ports: vec!["Example Surface".into(), "Example Keyboard".into()],
        midi_outputs: vec!["Example Surface".into()],
        ..DeviceLists::default()
    }
}

fn surfaces_dialog(current: AudioSettings) -> SettingsDialog {
    SettingsDialog::new(current, surface_lists()).with_preferences(
        KeymapEditor::new(Os::Windows, Profile::Standard, UserKeymap::default()),
        WorkflowPrefs::default(),
    )
}

/// The centre of the option labelled `label` in the section `title` of the tab that shows.
fn option_at(d: &SettingsDialog, title: &str, label: &str) -> Point {
    let sections = d.sections();
    let section = sections.iter().find(|s| s.title == title).unwrap();
    let choice = &section
        .options
        .iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("no option {label} in {title}"))
        .1;
    d.layout(VIEW)
        .options
        .iter()
        .find(|(_, _, c)| c == choice)
        .unwrap()
        .0
        .center()
}

fn pick(d: &mut SettingsDialog, title: &str, label: &str) {
    let at = option_at(d, title, label);
    down(d, at);
}

#[test]
fn the_surfaces_tab_offers_a_driver_and_the_two_ports() {
    let mut d = surfaces_dialog(AudioSettings::default());
    goto(&mut d, Tab::Surfaces);
    let titles: Vec<&str> = d.sections().iter().map(|s| s.title).collect();
    assert_eq!(
        titles,
        [
            "Control surface",
            "Surface MIDI input",
            "Surface MIDI output"
        ]
    );
    let sections = d.sections();
    let labels =
        |i: usize| -> Vec<String> { sections[i].options.iter().map(|(l, _)| l.clone()).collect() };
    assert_eq!(labels(0), ["None", "Control surface (MCU-compatible)"]);
    assert_eq!(labels(1), ["None", "Example Surface", "Example Keyboard"]);
    assert_eq!(labels(2), ["None", "Example Surface"]);
    // Nothing is chosen by default.
    for (_, _, c) in &d.layout(VIEW).options {
        assert_eq!(
            d.is_selected(c),
            matches!(
                c,
                Choice::SurfaceDriver(None)
                    | Choice::SurfaceInput(None)
                    | Choice::SurfaceOutput(None)
            )
        );
    }
    // The tab has no audio sections and no tab-strip overlap with the panel.
    assert!(d.layout(VIEW).panel.contains(d.layout(VIEW).apply.center()));
}

#[test]
fn choosing_a_surface_is_applied_with_the_settings() {
    let mut d = surfaces_dialog(AudioSettings::default());
    goto(&mut d, Tab::Surfaces);
    pick(
        &mut d,
        "Control surface",
        "Control surface (MCU-compatible)",
    );
    pick(&mut d, "Surface MIDI input", "Example Surface");
    pick(&mut d, "Surface MIDI output", "Example Surface");
    let apply = d.layout(VIEW).apply.center();
    let DialogOutcome::Apply(s) = down(&mut d, apply) else {
        panic!("apply");
    };
    assert_eq!(s.surface.driver.as_deref(), Some("mcu"));
    assert_eq!(s.surface.input.as_deref(), Some("Example Surface"));
    assert_eq!(s.surface.output.as_deref(), Some("Example Surface"));
    // Choosing None again clears one.
    let mut d = surfaces_dialog(s);
    goto(&mut d, Tab::Surfaces);
    pick(&mut d, "Surface MIDI output", "None");
    assert_eq!(d.draft().surface.output, None);
    assert_eq!(d.draft().surface.input.as_deref(), Some("Example Surface"));
}

#[test]
fn a_saved_surface_that_is_not_there_stays_selectable() {
    let current = AudioSettings {
        surface: crate::settings::SurfaceSettings {
            driver: Some("legacy".into()),
            input: Some("Old Desk".into()),
            output: Some("Old Desk".into()),
        },
        ..AudioSettings::default()
    };
    let mut d = surfaces_dialog(current);
    goto(&mut d, Tab::Surfaces);
    let sections = d.sections();
    let last = |i: usize| sections[i].options.last().unwrap().0.clone();
    assert_eq!(last(0), "legacy (unknown driver)");
    assert_eq!(last(1), "Old Desk (not found)");
    assert_eq!(last(2), "Old Desk (not found)");
    for (_, _, c) in &d.layout(VIEW).options {
        if matches!(c, Choice::SurfaceDriver(Some(n)) | Choice::SurfaceInput(Some(n)) | Choice::SurfaceOutput(Some(n)) if n == "legacy" || n == "Old Desk")
        {
            assert!(d.is_selected(c));
        }
    }
}

#[test]
fn the_other_tabs_do_not_show_the_surface_sections_and_back_to_audio_shows_the_audio_ones() {
    let mut d = surfaces_dialog(AudioSettings::default());
    assert!(d.sections().iter().all(|s| s.title != "Control surface"));
    goto(&mut d, Tab::Surfaces);
    assert!(d.sections().iter().all(|s| s.title != "Audio host"));
    goto(&mut d, Tab::Audio);
    assert!(d.sections().iter().any(|s| s.title == "Audio host"));
    let mut list = DisplayList::new();
    goto(&mut d, Tab::Surfaces);
    d.paint(&mut list, &Theme::dark(), VIEW);
    let texts = list.texts();
    for t in [
        "Control surfaces",
        "Surfaces",
        "Surface MIDI input",
        "Example Surface",
    ] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
}

#[test]
fn only_the_roles_a_gesture_reads_are_listed() {
    let mut d = dialog();
    goto(&mut d, Tab::Keyboard);
    scroll_to_end(&mut d);
    let listed: Vec<Role> = kb(&d)
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Role { role, .. } => Some(*role),
            _ => None,
        })
        .collect();
    let expected: Vec<Role> = Role::listed().collect();
    assert_eq!(listed, expected);
    assert!(!listed.contains(&Role::Constrain));
    assert!(listed.contains(&Role::FineAdjust) && listed.contains(&Role::CopyDrag));
}

#[test]
fn a_remap_of_an_unlisted_role_survives_a_save_and_load() {
    use atomik_ui::keymap::UserKeymap;
    let text = r#"{"version": 1, "roles": {"constrain": "Alt"}}"#;
    let (user, _) = UserKeymap::from_json(text).unwrap();
    assert!(user.roles.contains_key("constrain"));
    let (again, _) = UserKeymap::from_json(&user.to_json()).unwrap();
    assert_eq!(again.roles.get("constrain"), user.roles.get("constrain"));
}
