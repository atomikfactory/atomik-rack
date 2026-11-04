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
