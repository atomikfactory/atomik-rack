//! The piano roll's selection is a set of notes, not of positions: an edit that reorders the note
//! list (a quantize that moves a note past another) or an undo that puts the notes back must leave
//! the same notes selected.

mod support;

use atomik_model::{ClipContent, Note, RackDoc};
use atomik_ui::PianoRollView;
use proptest::prelude::*;
use support::song::*;
use support::*;

/// The song of the fixtures with the notes of the open clip replaced.
fn with_notes(notes: &[Note]) -> RackDoc {
    let mut doc = song_doc();
    let mut list = notes.to_vec();
    list.sort_by_key(|n| (n.tick, n.key));
    doc.song_mut().tracks[0].lanes[0].clips[0].content = ClipContent::Notes(list.into());
    doc
}

fn open(doc: &RackDoc) -> PianoRollView {
    piano_roll(doc, &registry())
}

/// Syncs the roll to `doc` as the app does after a command or an undo (a new revision).
fn sync(v: &mut PianoRollView, doc: &RackDoc, revision: u64) {
    v.sync_at(doc, &registry(), revision);
}

fn selected_notes(v: &PianoRollView) -> Vec<Note> {
    v.selected_notes()
        .into_iter()
        .map(|i| v.notes()[i])
        .collect()
}

#[test]
fn a_quantize_that_reorders_notes_keeps_the_same_notes_selected_and_so_does_its_undo() {
    // B (key 64) starts first; quantizing to a 120-tick grid puts both at 120 and the sort order
    // (start, key) puts A (key 60) first.
    let b = n(100, 60, 64, 0.8);
    let a = n(130, 60, 60, 0.8);
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    assert_eq!(v.notes(), &[b, a]);
    v.set_selected_notes(&[1]);
    sync(&mut v, &before, 1);
    assert_eq!(selected_notes(&v), vec![a]);

    let quantized = with_notes(&[n(120, 60, 60, 0.8), n(120, 60, 64, 0.8)]);
    sync(&mut v, &quantized, 2);
    assert_eq!(
        v.selected_notes(),
        vec![0],
        "A is now first: the selection followed it"
    );
    assert_eq!(selected_notes(&v), vec![n(120, 60, 60, 0.8)]);

    sync(&mut v, &before, 3);
    assert_eq!(selected_notes(&v), vec![a], "the undo put the notes back");
}

#[test]
fn a_selection_set_by_a_gesture_is_not_re_aimed() {
    // A gesture that adds a note selects it by its index in the clip it is about to be given.
    let first = n(0, 60, 60, 0.8);
    let before = with_notes(&[first]);
    let mut v = open(&before);
    sync(&mut v, &before, 1);
    let added = n(10, 60, 62, 0.8);
    let after = with_notes(&[first, added]);
    v.set_selected_notes(&[1]);
    sync(&mut v, &after, 2);
    assert_eq!(selected_notes(&v), vec![added]);
}

#[test]
fn an_edit_the_notes_cannot_be_followed_through_comes_back_with_its_undo() {
    // A transpose changes the keys: the old names find nothing, the indices stay, and the undo
    // makes the names find the notes again.
    let (a, b) = (n(0, 60, 60, 0.8), n(100, 60, 62, 0.8));
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    v.set_selected_notes(&[1]);
    sync(&mut v, &before, 1);
    let up = with_notes(&[n(0, 60, 61, 0.8), n(100, 60, 63, 0.8)]);
    sync(&mut v, &up, 2);
    // A note is added before them, which moves both along.
    let more = with_notes(&[n(0, 60, 61, 0.8), n(50, 60, 90, 0.8), n(100, 60, 63, 0.8)]);
    sync(&mut v, &more, 3);
    // The undo of both edits.
    let back = with_notes(&[a, b]);
    sync(&mut v, &back, 4);
    assert_eq!(selected_notes(&v), vec![b]);
}

#[test]
fn choosing_again_forgets_the_old_selection() {
    let (a, b) = (n(0, 60, 60, 0.8), n(100, 60, 62, 0.8));
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    v.set_selected_notes(&[1]);
    sync(&mut v, &before, 1);
    let up = with_notes(&[n(0, 60, 61, 0.8), n(100, 60, 63, 0.8)]);
    sync(&mut v, &up, 2);
    v.set_selected_notes(&[0]);
    sync(&mut v, &up, 2);
    sync(&mut v, &before, 3);
    assert_eq!(selected_notes(&v), vec![a], "the new choice stands");
}

fn note_strategy() -> impl Strategy<Value = Note> {
    (0i64..4_000, 1i64..500, 40u8..52, 1u32..=10)
        .prop_map(|(t, l, k, v)| n(t, l, k, v as f32 / 10.0))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the notes and the selection, quantizing them to a grid and undoing it leaves the
    /// same notes selected: before, the quantized notes that came from them, and back again.
    #[test]
    fn a_quantize_and_its_undo_keep_the_selection(
        notes in prop::collection::vec(note_strategy(), 1..24),
        picks in prop::collection::vec(any::<prop::sample::Index>(), 1..6),
        grid in prop::sample::select(vec![60i64, 120, 240, 480]),
    ) {
        let before = with_notes(&notes);
        let mut v = open(&before);
        let sorted: Vec<Note> = v.notes().to_vec();
        let chosen: std::collections::BTreeSet<usize> =
            picks.iter().map(|p| p.index(sorted.len())).collect();
        let chosen: Vec<usize> = chosen.into_iter().collect();
        v.set_selected_notes(&chosen);
        sync(&mut v, &before, 1);
        let picked: Vec<Note> = chosen.iter().map(|i| sorted[*i]).collect();

        let snap = |t: i64| (t + grid / 2).div_euclid(grid) * grid;
        let moved: Vec<Note> = sorted
            .iter()
            .map(|x| Note { tick: snap(x.tick), ..*x })
            .collect();
        let after = with_notes(&moved);
        sync(&mut v, &after, 2);
        // The selected notes are the quantized forms of the picked ones: same key and length,
        // and the same rank among the notes of their key.
        let rank = |list: &[Note], key: u8, upto: usize| {
            list[..upto].iter().filter(|n| n.key == key).count()
        };
        let mut want: Vec<(u8, usize)> = chosen
            .iter()
            .map(|i| (sorted[*i].key, rank(&sorted, sorted[*i].key, *i)))
            .collect();
        want.sort_unstable();
        let mut got: Vec<(u8, usize)> = v
            .selected_notes()
            .into_iter()
            .map(|i| (v.notes()[i].key, rank(v.notes(), v.notes()[i].key, i)))
            .collect();
        got.sort_unstable();
        prop_assert_eq!(got, want);

        sync(&mut v, &before, 3);
        let mut back = selected_notes(&v);
        back.sort_by_key(|n| (n.tick, n.key));
        let mut picked = picked;
        picked.sort_by_key(|n| (n.tick, n.key));
        prop_assert_eq!(back, picked);
    }
}

#[test]
fn undoing_a_join_selects_the_notes_the_join_was_made_on_so_it_can_be_joined_again() {
    let (a, b) = (n(0, 60, 60, 0.8), n(100, 60, 60, 0.8));
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    v.set_selected_notes(&[0, 1]);
    sync(&mut v, &before, 1);
    // Ctrl+J: the two notes become one, selected.
    let out = v.command("editor.join_notes", None, &before, &registry());
    assert!(out.is_taken());
    let joined = with_notes(&[n(0, 60, 200, 0.8)]);
    v.set_undo_depth(1);
    sync(&mut v, &joined, 2);
    assert_eq!(v.selected_notes(), vec![0]);
    // Ctrl+Z: the app names the depth it takes back; both notes are selected again.
    v.undo_selection(1);
    v.set_undo_depth(0);
    sync(&mut v, &before, 3);
    assert_eq!(v.selected_notes(), vec![0, 1], "both notes are selected");
    // So the next Ctrl+J joins them again.
    let out = v.command("editor.join_notes", None, &before, &registry());
    assert!(out.is_taken());
    // Redo and undo again: the same.
    v.redo_selection();
    v.set_undo_depth(1);
    sync(&mut v, &joined, 4);
    v.undo_selection(1);
    v.set_undo_depth(0);
    sync(&mut v, &before, 5);
    assert_eq!(v.selected_notes(), vec![0, 1]);
}

#[test]
fn undoing_two_edits_in_a_row_gives_each_its_own_selection() {
    let (a, b, c) = (n(0, 60, 60, 0.8), n(100, 60, 62, 0.8), n(200, 60, 64, 0.8));
    let first = with_notes(&[a, b, c]);
    let mut v = open(&first);
    v.set_selected_notes(&[0, 2]);
    sync(&mut v, &first, 1);
    // Delete the two selected notes.
    assert!(
        v.command("edit.delete", None, &first, &registry())
            .is_taken()
    );
    let second = with_notes(&[b]);
    v.set_undo_depth(1);
    sync(&mut v, &second, 2);
    // Then select the last note and delete it.
    v.set_selected_notes(&[0]);
    sync(&mut v, &second, 2);
    assert!(
        v.command("edit.delete", None, &second, &registry())
            .is_taken()
    );
    let third = with_notes(&[]);
    v.set_undo_depth(2);
    sync(&mut v, &third, 3);
    v.undo_selection(2);
    v.set_undo_depth(1);
    sync(&mut v, &second, 4);
    assert_eq!(selected_notes(&v), vec![b]);
    v.undo_selection(1);
    v.set_undo_depth(0);
    sync(&mut v, &first, 5);
    assert_eq!(selected_notes(&v), vec![a, c]);
}

#[test]
fn a_drag_of_many_syncs_is_one_edit_and_its_undo_finds_the_notes_before_it() {
    let (a, b) = (n(0, 60, 60, 0.8), n(100, 60, 62, 0.8));
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    v.set_selected_notes(&[0, 1]);
    sync(&mut v, &before, 1);
    // Two steps of one drag: both notes moved a little, then more, one step of the history.
    v.set_undo_depth(1);
    sync(
        &mut v,
        &with_notes(&[n(5, 60, 60, 0.8), n(105, 60, 62, 0.8)]),
        2,
    );
    sync(
        &mut v,
        &with_notes(&[n(9, 60, 60, 0.8), n(109, 60, 62, 0.8)]),
        3,
    );
    v.undo_selection(1);
    v.set_undo_depth(0);
    sync(&mut v, &before, 4);
    assert_eq!(selected_notes(&v), vec![a, b]);
}

#[test]
fn an_undo_that_is_not_one_the_view_saw_leaves_the_selection_to_the_following_rules() {
    let (a, b) = (n(0, 60, 60, 0.8), n(100, 60, 62, 0.8));
    let before = with_notes(&[a, b]);
    let mut v = open(&before);
    v.set_selected_notes(&[1]);
    sync(&mut v, &before, 1);
    v.undo_selection(99);
    sync(&mut v, &before, 2);
    assert_eq!(selected_notes(&v), vec![b]);
}
