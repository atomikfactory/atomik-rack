//! The controller area of the piano roll: pitch bend, aftertouch and CC lanes of one note clip,
//! drawn under the velocity lane on the roll's own time axis.
//!
//! The roll owns the geometry and the pointer routing; this module holds the pure pieces so they
//! can be tested without a view: the lane picker chips ([`Chip`]), the value mapping of a lane
//! ([`value_at_y`], [`y_of_value`]: MIDI resolution, a snap to the pitch bend centre), and the
//! edits of a clip's lane list ([`set_point`], [`remove_point`], [`move_point`],
//! [`toggle_curve`]), each returning the whole, canonical list that goes into one
//! `SongCommand::SetClipControllers` (a drag reuses one merge id, so it is one undo step).
//!
//! Gestures in the lane body: a press on a point drags it (time snaps to the roll's grid, the
//! value to the MIDI resolution), with the eraser or the secondary button it deletes the point,
//! with Ctrl or Cmd it toggles the point's curve (ramp or step); a press on empty space adds a
//! point and the drag draws freehand, one point per grid step the pointer crosses.

use atomik_model::controller::{ControllerKind, ControllerLane, canonical};
use atomik_model::{AutomationPoint, Curve};
use vello::kurbo::Rect;

/// Height of the chip row.
pub const CTL_HEADER_HEIGHT: f64 = 20.0;
/// Height of the lane body.
pub const CTL_BODY_HEIGHT: f64 = 84.0;
/// Height of the whole area (chips and lane).
pub const CTL_AREA_HEIGHT: f64 = CTL_HEADER_HEIGHT + CTL_BODY_HEIGHT;
/// Space above and below the value range inside the body.
pub const CTL_PAD: f64 = 8.0;
/// How close a press must be to a point to take it, in pixels.
pub const POINT_HIT_PX: f64 = 6.0;

/// The lanes the chip row names, in order (the rest are reached with the CC and poly steppers).
pub const PRESETS: [ControllerKind; 7] = [
    ControllerKind::PitchBend,
    ControllerKind::ChannelPressure,
    ControllerKind::Cc(1),
    ControllerKind::Cc(7),
    ControllerKind::Cc(10),
    ControllerKind::Cc(11),
    ControllerKind::Cc(64),
];

/// What a chip of the header row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// Show the lane `PRESETS[i]`.
    Preset(usize),
    /// Show the CC lane of the stepper's number; the arrows at its ends step the number.
    Cc,
    /// Show the poly aftertouch lane of the stepper's key; the arrows step the key.
    Poly,
    /// New points ramp to the next one, or hold (the curve of the points the pencil adds).
    Curve,
}

/// Which part of a stepper chip a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The left arrow.
    Down,
    /// The middle: show the lane.
    Body,
    /// The right arrow.
    Up,
}

/// The state of the controller area (the roll keeps one for all its clips).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneEditor {
    /// The area is shown under the velocity lane.
    pub open: bool,
    /// The lane being edited.
    pub kind: ControllerKind,
    /// The number of the CC stepper.
    pub cc: u8,
    /// The key of the poly aftertouch stepper.
    pub poly: u8,
    /// The curve of the points the pencil adds.
    pub curve: Curve,
}

impl Default for LaneEditor {
    fn default() -> Self {
        Self {
            open: false,
            kind: ControllerKind::PitchBend,
            cc: 74,
            poly: 60,
            curve: Curve::Linear,
        }
    }
}

impl LaneEditor {
    /// Shows `kind`, moving the matching stepper to its number.
    pub fn select(&mut self, kind: ControllerKind) {
        match kind {
            ControllerKind::Cc(n) if !PRESETS.contains(&kind) => self.cc = n,
            ControllerKind::PolyPressure(k) => self.poly = k,
            _ => {}
        }
        self.kind = kind;
    }

    /// Applies a press on a chip; `part` is the stepper part (ignored on the other chips).
    pub fn press(&mut self, chip: Chip, part: Part) {
        match chip {
            Chip::Preset(i) => {
                if let Some(k) = PRESETS.get(i) {
                    self.kind = *k;
                }
            }
            Chip::Cc => {
                self.cc = step_number(self.cc, part);
                self.kind = ControllerKind::Cc(self.cc);
            }
            Chip::Poly => {
                self.poly = step_number(self.poly, part);
                self.kind = ControllerKind::PolyPressure(self.poly);
            }
            Chip::Curve => {
                self.curve = match self.curve {
                    Curve::Linear => Curve::Step,
                    Curve::Step => Curve::Linear,
                };
            }
        }
    }

    /// True when `chip` is the one showing the lane being edited.
    pub fn is_active(&self, chip: Chip) -> bool {
        match chip {
            Chip::Preset(i) => PRESETS.get(i) == Some(&self.kind),
            Chip::Cc => self.kind == ControllerKind::Cc(self.cc) && !PRESETS.contains(&self.kind),
            Chip::Poly => matches!(self.kind, ControllerKind::PolyPressure(_)),
            Chip::Curve => self.curve == Curve::Step,
        }
    }

    /// The curve a point added to `kind` gets: the sustain pedal and the other on/off style
    /// controllers are steps whatever the chip says.
    pub fn new_curve(&self) -> Curve {
        if self.kind == ControllerKind::Cc(64) {
            Curve::Step
        } else {
            self.curve
        }
    }
}

/// The number after a stepper press: the arrows step by one (wrapping through 0..=127), the body
/// keeps it.
fn step_number(n: u8, part: Part) -> u8 {
    match part {
        Part::Down => n.checked_sub(1).unwrap_or(127),
        Part::Body => n,
        Part::Up => (n + 1) % 128,
    }
}

/// The text of a chip.
pub fn chip_label(chip: Chip, ed: &LaneEditor) -> String {
    match chip {
        Chip::Preset(i) => match PRESETS.get(i) {
            Some(ControllerKind::PitchBend) => "BEND".to_owned(),
            Some(ControllerKind::ChannelPressure) => "PRESS".to_owned(),
            Some(ControllerKind::Cc(1)) => "MOD 1".to_owned(),
            Some(ControllerKind::Cc(7)) => "VOL 7".to_owned(),
            Some(ControllerKind::Cc(10)) => "PAN 10".to_owned(),
            Some(ControllerKind::Cc(11)) => "EXPR 11".to_owned(),
            Some(ControllerKind::Cc(64)) => "SUST 64".to_owned(),
            _ => String::new(),
        },
        Chip::Cc => format!("< CC {} >", ed.cc),
        Chip::Poly => format!("< POLY {} >", ed.poly),
        Chip::Curve => match ed.curve {
            Curve::Linear => "RAMP".to_owned(),
            Curve::Step => "STEP".to_owned(),
        },
    }
}

/// The short name of a lane for the side panel: "BEND", "PRESS", "CC 74", "POLY 60".
pub fn kind_name(kind: ControllerKind) -> String {
    match kind {
        ControllerKind::PitchBend => "BEND".to_owned(),
        ControllerKind::ChannelPressure => "PRESS".to_owned(),
        ControllerKind::Cc(n) => format!("CC {n}"),
        ControllerKind::PolyPressure(k) => format!("POLY {k}"),
    }
}

/// Every chip, left to right.
pub fn chips() -> impl Iterator<Item = Chip> {
    (0..PRESETS.len())
        .map(Chip::Preset)
        .chain([Chip::Cc, Chip::Poly, Chip::Curve])
}

/// The rectangle of a chip in a header row that starts at `x0` and is `y0` from the top.
pub fn chip_rect(x0: f64, y0: f64, chip: Chip) -> Rect {
    let (offset, width) = match chip {
        Chip::Preset(i) => (i as f64 * 56.0, 52.0),
        Chip::Cc => (PRESETS.len() as f64 * 56.0 + 6.0, 70.0),
        Chip::Poly => (PRESETS.len() as f64 * 56.0 + 82.0, 78.0),
        Chip::Curve => (PRESETS.len() as f64 * 56.0 + 170.0, 46.0),
    };
    Rect::new(
        x0 + 6.0 + offset,
        y0 + 2.0,
        x0 + 6.0 + offset + width,
        y0 + 18.0,
    )
}

/// The part of a stepper chip at logical `x`.
pub fn part_at(rect: Rect, x: f64) -> Part {
    if x < rect.x0 + 14.0 {
        Part::Down
    } else if x > rect.x1 - 14.0 {
        Part::Up
    } else {
        Part::Body
    }
}

/// The chip at `(x, y)` of a header row (see [`chip_rect`]), with the part of a stepper.
pub fn chip_at(x0: f64, y0: f64, x: f64, y: f64) -> Option<(Chip, Part)> {
    chips().find_map(|chip| {
        let r = chip_rect(x0, y0, chip);
        (x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1).then(|| {
            let part = match chip {
                Chip::Cc | Chip::Poly => part_at(r, x),
                _ => Part::Body,
            };
            (chip, part)
        })
    })
}

/// The smallest step of a kind's value: the MIDI resolution (pitch bend 14 bit, the rest 7 bit).
pub fn value_step(kind: ControllerKind) -> f32 {
    match kind {
        ControllerKind::PitchBend => 1.0 / 8192.0,
        _ => 1.0 / 127.0,
    }
}

/// Rounds `v` to the kind's MIDI resolution inside its range.
pub fn quantize_value(kind: ControllerKind, v: f32) -> f32 {
    let (lo, hi) = kind.range();
    let step = value_step(kind);
    ((v / step).round() * step).clamp(lo, hi)
}

/// The value at logical `y` in a body whose value range spans `[top, top + height]` (the top is
/// the highest value). Rounded to the MIDI resolution; pitch bend within 1.5 % of the range of
/// the centre lands exactly on it.
pub fn value_at_y(kind: ControllerKind, y: f64, top: f64, height: f64) -> f32 {
    let (lo, hi) = kind.range();
    let f = (1.0 - (y - top) / height.max(1.0)).clamp(0.0, 1.0) as f32;
    let v = lo + f * (hi - lo);
    let v = if kind == ControllerKind::PitchBend && v.abs() < 0.03 {
        0.0
    } else {
        v
    };
    quantize_value(kind, v)
}

/// The logical y of `value` (the inverse of [`value_at_y`]).
pub fn y_of_value(kind: ControllerKind, value: f32, top: f64, height: f64) -> f64 {
    let (lo, hi) = kind.range();
    let f = f64::from((value - lo) / (hi - lo));
    top + (1.0 - f.clamp(0.0, 1.0)) * height
}

/// The readout of a value: `0..=127` for CCs and aftertouch, a signed `-8192..=8191` for pitch
/// bend.
pub fn value_text(kind: ControllerKind, value: f32) -> String {
    match kind {
        ControllerKind::PitchBend => {
            let v = (value * 8192.0).round().clamp(-8192.0, 8191.0) as i32;
            format!("{v:+}")
        }
        _ => format!("{}", (value * 127.0).round().clamp(0.0, 127.0) as i32),
    }
}

/// The lane of `kind` in `lanes`.
pub fn lane_of(lanes: &[ControllerLane], kind: ControllerKind) -> Option<&ControllerLane> {
    lanes.iter().find(|l| l.kind == kind)
}

/// The index of the point nearest to `(x, y)` within [`POINT_HIT_PX`] (in the same units as the
/// positions `at` returns for it), `None` when none is that close.
pub fn nearest_point(
    points: &[AutomationPoint],
    at: impl Fn(&AutomationPoint) -> (f64, f64),
    x: f64,
    y: f64,
) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in points.iter().enumerate() {
        let (px, py) = at(p);
        let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
        if d <= POINT_HIT_PX && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// `lanes` with the point of `kind` at `tick` set to `value` and `curve` (a point already at the
/// tick is replaced); the new index of the point.
pub fn set_point(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    tick: i64,
    value: f32,
    curve: Curve,
) -> (Vec<ControllerLane>, usize) {
    let mut out: Vec<ControllerLane> = lanes.to_vec();
    let lane = match out.iter().position(|l| l.kind == kind) {
        Some(i) => &mut out[i],
        None => {
            out.push(ControllerLane::new(kind, Vec::new()));
            out.last_mut().expect("just pushed")
        }
    };
    let p = AutomationPoint {
        tick,
        value: quantize_value(kind, value),
        curve,
    };
    let i = lane.points.partition_point(|q| q.tick < tick);
    if lane.points.get(i).is_some_and(|q| q.tick == tick) {
        lane.points[i] = p;
    } else {
        lane.points.insert(i, p);
    }
    (canonical(out), i)
}

/// `lanes` without point `index` of `kind` (the lane goes when it was the last point).
pub fn remove_point(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    index: usize,
) -> Vec<ControllerLane> {
    let mut out: Vec<ControllerLane> = lanes.to_vec();
    if let Some(lane) = out.iter_mut().find(|l| l.kind == kind)
        && index < lane.points.len()
    {
        lane.points.remove(index);
    }
    canonical(out)
}

/// `lanes` with point `index` of `kind` moved to `tick` and `value` (keeping its curve; a point
/// already at the tick is replaced); the new index of the point.
pub fn move_point(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    index: usize,
    tick: i64,
    value: f32,
) -> (Vec<ControllerLane>, usize) {
    let curve = lane_of(lanes, kind)
        .and_then(|l| l.points.get(index))
        .map_or(Curve::Linear, |p| p.curve);
    let without = remove_point(lanes, kind, index);
    set_point(&without, kind, tick, value, curve)
}

/// `lanes` with the curve of point `index` of `kind` flipped between ramp and step.
pub fn toggle_curve(
    lanes: &[ControllerLane],
    kind: ControllerKind,
    index: usize,
) -> Vec<ControllerLane> {
    let mut out: Vec<ControllerLane> = lanes.to_vec();
    if let Some(p) = out
        .iter_mut()
        .find(|l| l.kind == kind)
        .and_then(|l| l.points.get_mut(index))
    {
        p.curve = match p.curve {
            Curve::Linear => Curve::Step,
            Curve::Step => Curve::Linear,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
        AutomationPoint { tick, value, curve }
    }

    fn cc1(points: Vec<AutomationPoint>) -> Vec<ControllerLane> {
        vec![ControllerLane::new(ControllerKind::Cc(1), points)]
    }

    #[test]
    fn values_round_trip_through_y_at_midi_resolution() {
        let (top, h) = (10.0, 64.0);
        for kind in [
            ControllerKind::Cc(1),
            ControllerKind::ChannelPressure,
            ControllerKind::PitchBend,
        ] {
            let (lo, hi) = kind.range();
            assert_eq!(value_at_y(kind, top, top, h), hi);
            assert_eq!(value_at_y(kind, top + h, top, h), lo);
            assert_eq!(value_at_y(kind, top - 50.0, top, h), hi, "above the lane");
            assert_eq!(
                value_at_y(kind, top + h + 50.0, top, h),
                lo,
                "below the lane"
            );
            for v in [lo, hi, (lo + hi) / 2.0] {
                let y = y_of_value(kind, v, top, h);
                assert!(
                    (value_at_y(kind, y, top, h) - v).abs() <= value_step(kind),
                    "{kind:?} {v}"
                );
            }
        }
        // A CC is a multiple of 1/127.
        let v = value_at_y(ControllerKind::Cc(1), top + 20.0, top, h);
        assert!(((v * 127.0).round() - v * 127.0).abs() < 1e-4);
    }

    #[test]
    fn pitch_bend_snaps_to_its_centre() {
        let (top, h) = (0.0, 100.0);
        let mid = y_of_value(ControllerKind::PitchBend, 0.0, top, h);
        for dy in [-1.0, 0.0, 1.0] {
            assert_eq!(value_at_y(ControllerKind::PitchBend, mid + dy, top, h), 0.0);
        }
        assert!(value_at_y(ControllerKind::PitchBend, mid - 10.0, top, h) > 0.0);
    }

    #[test]
    fn readouts_use_the_midi_numbers() {
        assert_eq!(value_text(ControllerKind::Cc(1), 1.0), "127");
        assert_eq!(value_text(ControllerKind::Cc(1), 0.0), "0");
        assert_eq!(value_text(ControllerKind::PitchBend, 0.0), "+0");
        assert_eq!(value_text(ControllerKind::PitchBend, -1.0), "-8192");
        assert_eq!(value_text(ControllerKind::PitchBend, 1.0), "+8191");
    }

    #[test]
    fn set_point_inserts_in_order_replaces_at_a_tick_and_creates_the_lane() {
        let (l, i) = set_point(&[], ControllerKind::Cc(7), 100, 0.5, Curve::Step);
        assert_eq!((l.len(), i), (1, 0));
        let (l, i) = set_point(&l, ControllerKind::Cc(7), 50, 1.0, Curve::Linear);
        assert_eq!(i, 0);
        let (l, i) = set_point(&l, ControllerKind::Cc(7), 100, 0.25, Curve::Linear);
        assert_eq!(i, 1);
        let pts = &l[0].points;
        assert_eq!(pts.iter().map(|p| p.tick).collect::<Vec<_>>(), [50, 100]);
        assert_eq!(pts[1].curve, Curve::Linear, "the point at 100 was replaced");
        // A second kind keeps the lanes sorted by kind.
        let (l, _) = set_point(&l, ControllerKind::PitchBend, 0, 0.0, Curve::Linear);
        assert_eq!(l[0].kind, ControllerKind::PitchBend);
        assert_eq!(l[1].kind, ControllerKind::Cc(7));
    }

    #[test]
    fn remove_drops_the_lane_with_its_last_point() {
        let l = cc1(vec![pt(0, 0.5, Curve::Step), pt(10, 1.0, Curve::Step)]);
        let l = remove_point(&l, ControllerKind::Cc(1), 0);
        assert_eq!(l[0].points.len(), 1);
        let l = remove_point(&l, ControllerKind::Cc(1), 0);
        assert!(l.is_empty());
        assert!(remove_point(&l, ControllerKind::Cc(1), 3).is_empty());
    }

    #[test]
    fn move_keeps_the_curve_and_replaces_a_point_it_lands_on() {
        let l = cc1(vec![
            pt(0, 0.0, Curve::Linear),
            pt(100, 0.5, Curve::Step),
            pt(200, 1.0, Curve::Linear),
        ]);
        let (m, i) = move_point(&l, ControllerKind::Cc(1), 0, 150, 0.25);
        assert_eq!(i, 1);
        let ticks: Vec<i64> = m[0].points.iter().map(|p| p.tick).collect();
        assert_eq!(ticks, [100, 150, 200]);
        assert_eq!(m[0].points[1].curve, Curve::Linear);
        let (m, i) = move_point(&l, ControllerKind::Cc(1), 0, 200, 0.75);
        assert_eq!(
            (m[0].points.len(), i),
            (2, 1),
            "the point at 200 is replaced"
        );
        assert!((m[0].points[1].value - 0.75).abs() < 0.01);
    }

    #[test]
    fn toggle_curve_flips_one_point() {
        let l = cc1(vec![pt(0, 0.0, Curve::Linear), pt(10, 1.0, Curve::Linear)]);
        let t = toggle_curve(&l, ControllerKind::Cc(1), 1);
        assert_eq!(t[0].points[0].curve, Curve::Linear);
        assert_eq!(t[0].points[1].curve, Curve::Step);
        assert_eq!(toggle_curve(&t, ControllerKind::Cc(1), 1), l);
    }

    #[test]
    fn nearest_point_prefers_the_closest_within_reach() {
        let pts = [pt(0, 0.0, Curve::Step), pt(10, 0.0, Curve::Step)];
        let at = |p: &AutomationPoint| (p.tick as f64, 0.0);
        assert_eq!(nearest_point(&pts, at, 9.0, 1.0), Some(1));
        assert_eq!(nearest_point(&pts, at, 1.0, 2.0), Some(0));
        assert_eq!(nearest_point(&pts, at, 5.0, 40.0), None);
    }

    #[test]
    fn the_chips_pick_lanes_step_numbers_and_toggle_the_curve() {
        let mut ed = LaneEditor::default();
        assert_eq!(ed.kind, ControllerKind::PitchBend);
        ed.press(Chip::Preset(2), Part::Body);
        assert_eq!(ed.kind, ControllerKind::Cc(1));
        assert!(ed.is_active(Chip::Preset(2)) && !ed.is_active(Chip::Cc));
        ed.press(Chip::Cc, Part::Body);
        assert_eq!(ed.kind, ControllerKind::Cc(74));
        assert!(ed.is_active(Chip::Cc));
        ed.press(Chip::Cc, Part::Up);
        assert_eq!(ed.kind, ControllerKind::Cc(75));
        ed.cc = 127;
        ed.press(Chip::Cc, Part::Up);
        assert_eq!(ed.cc, 0, "wraps");
        ed.press(Chip::Cc, Part::Down);
        assert_eq!(ed.cc, 127);
        ed.press(Chip::Poly, Part::Body);
        assert_eq!(ed.kind, ControllerKind::PolyPressure(60));
        ed.press(Chip::Poly, Part::Down);
        assert_eq!(ed.kind, ControllerKind::PolyPressure(59));
        assert_eq!(ed.new_curve(), Curve::Linear);
        ed.press(Chip::Curve, Part::Body);
        assert_eq!(ed.new_curve(), Curve::Step);
        ed.press(Chip::Curve, Part::Body);
        ed.select(ControllerKind::Cc(64));
        assert_eq!(ed.new_curve(), Curve::Step, "the pedal always steps");
        // Selecting a preset CC leaves the stepper's number alone.
        let cc = ed.cc;
        ed.select(ControllerKind::Cc(1));
        assert_eq!(ed.cc, cc);
        ed.select(ControllerKind::Cc(99));
        assert_eq!(ed.cc, 99);
    }

    #[test]
    fn chips_do_not_overlap_and_are_found_by_position() {
        let all: Vec<Chip> = chips().collect();
        assert_eq!(all.len(), 10);
        for (i, a) in all.iter().enumerate() {
            let ra = chip_rect(100.0, 500.0, *a);
            for b in &all[i + 1..] {
                let rb = chip_rect(100.0, 500.0, *b);
                assert!(ra.x1 <= rb.x0 || rb.x1 <= ra.x0, "{a:?} overlaps {b:?}");
            }
            let c = ra.center();
            let (chip, _) = chip_at(100.0, 500.0, c.x, c.y).expect("found");
            assert_eq!(chip, *a);
        }
        let cc = chip_rect(0.0, 0.0, Chip::Cc);
        assert_eq!(part_at(cc, cc.x0 + 3.0), Part::Down);
        assert_eq!(part_at(cc, cc.center().x), Part::Body);
        assert_eq!(part_at(cc, cc.x1 - 3.0), Part::Up);
        assert!(chip_at(0.0, 0.0, 5000.0, 10.0).is_none());
    }
}
