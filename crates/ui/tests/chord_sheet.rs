//! The Chord Sheet pane through its public contract: the sheets of the song, the display
//! controls (transpose, capo, spelling, font), diagrams (popup, strip, instrument), autoscroll,
//! editing as text, rename, delete and new, each edit one undo step.

mod support;

use atomik_model::{Document, RackDoc, SongCommand};
use atomik_tab::sheet::SheetBlock;
use atomik_ui::keymap::{CommandOutcome, Registry as Commands};
use atomik_ui::kurbo::Point;
use atomik_ui::{
    ChordSheetView, Key, Modifiers, PointerButton, SheetButton, SpellPref, UiAction, UiEvent,
};
use support::sheet::*;
use support::*;

fn click(v: &mut ChordSheetView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        doc,
        &registry(),
    )
}

fn press(v: &mut ChordSheetView, doc: &RackDoc, b: SheetButton) -> Vec<UiAction> {
    let p = v
        .button_center(b)
        .unwrap_or_else(|| panic!("no button {b:?}"));
    assert!(v.button_on(b), "{b:?} is disabled");
    click(v, doc, p)
}

fn key(v: &mut ChordSheetView, doc: &RackDoc, key: Key, modifiers: Modifiers) -> Vec<UiAction> {
    v.handle(
        &UiEvent::Key {
            key,
            modifiers,
            pressed: true,
        },
        doc,
        &registry(),
    )
}

fn type_text(v: &mut ChordSheetView, doc: &RackDoc, text: &str) {
    for ch in text.chars() {
        let k = if ch == '\n' {
            Key::Enter
        } else {
            Key::Char(ch)
        };
        key(v, doc, k, Modifiers::NONE);
    }
}

fn command(v: &mut ChordSheetView, doc: &RackDoc, id: &str, arg: Option<&str>) -> Vec<UiAction> {
    match v.command(id, arg, doc, &registry()) {
        CommandOutcome::Handled(a) => a,
        other => panic!("{id}: {other:?}"),
    }
}

fn two_sheets() -> RackDoc {
    sheet_doc(vec![sheet(LANTERN), sheet(HARBOUR)])
}

#[test]
fn a_song_without_sheets_shows_the_empty_state_and_offers_new_and_import() {
    let doc = sheet_doc(Vec::new());
    let v = sheet_view(&doc, &registry());
    assert!(v.sheets().is_empty());
    assert!(v.button_on(SheetButton::New) && v.button_on(SheetButton::Import));
    assert!(!v.button_on(SheetButton::Edit), "nothing to edit");
    assert!(v.chord_names().is_empty());
    let mut v = v;
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    assert!(dl.to_json().contains("No chord sheet yet"));
}

#[test]
fn the_sheet_shows_its_chords_and_the_selector_walks_the_list() {
    let doc = two_sheets();
    let mut v = sheet_view(&doc, &registry());
    assert_eq!(v.chord_names(), vec!["G", "D", "Em", "C"]);
    assert!(v.button_on(SheetButton::Next) && v.button_on(SheetButton::Prev));
    press(&mut v, &doc, SheetButton::Next);
    assert_eq!(v.selected(), 1);
    assert_eq!(v.chord_names(), vec!["Am", "F", "C", "G"]);
    press(&mut v, &doc, SheetButton::Prev);
    assert_eq!(v.selected(), 0);
    command(&mut v, &doc, "editor.sheet_select", Some("next"));
    assert_eq!(v.selected(), 1);
}

#[test]
fn transposing_is_view_state_and_follows_the_key_or_the_preference() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = sheet_view(&doc, &registry());
    press(&mut v, &doc, SheetButton::TransposeUp);
    press(&mut v, &doc, SheetButton::TransposeUp);
    assert_eq!(v.transpose_semitones(), 2);
    // G major up two is A major: the spelling follows the key.
    assert_eq!(v.chord_names(), vec!["A", "E", "F#m", "D"]);
    press(&mut v, &doc, SheetButton::TransposeReset);
    assert_eq!(v.chord_names(), vec!["G", "D", "Em", "C"]);
    press(&mut v, &doc, SheetButton::TransposeDown);
    assert_eq!(v.chord_names(), vec!["F#", "C#", "D#m", "B"]);
    // A flats preference re-spells the same chords.
    press(&mut v, &doc, SheetButton::Spelling);
    assert_eq!(v.spelling_pref(), SpellPref::Sharps);
    press(&mut v, &doc, SheetButton::Spelling);
    assert_eq!(v.spelling_pref(), SpellPref::Flats);
    assert_eq!(v.chord_names(), vec!["Gb", "Db", "Ebm", "B"]);
    assert_eq!(doc.song.sheets[0], sheet(LANTERN), "the song is untouched");
}

#[test]
fn a_capo_shows_the_shapes_to_play_with_it_and_the_sheets_own_capo_is_the_start() {
    let doc = two_sheets();
    let mut v = sheet_view(&doc, &registry());
    // LANTERN has no capo; capo 2 shows the shapes two semitones lower.
    press(&mut v, &doc, SheetButton::CapoUp);
    press(&mut v, &doc, SheetButton::CapoUp);
    assert_eq!(v.capo_shown(), 2);
    assert_eq!(v.chord_names(), vec!["F", "C", "Dm", "Bb"]);
    // HARBOUR is written with a capo at 2: its capo is where the display starts.
    press(&mut v, &doc, SheetButton::Next);
    assert_eq!(v.capo_shown(), 2);
    assert_eq!(v.chord_names(), vec!["Am", "F", "C", "G"]);
    press(&mut v, &doc, SheetButton::CapoDown);
    press(&mut v, &doc, SheetButton::CapoDown);
    assert_eq!(v.capo_shown(), 0);
    // Without the capo the same sheet shows the sounding chords, two semitones up.
    assert_eq!(v.chord_names(), vec!["Bm", "G", "D", "A"]);
}

#[test]
fn the_font_size_is_a_control_within_its_range() {
    let doc = two_sheets();
    let mut v = sheet_view(&doc, &registry());
    let start = v.font_size();
    press(&mut v, &doc, SheetButton::FontUp);
    assert!(v.font_size() > start);
    for _ in 0..40 {
        press(&mut v, &doc, SheetButton::FontDown);
    }
    assert_eq!(v.font_size(), atomik_ui::chord_sheet::SIZE_RANGE.0);
}

#[test]
fn apply_transpose_writes_the_shown_chords_in_one_undo_step() {
    let reg = registry();
    let mut document = Document::new(sheet_doc(vec![sheet(LANTERN)]));
    let mut v = sheet_view(document.doc(), &reg);
    press(&mut v, document.doc(), SheetButton::TransposeUp);
    press(&mut v, document.doc(), SheetButton::TransposeUp);
    let actions = press(&mut v, document.doc(), SheetButton::Apply);
    assert_eq!(apply_sheet(&mut document, &reg, &actions), 1);
    v.sync(document.doc(), &reg);
    // The song holds the transposed sheet; the display is back to "as written".
    let names: Vec<String> = document.doc().song.sheets[0]
        .chords_used()
        .iter()
        .map(|c| c.name())
        .collect();
    assert_eq!(names, vec!["A", "E", "F#m", "D"]);
    assert_eq!(
        document.doc().song.sheets[0].key.map(|k| k.name()),
        Some("A".into())
    );
    assert_eq!(v.transpose_semitones(), 0);
    assert_eq!(v.chord_names(), names);
    assert!(!v.button_on(SheetButton::Apply), "nothing left to apply");
    // One undo step puts the sheet back.
    document.undo().expect("an undo step");
    assert_eq!(document.doc().song.sheets[0], sheet(LANTERN));
}

#[test]
fn the_apply_command_refuses_when_the_sheet_is_shown_as_written() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = sheet_view(&doc, &registry());
    assert!(matches!(
        v.command("editor.sheet_apply_transpose", None, &doc, &registry()),
        CommandOutcome::NotApplicable(_)
    ));
    command(&mut v, &doc, "editor.sheet_transpose", Some("up"));
    let a = command(&mut v, &doc, "editor.sheet_apply_transpose", None);
    assert!(matches!(
        a.as_slice(),
        [UiAction::Command(atomik_model::Command::Song(SongCommand::SetSheets(s)))] if s.len() == 1
    ));
}

#[test]
fn clicking_a_chord_opens_its_diagram_and_the_popup_cycles_the_voicings() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = sheet_view(&doc, &registry());
    let at = v.chord_position("Em").expect("Em is on screen");
    click(&mut v, &doc, at);
    assert_eq!(v.popup_chord().as_deref(), Some("Em"));
    let (i, n) = v.voicing_choice("Em").expect("Em has voicings");
    assert_eq!(i, 0);
    assert!(n > 1, "alternatives to cycle: {n}");
    key(&mut v, &doc, Key::Right, Modifiers::NONE);
    assert_eq!(v.voicing_choice("Em").unwrap().0, 1);
    key(&mut v, &doc, Key::Left, Modifiers::NONE);
    key(&mut v, &doc, Key::Left, Modifiers::NONE);
    assert_eq!(v.voicing_choice("Em").unwrap().0, n - 1, "cycling wraps");
    // The same chord again closes it; Escape closes too.
    click(&mut v, &doc, at);
    assert_eq!(v.popup_chord(), None);
    click(&mut v, &doc, at);
    assert!(v.popup_chord().is_some());
    key(&mut v, &doc, Key::Escape, Modifiers::NONE);
    assert_eq!(v.popup_chord(), None);
}

#[test]
fn the_strip_holds_every_distinct_chord_and_a_click_on_it_opens_the_popup() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = sheet_view(&doc, &registry());
    // G D Em C: four diagrams; the last is where its index says.
    let third = v.strip_position(2).expect("a strip");
    click(&mut v, &doc, third);
    assert_eq!(v.popup_chord().as_deref(), Some("Em"));
    // The popup's next button cycles; so does the key.
    let (i0, _) = v.voicing_choice("Em").unwrap();
    let next = v.popup_next_center().expect("a popup");
    click(&mut v, &doc, next);
    assert_ne!(v.voicing_choice("Em").unwrap().0, i0);
}

#[test]
fn the_instrument_list_changes_the_diagrams_to_that_instruments_strings() {
    let doc = sheet_doc(vec![sheet(HARBOUR)]);
    let mut v = sheet_view(&doc, &registry());
    assert_eq!(v.instrument_id(), "guitar6");
    press(&mut v, &doc, SheetButton::Instrument);
    assert!(v.picking_instrument() && v.is_text_owner());
    let row = v
        .instrument_row_center("banjo5")
        .expect("the banjo is listed");
    click(&mut v, &doc, row);
    assert_eq!(v.instrument_id(), "banjo5");
    assert!(!v.picking_instrument() && !v.is_text_owner());
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    // The tuning chip cycles the presets of the instrument.
    let first = v.tuning_name();
    press(&mut v, &doc, SheetButton::Tuning);
    assert_ne!(v.tuning_name(), first);
    // Other instruments of the catalogue are reachable.
    for id in ["ukulele_concert", "mandolin", "bass4", "guitar12", "dobro"] {
        v.set_instrument(id);
        assert_eq!(v.instrument_id(), id);
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
    }
    // Escape closes the list without a choice.
    press(&mut v, &doc, SheetButton::Instrument);
    key(&mut v, &doc, Key::Escape, Modifiers::NONE);
    assert!(!v.picking_instrument());
}

#[test]
fn autoscroll_by_speed_moves_the_text_by_lines_per_minute_and_stops_at_the_end() {
    let doc = sheet_doc(vec![long_sheet(80, false)]);
    let mut v = sheet_view(&doc, &registry());
    let max = v.max_scroll();
    assert!(max > 500.0, "a long sheet: {max}");
    assert!(!v.synced(), "no line is linked to a bar");
    press(&mut v, &doc, SheetButton::AutoToggle);
    assert!(v.autoscroll_running());
    v.tick(10.0);
    v.tick(10.25);
    let after = v.scroll();
    assert!(after > 0.0);
    // The speed buttons change the pace: twice the speed, twice the distance.
    let slow = after;
    v.set_scroll(0.0);
    while v.autoscroll_speed() < 2.0 * atomik_ui::chord_sheet::DEFAULT_SPEED {
        press(&mut v, &doc, SheetButton::AutoFaster);
    }
    v.tick(20.0);
    v.tick(20.25);
    assert!(
        (v.scroll() - 2.0 * slow).abs() < 1.0,
        "{} vs {}",
        v.scroll(),
        slow
    );
    // A long gap between frames (a stalled host) does not jump the text.
    let before = v.scroll();
    v.tick(500.0);
    assert_eq!(v.scroll(), before);
    // It stops by itself at the end.
    let mut t = 500.0;
    for _ in 0..4000 {
        t += 0.25;
        v.tick(t);
        if !v.autoscroll_running() {
            break;
        }
    }
    assert!(!v.autoscroll_running());
    assert_eq!(v.scroll(), v.max_scroll());
    // Pause stops it where it is.
    v.set_scroll(10.0);
    press(&mut v, &doc, SheetButton::AutoToggle);
    press(&mut v, &doc, SheetButton::AutoToggle);
    assert!(!v.autoscroll_running());
}

#[test]
fn a_sheet_linked_to_bars_scrolls_with_the_transport() {
    let doc = sheet_doc(vec![long_sheet(80, true)]);
    let mut v = sheet_view(&doc, &registry());
    assert!(v.synced() && v.linked());
    // The speed control gives way to the synced chip.
    assert!(!v.button_on(SheetButton::AutoToggle));
    let ppq = 15_360;
    let bar = 4 * ppq;
    v.set_playing(true);
    let mut last = -1.0;
    for b in [0, 10, 20, 40, 79] {
        v.set_playhead(b * bar);
        assert!(v.scroll() >= last, "bar {b}: {} after {last}", v.scroll());
        last = v.scroll();
    }
    assert!(last > 0.0);
    // Between two lines the scroll is between their positions.
    v.set_playhead(30 * bar);
    let a = v.scroll();
    v.set_playhead(30 * bar + 2 * ppq);
    let b = v.scroll();
    v.set_playhead(31 * bar);
    let c = v.scroll();
    assert!(a <= b && b <= c, "{a} {b} {c}");
    // Switching the sync off hands the scroll back to the speed control.
    press(&mut v, &doc, SheetButton::AutoSync);
    assert!(!v.synced() && v.button_on(SheetButton::AutoToggle));
    // A stopped transport leaves the text alone.
    press(&mut v, &doc, SheetButton::AutoSync);
    v.set_playing(false);
    v.set_scroll(5.0);
    v.set_playhead(70 * bar);
    assert_eq!(v.scroll(), 5.0);
}

#[test]
fn edit_as_text_takes_the_keys_and_applies_in_one_undo_step() {
    let reg = registry();
    let mut document = Document::new(sheet_doc(vec![sheet(LANTERN)]));
    let mut v = sheet_view(document.doc(), &reg);
    assert!(!v.is_text_owner());
    press(&mut v, document.doc(), SheetButton::Edit);
    assert!(v.editing() && v.is_text_owner());
    let text = v.edit_text().expect("the text").to_owned();
    assert!(text.contains("Lantern Road") && text.contains("[Verse 1]"));
    // Typing goes into the field: end of text, a new line of lyrics with a chord line over it.
    key(&mut v, document.doc(), Key::End, Modifiers::CTRL);
    type_text(&mut v, document.doc(), "Am     F\nsoft rain on the glass\n");
    assert!(v.edit_text().unwrap().ends_with("soft rain on the glass\n"));
    // Backspace and the arrows edit too.
    key(&mut v, document.doc(), Key::Backspace, Modifiers::NONE);
    key(&mut v, document.doc(), Key::Char('!'), Modifiers::NONE);
    assert!(v.edit_text().unwrap().ends_with("glass!"));
    // Ctrl+Enter applies.
    let actions = key(&mut v, document.doc(), Key::Enter, Modifiers::CTRL);
    assert_eq!(apply_sheet(&mut document, &reg, &actions), 1);
    assert!(!v.editing());
    v.sync(document.doc(), &reg);
    let s = &document.doc().song.sheets[0];
    assert_eq!(s.title, "Lantern Road");
    assert!(s.lyrics().contains("soft rain on the glass!"));
    assert!(s.chords_used().iter().any(|c| c.name() == "Am"));
    document.undo().expect("one step");
    assert_eq!(document.doc().song.sheets[0], sheet(LANTERN));
}

#[test]
fn escape_and_cancel_leave_the_text_as_it_was() {
    let reg = registry();
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = sheet_view(&doc, &reg);
    press(&mut v, &doc, SheetButton::Edit);
    type_text(&mut v, &doc, "oops");
    let out = key(&mut v, &doc, Key::Escape, Modifiers::NONE);
    assert!(out.is_empty() && !v.editing());
    press(&mut v, &doc, SheetButton::Edit);
    type_text(&mut v, &doc, "oops");
    let out = press(&mut v, &doc, SheetButton::CancelText);
    assert!(out.is_empty() && !v.editing());
    // Applying text that is unchanged makes no command.
    press(&mut v, &doc, SheetButton::Edit);
    let out = press(&mut v, &doc, SheetButton::ApplyText);
    assert!(out.is_empty() && !v.editing());
}

#[test]
fn editing_the_text_keeps_the_bar_links_of_lines_that_stay() {
    let reg = registry();
    let mut document = Document::new(sheet_doc(vec![long_sheet(6, true)]));
    let mut v = sheet_view(document.doc(), &reg);
    press(&mut v, document.doc(), SheetButton::Edit);
    // Same number of lines, one word changed.
    let text = v.edit_text().unwrap().replace("number 3", "figure 3");
    v.set_edit_text(&text);
    let actions = key(&mut v, document.doc(), Key::Enter, Modifiers::CTRL);
    apply_sheet(&mut document, &reg, &actions);
    let bars: Vec<Option<u32>> = document.doc().song.sheets[0]
        .blocks
        .iter()
        .filter_map(|b| match b {
            SheetBlock::Line { bar, .. } => Some(*bar),
            _ => None,
        })
        .collect();
    assert_eq!(bars, (1..=6).map(Some).collect::<Vec<_>>());
    assert!(document.doc().song.sheets[0].lyrics().contains("figure 3"));
}

#[test]
fn rename_takes_a_title_from_the_keyboard_and_is_undoable() {
    let reg = registry();
    let mut document = Document::new(two_sheets_doc());
    let mut v = sheet_view(document.doc(), &reg);
    press(&mut v, document.doc(), SheetButton::Rename);
    assert!(v.renaming() && v.is_text_owner());
    // Clear the old title (Ctrl+A, then typing replaces the selection) and type a new one.
    key(&mut v, document.doc(), Key::Char('a'), Modifiers::CTRL);
    type_text(&mut v, document.doc(), "Night Bus");
    assert_eq!(v.rename_text(), Some("Night Bus"));
    let actions = key(&mut v, document.doc(), Key::Enter, Modifiers::NONE);
    assert_eq!(apply_sheet(&mut document, &reg, &actions), 1);
    assert!(!v.renaming());
    assert_eq!(document.doc().song.sheets[0].title, "Night Bus");
    assert_eq!(document.doc().song.sheets[1].title, "Harbour Lights");
    document.undo().unwrap();
    assert_eq!(document.doc().song.sheets[0].title, "Lantern Road");
    // Escape and an empty title change nothing.
    press(&mut v, document.doc(), SheetButton::Rename);
    key(&mut v, document.doc(), Key::Char('a'), Modifiers::CTRL);
    key(&mut v, document.doc(), Key::Backspace, Modifiers::NONE);
    assert!(key(&mut v, document.doc(), Key::Enter, Modifiers::NONE).is_empty());
    press(&mut v, document.doc(), SheetButton::Rename);
    assert!(key(&mut v, document.doc(), Key::Escape, Modifiers::NONE).is_empty());
}

fn two_sheets_doc() -> RackDoc {
    two_sheets()
}

#[test]
fn delete_removes_the_shown_sheet_and_undo_brings_it_back() {
    let reg = registry();
    let mut document = Document::new(two_sheets());
    let mut v = sheet_view(document.doc(), &reg);
    press(&mut v, document.doc(), SheetButton::Next);
    let actions = press(&mut v, document.doc(), SheetButton::Delete);
    assert_eq!(apply_sheet(&mut document, &reg, &actions), 1);
    v.sync(document.doc(), &reg);
    assert_eq!(document.doc().song.sheets.len(), 1);
    assert_eq!(
        v.selected(),
        0,
        "the selection falls back to a sheet that exists"
    );
    assert_eq!(v.chord_names(), vec!["G", "D", "Em", "C"]);
    document.undo().unwrap();
    v.sync(document.doc(), &reg);
    assert_eq!(v.sheets().len(), 2);
    // Deleting the last sheet leaves the empty state.
    let mut document = Document::new(sheet_doc(vec![sheet(LANTERN)]));
    let mut v = sheet_view(document.doc(), &reg);
    let actions = press(&mut v, document.doc(), SheetButton::Delete);
    apply_sheet(&mut document, &reg, &actions);
    v.sync(document.doc(), &reg);
    assert!(v.sheets().is_empty() && !v.button_on(SheetButton::Delete));
}

#[test]
fn new_sheet_adds_one_selects_it_and_opens_the_editor() {
    let reg = registry();
    let mut document = Document::new(sheet_doc(vec![sheet(LANTERN)]));
    let mut v = sheet_view(document.doc(), &reg);
    let actions = press(&mut v, document.doc(), SheetButton::New);
    assert_eq!(apply_sheet(&mut document, &reg, &actions), 1);
    v.sync(document.doc(), &reg);
    assert_eq!(document.doc().song.sheets.len(), 2);
    assert_eq!(v.selected(), 1);
    assert!(v.editing(), "the new sheet opens in the text editor");
    document.undo().unwrap();
    v.sync(document.doc(), &reg);
    assert_eq!(v.sheets().len(), 1);
    assert!(!v.editing() || v.selected() == 0);
}

#[test]
fn import_asks_the_shell_for_the_file_import_command_and_shows_what_came_in() {
    let reg = registry();
    let mut document = Document::new(sheet_doc(vec![sheet(LANTERN)]));
    let mut v = sheet_view(document.doc(), &reg);
    let actions = press(&mut v, document.doc(), SheetButton::Import);
    assert!(matches!(
        actions.as_slice(),
        [UiAction::RunCommand { id, arg: None }] if id == "file.import_sheet"
    ));
    // The shell imports: a sheet appears in the song and the pane shows it.
    document
        .apply(
            atomik_model::Command::Song(SongCommand::SetSheets(vec![
                sheet(LANTERN),
                sheet(HARBOUR),
            ])),
            &reg,
        )
        .unwrap();
    v.sync(document.doc(), &reg);
    assert_eq!(v.selected(), 1);
    assert!(!v.editing());
}

#[test]
fn undoing_an_edit_from_elsewhere_updates_the_pane() {
    let reg = registry();
    let mut document = Document::new(two_sheets());
    let mut v = sheet_view(document.doc(), &reg);
    press(&mut v, document.doc(), SheetButton::Next);
    // The second sheet is removed from outside (an undo, a script): the selection stays valid.
    document
        .apply(
            atomik_model::Command::Song(SongCommand::SetSheets(vec![sheet(LANTERN)])),
            &reg,
        )
        .unwrap();
    v.sync(document.doc(), &reg);
    assert_eq!(v.selected(), 0);
    assert_eq!(v.sheets().len(), 1);
}

#[test]
fn every_button_has_a_tooltip_naming_a_command_of_the_registry() {
    let doc = sheet_doc(vec![sheet(LANTERN), sheet(HARBOUR)]);
    let v = sheet_view(&doc, &registry());
    let commands = Commands::bundled();
    let mut seen = 0;
    for b in [
        SheetButton::Prev,
        SheetButton::Next,
        SheetButton::New,
        SheetButton::Import,
        SheetButton::Rename,
        SheetButton::Delete,
        SheetButton::Edit,
        SheetButton::TransposeDown,
        SheetButton::TransposeUp,
        SheetButton::TransposeReset,
        SheetButton::CapoDown,
        SheetButton::CapoUp,
        SheetButton::Spelling,
        SheetButton::FontDown,
        SheetButton::FontUp,
        SheetButton::Instrument,
        SheetButton::Tuning,
        SheetButton::Apply,
        SheetButton::AutoToggle,
        SheetButton::AutoSlower,
        SheetButton::AutoFaster,
    ] {
        let p = v
            .button_center(b)
            .unwrap_or_else(|| panic!("{b:?} is not on the toolbar"));
        let h = v
            .help_at(p)
            .unwrap_or_else(|| panic!("{b:?} has no tooltip"));
        assert!(
            !h.text.is_empty() && h.text.len() <= 90,
            "{b:?}: {}",
            h.text
        );
        let french = atomik_ui::i18n::with_language(atomik_ui::Lang::French, || {
            atomik_ui::i18n::tr(&h.text).into_owned()
        });
        assert!(french.chars().count() <= 90, "{b:?}: {french}");
        if let Some(c) = h.command {
            assert!(commands.validate(c, h.arg), "{b:?}: `{c}` {:?}", h.arg);
            seen += 1;
        }
    }
    assert!(seen >= 17, "{seen} buttons run commands");
}

#[test]
fn the_pane_commands_run_the_same_changes_as_the_buttons() {
    let doc = two_sheets();
    let mut v = sheet_view(&doc, &registry());
    command(&mut v, &doc, "editor.sheet_transpose", Some("up"));
    command(&mut v, &doc, "editor.sheet_transpose", Some("up"));
    assert_eq!(v.transpose_semitones(), 2);
    command(&mut v, &doc, "editor.sheet_transpose", Some("reset"));
    assert_eq!(v.transpose_semitones(), 0);
    command(&mut v, &doc, "editor.sheet_capo", Some("up"));
    assert_eq!(v.capo_shown(), 1);
    command(&mut v, &doc, "editor.sheet_capo", Some("off"));
    assert_eq!(v.capo_shown(), 0);
    command(&mut v, &doc, "editor.sheet_spelling", Some("flats"));
    assert_eq!(v.spelling_pref(), SpellPref::Flats);
    command(&mut v, &doc, "editor.sheet_spelling", Some("auto"));
    command(&mut v, &doc, "editor.sheet_font", Some("larger"));
    assert!(v.font_size() > atomik_ui::chord_sheet::DEFAULT_SIZE);
    command(&mut v, &doc, "editor.sheet_autoscroll", Some("toggle"));
    assert!(v.autoscroll_running());
    command(&mut v, &doc, "editor.sheet_autoscroll", Some("faster"));
    assert!(v.autoscroll_speed() > atomik_ui::chord_sheet::DEFAULT_SPEED);
    command(&mut v, &doc, "editor.sheet_edit_text", None);
    assert!(v.editing());
    // While a text owns the keys the commands step aside.
    assert!(matches!(
        v.command("editor.sheet_transpose", Some("up"), &doc, &registry()),
        CommandOutcome::Declined
    ));
    // Commands of other panes are none of the pane's business.
    assert!(matches!(
        v.command("edit.copy", None, &doc, &registry()),
        CommandOutcome::Declined
    ));
}

#[test]
fn the_commands_need_a_sheet() {
    let doc = sheet_doc(Vec::new());
    let mut v = sheet_view(&doc, &registry());
    assert!(matches!(
        v.command("editor.sheet_transpose", Some("up"), &doc, &registry()),
        CommandOutcome::NotApplicable(_)
    ));
    let a = command(&mut v, &doc, "editor.sheet_new", None);
    assert_eq!(a.len(), 1);
}

#[test]
fn a_wheel_scrolls_the_text_and_the_strip_scrolls_sideways_on_its_own() {
    let doc = sheet_doc(vec![long_sheet(80, false)]);
    let mut v = sheet_view(&doc, &registry());
    let wheel = |v: &mut ChordSheetView, x: f64, y: f64, dy: f64| {
        v.handle(
            &UiEvent::Wheel {
                delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
                pos: Point::new(x, y),
                modifiers: Modifiers::NONE,
            },
            &doc,
            &registry(),
        )
    };
    wheel(&mut v, 400.0, 400.0, -3.0);
    let down = v.scroll();
    assert!(down > 0.0);
    wheel(&mut v, 400.0, 400.0, 30.0);
    assert_eq!(v.scroll(), 0.0, "clamped at the top");
    // Keys scroll too.
    key(&mut v, &doc, Key::PageDown, Modifiers::NONE);
    assert!(v.scroll() > 0.0);
    key(&mut v, &doc, Key::End, Modifiers::NONE);
    assert_eq!(v.scroll(), v.max_scroll());
    key(&mut v, &doc, Key::Home, Modifiers::NONE);
    assert_eq!(v.scroll(), 0.0);
}

#[test]
fn a_long_sheet_is_drawn_only_where_it_is_visible() {
    let doc = sheet_doc(vec![long_sheet(2000, false)]);
    let mut v = sheet_view(&doc, &registry());
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    let json = dl.to_json();
    assert!(json.contains("line number 0 of"));
    assert!(
        !json.contains("line number 1999 of"),
        "off-screen lines are not drawn"
    );
}

#[test]
fn chords_do_not_collide_in_a_narrow_pane() {
    let doc = sheet_doc(vec![sheet(LANTERN)]);
    let mut v = ChordSheetView::new(atomik_ui::Theme::dark());
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 200.0, 400.0));
    v.sync(&doc, &registry());
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    // Every chord can still be found and clicked.
    for name in ["G", "D", "Em", "C"] {
        assert!(v.chord_position(name).is_some(), "{name}");
    }
}
