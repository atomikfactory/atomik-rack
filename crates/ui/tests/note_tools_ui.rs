//! The note tools in the editors: the Notes menu commands of the piano roll (legato, join,
//! chop, velocity, alter, random and absolute pitch, select by pitch), the velocity lane's
//! pencil, line and selected-only strokes, and the arrangement's note tool on whole clips.

mod support;

use atomik_model::{ClipContent, ClipId, Note, NoteTool, SongCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::Point;
use atomik_ui::{Modifiers, PianoRollView, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

fn handled(out: CommandOutcome) -> Vec<UiAction> {
    match out {
        CommandOutcome::Handled(a) => a,
        other => panic!("expected Handled, got {other:?}"),
    }
}

fn roll() -> (
    PianoRollView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    (v, doc, reg)
}

fn contents(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                content: ClipContent::Notes(n),
                merge,
                ..
            } => Some((n.into_vec(), merge)),
            _ => None,
        })
        .collect()
}

fn tools(actions: &[UiAction]) -> Vec<(Vec<ClipId>, Option<Vec<usize>>, NoteTool)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::ApplyNoteTool {
                clips,
                selection,
                tool,
            } => Some((clips, selection, tool)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_velocity_tool_is_one_command_over_the_selection() {
    let (mut v, doc, reg) = roll();
    v.set_selected_notes(&[2, 3]);
    let out = handled(v.command("editor.note_tool", Some("velocity_set:127"), &doc, &reg));
    let t = tools(&out);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].0, vec![ClipId(1)]);
    assert_eq!(t[0].1, Some(vec![2, 3]));
    assert_eq!(t[0].2, NoteTool::SetVelocity(1.0));
    assert_eq!(v.selected_notes(), vec![2, 3]);
    // Nothing selected: every note of the clip.
    v.set_selected_notes(&[]);
    let out = handled(v.command("editor.note_tool", Some("velocity_scale:50"), &doc, &reg));
    let t = tools(&out);
    assert_eq!(t[0].1, None);
    assert_eq!(t[0].2, NoteTool::ScaleVelocity(0.5));
}

#[test]
fn an_unknown_argument_is_declined_and_a_no_op_sends_nothing() {
    let (mut v, doc, reg) = roll();
    assert_eq!(
        v.command("editor.note_tool", Some("nonsense:1"), &doc, &reg),
        CommandOutcome::Declined
    );
    assert_eq!(
        v.command("editor.note_tool", None, &doc, &reg),
        CommandOutcome::Declined
    );
    // Scaling by 100 % changes nothing: no command.
    let out = handled(v.command("editor.note_tool", Some("velocity_scale:90"), &doc, &reg));
    assert_eq!(tools(&out).len(), 1);
    let doc2 = {
        let mut d = song_doc();
        d.song_mut().tracks[0].lanes[0].clips[0].content =
            ClipContent::Notes(vec![n(0, 100, 60, 1.0)].into());
        d
    };
    let mut v2 = piano_roll(&doc2, &reg);
    v2.sync(&doc2, &reg);
    let out = handled(v2.command("editor.note_tool", Some("velocity_set:127"), &doc2, &reg));
    assert!(out.is_empty());
}

#[test]
fn a_pitch_tool_keeps_the_selection_on_the_same_notes() {
    let (mut v, doc, reg) = roll();
    let before = v.notes().to_vec();
    v.set_selected_notes(&[0, 5, 9]);
    let out = handled(v.command("editor.note_tool", Some("random_pitch:7"), &doc, &reg));
    assert_eq!(tools(&out).len(), 1);
    // The tool is the same one the model computes, so the roll can place its selection.
    let NoteTool::RandomizePitch { range: 7, seed } = tools(&out)[0].2 else {
        panic!("random pitch")
    };
    let sel: std::collections::BTreeSet<usize> = [0, 5, 9].into_iter().collect();
    let (after, index) = atomik_model::note_tools::apply_note_tool_indexed(
        &before,
        Some(&sel),
        1,
        NoteTool::RandomizePitch { range: 7, seed },
    );
    let expected: Vec<usize> = {
        let mut e: Vec<usize> = sel.iter().map(|&i| index[i]).collect();
        e.sort_unstable();
        e
    };
    assert_eq!(v.selected_notes(), expected);
    for &i in &sel {
        assert_eq!(after[index[i]].tick, before[i].tick);
    }
}

#[test]
fn random_tools_use_a_new_seed_for_each_run_and_the_same_one_in_a_new_view() {
    let (mut a, doc, reg) = roll();
    let (mut b, _, _) = roll();
    let seed_of = |out: &[UiAction]| match tools(out)[0].2 {
        NoteTool::RandomizeVelocity { seed, .. } => seed,
        t => panic!("{t:?}"),
    };
    let first = seed_of(&handled(a.command(
        "editor.note_tool",
        Some("velocity_random:50"),
        &doc,
        &reg,
    )));
    let second = seed_of(&handled(a.command(
        "editor.note_tool",
        Some("velocity_random:50"),
        &doc,
        &reg,
    )));
    let fresh = seed_of(&handled(b.command(
        "editor.note_tool",
        Some("velocity_random:50"),
        &doc,
        &reg,
    )));
    assert_ne!(first, second);
    assert_eq!(first, fresh, "the same command on the same notes repeats");
}

#[test]
fn legato_join_and_chop_are_single_note_list_edits() {
    let (mut v, doc, reg) = roll();
    v.set_selected_notes(&[0, 1, 2]);
    let out = handled(v.command("editor.legato", None, &doc, &reg));
    let c = contents(&out);
    assert_eq!(c.len(), 1);
    let before = bass_line();
    assert_eq!(c[0].0[0].length, before[1].tick - before[0].tick);
    assert_eq!(c[0].0[3], before[3], "unselected notes are untouched");
    assert_eq!(
        v.command("editor.legato", Some("sideways"), &doc, &reg),
        CommandOutcome::Declined
    );

    // Join needs a selection; the notes of one key become one, a gap filled.
    let mut doc2 = song_doc();
    doc2.song_mut().tracks[0].lanes[0].clips[0].content = ClipContent::Notes(
        vec![
            n(0, PPQ, 60, 0.8),
            n(PPQ, PPQ, 60, 0.6),
            n(4 * PPQ, PPQ, 60, 0.7),
        ]
        .into(),
    );
    let mut v2 = piano_roll(&doc2, &reg);
    assert_eq!(
        v2.command("editor.join_notes", None, &doc2, &reg),
        CommandOutcome::Declined
    );
    v2.set_selected_notes(&[0, 1, 2]);
    let out = handled(v2.command("editor.join_notes", None, &doc2, &reg));
    let c = contents(&out);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0.len(), 1);
    assert_eq!(c[0].0[0].length, 5 * PPQ);
    assert_eq!(v2.selected_notes(), vec![0]);

    // Chop at 1/8 notes: every beat-long note becomes two.
    let mut v3 = piano_roll(&doc2, &reg);
    v3.set_selected_notes(&[2]);
    let out = handled(v3.command("editor.chop", Some("eighth"), &doc2, &reg));
    let c = contents(&out);
    assert_eq!(c[0].0.len(), 4);
    assert_eq!(v3.selected_notes().len(), 2);
    // Triplets divide a beat in three.
    let out = handled(v3.command("editor.chop", Some("triplet_eighth"), &doc2, &reg));
    assert!(!contents(&out).is_empty());
}

#[test]
fn select_same_pitch_by_command_and_by_double_click_on_the_key() {
    let (mut v, doc, reg) = roll();
    assert_eq!(
        v.command("editor.select_same_pitch", None, &doc, &reg),
        CommandOutcome::Declined
    );
    v.set_selected_notes(&[0]);
    handled(v.command("editor.select_same_pitch", None, &doc, &reg));
    let bass = bass_line();
    let same: Vec<usize> = (0..bass.len())
        .filter(|&i| bass[i].key == bass[0].key)
        .collect();
    assert!(same.len() > 1);
    assert_eq!(v.selected_notes(), same);

    v.set_selected_notes(&[]);
    let p = v.keyboard_point(43);
    let _ = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    let g: Vec<usize> = (0..bass.len()).filter(|&i| bass[i].key == 43).collect();
    assert_eq!(v.selected_notes(), g);
}

fn lane_point(v: &PianoRollView, between: usize, vel: f32) -> Point {
    // Midway between two bars so the press lands on the empty lane.
    let a = v.velocity_point(between, vel).unwrap();
    let b = v.velocity_point(between + 1, vel).unwrap();
    Point::new((a.x + b.x) / 2.0, a.y)
}

fn at(v: &PianoRollView, index: usize, vel: f32) -> Point {
    let p = v.velocity_point(index, vel).unwrap();
    Point::new(p.x + 3.0, p.y)
}

fn down_mod(pos: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers,
        clicks: 1,
    }
}

#[test]
fn the_velocity_pencil_sets_the_bars_it_passes_in_one_undo_step() {
    let (mut v, doc, reg) = roll();
    let base = bass_line();
    let mut emitted = v.handle(&down(lane_point(&v, 2, 0.2)), &doc, &reg);
    emitted.extend(v.handle(&mv(at(&v, 5, 0.2)), &doc, &reg));
    let c = contents(&emitted);
    assert!(!c.is_empty());
    let merges: std::collections::BTreeSet<_> = c.iter().map(|(_, m)| *m).collect();
    assert_eq!(merges.len(), 1, "one merge id for the whole stroke");
    assert!(merges.iter().all(Option::is_some));
    let last = &c.last().unwrap().0;
    for (i, note) in last.iter().enumerate().take(6).skip(3) {
        assert!(
            (note.velocity - 0.2).abs() < 0.03,
            "note {i}: {}",
            note.velocity
        );
    }
    assert_eq!(last[2], base[2]);
    assert_eq!(last[6], base[6]);
    assert_eq!(
        v.handle(&up(at(&v, 5, 0.2)), &doc, &reg),
        vec![UiAction::EndMerge]
    );
}

#[test]
fn the_constrain_role_draws_a_straight_ramp() {
    let (mut v, doc, reg) = roll();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let _ = v.handle(&down_mod(lane_point(&v, 0, 0.1), shift), &doc, &reg);
    let out = v.handle(&mv(at(&v, 4, 0.9)), &doc, &reg);
    let all = contents(&out);
    let notes = &all.last().unwrap().0;
    let vel: Vec<f32> = notes[1..=4].iter().map(|n| n.velocity).collect();
    assert!(vel.windows(2).all(|w| w[0] < w[1]), "{vel:?}");
    assert!(vel[0] > 0.1 && vel[3] < 0.95);
}

#[test]
fn a_stroke_with_alt_only_edits_the_selected_notes() {
    let (mut v, doc, reg) = roll();
    let base = bass_line();
    v.set_selected_notes(&[4]);
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    let _ = v.handle(&down_mod(lane_point(&v, 2, 0.15), alt), &doc, &reg);
    let out = v.handle(&mv(at(&v, 6, 0.15)), &doc, &reg);
    let all = contents(&out);
    let notes = &all.last().unwrap().0;
    assert!((notes[4].velocity - 0.15).abs() < 0.03);
    assert_eq!(notes[3], base[3]);
    assert_eq!(notes[5], base[5]);
}

#[test]
fn an_arrangement_tool_applies_to_the_selected_clips() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    assert!(matches!(
        v.command("sequencer.note_tool", Some("velocity_set:64"), &doc, &reg),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_selected_clips(&[ClipId(5), ClipId(1)]);
    let out = handled(v.command("sequencer.note_tool", Some("alter:50"), &doc, &reg));
    let t = tools(&out);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].0, vec![ClipId(1), ClipId(5)]);
    assert_eq!(t[0].1, None);
    assert!(matches!(t[0].2, NoteTool::AlterNotes { chance, .. } if (chance - 0.5).abs() < 1e-6));
    assert_eq!(
        v.command("sequencer.note_tool", Some("bogus"), &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn key_labels_name_the_keyboard_strip_without_touching_the_notes() {
    let (mut v, doc, reg) = roll();
    let paint = |v: &mut PianoRollView| {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    let plain = paint(&mut v);
    assert!(!plain.contains("kick_a"));
    let before = v.notes().to_vec();
    v.set_key_labels(Some(vec![(36, "kick_a".into())]));
    assert!(paint(&mut v).contains("kick_a"));
    assert_eq!(v.notes(), before);
    v.set_key_labels(None);
    assert_eq!(paint(&mut v), plain);
}

#[test]
fn extract_notes_sends_the_selection_as_one_workflow_command() {
    let (mut v, doc, reg) = roll();
    assert_eq!(
        v.command("editor.extract_notes", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing is selected"
    );
    v.set_selected_notes(&[0, 2]);
    let out = handled(v.command("editor.extract_notes", None, &doc, &reg));
    let sent: Vec<_> = out
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(atomik_model::Command::Workflow(
                atomik_model::WorkflowCommand::ExtractNotes {
                    selection,
                    mode,
                    to,
                    ..
                },
            )) => Some((selection.clone(), *mode, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![(vec![0, 2], atomik_model::ExtractMode::Move, None)]
    );
}
