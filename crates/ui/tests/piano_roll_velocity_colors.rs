//! The piano roll's note fills follow the velocity: a cool dim fill for a soft note, warmer and
//! brighter as it gets louder; a selected note is lifted, a muted one is a faint hollow fill.

mod support;

use atomik_model::RackDoc;
use atomik_ui::peniko::{Brush, Color};
use atomik_ui::theme::relative_luminance;
use atomik_ui::{DisplayList, Geom, Op, PianoRollView, SeqTheme, Theme};
use support::song::*;
use support::*;

/// The solid fill of the rounded rectangle drawn for note `i`.
fn note_fill(v: &mut PianoRollView, doc: &RackDoc, i: usize) -> Color {
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    let r = v.note_rect(i).expect("note rect");
    dl.ops()
        .iter()
        .find_map(|op| match op {
            Op::Fill {
                shape: Geom::RoundedRect(rr),
                brush: Brush::Solid(c),
            } if (rr.rect().x0 - r.x0.max(v.bounds().x0)).abs() < 8.0
                && (rr.rect().y0 - (r.y0 + 0.5)).abs() < 0.01
                && (rr.rect().x1 - r.x1).abs() < 8.0 =>
            {
                Some(*c)
            }
            _ => None,
        })
        .expect("the note's fill")
}

#[test]
fn the_ramp_runs_from_dim_and_cool_to_bright_and_warm() {
    let st = SeqTheme::from_theme(&Theme::dark());
    let lo = st.velocity_color(0.0);
    let mid = st.velocity_color(0.5);
    let hi = st.velocity_color(1.0);
    assert!(relative_luminance(lo) < relative_luminance(mid));
    assert!(relative_luminance(mid) < relative_luminance(hi));
    // Cool at the bottom (blue over red), warm at the top (red over blue).
    assert!(lo.components[2] > lo.components[0]);
    assert!(hi.components[0] > hi.components[2]);
    let mut last = -1.0;
    for i in 0..=20 {
        let l = relative_luminance(st.velocity_color(i as f32 / 20.0));
        assert!(l >= last - 1e-4, "brightness never falls as velocity rises");
        last = l;
    }
}

#[test]
fn a_note_is_filled_by_its_velocity() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let st = SeqTheme::from_theme(&Theme::dark());
    // The bass line's velocity rises with the index.
    let soft = note_fill(&mut v, &doc, 0);
    let loud = note_fill(&mut v, &doc, 15);
    assert_eq!(soft, st.velocity_color(0.45));
    assert_eq!(loud, st.velocity_color(0.45 + 0.035 * 15.0));
    assert!(relative_luminance(loud) > relative_luminance(soft));
}

#[test]
fn a_selected_note_is_lifted_from_its_velocity_colour() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    let plain = note_fill(&mut v, &doc, 3);
    v.set_selected_notes(&[3]);
    let selected = note_fill(&mut v, &doc, 3);
    assert_ne!(plain, selected);
    assert!(relative_luminance(selected) > relative_luminance(plain));
}
