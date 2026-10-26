//! The context menu stays cheap with a big clip open behind it: opening it, moving over it and
//! entering a submenu repaint only when something changed.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, Note, NoteSnapshot, WorkflowCommand};
use atomik_ui::PointerButton;
use atomik_ui::display_list::DisplayList;

fn big_roll(notes: usize) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    s.execute(AppCommand::TogglePianoRoll);
    s.set_focus(Pane::PianoRoll);
    let clip = s.core().piano_roll_clip().expect("a note clip");
    let all: Vec<Note> = (0..notes as i64)
        .map(|i| Note::new((i / 100) * 10, 10, (i % 100) as u8 + 10, 0.8))
        .collect();
    let span = all.last().map_or(1, |n| n.tick + n.length);
    s.core_mut()
        .apply_command(Command::Workflow(WorkflowCommand::PasteNotes {
            clip,
            at_tick: 0,
            notes: NoteSnapshot { notes: all, span },
        }))
        .unwrap();
    s
}

fn paint(s: &mut Shell<NullBackend>) {
    s.paint(&mut DisplayList::new());
}

fn mv(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: at,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn hovering_a_context_menu_repaints_only_when_the_menu_changes() {
    let mut s = big_roll(5000);
    assert!(s.core().piano_roll().notes().len() >= 5000);
    let roll = s.core().piano_roll();
    let spot = Point::new(roll.bounds().x1 - 40.0, roll.ruler_y() + 60.0);
    paint(&mut s);
    for ev in [
        UiEvent::PointerDown {
            pos: spot,
            button: PointerButton::Secondary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: spot,
            button: PointerButton::Secondary,
            modifiers: Modifiers::NONE,
        },
    ] {
        s.handle(&ev);
    }
    assert!(s.context_menu().is_some());
    paint(&mut s);
    // Hover along the first panel: another move inside the same row must not repaint.
    let rects = s.context_menu().unwrap().row_rects();
    let a = rects[0].center();
    mv(&mut s, a);
    paint(&mut s);
    mv(&mut s, Point::new(a.x + 1.0, a.y));
    assert!(!s.needs_repaint(), "a move inside the same row repaints");
    let mut moves = 0;
    let mut repaints = 0;
    for i in 0..200 {
        let p = Point::new(a.x + 2.0 + (i % 5) as f64, a.y);
        mv(&mut s, p);
        moves += 1;
        if s.needs_repaint() {
            repaints += 1;
            paint(&mut s);
        }
    }
    assert_eq!(
        repaints, 0,
        "{repaints} of {moves} moves inside one row repainted"
    );
}

#[test]
fn a_hover_that_changes_the_highlight_or_opens_a_submenu_repaints() {
    let mut s = big_roll(200);
    let roll = s.core().piano_roll();
    let spot = Point::new(roll.bounds().x1 - 40.0, roll.ruler_y() + 60.0);
    for pressed in [true, false] {
        let ev = if pressed {
            UiEvent::PointerDown {
                pos: spot,
                button: PointerButton::Secondary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos: spot,
                button: PointerButton::Secondary,
                modifiers: Modifiers::NONE,
            }
        };
        s.handle(&ev);
    }
    paint(&mut s);
    let rects = s.context_menu().unwrap().row_rects();
    let rows: Vec<Rect> = rects.into_iter().filter(|r| r.height() > 10.0).collect();
    mv(&mut s, rows[0].center());
    assert!(s.needs_repaint(), "a new highlight repaints");
    paint(&mut s);
    mv(&mut s, rows[1].center());
    assert!(s.needs_repaint(), "moving to the next row repaints");
    paint(&mut s);
    let sub = s.context_menu().unwrap().submenu_center("Chords").unwrap();
    mv(&mut s, sub);
    assert!(s.needs_repaint());
    paint(&mut s);
    assert_eq!(s.context_menu().unwrap().open_path(), ["Chords"]);
}
