//! Automation recording: the write modes, the pass recorder and the thinning of recorded points.
//!
//! Pure data and arithmetic on song ticks; it knows nothing of the engine, the UI or the
//! document. The app feeds a [`PassRecorder`] with the parameter moves it sees while the
//! transport records, then turns the [`Segment`]s of a finished pass into automation clips with
//! [`segment_clips`] (thinned by [`thin_points`]) and adds them as one undo step.
//!
//! The three write modes decide which stretches of the lane a pass replaces:
//!
//! * **Overwrite**: the whole pass, from its start to its end, whether or not the control is
//!   touched (the first point is the value the parameter had at the start).
//! * **Touch**: only the stretches while the control is held; the lane plays back untouched
//!   before the touch and after the release.
//! * **Latch**: from the first touch to the end of the pass.
//!
//! A written stretch becomes a new clip on the lane, and a later clip hides an earlier one where
//! they overlap (the lane's flattening rule), so "replace" needs no deletion of the old points
//! and the old clips come back in full when the take is undone.

use crate::song::{AutomationPoint, Curve, TempoEvent};
use crate::{TEMPO_MAX, TEMPO_MIN};

/// How hard Cleanup Automation thins: the tolerance is a fraction of the parameter's range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinLevel {
    /// 0.5 % of the range: only points the line reproduces almost exactly. The default.
    #[default]
    Normal,
    /// 2 % of the range.
    Heavy,
    /// 5 % of the range.
    Maximum,
}

impl ThinLevel {
    /// Every level, from the lightest.
    pub const ALL: [ThinLevel; 3] = [ThinLevel::Normal, ThinLevel::Heavy, ThinLevel::Maximum];

    /// The tolerance as a fraction of the parameter's range.
    pub const fn fraction(self) -> f32 {
        match self {
            ThinLevel::Normal => 0.005,
            ThinLevel::Heavy => 0.02,
            ThinLevel::Maximum => 0.05,
        }
    }

    /// The level whose fraction is nearest `fraction`.
    pub fn nearest(fraction: f32) -> ThinLevel {
        let f = if fraction.is_finite() {
            fraction
        } else {
            0.005
        };
        let mut best = ThinLevel::Normal;
        for l in ThinLevel::ALL {
            if (l.fraction() - f).abs() < (best.fraction() - f).abs() {
                best = l;
            }
        }
        best
    }

    /// The plain label.
    pub const fn label(self) -> &'static str {
        match self {
            ThinLevel::Normal => "Normal",
            ThinLevel::Heavy => "Heavy",
            ThinLevel::Maximum => "Maximum",
        }
    }

    /// The id used in command arguments.
    pub const fn id(self) -> &'static str {
        match self {
            ThinLevel::Normal => "normal",
            ThinLevel::Heavy => "heavy",
            ThinLevel::Maximum => "maximum",
        }
    }

    /// The level with this id.
    pub fn from_id(id: &str) -> Option<ThinLevel> {
        ThinLevel::ALL.into_iter().find(|l| l.id() == id)
    }
}

/// How a recording pass writes into an automation lane (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WriteMode {
    /// Replace everything from the start to the end of the pass.
    Overwrite,
    /// Write only while the control is held. The default.
    #[default]
    Touch,
    /// Write from the first touch until the end of the pass.
    Latch,
}

impl WriteMode {
    /// Every mode, in the order the UI cycles through them.
    pub const ALL: [WriteMode; 3] = [WriteMode::Overwrite, WriteMode::Touch, WriteMode::Latch];

    /// The name stored in project files.
    pub fn name(self) -> &'static str {
        match self {
            WriteMode::Overwrite => "overwrite",
            WriteMode::Touch => "touch",
            WriteMode::Latch => "latch",
        }
    }

    /// The mode of a stored name.
    pub fn from_name(name: &str) -> Option<WriteMode> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The next mode (Overwrite, Touch, Latch, and round again).
    pub fn next(self) -> WriteMode {
        match self {
            WriteMode::Overwrite => WriteMode::Touch,
            WriteMode::Touch => WriteMode::Latch,
            WriteMode::Latch => WriteMode::Overwrite,
        }
    }
}

/// One stretch a pass wrote: `[start, end)` in song ticks and the points inside it, sorted by
/// tick with unique ticks, the first at `start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// First tick.
    pub start: i64,
    /// End tick, exclusive; always above the last point's tick.
    pub end: i64,
    /// Recorded `(tick, value)` pairs.
    pub points: Vec<(i64, f32)>,
}

/// Records one pass over one lane (a pass runs from the transport's start, or a loop wrap, to
/// its stop or the next wrap). Ticks must not go backwards between calls; a call with an earlier
/// tick is treated as being at the latest one seen.
#[derive(Debug, Clone)]
pub struct PassRecorder {
    mode: WriteMode,
    segments: Vec<Segment>,
    open: Option<Segment>,
    now: i64,
}

impl PassRecorder {
    /// A recorder for a pass that starts at `tick` while the parameter holds `value`. Overwrite
    /// opens its stretch at once with that value; the other modes wait for a touch.
    pub fn new(mode: WriteMode, tick: i64, value: f32) -> Self {
        let open = (mode == WriteMode::Overwrite).then(|| Segment {
            start: tick,
            end: tick,
            points: vec![(tick, value)],
        });
        Self {
            mode,
            segments: Vec::new(),
            open,
            now: tick,
        }
    }

    /// The parameter took `value` at `tick`. `held` says the control is being held (a drag in
    /// progress); Touch writes only a held move, and a move that is not held touches for an
    /// instant: one point, released at once. Overwrite and Latch ignore `held`.
    pub fn change(&mut self, tick: i64, value: f32, held: bool) {
        let tick = tick.max(self.now);
        self.now = tick;
        if self.open.is_none() {
            self.open = Some(Segment {
                start: tick,
                end: tick,
                points: Vec::new(),
            });
        }
        if let Some(seg) = &mut self.open {
            match seg.points.last_mut() {
                // The last move in a tick wins.
                Some(last) if last.0 == tick => last.1 = value,
                _ => seg.points.push((tick, value)),
            }
        }
        if self.mode == WriteMode::Touch && !held {
            self.release(tick);
        }
    }

    /// The control was let go at `tick`. Only Touch ends its stretch there.
    pub fn release(&mut self, tick: i64) {
        self.now = tick.max(self.now);
        if self.mode == WriteMode::Touch {
            self.close(self.now);
        }
    }

    /// Whether a stretch is being written (Touch: the control is held).
    pub fn is_writing(&self) -> bool {
        self.open.is_some()
    }

    /// Ends the pass at `end` and returns what it wrote, in order.
    pub fn finish(mut self, end: i64) -> Vec<Segment> {
        self.close(end.max(self.now));
        self.segments
    }

    fn close(&mut self, end: i64) {
        let Some(mut seg) = self.open.take() else {
            return;
        };
        let last = seg.points.last().map_or(seg.start, |p| p.0);
        seg.end = end.max(last + 1);
        self.segments.push(seg);
    }
}

/// The clips of a pass: for each [`Segment`], its start tick, its length and its points (ticks
/// relative to the start, as `Linear` points) thinned with `tolerance` (see [`thin_points`]).
pub fn segment_clips(
    segments: &[Segment],
    tolerance: f32,
) -> Vec<(i64, i64, Vec<AutomationPoint>)> {
    segments
        .iter()
        .filter(|s| !s.points.is_empty() && s.end > s.start)
        .map(|s| {
            let points: Vec<AutomationPoint> = s
                .points
                .iter()
                .map(|&(tick, value)| AutomationPoint {
                    tick: tick - s.start,
                    value,
                    curve: Curve::Linear,
                })
                .collect();
            (s.start, s.end - s.start, thin_points(&points, tolerance))
        })
        .collect()
}

/// Removes points that the straight line between their kept neighbours reproduces within
/// `tolerance` (in value units), keeping the shape: the Ramer-Douglas-Peucker algorithm (Ramer
/// 1972, Douglas and Peucker 1973) with the vertical distance in value, since an automation
/// lane is a function of time.
///
/// The first and the last point are always kept; so is every point with a `Step` curve (it
/// ends a run, the points between two such anchors are thinned on their own). The points must
/// be sorted by tick. A `tolerance` that is not above zero keeps every point. Iterative, so the
/// depth of the input does not matter.
pub fn thin_points(points: &[AutomationPoint], tolerance: f32) -> Vec<AutomationPoint> {
    if points.len() < 3 || tolerance.is_nan() || tolerance <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    let mut run_start = 0;
    for i in 1..points.len() {
        if points[i].curve == Curve::Step || i == points.len() - 1 {
            keep[i] = true;
            thin_run(points, run_start, i, tolerance, &mut keep);
            run_start = i;
        }
    }
    points
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

/// RDP over `points[from..=to]` (both ends already kept), marking the survivors in `keep`.
fn thin_run(points: &[AutomationPoint], from: usize, to: usize, tolerance: f32, keep: &mut [bool]) {
    let mut stack = vec![(from, to)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let span = (pb.tick - pa.tick) as f64;
        let mut worst = (0.0_f64, a);
        for (k, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let t = if span > 0.0 {
                (p.tick - pa.tick) as f64 / span
            } else {
                0.0
            };
            let line = f64::from(pa.value) + (f64::from(pb.value) - f64::from(pa.value)) * t;
            let err = (f64::from(p.value) - line).abs();
            if err > worst.0 {
                worst = (err, k);
            }
        }
        if worst.0 > f64::from(tolerance) {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
}

/// The tempo map a tempo pass leaves: `base` with the events inside each recorded stretch
/// replaced by the stretch's points, thinned with `tolerance` in BPM (see [`thin_points`]). The
/// points ramp into each other, the last of a stretch holds until the next event of the base
/// map. Stretches apply in order, so a later pass lies on top of an earlier one; the events
/// outside every stretch are the base map's own. The BPM is rounded to 0.01 and kept inside the
/// supported range, so the result is always a valid map when `base` is.
pub fn tempo_pass_map(
    base: &[TempoEvent],
    segments: &[Segment],
    tolerance: f32,
) -> Vec<TempoEvent> {
    let mut map = base.to_vec();
    for seg in segments.iter().filter(|s| !s.points.is_empty()) {
        let points: Vec<AutomationPoint> = seg
            .points
            .iter()
            .map(|&(tick, value)| AutomationPoint {
                tick,
                value,
                curve: Curve::Linear,
            })
            .collect();
        let kept = thin_points(&points, tolerance);
        map.retain(|e| e.tick < seg.start || e.tick >= seg.end);
        let n = kept.len();
        map.extend(kept.iter().enumerate().map(|(i, p)| TempoEvent {
            tick: p.tick,
            bpm: ((f64::from(p.value) * 100.0).round() / 100.0).clamp(TEMPO_MIN, TEMPO_MAX),
            ramp: i + 1 < n,
        }));
        map.sort_by_key(|e| e.tick);
        // A point at tick 0 replaced the first event; the map never starts later than that.
        if map.first().is_some_and(|e| e.tick != 0)
            && let Some(first) = base.first()
        {
            map.insert(0, *first);
        }
        if let Some(last) = map.last_mut() {
            last.ramp = false;
        }
    }
    map
}
