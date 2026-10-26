//! `Shell` quantize tests: Edit > Quantize Options, the panel's settings, APPLY on the selected
//! clips or notes as one undo step, EXTRACT and DELETE of grooves, and the panel closing.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_model::{ClipContent, ClipId, Command, Note, SongCommand};
use atomik_ui::{DisplayList, QuantizeRow};

const S: i64 = atomik_core::PPQ / 4;

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

/// The demo project with its first note clip replaced by loose notes; the clip's id. The notes
/// are 100, 130 and 40 ticks after a line of the song's grid, whatever the clip's origin is.
fn loose(s: &mut Shell<NullBackend>) -> ClipId {
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    let id = s
        .core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .find(|c| c.content.is_notes())
        .map(|c| c.id)
        .expect("a note clip");
    let (_, _, c) = s.core().doc().song.clip(id).unwrap();
    // The content tick of the first grid line at or after one step into the clip.
    let line = S * 8 - (c.start - c.offset).rem_euclid(S);
    let notes = vec![
        Note::new(line + 100, S, 60, 0.8),
        Note::new(line + S + 130, S, 62, 0.8),
        Note::new(line + 3 * S + 40, S, 64, 0.8),
    ];
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(notes),
            merge: None,
        }))
        .unwrap();
    id
}

fn notes(s: &Shell<NullBackend>, id: ClipId) -> Vec<Note> {
    match &s.core().doc().song.clip(id).unwrap().2.content {
        ClipContent::Notes(n) => n.clone(),
        _ => unreachable!(),
    }
}

/// How far each note of the clip is after the grid line before it.
fn off_grid(s: &Shell<NullBackend>, id: ClipId) -> Vec<i64> {
    let (_, _, c) = s.core().doc().song.clip(id).unwrap();
    notes(s, id)
        .iter()
        .map(|n| (c.start - c.offset + n.tick).rem_euclid(S))
        .collect()
}

fn press_apply(s: &mut Shell<NullBackend>) {
    let p = s.quantize_view().unwrap().apply_rect().center();
    click(s, p);
}

fn step(s: &mut Shell<NullBackend>, row: QuantizeRow, plus: bool) {
    let v = s.quantize_view().unwrap();
    let p = if plus {
        v.plus_rect(row).center()
    } else {
        v.minus_rect(row).center()
    };
    click(s, p);
}

fn texts(s: &mut Shell<NullBackend>) -> Vec<String> {
    let mut dl = DisplayList::new();
    s.paint(&mut dl);
    dl.texts().into_iter().map(str::to_owned).collect()
}

#[test]
fn the_edit_menu_offers_quantize_options() {
    let s = shell();
    let item = s
        .menu_model()
        .iter()
        .find(|m| m.title == "Edit")
        .unwrap()
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Item(i) if i.command == AppCommand::QuantizeOptions => Some(i.clone()),
            _ => None,
        })
        .expect("Edit > Quantize Options");
    assert_eq!(item.label, "Quantize Options\u{2026}");
    assert_eq!(
        item.command.command_id(),
        Some(("edit.quantize_options", None))
    );
}

#[test]
fn the_command_opens_the_panel_and_escape_closes_it() {
    let mut s = shell();
    assert!(!s.quantize_open());
    assert!(s.execute(AppCommand::QuantizeOptions).is_empty());
    assert!(s.quantize_open());
    let t = texts(&mut s);
    for want in ["QUANTIZE", "APPLY", "Applies to: nothing selected"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.quantize_open());
}

#[test]
fn the_settings_are_kept_between_openings() {
    let mut s = shell();
    s.execute(AppCommand::QuantizeOptions);
    step(&mut s, QuantizeRow::Swing, true);
    step(&mut s, QuantizeRow::Strength, false);
    let close = s.quantize_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.quantize_open());
    assert_eq!(s.quantize_values().swing, 51);
    s.execute(AppCommand::QuantizeOptions);
    assert_eq!(s.quantize_view().unwrap().values().swing, 51);
    assert_eq!(s.quantize_view().unwrap().values().strength, 95);
}

#[test]
fn apply_with_nothing_selected_changes_nothing() {
    let mut s = shell();
    let id = loose(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    let before = notes(&s, id);
    s.execute(AppCommand::QuantizeOptions);
    s.frame();
    press_apply(&mut s);
    assert_eq!(notes(&s, id), before);
}

#[test]
fn apply_quantizes_the_selected_clips_as_one_undo_step() {
    let mut s = shell();
    let id = loose(&mut s);
    assert_ne!(off_grid(&s, id), [0, 0, 0]);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.execute(AppCommand::QuantizeOptions);
    assert_eq!(s.quantize_view().unwrap().params().grid, S, "1/16 to begin");
    assert!(
        texts(&mut s)
            .iter()
            .any(|x| x == "Applies to: 1 selected clip")
    );
    press_apply(&mut s);
    assert_eq!(off_grid(&s, id), [0, 0, 0]);
    assert!(s.core().last_rejection().is_none());
    // Undo restores the loose notes in one step.
    s.core_mut().undo();
    assert_ne!(off_grid(&s, id), [0, 0, 0]);
    // A built-in groove from the picker: the straight one lands on the lines too.
    step(&mut s, QuantizeRow::Groove, true);
    assert_eq!(
        s.quantize_view().unwrap().values().groove.as_deref(),
        Some("Straight")
    );
    press_apply(&mut s);
    assert_eq!(off_grid(&s, id), [0, 0, 0]);
}

#[test]
fn apply_in_the_piano_roll_works_on_the_selected_notes_only() {
    let mut s = shell();
    let id = loose(&mut s);
    s.core_mut().open_piano_roll(id);
    s.set_focus(Pane::PianoRoll);
    let before = off_grid(&s, id);
    s.core_mut().piano_roll_mut().set_selected_notes(&[1]);
    s.execute(AppCommand::QuantizeOptions);
    assert!(
        texts(&mut s)
            .iter()
            .any(|x| x == "Applies to: 1 selected note")
    );
    press_apply(&mut s);
    assert_eq!(off_grid(&s, id), [before[0], 0, before[2]]);
    // With no selection, every note of the open clip.
    s.core_mut().piano_roll_mut().set_selected_notes(&[]);
    s.frame();
    press_apply(&mut s);
    assert_eq!(off_grid(&s, id), [0, 0, 0]);
    // Already on the grid: the model says so.
    press_apply(&mut s);
    assert!(s.core().last_rejection().is_some());
}

#[test]
fn extract_adds_a_groove_chooses_it_and_delete_removes_it() {
    let mut s = shell();
    let id = loose(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.execute(AppCommand::QuantizeOptions);
    let p = s.quantize_view().unwrap().extract_rect().center();
    click(&mut s, p);
    let song = &s.core().doc().song;
    assert_eq!(song.grooves.len(), 1);
    assert_eq!(song.grooves[0].name, "Groove 1");
    assert_eq!(
        s.quantize_view().unwrap().values().groove.as_deref(),
        Some("Groove 1"),
        "the new groove is chosen"
    );
    assert_eq!(s.quantize_view().unwrap().library().len(), 1);
    // A second one gets the next name.
    click(&mut s, p);
    assert_eq!(s.core().doc().song.grooves[1].name, "Groove 2");
    let del = s.quantize_view().unwrap().delete_rect(0).unwrap().center();
    click(&mut s, del);
    let names: Vec<_> = s
        .core()
        .doc()
        .song
        .grooves
        .iter()
        .map(|g| g.name.as_str())
        .collect();
    assert_eq!(names, ["Groove 2"]);
    s.core_mut().undo();
    s.frame();
    assert_eq!(s.core().doc().song.grooves.len(), 2);
    assert_eq!(s.quantize_view().map(|v| v.library().len()), Some(2));
}

#[test]
fn extract_without_a_single_note_clip_says_so() {
    let mut s = shell();
    let _ = loose(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.execute(AppCommand::QuantizeOptions);
    s.frame();
    let p = s.quantize_view().unwrap().extract_rect().center();
    click(&mut s, p);
    assert!(s.core().doc().song.grooves.is_empty());
}

#[test]
fn the_registry_command_reaches_the_panel() {
    let mut s = shell();
    let taken = s.run_command("edit.quantize_options", None);
    assert!(taken.is_some());
    assert!(s.quantize_open());
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    let (key, modifiers) = atomik_ui::keymap::script::event_for(Os::Windows, chord).unwrap();
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: key.clone(),
            modifiers,
            pressed,
        });
    }
}

/// Opens the panel, steps the grid down to 1/8 and closes it: the last settings are 1/8.
fn last_settings_are_an_eighth(s: &mut Shell<NullBackend>) {
    s.execute(AppCommand::QuantizeOptions);
    step(s, QuantizeRow::Grid, false);
    step(s, QuantizeRow::Grid, false);
    let close = s.quantize_view().unwrap().close_rect().center();
    click(s, close);
    assert!(!s.quantize_open());
    assert_eq!(s.quantize_values().params().grid, 2 * S);
}

/// How far each note is after the 1/8 line before it.
fn off_eighth(s: &Shell<NullBackend>, id: ClipId) -> Vec<i64> {
    let (_, _, c) = s.core().doc().song.clip(id).unwrap();
    notes(s, id)
        .iter()
        .map(|n| (c.start - c.offset + n.tick).rem_euclid(2 * S))
        .collect()
}

#[test]
fn ctrl_k_quantizes_the_selected_clips_with_the_last_settings() {
    let mut s = shell();
    let id = loose(&mut s);
    last_settings_are_an_eighth(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    tap(&mut s, "Cmd+K");
    assert_eq!(
        off_eighth(&s, id),
        [0, 0, 0],
        "the panel's grid, not the snap"
    );
    assert!(!s.quantize_open(), "no panel for the key");
    // One undo step.
    s.core_mut().undo();
    assert_ne!(off_eighth(&s, id), [0, 0, 0]);
}

#[test]
fn ctrl_k_in_the_roll_quantizes_the_selected_notes_with_the_last_settings() {
    let mut s = shell();
    let id = loose(&mut s);
    last_settings_are_an_eighth(&mut s);
    s.core_mut().open_piano_roll(id);
    s.set_focus(Pane::PianoRoll);
    let before = off_eighth(&s, id);
    s.core_mut().piano_roll_mut().set_selected_notes(&[2]);
    tap(&mut s, "Cmd+K");
    assert_eq!(off_eighth(&s, id), [before[0], before[1], 0]);
    // Nothing selected: every note of the clip.
    s.core_mut().piano_roll_mut().set_selected_notes(&[]);
    tap(&mut s, "Cmd+K");
    assert_eq!(off_eighth(&s, id), [0, 0, 0]);
}

#[test]
fn the_rolls_quant_button_applies_and_its_neighbour_opens_the_panel() {
    let mut s = shell();
    let id = loose(&mut s);
    last_settings_are_an_eighth(&mut s);
    s.core_mut().open_piano_roll(id);
    s.set_focus(Pane::PianoRoll);
    s.frame();
    let q = s.core().piano_roll().quantize_button_center();
    click(&mut s, q);
    assert_eq!(off_eighth(&s, id), [0, 0, 0]);
    assert!(!s.quantize_open());
    // The options button opens the panel with the same settings.
    let o = s.core().piano_roll().quantize_options_button_center();
    click(&mut s, o);
    assert!(s.quantize_open());
    assert_eq!(s.quantize_view().unwrap().params().grid, 2 * S);
}

#[test]
fn a_collision_keeps_both_notes() {
    // Two notes of one key that quantize onto the same tick both stay (the later one moves on).
    let mut s = shell();
    let id = loose(&mut s);
    let (_, _, c) = s.core().doc().song.clip(id).unwrap();
    let line = S * 8 - (c.start - c.offset).rem_euclid(S);
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(vec![
                Note::new(line + 100, S, 60, 0.8),
                Note::new(line + 200, S, 60, 0.8),
            ]),
            merge: None,
        }))
        .unwrap();
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    tap(&mut s, "Cmd+K");
    let n = notes(&s, id);
    assert_eq!(n.len(), 2, "no note is lost");
    assert_ne!(n[0].tick, n[1].tick);
}

#[test]
fn the_edit_menu_quantize_applies_the_last_settings_to_the_selection() {
    let mut s = shell();
    let id = loose(&mut s);
    let find = |s: &Shell<NullBackend>| {
        s.menu_model()
            .iter()
            .find(|m| m.title == "Edit")
            .unwrap()
            .items()
            .find(|i| i.label == "Quantize")
            .cloned()
            .expect("Edit > Quantize")
    };
    let item = find(&s);
    assert_eq!(item.command, AppCommand::QuantizeApply);
    assert_eq!(item.command.command_id(), Some(("editor.quantize", None)));
    assert_eq!(item.shortcut.as_deref(), Some("Ctrl+K"));
    // Like Cut and Copy, it needs a selection.
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    assert!(!find(&s).enabled);
    last_settings_are_an_eighth(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    assert!(find(&s).enabled);
    assert!(s.execute(AppCommand::QuantizeApply).is_empty());
    assert_eq!(off_eighth(&s, id), [0, 0, 0]);
    assert!(!s.quantize_open(), "no panel for the menu item");
}

#[test]
fn dragging_a_value_in_the_panel_steps_it_and_leaves_the_undo_history_alone() {
    let mut s = shell();
    s.execute(AppCommand::QuantizeOptions);
    let grid = s.quantize_view().unwrap().values().grid;
    let p = s
        .quantize_view()
        .unwrap()
        .value_rect(QuantizeRow::Grid)
        .center();
    let at = |dy: f64| atomik_ui::kurbo::Point::new(p.x, p.y + dy);
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerMove {
        pos: at(-10.0),
        modifiers: Modifiers::NONE,
    });
    s.handle(&UiEvent::PointerUp {
        pos: at(-10.0),
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.quantize_view().unwrap().values().grid, grid + 2);
    assert!(
        !s.core().document().can_undo(),
        "the panel's settings are not the song"
    );
    assert_eq!(s.quantize_values().grid, grid + 2, "and the app keeps them");
}
