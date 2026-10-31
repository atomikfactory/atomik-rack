//! A playhead that moves less than a pixel asks for no repaint: the lane and clip layers of
//! the arrangement and the roll are redrawn only when the line itself moves.

mod support;

use atomik_ui::DisplayList;
use support::song::*;
use support::*;

#[test]
fn the_arrangement_repaints_when_the_playhead_crosses_a_pixel_only() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let mut dl = DisplayList::new();
    v.set_playhead(0);
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint());
    // One tick is a small fraction of a pixel at the default zoom.
    v.set_playhead(1);
    assert!(!v.needs_repaint(), "a sub-pixel move paints nothing");
    v.set_playhead(1);
    assert!(!v.needs_repaint());
    v.set_playhead(480);
    assert!(v.needs_repaint(), "a whole-pixel move repaints");
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint());
}

#[test]
fn the_roll_repaints_when_the_playhead_crosses_a_pixel_only() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint());
    let start = v.x_to_tick(v.tick_to_x(0) + 200.0);
    v.set_playhead(start);
    assert!(v.needs_repaint());
    v.paint(&mut dl, &doc, &reg);
    v.set_playhead(start + 1);
    assert!(!v.needs_repaint(), "one tick stays on the same pixel");
}
