//! The piano roll dims what the clip does not play: notes under a later clip of the lane, and
//! the tail of a note past the clip's end.

mod support;

use atomik_model::RackDoc;
use atomik_ui::peniko::{Brush, Color};
use atomik_ui::{DisplayList, Geom, Op, PianoRollView, SeqTheme, Theme};
use support::song::*;
use support::*;

/// The number of note veils (translucent lane-coloured rectangles) a paint draws.
fn veils(v: &mut PianoRollView, doc: &RackDoc) -> usize {
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    let veil: Color = SeqTheme::from_theme(&Theme::dark()).lane.with_alpha(0.62);
    dl.ops()
        .iter()
        .filter(|op| {
            matches!(op, Op::Fill { shape: Geom::Rect(_), brush: Brush::Solid(c) } if *c == veil)
        })
        .count()
}

#[test]
fn a_clip_nothing_hides_has_no_dimmed_notes() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    assert_eq!(veils(&mut v, &doc), 0);
}

#[test]
fn notes_under_a_later_clip_are_dimmed() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let hider = notes_clip(40, 2 * BAR, BAR, vec![n(0, PPQ, 50, 0.5)]);
    doc.song_mut().tracks[0].lanes[0].clips.push(hider);
    v.sync(&doc, &reg);
    let n = veils(&mut v, &doc);
    assert!(n > 0, "the bass notes inside the hidden bar are veiled");
    // Muting the hiding clip lets the notes sound again.
    doc.song_mut().tracks[0].lanes[0]
        .clips
        .last_mut()
        .unwrap()
        .muted = true;
    v.sync(&doc, &reg);
    assert_eq!(veils(&mut v, &doc), 0);
}

#[test]
fn the_tail_of_a_note_past_the_clip_end_is_dimmed() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    // Shorten the clip so its last notes run over the end.
    doc.song_mut().tracks[0].lanes[0].clips[0].length = 2 * PPQ;
    v.sync(&doc, &reg);
    assert!(veils(&mut v, &doc) > 0);
}

/// A looped bass clip two loops long, with a note in each half of its window.
fn looped_doc() -> (RackDoc, atomik_model::ClipId) {
    let mut doc = song_doc();
    let clip = &mut doc.song_mut().tracks[0].lanes[0].clips[0];
    clip.offset = 0;
    clip.length = 2 * BAR;
    clip.loop_length = Some(BAR);
    clip.content =
        atomik_model::ClipContent::Notes(vec![n(0, PPQ, 40, 0.8), n(2 * PPQ, PPQ, 43, 0.8)].into());
    let id = clip.id;
    (doc, id)
}

#[test]
fn a_looped_clip_nothing_hides_has_no_dimmed_notes() {
    let (doc, _) = looped_doc();
    let reg = registry();
    let mut v = piano_roll(&doc, &reg);
    assert_eq!(veils(&mut v, &doc), 0);
}

#[test]
fn the_part_of_a_loop_no_repetition_plays_is_dimmed() {
    let (mut doc, id) = looped_doc();
    let reg = registry();
    let mut v = piano_roll(&doc, &reg);
    let start = doc.song.clip(id).unwrap().2.start;
    // A later clip takes over half a bar in: the second half of the window is never heard.
    let hider = notes_clip(40, start + 2 * PPQ, 4 * BAR, vec![n(0, PPQ, 50, 0.5)]);
    doc.song_mut().tracks[0].lanes[0].clips.push(hider);
    v.sync(&doc, &reg);
    assert!(
        veils(&mut v, &doc) > 0,
        "the note in the unheard half is veiled"
    );
    // A hider that only covers the second repetition leaves every part heard once.
    doc.song_mut().tracks[0].lanes[0]
        .clips
        .last_mut()
        .unwrap()
        .start = start + BAR;
    v.sync(&doc, &reg);
    assert_eq!(veils(&mut v, &doc), 0);
}
