//! The piano roll's registry commands and tools: the
//! shell resolves a key to a command id and offers it to the focused view.

mod support;

use atomik_model::{ClipContent, ClipId, Note, SongCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{DisplayList, Modifiers, PianoRollView, Tool, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (
    PianoRollView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    (v, doc, reg)
}

fn notes_of(actions: &[UiAction]) -> Vec<Vec<Note>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                id: ClipId(1),
                content: ClipContent::Notes(n),
                ..
            } => Some(n),
            _ => None,
        })
        .collect()
}

fn up_mods(pos: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers,
    }
}

fn click(
    v: &mut PianoRollView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
    m: Modifiers,
) -> Vec<UiAction> {
    run(
        |e| v.handle(e, doc, reg),
        &[down_mods(p, m, 1), up_mods(p, m)],
    )
}

fn off_grid_doc() -> atomik_model::RackDoc {
    let mut doc = song_doc();
    doc.song.tracks[0].lanes[0].clips[0].content = ClipContent::Notes(vec![
        n(100, PPQ, 60, 0.8),
        n(PPQ - 200, PPQ, 62, 0.8),
        n(2 * PPQ + 1000, PPQ, 64, 0.8),
    ]);
    doc
}

// ---------------------------------------------------------------- edit commands

#[test]
fn select_all_delete_and_the_selection_rules() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected"
    );
    assert!(v.command("edit.select_all", None, &doc, &reg).is_taken());
    assert_eq!(v.selected_notes().len(), 16);
    v.set_selected_notes(&[0, 2]);
    let a = v.command("edit.delete", None, &doc, &reg);
    let notes = &notes_of(a.actions())[0];
    assert_eq!(notes.len(), 14);
    assert!(v.selected_notes().is_empty());
}

#[test]
fn quantize_transpose_octave_and_nudge_are_commands() {
    let (doc, reg) = (off_grid_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    v.set_selected_notes(&[0, 1]);
    let a = v.command("editor.quantize", None, &doc, &reg);
    let ticks: Vec<i64> = notes_of(a.actions())[0].iter().map(|n| n.tick).collect();
    assert_eq!(ticks, vec![0, PPQ, 2 * PPQ + 1000]);
    // Nothing selected: everything.
    v.set_selected_notes(&[]);
    let a = v.command("editor.quantize", None, &doc, &reg);
    let ticks: Vec<i64> = notes_of(a.actions())[0].iter().map(|n| n.tick).collect();
    assert_eq!(ticks, vec![0, PPQ, 2 * PPQ]);

    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    assert_eq!(
        v.command("editor.transpose_up", None, &doc, &reg),
        CommandOutcome::Declined,
        "arrows without a selection do nothing"
    );
    v.set_selected_notes(&[0, 2]);
    for (id, d0, d2) in [
        ("editor.transpose_up", 37, 44),
        ("editor.transpose_down", 35, 42),
        ("editor.octave_up", 48, 55),
        ("editor.octave_down", 24, 31),
    ] {
        let a = v.command(id, None, &doc, &reg);
        let notes = &notes_of(a.actions())[0];
        assert_eq!((notes[0].key, notes[2].key), (d0, d2), "{id}");
        assert_eq!(notes[1], bass_line()[1]);
    }
    v.set_snap(Snap::Beat);
    let a = v.command("editor.nudge_right", None, &doc, &reg);
    let notes = &notes_of(a.actions())[0];
    assert_eq!(
        (notes[0].tick, notes[2].tick),
        (PPQ, 3 * PPQ),
        "by the snap step"
    );
    v.set_selected_notes(&[2, 3]);
    let a = v.command("editor.nudge_left", None, &doc, &reg);
    let notes = &notes_of(a.actions())[0];
    assert_eq!(notes.len(), 16);
    assert!(notes.iter().any(|n| n.tick == PPQ && n.key == 43));
    for w in notes.windows(2) {
        assert!((w[0].tick, w[0].key) <= (w[1].tick, w[1].key), "sorted");
    }
}

#[test]
fn nudge_by_beat_and_by_tick_ignore_the_snap_step() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Off);
    assert_eq!(
        v.command("editor.nudge_right_beat", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected: nothing to nudge"
    );
    v.set_selected_notes(&[1, 3]);
    for (id, d) in [
        ("editor.nudge_right_beat", PPQ),
        ("editor.nudge_right_tick", 1),
        ("editor.nudge_left_beat", -PPQ),
        ("editor.nudge_left_tick", -1),
    ] {
        let t0 = bass_line()[1].tick;
        let t2 = bass_line()[3].tick;
        let (e0, e2) = ((t0 + d).max(0), (t2 + d).max(0));
        v.set_selected_notes(&[1, 3]);
        let a = v.command(id, None, &doc, &reg);
        let notes = &notes_of(a.actions())[0];
        assert_eq!(notes.len(), bass_line().len(), "{id}");
        assert!(
            notes
                .iter()
                .any(|n| n.tick == e0 && n.key == bass_line()[1].key),
            "{id}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.tick == e2 && n.key == bass_line()[3].key),
            "{id}"
        );
    }
    // Clamped at zero: the first note is at tick 0, so a tick to the left changes nothing.
    v.set_selected_notes(&[0]);
    let a = v.command("editor.nudge_left_tick", None, &doc, &reg);
    assert!(notes_of(a.actions()).is_empty(), "clamped at 0, no edit");
    // The snap step does not matter.
    v.set_snap(Snap::Bar);
    v.set_selected_notes(&[0]);
    let a = v.command("editor.nudge_right_tick", None, &doc, &reg);
    assert!(notes_of(a.actions())[0].iter().any(|n| n.tick == 1));
}

#[test]
fn commands_that_are_not_the_rolls_decline() {
    let (mut v, doc, reg) = setup();
    for id in [
        "sequencer.split_at_playhead",
        "sequencer.open_clip",
        "edit.duplicate",
        "file.save",
        "sequencer.select_next_clip",
    ] {
        assert_eq!(
            v.command(id, None, &doc, &reg),
            CommandOutcome::Declined,
            "{id}"
        );
    }
}

// ---------------------------------------------------------------- snap, zoom, follow

#[test]
fn snap_toggle_set_snap_and_zoom() {
    let (mut v, doc, reg) = setup();
    v.set_snap(Snap::Beat);
    v.command("sequencer.snap_toggle", None, &doc, &reg);
    assert_eq!(v.snap(), Snap::Off);
    v.command("sequencer.snap_toggle", None, &doc, &reg);
    assert_eq!(v.snap(), Snap::Beat);
    v.command("sequencer.set_snap", Some("bar"), &doc, &reg);
    assert_eq!(v.snap(), Snap::Bar);
    let z = v.px_per_tick();
    v.command("view.zoom_in", None, &doc, &reg);
    assert!(v.px_per_tick() > z);
    v.command("view.zoom_out", None, &doc, &reg);
    assert!((v.px_per_tick() - z).abs() < 1e-12);
}

#[test]
fn zoom_to_selection_fits_the_notes_and_toggles_back_to_the_whole_clip() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[4, 5]);
    v.command("sequencer.zoom_selection", None, &doc, &reg);
    let right = ROLL_BOUNDS.x1;
    let (n4, n5) = (v.note_rect(4).unwrap(), v.note_rect(5).unwrap());
    assert!(n4.x0 >= 56.0 - 1e-6 && n5.x1 <= right + 1e-6);
    assert!(
        n5.x1 - n4.x0 > 0.8 * (right - 56.0),
        "the notes fill the view"
    );
    v.command("sequencer.zoom_selection", None, &doc, &reg);
    let last = v.note_rect(15).unwrap();
    assert!(v.note_rect(0).unwrap().x0 >= 56.0 - 1e-6 && last.x1 <= right + 1e-6);
}

#[test]
fn follow_pages_the_roll_to_the_playhead_while_playing() {
    let (mut v, doc, reg) = setup();
    v.command("sequencer.follow", None, &doc, &reg);
    assert!(v.follow());
    v.set_playing(true);
    for _ in 0..6 {
        v.command("view.zoom_in", None, &doc, &reg);
    }
    // The playhead (song bar 4) is far right of the zoomed view: the roll pages to it.
    assert!(v.tick_to_x(3 * BAR) > ROLL_BOUNDS.x1);
    v.set_playhead(3 * BAR);
    let x = v.tick_to_x(3 * BAR);
    assert!((56.0..=ROLL_BOUNDS.x1).contains(&x), "x {x}");
    v.set_playing(false);
}

// ---------------------------------------------------------------- tools

#[test]
fn tool_commands_and_toolbar_buttons() {
    let (mut v, doc, reg) = setup();
    for (id, tool) in [
        ("tool.razor", Tool::Razor),
        ("tool.eraser", Tool::Eraser),
        ("tool.pencil", Tool::Pencil),
        ("tool.select", Tool::Select),
    ] {
        assert!(v.command(id, None, &doc, &reg).is_taken());
        assert_eq!(v.tool(), tool);
    }
    for tool in [Tool::Razor, Tool::Eraser, Tool::Pencil, Tool::Select] {
        let p = v.tool_button_center(tool);
        let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
        assert!(a.is_empty());
        assert_eq!(v.tool(), tool);
    }
}

#[test]
fn the_alternate_tool_modifier_updates_the_toolbar() {
    let (mut v, doc, reg) = setup();
    v.set_tool(Tool::Eraser);
    v.paint(&mut DisplayList::new(), &doc, &reg);
    assert!(!v.needs_repaint());
    let _ = v.handle(&UiEvent::ModifiersChanged { modifiers: alt() }, &doc, &reg);
    assert_eq!(v.shown_tool(), Tool::Select);
    assert!(v.needs_repaint());
}

#[test]
fn the_razor_splits_the_note_under_the_pointer_at_the_snapped_position() {
    let (mut v, doc, reg) = setup();
    v.set_tool(Tool::Razor);
    let p = Point::new(v.tick_to_x(7000), v.key_center_y(36));
    let a = click(&mut v, &doc, &reg, p, Modifiers::NONE);
    let notes = &notes_of(&a)[0];
    assert_eq!(notes.len(), 17);
    let first = notes.iter().find(|n| n.tick == 0).unwrap();
    assert_eq!((first.key, first.length), (36, 7680));
    let second = notes
        .iter()
        .find(|n| n.tick == 7680 && n.key == 36)
        .unwrap();
    assert_eq!(second.length, 13440 - 7680);
    assert_eq!(second.velocity, first.velocity);
    assert!(v.selected_notes().is_empty());
    // At a note edge nothing splits.
    let edge = Point::new(v.tick_to_x(0) + 1.0, v.key_center_y(36));
    let a = click(&mut v, &doc, &reg, edge, Modifiers::NONE);
    assert!(notes_of(&a).is_empty());
}

#[test]
fn the_eraser_removes_a_note_and_the_pencil_leaves_notes_alone_but_draws_on_the_grid() {
    let (mut v, doc, reg) = setup();
    v.set_tool(Tool::Eraser);
    let on_note = Point::new(v.tick_to_x(7000), v.key_center_y(36));
    let a = click(&mut v, &doc, &reg, on_note, Modifiers::NONE);
    assert_eq!(notes_of(&a)[0].len(), 15);
    let grid = Point::new(v.tick_to_x(4 * PPQ + 10), v.key_center_y(55));
    assert!(click(&mut v, &doc, &reg, grid, Modifiers::NONE).is_empty());
    v.set_tool(Tool::Pencil);
    assert!(click(&mut v, &doc, &reg, on_note, Modifiers::NONE).is_empty());
    assert!(v.selected_notes().is_empty());
    let a = click(&mut v, &doc, &reg, grid, Modifiers::NONE);
    assert!(!notes_of(&a).is_empty(), "the pencil draws");
    // The pencil also draws with Shift held (no band select).
    let a = click(&mut v, &doc, &reg, grid, Modifiers::SHIFT);
    assert!(!notes_of(&a).is_empty());
}

#[test]
fn select_keeps_its_gestures_and_alt_makes_the_alternate_tool() {
    let (mut v, doc, reg) = setup();
    let on_note = Point::new(v.tick_to_x(7000), v.key_center_y(36));
    // Select: a click selects the note, no edit.
    assert!(notes_of(&click(&mut v, &doc, &reg, on_note, Modifiers::NONE)).is_empty());
    assert_eq!(v.selected_notes(), vec![0]);
    // Razor + Alt: the alternate tool is Select, so no split.
    v.set_tool(Tool::Razor);
    assert!(notes_of(&click(&mut v, &doc, &reg, on_note, alt())).is_empty());
    // Eraser + Alt: Select too (no removal).
    v.set_tool(Tool::Eraser);
    assert!(notes_of(&click(&mut v, &doc, &reg, on_note, alt())).is_empty());
    // Select + Alt: the pencil, which leaves a note alone.
    v.set_tool(Tool::Select);
    v.set_selected_notes(&[]);
    assert!(notes_of(&click(&mut v, &doc, &reg, on_note, alt())).is_empty());
    assert!(v.selected_notes().is_empty());
}

#[test]
fn ctrl_alt_disables_snap_and_the_legacy_roles_keep_alt_for_it() {
    let (mut v, doc, reg) = setup();
    v.set_snap(Snap::Beat);
    let p = Point::new(v.tick_to_x(7000), v.key_center_y(36));
    let beat_px = PPQ as f64 * v.px_per_tick();
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let drag = |v: &mut PianoRollView, m: Modifiers| {
        v.set_selected_notes(&[]);
        let to = p + atomik_ui::kurbo::Vec2::new(beat_px * 0.6, 0.0);
        run(
            |e| v.handle(e, &doc, &reg),
            &[down(p), mv_with(to, m), up(to)],
        )
    };
    let snapped = drag(&mut v, Modifiers::NONE);
    assert!(notes_of(&snapped)[0].iter().all(|n| n.tick % PPQ == 0));
    let free = drag(&mut v, ctrl_alt);
    assert!(
        notes_of(&free)[0].iter().any(|n| n.tick % PPQ != 0),
        "Ctrl+Alt: not on the beat"
    );
    // Alt alone is the alternate tool (it does not touch snap) with the default roles ...
    let alt_only = drag(&mut v, alt());
    assert!(notes_of(&alt_only)[0].iter().all(|n| n.tick % PPQ == 0));
    // ... and the legacy roles keep Alt alone as snap off.
    v.set_roles(Roles::legacy(Os::Windows));
    let free = drag(&mut v, alt());
    assert!(notes_of(&free)[0].iter().any(|n| n.tick % PPQ != 0));
}

// ---------------------------------------------------------------- clipboard commands

#[test]
fn duplicate_copies_the_selected_notes_in_one_command() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected"
    );
    v.set_selected_notes(&[3, 2]);
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(atomik_model::Command::Workflow(
            atomik_model::WorkflowCommand::DuplicateNotes {
                clip: ClipId(1),
                indices: vec![2, 3]
            }
        ))])
    );
}

#[test]
fn right_click_on_the_grid_asks_for_the_notes_menu() {
    let (mut v, doc, reg) = setup();
    // An empty spot: above every note of the bass line, near the top of the grid.
    let at = Point::new(v.tick_to_x(2 * BAR), 150.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            UiEvent::PointerDown {
                pos: at,
                button: atomik_ui::PointerButton::Secondary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            },
            UiEvent::PointerUp {
                pos: at,
                button: atomik_ui::PointerButton::Secondary,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert!(
        a.contains(&UiAction::ContextMenu {
            target: atomik_ui::ContextTarget::Notes,
            at
        }),
        "{a:?}"
    );
}

// ------------------------------------------------------------------ note mute

#[test]
fn toggle_note_mute_flips_only_the_selected_notes_in_one_command() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[0, 2]);
    let before = v.notes().to_vec();
    let out = v.command("editor.toggle_note_mute", None, &doc, &reg);
    let sets = notes_of(out.actions());
    assert_eq!(sets.len(), 1, "one SetClipContent, one undo step");
    let got = &sets[0];
    assert_eq!(got.len(), before.len());
    for (i, n) in got.iter().enumerate() {
        assert_eq!(n.muted, i == 0 || i == 2, "note {i}");
        assert_eq!(
            (n.tick, n.length, n.key),
            (before[i].tick, before[i].length, before[i].key)
        );
        assert_eq!(n.velocity, before[i].velocity);
    }
}

#[test]
fn toggle_note_mute_unmutes_a_muted_selection_and_is_quiet_without_one() {
    let (mut v, mut doc, reg) = setup();
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if let ClipContent::Notes(ns) = &mut c.content
                    && c.id == ClipId(1)
                {
                    for n in ns.iter_mut() {
                        n.muted = true;
                    }
                }
            }
        }
    }
    v.sync(&doc, &reg);
    v.set_selected_notes(&[1]);
    let out = v.command("editor.toggle_note_mute", None, &doc, &reg);
    let got = &notes_of(out.actions())[0];
    assert!(!got[1].muted && got[0].muted && got[2].muted);
    v.set_selected_notes(&[]);
    let out = v.command("editor.toggle_note_mute", None, &doc, &reg);
    assert_eq!(out, CommandOutcome::Handled(vec![]));
}
