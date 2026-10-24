//! The snapshot bar of the console view: a slim row under the toolbar with a chip per mixer
//! snapshot, a `+` button that stores the mix, the A/B compare toggle and a status text. The state
//! lives in [`crate::ConsoleView`], which places the bar and routes the pointer here; the model is
//! [`atomik_model::MixerSnapshot`].
//!
//! A chip is the snapshot's name. The chip of the snapshot the mix was last recalled from (or that
//! the mix matches exactly) is lit; when the mix has moved since, it carries a small dot. While a
//! compare is on, the A/B button is lit and the status says which side is live (`A: working mix`
//! or `B: <name>`); the chip being compared with is outlined.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::console::small_button;
use crate::paint::{self, fit_label};
use crate::seq_paint::{self as sp, SeqTheme};
use atomik_model::RackDoc;
use vello::kurbo::{Rect, RoundedRect};

/// Height of the bar.
pub const BAR_H: f64 = 24.0;
const LABEL_W: f64 = 76.0;
const CHIP_MIN_W: f64 = 28.0;
const CHIP_MAX_W: f64 = 84.0;
const GAP: f64 = 4.0;
const PLUS_W: f64 = 24.0;
const AB_W: f64 = 36.0;
const END_W: f64 = 34.0;
/// Room kept for the status text before the chips stop being drawn.
const STATUS_MIN_W: f64 = 120.0;

/// What a chip draws and answers hits for, copied from the document by `sync`.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapView {
    /// The snapshot id.
    pub id: u32,
    /// The name.
    pub name: String,
    /// The mix matches the snapshot exactly (everything it covers).
    pub matches: bool,
}

/// The state of the compare, if one is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompareView {
    /// The snapshot being compared with.
    pub snapshot: u32,
    /// The snapshot's side is live (B), else the working mix (A).
    pub on_snapshot: bool,
}

/// The places of the bar's parts.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapLayout {
    /// The label at the left.
    pub label: Rect,
    /// A chip per snapshot that fits, by index into the view's list.
    pub chips: Vec<(usize, Rect)>,
    /// The `+` button.
    pub store: Rect,
    /// The A/B button.
    pub compare: Rect,
    /// The END button (only while comparing).
    pub end: Option<Rect>,
    /// What is left for the status text.
    pub status: Rect,
}

/// The width of the chip for `name`.
fn chip_width(name: &str) -> f64 {
    (paint::estimate_width(name, 9.0) + 18.0).clamp(CHIP_MIN_W, CHIP_MAX_W)
}

/// Lays the bar out inside `bar` (its height is [`BAR_H`]).
pub fn layout(bar: Rect, snaps: &[SnapView], comparing: bool) -> SnapLayout {
    let (y0, y1) = (bar.y0 + 3.0, bar.y1 - 3.0);
    let label = Rect::new(bar.x0 + 10.0, bar.y0, bar.x0 + 10.0 + LABEL_W, bar.y1);
    // The buttons after the chips and the status need room: chips that do not fit are left out.
    let tail = PLUS_W + GAP + AB_W + if comparing { GAP + END_W } else { 0.0 } + STATUS_MIN_W;
    let limit = bar.x1 - 8.0 - tail;
    let mut x = label.x1 + 4.0;
    let mut chips = Vec::new();
    for (i, s) in snaps.iter().enumerate() {
        let w = chip_width(&s.name);
        if x + w > limit {
            break;
        }
        chips.push((i, Rect::new(x, y0, x + w, y1)));
        x += w + GAP;
    }
    let store = Rect::new(x, y0, x + PLUS_W, y1);
    x = store.x1 + GAP + 8.0;
    let compare = Rect::new(x, y0, x + AB_W, y1);
    x = compare.x1 + GAP;
    let end = comparing.then(|| {
        let r = Rect::new(x, y0, x + END_W, y1);
        x = r.x1 + GAP;
        r
    });
    let status = Rect::new((x + 6.0).min(bar.x1), bar.y0, (bar.x1 - 8.0).max(x), bar.y1);
    SnapLayout {
        label,
        chips,
        store,
        compare,
        end,
        status,
    }
}

/// The text of the status area: the side that is live while comparing, else what recalling the
/// active snapshot skips.
pub fn status_text(
    snaps: &[SnapView],
    compare: Option<CompareView>,
    active: Option<u32>,
    notes: &[String],
) -> Option<String> {
    let name_of = |id: u32| {
        snaps
            .iter()
            .find(|s| s.id == id)
            .map_or("", |s| s.name.as_str())
    };
    if let Some(c) = compare {
        return Some(if c.on_snapshot {
            format!("B: {}", name_of(c.snapshot))
        } else {
            "A: working mix".to_owned()
        });
    }
    let first = notes.first()?;
    let name = active.map_or("", name_of);
    Some(match notes.len() {
        1 => format!("{name}: {first}"),
        n => format!("{name}: {first} (+{} more)", n - 1),
    })
}

/// The snapshot that counts as the current one: the one being compared with, else the first the
/// mix matches exactly, else the one the host says the mix was last recalled from (when it still
/// exists).
pub fn current(snaps: &[SnapView], compare: Option<CompareView>, hint: Option<u32>) -> Option<u32> {
    if let Some(c) = compare {
        return Some(c.snapshot);
    }
    snaps
        .iter()
        .find(|s| s.matches)
        .map(|s| s.id)
        .or_else(|| hint.filter(|h| snaps.iter().any(|s| s.id == *h)))
}

/// Builds the chips from the document: a chip is `matches` when recalling its snapshot would
/// change nothing.
pub fn views(doc: &RackDoc, registry: &atomik_device_sdk::Registry) -> Vec<SnapView> {
    doc.console
        .snapshots
        .iter()
        .map(|s| SnapView {
            id: s.id,
            name: s.name.clone(),
            matches: doc.snapshot_matches(s.id, registry, atomik_model::SnapshotScope::ALL),
        })
        .collect()
}

/// Everything the painter needs.
pub struct Paint<'a> {
    pub st: &'a SeqTheme,
    pub snaps: &'a [SnapView],
    pub compare: Option<CompareView>,
    pub current: Option<u32>,
    pub notes: &'a [String],
    /// The chip whose name is being typed, and the text.
    pub editing: Option<(u32, &'a str)>,
}

/// Paints the bar into `bar`.
pub fn paint(c: &mut dyn Canvas, bar: Rect, p: &Paint<'_>) {
    let t = &p.st.base;
    let l = layout(bar, p.snaps, p.compare.is_some());
    sp::rect(c, bar, p.st.header);
    sp::line(
        c,
        bar.x0,
        bar.y1 - 0.5,
        bar.x1,
        bar.y1 - 0.5,
        p.st.separator,
    );
    c.text(
        &TextRun::new(
            "SNAPSHOTS",
            pt(l.label.x0, bar.center().y + 3.2),
            8.5,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.0),
    );
    for (i, r) in &l.chips {
        let s = &p.snaps[*i];
        if let Some((id, text)) = p.editing
            && id == s.id
        {
            edit_field(c, p.st, *r, text);
            continue;
        }
        let is_current = p.current == Some(s.id);
        let lit = is_current.then_some(t.highlight);
        small_button(c, p.st, *r, &s.name, lit, true, 9.0);
        if p.compare.is_some_and(|cv| cv.snapshot == s.id) {
            paint::outline(c, r.inset(1.0), 3.0, 1.4, t.cable_cv);
        }
        // The mix moved since this snapshot was recalled.
        if is_current && !s.matches && p.compare.is_none() {
            c.fill_with(
                RoundedRect::from_rect(
                    Rect::new(r.x1 - 7.0, r.y0 + 2.0, r.x1 - 3.0, r.y0 + 6.0),
                    2.0,
                ),
                t.warning,
            );
        }
    }
    small_button(
        c,
        p.st,
        l.store,
        "+",
        None,
        p.snaps.len() < atomik_model::MAX_SNAPSHOTS,
        11.0,
    );
    let comparing = p.compare.is_some();
    small_button(
        c,
        p.st,
        l.compare,
        "A/B",
        comparing.then_some(t.cable_cv),
        !p.snaps.is_empty(),
        8.5,
    );
    if let Some(r) = l.end {
        small_button(c, p.st, r, "END", None, true, 8.0);
    }
    if let Some(text) = status_text(p.snaps, p.compare, p.current, p.notes) {
        let color = match p.compare {
            Some(cv) if cv.on_snapshot => t.cable_cv,
            Some(_) => t.highlight,
            None => t.text_muted,
        };
        let text = fit_label(&text, l.status.width(), 9.0);
        sp::label(
            c,
            &text,
            l.status.x0,
            bar.center().y + 3.2,
            9.0,
            color,
            Align::Start,
        );
    }
}

/// The text field drawn over a chip while its name is typed.
fn edit_field(c: &mut dyn Canvas, st: &SeqTheme, chip: Rect, text: &str) {
    let t = &st.base;
    let r = Rect::new(chip.x0, chip.y0, chip.x0 + chip.width().max(96.0), chip.y1);
    c.fill_with(RoundedRect::from_rect(r, 3.0), t.lcd);
    c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.2, t.focus);
    let room = r.width() - 12.0;
    let mut shown: &str = text;
    while paint::estimate_width(shown, 9.0) > room {
        let mut chars = shown.chars();
        chars.next();
        shown = chars.as_str();
    }
    c.text(&TextRun::new(shown, pt(r.x0 + 5.0, r.center().y + 3.2), 9.0, t.text).weight(700));
    let cx = r.x0 + 5.0 + paint::estimate_width(shown, 9.0);
    c.stroke_with(
        vello::kurbo::Line::new(pt(cx + 1.0, r.y0 + 4.0), pt(cx + 1.0, r.y1 - 4.0)),
        1.2,
        t.focus,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snaps(names: &[&str]) -> Vec<SnapView> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| SnapView {
                id: i as u32 + 1,
                name: (*n).to_owned(),
                matches: false,
            })
            .collect()
    }

    #[test]
    fn chips_follow_each_other_and_the_store_button_follows_the_last() {
        let bar = Rect::new(0.0, 0.0, 1200.0, BAR_H);
        let s = snaps(&["A", "B", "Verse"]);
        let l = layout(bar, &s, false);
        assert_eq!(l.chips.len(), 3);
        for w in l.chips.windows(2) {
            assert!(w[0].1.x1 < w[1].1.x0);
        }
        assert!(l.chips[2].1.x1 < l.store.x0);
        assert!(l.store.x1 < l.compare.x0);
        assert!(l.end.is_none());
        assert!(layout(bar, &s, true).end.is_some());
    }

    #[test]
    fn a_narrow_bar_drops_the_chips_that_do_not_fit_and_keeps_the_buttons() {
        let bar = Rect::new(0.0, 0.0, 520.0, BAR_H);
        let s = snaps(&[
            "Snapshot one",
            "Snapshot two",
            "Snapshot three",
            "Snapshot four",
        ]);
        let l = layout(bar, &s, true);
        assert!(l.chips.len() < s.len());
        assert!(l.end.is_some_and(|e| e.x1 <= bar.x1));
        assert!(l.compare.x1 <= bar.x1);
    }

    #[test]
    fn the_current_snapshot_is_the_compared_one_then_the_matching_one_then_the_hint() {
        let mut s = snaps(&["A", "B", "C"]);
        assert_eq!(current(&s, None, None), None);
        assert_eq!(current(&s, None, Some(2)), Some(2));
        assert_eq!(
            current(&s, None, Some(9)),
            None,
            "a deleted hint is dropped"
        );
        s[0].matches = true;
        assert_eq!(current(&s, None, Some(2)), Some(1), "the exact match wins");
        let c = CompareView {
            snapshot: 3,
            on_snapshot: true,
        };
        assert_eq!(current(&s, Some(c), None), Some(3));
    }

    #[test]
    fn the_status_names_the_live_side_or_what_was_skipped() {
        let s = snaps(&["A", "Verse"]);
        let on = |on_snapshot| {
            Some(CompareView {
                snapshot: 2,
                on_snapshot,
            })
        };
        assert_eq!(status_text(&s, on(true), None, &[]).unwrap(), "B: Verse");
        assert_eq!(
            status_text(&s, on(false), None, &[]).unwrap(),
            "A: working mix"
        );
        assert_eq!(status_text(&s, None, Some(2), &[]), None);
        let notes = vec!["ch-3 no longer exists".to_owned()];
        assert_eq!(
            status_text(&s, None, Some(2), &notes).unwrap(),
            "Verse: ch-3 no longer exists"
        );
        let two = vec!["x".to_owned(), "y".to_owned()];
        assert!(
            status_text(&s, None, Some(1), &two)
                .unwrap()
                .ends_with("(+1 more)")
        );
    }
}
