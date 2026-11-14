//! Comp mode of the audio clip editor: the takes of a clip's take group as stacked lanes under a
//! comp lane, a larger and sample-accurate view of the comp the arrangement already keeps (one
//! comp model: the group's [`CompSegment`]s, edited by [`SongCommand::SetCompRange`]).
//!
//! ```text
//! comp lane   [ 3 ][   1   ][ 2 ]        which take plays where (a gap is silence)
//! take 3      ~~~~~~~~~~~~~~~~~~~~~~     newest take on top, its played parts tinted
//! take 2      ~~~~~~~~~~~~~~~~~~~~~~
//! take 1      ~~~~~~~~~~~~~~~~~~~~~~
//! ```
//!
//! * **Audition.** A press on a take's lane plays that take from the pointer (the same
//!   unprocessed audition as the speaker, [`UiAction::AuditionSpan`]) until the release.
//! * **Choose.** A drag on a take's lane puts the dragged range of that take into the comp:
//!   one `SetCompRange` on the release, with the crossfade (XFADE, 10 ms to start with) at the
//!   cuts where another take meets it. The range stays selected.
//! * **Keys.** Up and Down move the selected range to the take above or below (one
//!   `SetCompRange`); without a range they move the selected lane.
//! * **Comp lane.** A click selects the stretch that plays there and its take.
//! * **Flatten Comp** renders the comp to one new audio file and replaces the group by one clip
//!   of it ([`UiAction::FlattenComp`]).
//!
//! Every edit is one undo step. The takes of a group that are not audio have no lane: the mode
//! then says so.

use super::{AudioEditorView, DRAG_THRESHOLD, Drag, Hit};
use crate::action::UiAction;
use crate::audio;
use crate::canvas::{Align, Canvas};
use crate::seq_paint as sp;
use crate::wave_paint::{MAX_COLUMNS, WaveChannel, WaveDetail, paint_channel};
use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, CompSegment, RackDoc, Song, SongCommand,
    audition_span,
};
use vello::kurbo::{Point, Rect};

/// Height of the comp lane at the top.
pub(super) const COMP_H: f64 = 34.0;
const ROW_GAP: f64 = 2.0;
const ROW_MIN: f64 = 28.0;
const ROW_MAX: f64 = 72.0;
/// The crossfade a new comp cut gets to start with, in milliseconds.
pub const XFADE_DEFAULT_MS: u32 = 10;
const XFADE_MAX_MS: u32 = 500;
const XFADE_STEP_MS: u32 = 5;

/// One take of the group as a lane: an audio clip with its time curve.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Lane {
    pub(super) clip: ClipId,
    pub(super) start: i64,
    pub(super) length: i64,
    pub(super) muted: bool,
    pub(super) content: AudioClipContent,
    /// Frames of the asset (0 when it is unknown).
    pub(super) frames: u64,
    /// `(ticks from the take start, source frames from its offset)`, ascending in tick.
    pub(super) curve: Vec<(f64, f64)>,
}

/// The take group of the open clip, as the document holds it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Group {
    pub(super) id: u32,
    /// The extent of the group: from the earliest take start to the latest take end.
    pub(super) range: (i64, i64),
    /// One entry per take, bottom to top; `None` for a take that is not audio.
    pub(super) lanes: Vec<Option<Lane>>,
    pub(super) comp: Vec<CompSegment>,
}

impl Group {
    /// The group the clip `id` belongs to: a take of it, or a clip compiled from its comp.
    fn read(doc: &RackDoc, rate: u32, id: ClipId) -> Option<Group> {
        let (_, _, g) = doc
            .song
            .take_groups()
            .find(|(_, _, g)| g.takes.iter().any(|t| t.id == id) || g.clips.contains(&id))?;
        let lanes = g
            .takes
            .iter()
            .map(|t| {
                let ClipContent::Audio(a) = &t.content else {
                    return None;
                };
                let frames = doc.assets.get(&a.asset).map_or(0, |m| m.frames);
                Some(Lane {
                    clip: t.id,
                    start: t.start,
                    length: t.length,
                    muted: t.muted,
                    curve: super::framed_and_curve(&doc.song, rate, t.start, t.length, a).1,
                    content: a.clone(),
                    frames,
                })
            })
            .collect();
        Some(Group {
            id: g.id,
            range: g.range(),
            lanes,
            comp: g.comp.clone(),
        })
    }

    fn audio(&self) -> bool {
        !self.lanes.is_empty() && self.lanes.iter().all(Option::is_some)
    }
}

/// The state of Comp mode: view state only, the comp itself is the document's.
#[derive(Debug)]
pub(super) struct Comp {
    pub(super) group: Option<Group>,
    /// The selected lane (a take index).
    pub(super) take: usize,
    /// The chosen range in song ticks, on the selected lane.
    pub(super) sel: Option<(i64, i64)>,
    /// The crossfade at the cuts of a new range, in milliseconds.
    pub(super) ms: u32,
    /// [`Comp::ms`] in ticks at the start of the group.
    pub(super) xfade_ticks: u64,
}

impl Default for Comp {
    fn default() -> Self {
        Self {
            group: None,
            take: 0,
            sel: None,
            ms: XFADE_DEFAULT_MS,
            xfade_ticks: 0,
        }
    }
}

/// A drag on a lane.
#[derive(Debug, Clone)]
pub(super) struct CompDrag {
    take: usize,
    /// Song tick at the press.
    anchor: i64,
    now: i64,
    start: Point,
    moved: bool,
    /// An audition is playing and must be stopped on the release.
    playing: bool,
}

/// `ms` milliseconds at the start of the group, in ticks.
fn ticks_for_ms(song: &Song, rate: u32, at: i64, ms: u32) -> u64 {
    let clock = song.frame_clock(rate);
    let frames = u64::from(rate) * u64::from(ms) / 1000;
    (clock.tick_at(clock.at(at) + frames) - at).max(0) as u64
}

impl AudioEditorView {
    /// Reads the group of the open clip (called by the sync).
    pub(super) fn sync_comp(&mut self, doc: &RackDoc) {
        let group = self
            .clip
            .and_then(|id| Group::read(doc, doc.sample_rate, id));
        if group != self.comp.group {
            self.comp.group = group;
            self.repaint = true;
        }
        let n = self.comp.group.as_ref().map_or(0, |g| g.lanes.len());
        self.comp.take = self.comp.take.min(n.saturating_sub(1));
        if let Some(g) = &self.comp.group {
            self.comp.xfade_ticks =
                ticks_for_ms(&doc.song, doc.sample_rate, g.range.0, self.comp.ms);
        }
    }

    /// The group's start tick and length while Comp mode shows it (the axis of the mode).
    pub(super) fn comp_span(&self) -> Option<(i64, i64)> {
        let g = self.comp.group.as_ref()?;
        Some((g.range.0, (g.range.1 - g.range.0).max(1)))
    }

    /// Whether the open clip has a group of audio takes to comp.
    pub fn comp_ready(&self) -> bool {
        self.comp.group.as_ref().is_some_and(Group::audio)
    }

    /// The selected take of the group, if any.
    pub fn comp_take(&self) -> Option<usize> {
        self.comp.group.as_ref().map(|_| self.comp.take)
    }

    /// The chosen range in song ticks.
    pub fn comp_selection(&self) -> Option<(i64, i64)> {
        self.comp.sel
    }

    /// The crossfade of new comp cuts, in milliseconds.
    pub fn comp_xfade_ms(&self) -> u32 {
        self.comp.ms
    }

    /// How many takes the group has.
    pub fn comp_takes(&self) -> usize {
        self.comp.group.as_ref().map_or(0, |g| g.lanes.len())
    }

    /// The rows the lanes get: the row height and the display row of the first lane shown (the
    /// newest take is row 0; the selected lane is always in view).
    fn comp_rows(&self) -> (f64, usize, usize) {
        let n = self.comp_takes().max(1);
        let w = self.layout().wave;
        let avail = (w.height() - COMP_H - ROW_GAP).max(ROW_MIN);
        let h = (avail / n as f64 - ROW_GAP).clamp(ROW_MIN, ROW_MAX);
        let visible = ((avail / (h + ROW_GAP)).floor() as usize).clamp(1, n);
        let row = n - 1 - self.comp.take.min(n - 1);
        let first = if row < visible { 0 } else { row + 1 - visible };
        (h, first, visible)
    }

    /// The rectangle of take `take`'s lane, `None` when it is scrolled out of view.
    fn comp_lane_rect(&self, take: usize) -> Option<Rect> {
        let n = self.comp_takes();
        if take >= n {
            return None;
        }
        let (h, first, visible) = self.comp_rows();
        let row = n - 1 - take;
        if row < first || row >= first + visible {
            return None;
        }
        let w = self.layout().wave;
        let y0 = w.y0 + COMP_H + ROW_GAP + (row - first) as f64 * (h + ROW_GAP);
        Some(Rect::new(w.x0, y0, w.x1, (y0 + h).min(w.y1)))
    }

    /// The centre of take `take`'s lane (for tests and the host).
    pub fn comp_lane_center(&self, take: usize) -> Option<Point> {
        self.comp_lane_rect(take).map(|r| r.center())
    }

    /// Centres of the crossfade "less" and "more" buttons.
    pub fn xfade_button_centers(&self) -> (Point, Point) {
        let l = self.layout();
        (l.xfade_down.center(), l.xfade_up.center())
    }

    /// Centre of the FLATTEN COMP button.
    pub fn flatten_comp_button_center(&self) -> Point {
        self.layout().flatten_comp.center()
    }

    /// The centre of the comp lane.
    pub fn comp_top_center(&self) -> Point {
        let w = self.layout().wave;
        Point::new(w.center().x, w.y0 + COMP_H / 2.0)
    }

    /// What the pointer is over in Comp mode, below the toolbar.
    pub(super) fn comp_hit(&self, pos: Point) -> Hit {
        let w = self.layout().wave;
        if !w.contains(pos) || self.comp.group.is_none() {
            return Hit::None;
        }
        if pos.y < w.y0 + COMP_H {
            return Hit::CompTop;
        }
        (0..self.comp_takes())
            .find(|t| self.comp_lane_rect(*t).is_some_and(|r| r.contains(pos)))
            .map_or(Hit::None, Hit::CompLane)
    }

    /// The song tick at logical `x`, clamped to the group.
    fn comp_tick(&self, x: f64) -> i64 {
        let (start, len) = self.comp_span().unwrap_or((0, 1));
        (start + self.tick_of(x).round() as i64).clamp(start, start + len)
    }

    /// A press in Comp mode: on a lane it starts the audition and a drag, on the comp lane it
    /// selects the stretch there.
    pub(super) fn comp_press(
        &mut self,
        hit: Hit,
        pos: Point,
        doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) {
        match hit {
            Hit::CompLane(take) => {
                self.comp.take = take;
                let at = self.comp_tick(pos.x);
                let playing = self.comp_audition(take, at, doc, out);
                self.drag = Drag::Comp(CompDrag {
                    take,
                    anchor: at,
                    now: at,
                    start: pos,
                    moved: false,
                    playing,
                });
            }
            Hit::CompTop => {
                let at = self.comp_tick(pos.x);
                let seg = self
                    .comp
                    .group
                    .as_ref()
                    .and_then(|g| g.comp.iter().find(|s| s.start <= at && at < s.end).copied());
                match seg {
                    Some(s) => {
                        self.comp.take = s.take;
                        self.comp.sel = Some((s.start, s.end));
                    }
                    None => self.comp.sel = None,
                }
            }
            _ => {}
        }
    }

    /// Plays take `take` from song tick `at` (the speaker's audition); true when it plays.
    fn comp_audition(
        &mut self,
        take: usize,
        at: i64,
        doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Some(Some(lane)) = self.comp.group.as_ref().and_then(|g| g.lanes.get(take)) else {
            return false;
        };
        if self.missing.contains(&lane.content.asset) {
            self.status = "The audio file of that take is missing".into();
            return false;
        }
        let span = audition_span(
            &doc.song,
            self.rate,
            lane.start,
            &lane.content,
            lane.length,
            at,
            lane.frames,
        );
        match span {
            Some(span) => {
                out.push(UiAction::AuditionSpan {
                    asset: lane.content.asset.clone(),
                    start_frame: span.start_frame,
                    end_frame: span.end_frame,
                    reverse: span.reverse,
                    gain_db: lane.content.gain_db,
                });
                true
            }
            None => {
                self.status = "That take plays nothing from there".into();
                false
            }
        }
    }

    /// The pointer moves during a lane drag.
    pub(super) fn comp_move(&mut self, pos: Point) {
        let at = self.comp_tick(pos.x);
        if let Drag::Comp(d) = &mut self.drag {
            d.now = at;
            d.moved |= pos.distance(d.start) > DRAG_THRESHOLD;
            if d.moved {
                let (a, b) = (d.anchor.min(d.now), d.anchor.max(d.now));
                self.comp.sel = Some((a, b));
            }
        }
    }

    /// The release of a lane drag: stops the audition; a drag chooses the range.
    pub(super) fn comp_release(&mut self, d: CompDrag, out: &mut Vec<UiAction>) {
        if d.playing {
            out.push(UiAction::StopAudition);
        }
        if !d.moved {
            return;
        }
        let (a, b) = (d.anchor.min(d.now), d.anchor.max(d.now));
        if a < b {
            self.comp.sel = Some((a, b));
            out.extend(self.comp_range_action(d.take, a, b));
        }
    }

    /// Esc during a lane drag: nothing is chosen.
    pub(super) fn comp_cancel(&mut self, d: CompDrag, out: &mut Vec<UiAction>) {
        if d.playing {
            out.push(UiAction::StopAudition);
        }
        self.comp.sel = None;
    }

    /// The command that makes `[a, b)` play from `take`, clipped to the group.
    fn comp_range_action(&mut self, take: usize, a: i64, b: i64) -> Option<UiAction> {
        let g = self.comp.group.as_ref()?;
        let (a, b) = (a.max(g.range.0), b.min(g.range.1));
        if a >= b {
            return None;
        }
        Some(UiAction::Command(Command::Song(
            SongCommand::SetCompRange {
                group: g.id,
                take,
                start: a,
                end: b,
                xfade: self.comp.xfade_ticks,
            },
        )))
    }

    /// Up and Down in Comp mode: the chosen range moves to the take above or below (the newest
    /// take is on top), or just the selected lane when there is no range.
    pub(super) fn comp_step(&mut self, above: bool, out: &mut Vec<UiAction>) {
        let n = self.comp_takes();
        if n == 0 {
            return;
        }
        let take = if above {
            (self.comp.take + 1).min(n - 1)
        } else {
            self.comp.take.saturating_sub(1)
        };
        if take == self.comp.take {
            return;
        }
        self.comp.take = take;
        if let Some((a, b)) = self.comp.sel {
            out.extend(self.comp_range_action(take, a, b));
        }
    }

    /// The crossfade buttons: a step more or less, within 0..=500 ms.
    pub(super) fn comp_step_xfade(&mut self, more: bool, doc: &RackDoc) {
        let ms = if more {
            (self.comp.ms + XFADE_STEP_MS).min(XFADE_MAX_MS)
        } else {
            self.comp.ms.saturating_sub(XFADE_STEP_MS)
        };
        self.comp.ms = ms;
        if let Some(g) = &self.comp.group {
            self.comp.xfade_ticks = ticks_for_ms(&doc.song, doc.sample_rate, g.range.0, ms);
        }
    }

    /// FLATTEN COMP: asks the app to render the comp.
    pub(super) fn comp_flatten(&mut self, out: &mut Vec<UiAction>) {
        let Some(g) = &self.comp.group else { return };
        if !g.audio() || g.comp.is_empty() {
            self.status = "The comp plays nothing to render".into();
            return;
        }
        out.push(UiAction::FlattenComp(g.id));
    }

    /// The hint of Comp mode for the footer.
    pub(super) fn comp_hint(&self) -> String {
        match &self.comp.group {
            None => "This clip has no takes: record over it or loop-record to get some".to_owned(),
            Some(g) if !g.audio() => "Comp mode needs audio takes".to_owned(),
            Some(_) => "Click a take to hear it  ·  drag to put that range in the comp  ·  Up and Down move it to the take above or below".to_owned(),
        }
    }

    // ------------------------------------------------------------------ painting

    pub(super) fn paint_comp(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let w = self.layout().wave;
        c.push_clip(&w.into());
        sp::rect(c, w, st.lane_auto);
        sp::grid(c, st, &self.axis(), w, &self.bars);
        let Some(g) = self.comp.group.as_ref().filter(|g| g.audio()) else {
            sp::label(
                c,
                &self.comp_hint(),
                w.center().x,
                w.center().y,
                10.0,
                t.text_muted,
                Align::Center,
            );
            c.pop_clip();
            return;
        };
        let x_of = |tick: i64| self.x_of((tick - g.range.0) as f64);
        // The comp lane.
        let top = Rect::new(w.x0, w.y0, w.x1, w.y0 + COMP_H);
        sp::rect(c, top, st.header);
        for s in &g.comp {
            let (xa, xb) = (x_of(s.start).max(w.x0), x_of(s.end).min(w.x1));
            if xb <= xa {
                continue;
            }
            let lit = self.comp.sel == Some((s.start, s.end)) && self.comp.take == s.take;
            let r = Rect::new(xa, top.y0 + 3.0, xb, top.y1 - 3.0);
            sp::rect(c, r, self.accent.with_alpha(if lit { 0.7 } else { 0.4 }));
            sp::label(
                c,
                &format!("{}", s.take + 1),
                (xa + 8.0).min(xb),
                top.center().y + 3.5,
                10.0,
                t.text,
                Align::Start,
            );
            if s.xfade > 0 {
                // The crossfade runs from the cut forwards: a small bar under the lane.
                let xe = x_of(s.start + s.xfade as i64).min(w.x1).max(xa + 2.0);
                sp::rect(c, Rect::new(xa, top.y1 - 5.0, xe, top.y1 - 2.0), t.text);
            }
        }
        sp::line(c, w.x0, top.y1 - 0.5, w.x1, top.y1 - 0.5, st.separator);
        // The takes.
        for take in 0..g.lanes.len() {
            let Some(rect) = self.comp_lane_rect(take) else {
                continue;
            };
            let Some(lane) = g.lanes[take].as_ref() else {
                continue;
            };
            let selected = take == self.comp.take;
            sp::rect(c, rect, st.header);
            c.push_clip(&rect.into());
            self.paint_lane_wave(c, lane, g.range.0, rect);
            // Where this take plays.
            for s in g.comp.iter().filter(|s| s.take == take) {
                let (xa, xb) = (x_of(s.start).max(rect.x0), x_of(s.end).min(rect.x1));
                if xb > xa {
                    sp::rect(
                        c,
                        Rect::new(xa, rect.y0, xb, rect.y1),
                        self.accent.with_alpha(0.22),
                    );
                }
            }
            // The range being chosen or chosen last.
            if selected && let Some((a, b)) = self.comp.sel {
                let (xa, xb) = (x_of(a).max(rect.x0), x_of(b).min(rect.x1));
                if xb > xa {
                    sp::rect(
                        c,
                        Rect::new(xa, rect.y0, xb, rect.y1),
                        t.focus.with_alpha(0.28),
                    );
                }
            }
            sp::label(
                c,
                &format!("TAKE {}", take + 1),
                rect.x0 + 6.0,
                rect.y0 + 10.0,
                8.0,
                t.text_muted,
                Align::Start,
            );
            c.pop_clip();
            if selected {
                sp::line(
                    c,
                    rect.x0,
                    rect.y0 + 0.5,
                    rect.x1,
                    rect.y0 + 0.5,
                    self.accent,
                );
                sp::line(
                    c,
                    rect.x0,
                    rect.y1 - 0.5,
                    rect.x1,
                    rect.y1 - 0.5,
                    self.accent,
                );
            }
        }
        let n = g.lanes.len();
        let (_, first, visible) = self.comp_rows();
        if visible < n {
            let hidden = n - visible;
            sp::label(
                c,
                &format!("+{hidden}"),
                w.x1 - 8.0,
                w.y1 - 6.0,
                9.0,
                t.text_muted,
                Align::End,
            );
            let _ = first;
        }
        c.pop_clip();
    }

    /// The waveform of one take in its lane: one linear piece per pair of knots of its curve.
    fn paint_lane_wave(&self, c: &mut dyn Canvas, lane: &Lane, group_start: i64, rect: Rect) {
        let asset = lane.content.asset.as_str();
        let Some(peaks) = self.peaks.get(asset) else {
            sp::label(
                c,
                if self.missing.contains(asset) {
                    "Missing asset: the audio file is not available"
                } else {
                    "no waveform"
                },
                rect.center().x,
                rect.center().y + 3.0,
                9.0,
                self.st.base.text_muted,
                Align::Center,
            );
            return;
        };
        let offset = lane.content.source_offset as f64;
        let samples = self.samples.get(asset).map(|s| &s[..]);
        let wave = self.st.audio_wave(self.accent, lane.muted);
        let shift = (lane.start - group_start) as f64;
        for pair in lane.curve.windows(2) {
            let ((t0, s0), (t1, s1)) = (pair[0], pair[1]);
            let (xa, xb) = (self.x_of(shift + t0), self.x_of(shift + t1));
            let (vx0, vx1) = (xa.max(rect.x0).floor(), xb.min(rect.x1).ceil());
            if xb - xa < 0.5 || vx1 <= vx0 {
                continue;
            }
            let slope = (s1 - s0) / (xb - xa);
            if slope == 0.0 {
                continue;
            }
            let detail = WaveDetail::pick(peaks, samples, slope.abs());
            let bands = audio::channel_bands(rect.y0 + 3.0, rect.y1 - 3.0, detail.channels());
            let columns = ((vx1 - vx0) as usize).min(MAX_COLUMNS);
            let source = |x: f64| offset + s0 + (x - xa) * slope;
            let frame_x = |f: f64| xa + (f - offset - s0) / slope;
            c.push_clip(&Rect::new(vx0, rect.y0, vx1, rect.y1).into());
            for (ch, (cy, half)) in bands.into_iter().enumerate() {
                let channel = WaveChannel {
                    detail,
                    ch,
                    cy,
                    half,
                    gain: 1.0,
                    color: wave,
                    x_start: vx0,
                    columns,
                    line_x: (vx0, vx1),
                    limit: lane.frames,
                    frames_per_pixel: slope.abs(),
                };
                paint_channel(c, &channel, &source, &frame_x);
            }
            c.pop_clip();
        }
    }

    /// The toolbar of Comp mode: the crossfade length and Flatten Comp.
    pub(super) fn paint_comp_toolbar(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let l = self.layout();
        let ty = l.bar.center().y + 2.5;
        super::paint::muted_label(self, c, "XFADE", l.xfade_down.x0 - 38.0, ty);
        sp::button(c, st, l.xfade_down, "-", None, self.hover == Hit::XfadeDown);
        sp::label(
            c,
            &format!("{} ms", self.comp.ms),
            (l.xfade_down.x1 + l.xfade_up.x0) / 2.0,
            l.bar.center().y + 4.0,
            10.0,
            st.base.text,
            Align::Center,
        );
        sp::button(c, st, l.xfade_up, "+", None, self.hover == Hit::XfadeUp);
        let ready =
            self.comp_ready() && self.comp.group.as_ref().is_some_and(|g| !g.comp.is_empty());
        sp::button(
            c,
            st,
            l.flatten_comp,
            "FLATTEN COMP",
            None,
            ready && self.hover == Hit::FlattenComp,
        );
        if !ready {
            sp::rect(c, l.flatten_comp, st.base.background.with_alpha(0.55));
        }
    }

    /// The takes count shown at the right of the footer.
    pub(super) fn comp_footer_info(&self) -> String {
        let n = self.comp_takes();
        format!("{n} {}", if n == 1 { "take" } else { "takes" })
    }
}
