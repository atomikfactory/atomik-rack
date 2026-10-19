//! The breadcrumb bar of the rack view inside a container: a Back button
//! and `Project > A > B`, one clickable segment per scope level. It is shown only while the rack
//! shows the inside of a container; the top level has no bar.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{estimate_width, fit_label};
use crate::theme::{Theme, mix};
use vello::kurbo::{Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the bar, logical units.
pub const BAR_HEIGHT: f64 = 28.0;
/// Width of the Back button.
const BACK_W: f64 = 52.0;
const SEG_PAD: f64 = 8.0;
const SEP_W: f64 = 16.0;
const TEXT_SIZE: f32 = 11.0;
/// Longest segment label, in characters.
const MAX_LABEL: usize = 24;

/// What a point of the bar hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrumbHit {
    /// The Back button (up one level).
    Back,
    /// Segment `k`: `0` is the project, `k` the container `k` levels deep.
    Segment(usize),
}

/// The label of the project segment.
pub const PROJECT_LABEL: &str = "Project";

/// The rectangles of the Back button and of every segment, inside the bar `bar`. `labels` are
/// the segment labels, the project first.
pub fn layout(bar: Rect, labels: &[String]) -> Vec<(CrumbHit, Rect)> {
    let y0 = bar.y0 + 4.0;
    let y1 = bar.y1 - 4.0;
    let mut x = bar.x0 + 8.0;
    let mut out = vec![(CrumbHit::Back, Rect::new(x, y0, x + BACK_W, y1))];
    x += BACK_W + 10.0;
    for (k, label) in labels.iter().enumerate() {
        let w = estimate_width(&shown(label), TEXT_SIZE) + 2.0 * SEG_PAD;
        out.push((CrumbHit::Segment(k), Rect::new(x, y0, x + w, y1)));
        x += w + SEP_W;
    }
    out
}

fn shown(label: &str) -> String {
    fit_label(
        label,
        MAX_LABEL as f64 * f64::from(TEXT_SIZE) * 0.56,
        TEXT_SIZE,
    )
}

/// What a point hits, if anything.
pub fn hit(bar: Rect, labels: &[String], p: vello::kurbo::Point) -> Option<CrumbHit> {
    layout(bar, labels)
        .into_iter()
        .find(|(_, r)| r.contains(p))
        .map(|(h, _)| h)
}

/// Paints the bar. The last segment is the current scope (not clickable, drawn bright).
pub fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    bar: Rect,
    labels: &[String],
    hover: Option<CrumbHit>,
) {
    c.fill_with(bar, mix(theme.plate, theme.background, 0.3));
    c.stroke_with(
        Line::new(pt(bar.x0, bar.y1 - 0.5), pt(bar.x1, bar.y1 - 0.5)),
        1.0,
        theme.bevel_dark,
    );
    let last = labels.len().saturating_sub(1);
    for (h, r) in layout(bar, labels) {
        match h {
            CrumbHit::Back => {
                let face = if hover == Some(h) {
                    mix(theme.button, Color::WHITE, 0.12)
                } else {
                    theme.button
                };
                c.fill_with(RoundedRect::from_rect(r, 3.0), face);
                c.stroke_with(
                    RoundedRect::from_rect(r.inset(-0.5), 3.5),
                    1.0,
                    theme.bevel_dark,
                );
                c.text(
                    &TextRun::new(
                        "< BACK",
                        pt(r.center().x, r.center().y + 3.5),
                        9.5,
                        theme.text,
                    )
                    .align(Align::Center)
                    .weight(700),
                );
            }
            CrumbHit::Segment(k) => {
                if k > 0 {
                    c.text(
                        &TextRun::new(
                            ">",
                            pt(r.x0 - SEP_W / 2.0, r.center().y + 4.0),
                            TEXT_SIZE,
                            theme.text_muted,
                        )
                        .align(Align::Center),
                    );
                }
                if hover == Some(h) && k != last {
                    c.fill_with(
                        RoundedRect::from_rect(r, 3.0),
                        mix(theme.button, Color::WHITE, 0.08),
                    );
                }
                let col = if k == last {
                    theme.text
                } else {
                    theme.text_muted
                };
                c.text(
                    &TextRun::new(
                        shown(&labels[k]),
                        pt(r.x0 + SEG_PAD, r.center().y + 4.0),
                        TEXT_SIZE,
                        col,
                    )
                    .weight(if k == last { 700 } else { 500 }),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello::kurbo::Point;

    fn labels() -> Vec<String> {
        vec!["Project".into(), "Layer".into(), "Fx".into()]
    }

    #[test]
    fn segments_follow_the_back_button_left_to_right() {
        let bar = Rect::new(0.0, 0.0, 800.0, BAR_HEIGHT);
        let l = layout(bar, &labels());
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].0, CrumbHit::Back);
        for w in l.windows(2) {
            assert!(w[0].1.x1 < w[1].1.x0, "no overlap: {w:?}");
        }
        let (h, r) = &l[2];
        assert_eq!(*h, CrumbHit::Segment(1));
        assert_eq!(hit(bar, &labels(), r.center()), Some(CrumbHit::Segment(1)));
        assert_eq!(hit(bar, &labels(), Point::new(790.0, 14.0)), None);
    }
}
