//! The Chord Sheet pane in the app core: opening and closing with the other editors, the pane's
//! commands from any focus, a sheet imported from a file showing in the pane, and every edit of the
//! pane being one undo step on the song's sheets.

use super::tests::rack_only_core;
use super::*;
use crate::sink::RecordingSink;
use atomik_tab::sheet::parse_sheet;
use atomik_ui::{Key, Modifiers, PointerButton, SheetButton, UiEvent};

fn sheet(text: &str) -> atomik_tab::sheet::ChordSheet {
    parse_sheet(text).expect("a made-up sheet reads").sheet
}

const LANTERN: &str = "title: Lantern Road\nkey: G\n\n[Verse]\nG       D\nThe lantern swings\nEm      C\nalong the road\n";

/// A core whose song holds one sheet, with the pane open.
fn core_with_sheet() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.apply_command(Command::Song(SongCommand::SetSheets(vec![sheet(LANTERN)])))
        .unwrap();
    c.open_chord_sheet();
    c
}

/// Sends an event to the pane (the core applies the actions it returns).
fn run(c: &mut AppCore<RecordingSink>, event: UiEvent) {
    c.handle_pane_event(Pane::ChordSheet, &event);
}

fn click(c: &mut AppCore<RecordingSink>, b: SheetButton) {
    let p = c.chord_sheet().button_center(b).expect("a button");
    run(
        c,
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
    );
}

fn press_key(c: &mut AppCore<RecordingSink>, key: Key) {
    run(
        c,
        UiEvent::Key {
            key,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
    );
}

#[test]
fn opening_the_pane_closes_the_other_editors_and_they_close_it() {
    let mut c = core_with_sheet();
    assert!(c.chord_sheet_open());
    assert_eq!(c.chord_sheet().sheets().len(), 1, "the pane reads the song");
    c.close_chord_sheet();
    assert!(!c.chord_sheet_open());
    c.open_chord_sheet();
    assert!(c.close_open_editor());
    assert!(!c.chord_sheet_open());
    assert!(!c.close_open_editor(), "nothing else is open");
}

#[test]
fn closing_the_pane_drops_a_text_being_edited() {
    let mut c = core_with_sheet();
    click(&mut c, SheetButton::Edit);
    assert!(c.chord_sheet().editing() && c.chord_sheet().is_text_owner());
    c.close_chord_sheet();
    assert!(!c.chord_sheet().editing() && !c.chord_sheet().is_text_owner());
}

#[test]
fn the_panes_commands_reach_it_from_any_focus_and_only_while_it_is_open() {
    let mut c = core_with_sheet();
    // The focus is elsewhere (the rack): the command still runs in the pane.
    assert!(
        c.pane_command(Pane::Rack, "editor.sheet_transpose", Some("up"))
            .is_some()
    );
    assert_eq!(c.chord_sheet().transpose_semitones(), 1);
    c.pane_command(Pane::Arrangement, "editor.sheet_capo", Some("up"))
        .unwrap();
    assert_eq!(c.chord_sheet().capo_shown(), 1);
    c.close_chord_sheet();
    assert!(
        c.pane_command(Pane::Rack, "editor.sheet_transpose", Some("up"))
            .is_none(),
        "a closed pane takes no commands"
    );
}

#[test]
fn a_command_that_cannot_run_is_reported_and_counts_as_taken() {
    let mut c = rack_only_core();
    c.open_chord_sheet();
    // No sheet in the song: transposing says so.
    assert!(
        c.pane_command(Pane::Rack, "editor.sheet_transpose", Some("up"))
            .is_some()
    );
    assert!(c.last_rejection().unwrap().contains("no chord sheet"));
}

#[test]
fn apply_transpose_is_one_undo_step_on_the_songs_sheets() {
    let mut c = core_with_sheet();
    let before = c.doc().song.sheets.clone();
    for _ in 0..2 {
        c.pane_command(Pane::Rack, "editor.sheet_transpose", Some("up"))
            .unwrap();
    }
    // The view only shows it until it is applied.
    assert_eq!(c.doc().song.sheets, before);
    c.pane_command(Pane::Rack, "editor.sheet_apply_transpose", None)
        .unwrap();
    let names: Vec<String> = c.doc().song.sheets[0]
        .chords_used()
        .iter()
        .map(|x| x.name())
        .collect();
    assert_eq!(names, vec!["A", "E", "F#m", "D"]);
    assert_eq!(c.chord_sheet().transpose_semitones(), 0);
    assert!(c.undo());
    assert_eq!(c.doc().song.sheets, before);
}

#[test]
fn new_rename_and_delete_each_make_one_undo_step() {
    let mut c = core_with_sheet();
    click(&mut c, SheetButton::New);
    assert_eq!(c.doc().song.sheets.len(), 2);
    assert_eq!(c.chord_sheet().selected(), 1);
    assert!(
        c.chord_sheet().editing(),
        "the new sheet opens in the editor"
    );
    // Leave the editor, rename through the pane's box.
    c.chord_sheet_mut().leave_text();
    c.pane_command(Pane::Rack, "editor.sheet_rename", None)
        .unwrap();
    assert!(c.chord_sheet().renaming());
    press_key(&mut c, Key::Char('X'));
    press_key(&mut c, Key::Enter);
    assert!(c.doc().song.sheets[1].title.ends_with('X'));
    c.pane_command(Pane::Rack, "editor.sheet_delete", None)
        .unwrap();
    assert_eq!(c.doc().song.sheets.len(), 1);
    // New, rename and delete are three steps, undone one at a time.
    assert!(c.undo());
    assert_eq!(c.doc().song.sheets.len(), 2);
    assert!(c.undo());
    assert!(!c.doc().song.sheets[1].title.ends_with('X'));
    assert!(c.undo());
    assert_eq!(c.doc().song.sheets.len(), 1);
}

#[test]
fn a_sheet_imported_from_a_file_shows_in_the_open_pane() {
    let dir = std::env::temp_dir().join(format!("atomik-app-sheet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tune.cho");
    std::fs::write(&path, "{title: Made Up}\n[C]Hello [G]there\n").unwrap();
    let mut c = core_with_sheet();
    c.import_sheet(&path);
    assert_eq!(c.doc().song.sheets.len(), 2);
    // The pane shows what came in (the view syncs after the command).
    assert_eq!(c.chord_sheet().sheets().len(), 2);
    assert_eq!(c.chord_sheet().selected(), 1);
    assert_eq!(c.chord_sheet().chord_names(), vec!["C", "G"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_speed_autoscroll_runs_on_the_frame_clock_only_while_the_pane_is_open() {
    let mut c = rack_only_core();
    let mut long = String::from("title: Long\n\n");
    for i in 0..80 {
        long.push_str(&format!("Am      F\nline {i} of the made-up song\n"));
    }
    c.apply_command(Command::Song(SongCommand::SetSheets(vec![sheet(&long)])))
        .unwrap();
    c.open_chord_sheet();
    c.chord_sheet_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 900.0, 400.0));
    click(&mut c, SheetButton::AutoToggle);
    c.chord_sheet_tick(1.0);
    c.chord_sheet_tick(1.25);
    assert!(c.chord_sheet().scroll() > 0.0);
    let at = c.chord_sheet().scroll();
    c.close_chord_sheet();
    c.chord_sheet_tick(1.5);
    assert_eq!(
        c.chord_sheet().scroll(),
        at,
        "a closed pane does not scroll"
    );
}

#[test]
fn the_pane_follows_the_transport_through_the_core() {
    let mut c = rack_only_core();
    // Lines linked to bars: the core hands the playhead and the play state to the pane.
    let mut s = atomik_tab::sheet::ChordSheet::default();
    for i in 0..60u32 {
        s.blocks.push(atomik_tab::sheet::SheetBlock::Line {
            text: format!("line {i} of the made-up song"),
            chords: vec![(0, atomik_tab::ChordSymbol::parse("Am").unwrap())],
            bar: Some(i + 1),
        });
    }
    c.apply_command(Command::Song(SongCommand::SetSheets(vec![s])))
        .unwrap();
    c.open_chord_sheet();
    c.chord_sheet_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 900.0, 300.0));
    assert!(c.chord_sheet().synced());
    // Playing at bar 40 puts the text far down.
    c.chord_sheet_mut().set_playing(true);
    c.chord_sheet_mut().set_playhead(39 * 4 * atomik_core::PPQ);
    assert!(c.chord_sheet().scroll() > 100.0);
}
