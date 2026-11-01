//! Take groups in the [`ArrangementView`]: a thin comp strip under the lane (a note lane or the
//! audio lane) of a track that has a group, and, when it is opened, one row per take (stacked sub-lanes).
//!
//! * The **group row** (`TAKE_GROUP_HEIGHT` tall) shows the comp: each segment in the colour of
//!   its take. Its header has a chevron that opens or closes the take rows, the take count, and a
//!   FLATTEN button (`FlattenTakes`: the comp clips stay as plain clips, the takes go).
//! * A **take row** (`TAKE_ROW_HEIGHT` tall) draws one take over its extent: dim where the comp
//!   uses another take, in the take's colour where it plays. Its header names the take; a click
//!   on the name picks it (`PickTake`: it plays the whole range alone) and a click on the
//!   cross deletes it (`DeleteTake`).
//! * **Comping** is a drag across a take row: the snapped range between the press and the
//!   release becomes that take's part of the comp (`SetCompSegment`), one undo step; a click
//!   without a drag in the row picks the whole take. Escape cancels a drag.
//! * The **comp strip** is edited too. A drag across it silences the range (`SetCompSilence`); a
//!   drag of a cut between two takes moves the cut (`SetCompSegment` for the take that grows);
//!   when a cut has a crossfade its tail has a handle that sets the length (`SetCompXfade`). A
//!   right-click on the strip offers the crossfade lengths of the nearest cut, silencing a
//!   segment and deleting the unused takes; a right-click on a take row offers duplicate,
//!   move up or down, delete, and for audio the level and offset (`SetTake`).
//! * An audio take row draws the waveform of the part of its asset the take plays, so a take
//!   moved by its offset or turned down shows it.
//!
//! Which groups are open is saved with the project (`SetTakesOpen`; the view shows a click at
//! once). The rows are laid out below the row of the lane that holds the group, before the row
//! of the next lane.

use super::{ArrangementView, Drag, Hit, Rect, Row, RowKind, TRACK_LIST_WIDTH, song_cmd};
use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::event::Modifiers;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::{PPQ, snap_round};
use atomik_model::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN};
use atomik_model::takes::clear_segment;
use atomik_model::{Clip, ClipContent, SongCommand, TakeGroup};
use std::collections::BTreeSet;
use vello::kurbo::{BezPath, Point, RoundedRect};
use vello::peniko::Color;

/// Height of a group's comp strip row.
pub const TAKE_GROUP_HEIGHT: f64 = 20.0;
/// Height of a take row.
pub const TAKE_ROW_HEIGHT: f64 = 22.0;
/// Colours of the takes, cycled by take number.
const TAKE_COLORS: [u32; 6] = [0x4c9aff, 0xffb454, 0x7bd88f, 0xe06c9f, 0xb48ef5, 0x56c2c9];

/// The colour of take `i` (0-based).
pub fn take_color(i: usize) -> Color {
    let c = TAKE_COLORS[i % TAKE_COLORS.len()];
    Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Pixels either side of a cut or crossfade tail that grab it in the comp strip.
const HANDLE_PX: f64 = 4.0;
/// Pixels either side of a cut within which a right-click on the strip offers its crossfades.
const CUT_MENU_PX: f64 = 8.0;
/// The crossfade lengths of the strip menu: a label and the note division (`None`: no crossfade).
const XFADES: &[(&str, Option<i64>)] = &[
    ("No Crossfade", None),
    ("Crossfade 1/32", Some(32)),
    ("Crossfade 1/16", Some(16)),
    ("Crossfade 1/8", Some(8)),
    ("Crossfade 1/4", Some(4)),
];

/// What a drag in a take row or the comp strip is doing (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompDrag {
    /// A drag across a take row: the range plays from that take.
    Swipe,
    /// A drag across the strip: the range goes silent.
    Silence,
    /// A cut between takes `left` and `right` at tick `at`: the drag moves it, between the outer
    /// ends `lo` and `hi` of the two segments.
    Cut {
        left: usize,
        right: usize,
        at: i64,
        lo: i64,
        hi: i64,
    },
    /// The crossfade tail of the cut at tick `at`: the drag sets its length.
    Xfade { at: i64 },
}

/// What the view remembers about take groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TakesUi {
    /// The groups whose take rows are shown.
    pub open: BTreeSet<u32>,
}

impl ArrangementView {
    /// Whether the take rows of group `group` are shown.
    pub fn takes_open(&self, group: u32) -> bool {
        self.tk.open.contains(&group)
    }

    /// Shows or hides the take rows of a group.
    pub fn set_takes_open(&mut self, group: u32, open: bool) {
        let changed = if open {
            self.tk.open.insert(group)
        } else {
            self.tk.open.remove(&group)
        };
        if changed {
            self.synced = false;
            self.repaint = true;
        }
    }

    /// The rows that follow lane `lane` (a note lane or the audio lane) of track `ti`: per take
    /// group its strip and, when open, the take rows. Heights are fixed (they do not follow the
    /// vertical zoom).
    pub(super) fn take_rows(&self, ti: usize, lane: usize) -> Vec<(RowKind, f64)> {
        let mut rows = Vec::new();
        let Some(lane) = self.song.tracks.get(ti).and_then(|t| t.lanes.get(lane)) else {
            return rows;
        };
        for g in &lane.take_groups {
            rows.push((
                RowKind::TakeGroup {
                    track: ti,
                    group: g.id,
                },
                TAKE_GROUP_HEIGHT,
            ));
            if self.tk.open.contains(&g.id) {
                for take in 0..g.takes.len() {
                    rows.push((
                        RowKind::Take {
                            track: ti,
                            group: g.id,
                            take,
                        },
                        TAKE_ROW_HEIGHT,
                    ));
                }
            }
        }
        rows
    }

    /// Forgets the open state of groups that no longer exist.
    pub(super) fn takes_sync(&mut self) {
        let song = &self.song;
        self.tk.open.retain(|id| song.take_group(*id).is_some());
    }

    fn group(&self, id: u32) -> Option<&TakeGroup> {
        self.song.take_group(id).map(|(_, _, g)| g)
    }

    fn take_row(&self, kind: &RowKind) -> Option<&Row> {
        self.rows.iter().find(|r| &r.kind == kind)
    }

    // ------------------------------------------------------------------ geometry

    /// The chevron and label area of a group row's header: the click target that opens and
    /// closes the take rows.
    pub(super) fn group_toggle_rect(&self, row: Rect) -> Rect {
        Rect::new(row.x0, row.y0, row.x0 + 132.0, row.y1)
    }

    /// The FLATTEN button of a group row's header.
    pub(super) fn flatten_rect(&self, row: Rect) -> Rect {
        Rect::new(
            row.x0 + TRACK_LIST_WIDTH - 66.0,
            row.y0 + 2.0,
            row.x0 + TRACK_LIST_WIDTH - 8.0,
            row.y1 - 2.0,
        )
    }

    /// The cross that deletes a take, in a take row's header.
    pub(super) fn take_delete_rect(&self, row: Rect) -> Rect {
        Rect::new(
            row.x0 + TRACK_LIST_WIDTH - 28.0,
            row.y0 + 3.0,
            row.x0 + TRACK_LIST_WIDTH - 10.0,
            row.y1 - 3.0,
        )
    }

    /// A header point of group `group` (its chevron), for tests and hosts.
    pub fn take_group_toggle_center(&self, group: u32) -> Option<Point> {
        let track = self
            .song
            .take_group(group)
            .map(|(t, _, _)| t.device.clone())?;
        let ti = self.song.tracks.iter().position(|t| t.device == track)?;
        let row = self.take_row(&RowKind::TakeGroup { track: ti, group })?;
        Some(self.group_toggle_rect(self.row_rect(row)).center())
    }

    /// The centre of the FLATTEN button of a group.
    pub fn take_flatten_center(&self, group: u32) -> Option<Point> {
        let ti = self.group_track(group)?;
        let row = self.take_row(&RowKind::TakeGroup { track: ti, group })?;
        Some(self.flatten_rect(self.row_rect(row)).center())
    }

    /// The centre of the name of take `take` in its row header (a click picks the take).
    pub fn take_label_center(&self, group: u32, take: usize) -> Option<Point> {
        let ti = self.group_track(group)?;
        let row = self.take_row(&RowKind::Take {
            track: ti,
            group,
            take,
        })?;
        let r = self.row_rect(row);
        Some(Point::new(r.x0 + 70.0, r.center().y))
    }

    /// The centre of the cross that deletes take `take`.
    pub fn take_delete_center(&self, group: u32, take: usize) -> Option<Point> {
        let ti = self.group_track(group)?;
        let row = self.take_row(&RowKind::Take {
            track: ti,
            group,
            take,
        })?;
        Some(self.take_delete_rect(self.row_rect(row)).center())
    }

    /// The point of take row `take` of `group` at song tick `tick` (for a pointer script).
    pub fn take_point(&self, group: u32, take: usize, tick: i64) -> Option<Point> {
        let ti = self.group_track(group)?;
        let row = self.take_row(&RowKind::Take {
            track: ti,
            group,
            take,
        })?;
        Some(Point::new(
            self.axis.tick_to_x(tick),
            self.row_rect(row).center().y,
        ))
    }

    /// The point of the comp strip of `group` at song tick `tick` (for a pointer script).
    pub fn take_strip_point(&self, group: u32, tick: i64) -> Option<Point> {
        let ti = self.group_track(group)?;
        let row = self.take_row(&RowKind::TakeGroup { track: ti, group })?;
        Some(Point::new(
            self.axis.tick_to_x(tick),
            self.row_rect(row).center().y,
        ))
    }

    fn group_track(&self, group: u32) -> Option<usize> {
        let device = self
            .song
            .take_group(group)
            .map(|(t, _, _)| t.device.clone())?;
        self.song.tracks.iter().position(|t| t.device == device)
    }

    // ------------------------------------------------------------------ hit testing

    /// What is under `pos` in a take row or group row (`None` for another row kind).
    pub(super) fn takes_hit(&self, kind: &RowKind, rr: Rect, pos: Point) -> Option<Hit> {
        match kind {
            RowKind::TakeGroup { group, .. } => Some(if pos.x < self.axis.x0 {
                if self.flatten_rect(rr).contains(pos) {
                    Hit::TakeFlatten(*group)
                } else if self.group_toggle_rect(rr).contains(pos) {
                    Hit::TakeToggle(*group)
                } else {
                    Hit::Empty
                }
            } else {
                Hit::TakeStrip(*group)
            }),
            RowKind::Take { group, take, .. } => Some(if pos.x < self.axis.x0 {
                if self.take_delete_rect(rr).contains(pos) {
                    Hit::TakeDelete(*group, *take)
                } else {
                    Hit::TakePick(*group, *take)
                }
            } else {
                Hit::TakeSpan(*group, *take)
            }),
            RowKind::Lane { .. }
            | RowKind::Folder { .. }
            | RowKind::AddTrack(_)
            | RowKind::AddAudio => None,
        }
    }

    // ------------------------------------------------------------------ gestures

    /// A press on a take control. `true` when the hit was one.
    pub(super) fn takes_down(
        &mut self,
        hit: &Hit,
        pos: Point,
        modifiers: Modifiers,
        doc: &atomik_model::RackDoc,
        registry: &atomik_device_sdk::Registry,
        out: &mut Vec<crate::action::UiAction>,
    ) -> bool {
        match hit {
            Hit::TakeToggle(group) => {
                let open = !self.tk.open.contains(group);
                self.set_takes_open(*group, open);
                self.sync(doc, registry);
                // The rows open at once; the song keeps the state for the file and for undo.
                out.push(song_cmd(SongCommand::SetTakesOpen {
                    take_group: *group,
                    open,
                }));
            }
            Hit::TakeFlatten(group) => {
                out.push(song_cmd(SongCommand::FlattenTakes { group: *group }));
            }
            Hit::TakePick(group, take) => {
                out.push(song_cmd(SongCommand::PickTake {
                    group: *group,
                    take: *take,
                }));
            }
            Hit::TakeDelete(group, take) => {
                out.push(song_cmd(SongCommand::DeleteTake {
                    group: *group,
                    take: *take,
                }));
            }
            Hit::TakeSpan(group, take) => {
                let t = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
                self.drag = Drag::Comp {
                    start: pos,
                    group: *group,
                    take: *take,
                    t0: t,
                    t1: t,
                    moved: false,
                    mode: CompDrag::Swipe,
                };
            }
            Hit::TakeStrip(group) => {
                let t = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
                let (mode, t0) = match self.strip_handle(*group, pos.x) {
                    Some((mode, at)) => (mode, at),
                    None => (CompDrag::Silence, t),
                };
                self.drag = Drag::Comp {
                    start: pos,
                    group: *group,
                    take: 0,
                    t0,
                    t1: t0,
                    moved: false,
                    mode,
                };
            }
            _ => return false,
        }
        true
    }

    /// Pointer motion during a comp drag.
    pub(super) fn comp_move(&mut self, pos: Point, modifiers: Modifiers) {
        let t = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
        if let Drag::Comp {
            start,
            t1,
            moved,
            mode,
            ..
        } = &mut self.drag
        {
            *moved |= pos.distance(*start) > super::DRAG_THRESHOLD;
            *t1 = match *mode {
                // A cut stays inside the two segments it divides.
                CompDrag::Cut { lo, hi, .. } => t.clamp(lo + 1, (hi - 1).max(lo + 1)),
                CompDrag::Xfade { at, .. } => t.max(at),
                CompDrag::Swipe | CompDrag::Silence => t,
            };
            self.repaint = true;
        }
    }

    /// The end of a comp drag: the range is the take's part of the comp, or, with no drag, the
    /// whole take is picked. Only commands the model accepts are sent.
    pub(super) fn comp_up(
        &mut self,
        group: u32,
        take: usize,
        (t0, t1): (i64, i64),
        moved: bool,
        mode: CompDrag,
        out: &mut Vec<crate::action::UiAction>,
    ) {
        let Some(g) = self.group(group) else {
            return;
        };
        let (rs, re) = g.range();
        match mode {
            CompDrag::Swipe => {
                if !moved {
                    out.push(song_cmd(SongCommand::PickTake { group, take }));
                    return;
                }
                let (start, end) = (t0.min(t1).max(rs), t0.max(t1).min(re));
                if start < end {
                    out.push(song_cmd(SongCommand::SetCompSegment {
                        group,
                        take,
                        start,
                        end,
                    }));
                }
            }
            CompDrag::Silence => {
                let (start, end) = (t0.min(t1).max(rs), t0.max(t1).min(re));
                if moved && start < end {
                    let mut comp = g.comp.clone();
                    clear_segment(&mut comp, start, end);
                    if comp != g.comp {
                        out.push(song_cmd(SongCommand::SetCompSilence { group, start, end }));
                    }
                }
            }
            CompDrag::Cut {
                left, right, at, ..
            } => {
                if !moved || t1 == at {
                    return;
                }
                // The take that grows takes the range the cut moved over.
                let cmd = if t1 > at {
                    SongCommand::SetCompSegment {
                        group,
                        take: left,
                        start: at,
                        end: t1,
                    }
                } else {
                    SongCommand::SetCompSegment {
                        group,
                        take: right,
                        start: t1,
                        end: at,
                    }
                };
                out.push(song_cmd(cmd));
            }
            CompDrag::Xfade { at } => {
                let ticks = (t1 - at).max(0) as u64;
                let current = g.comp.iter().find(|s| s.start == at).map(|s| s.xfade);
                if moved && current.is_some_and(|c| c != ticks) {
                    out.push(song_cmd(SongCommand::SetCompXfade {
                        group,
                        at,
                        xfade: ticks,
                    }));
                }
            }
        }
    }

    /// The cuts of a group's comp: `(left take, right take, tick, lo, hi)` for each place where
    /// a segment of one take ends exactly where a segment of another starts; `lo` and `hi` are
    /// the outer ends of the two segments.
    fn cuts(g: &TakeGroup) -> Vec<(usize, usize, i64, i64, i64)> {
        g.comp
            .windows(2)
            .filter(|w| w[0].end == w[1].start && w[0].take != w[1].take)
            .map(|w| (w[0].take, w[1].take, w[1].start, w[0].start, w[1].end))
            .collect()
    }

    /// Whether every take of the group is audio (the crossfade and the offset need it).
    fn audio_group(g: &TakeGroup) -> bool {
        g.takes.iter().all(|t| t.content.is_audio())
    }

    /// The cut handle or crossfade-tail handle of group `group` at x `x`, with the cut tick.
    fn strip_handle(&self, group: u32, x: f64) -> Option<(CompDrag, i64)> {
        let g = self.group(group)?;
        if Self::audio_group(g) {
            for s in g.comp.iter().filter(|s| s.xfade > 0) {
                let tail = self.axis.tick_to_x(s.start.saturating_add(s.xfade as i64));
                if (x - tail).abs() <= HANDLE_PX && Self::cuts(g).iter().any(|c| c.2 == s.start) {
                    return Some((CompDrag::Xfade { at: s.start }, s.start));
                }
            }
        }
        Self::cuts(g)
            .into_iter()
            .find(|c| (x - self.axis.tick_to_x(c.2)).abs() <= HANDLE_PX)
            .map(|(left, right, at, lo, hi)| {
                (
                    CompDrag::Cut {
                        left,
                        right,
                        at,
                        lo,
                        hi,
                    },
                    at,
                )
            })
    }

    /// Opens the right-click menu of a take row or of a group's comp strip.
    pub(super) fn takes_menu(&mut self, hit: &Hit, pos: Point) {
        let (group, take) = match hit {
            Hit::TakePick(g, t) | Hit::TakeDelete(g, t) | Hit::TakeSpan(g, t) => (*g, Some(*t)),
            Hit::TakeStrip(g) => (*g, None),
            _ => return,
        };
        let Some(g) = self.group(group) else {
            return;
        };
        let Some(device) = self
            .song
            .take_group(group)
            .map(|(t, _, _)| t.device.clone())
        else {
            return;
        };
        let mut rows: Vec<(String, SongCommand)> = Vec::new();
        let n = g.takes.len();
        match take {
            Some(t) => {
                if n < atomik_model::MAX_TAKES {
                    rows.push((
                        "Duplicate Take".into(),
                        SongCommand::DuplicateTake { group, take: t },
                    ));
                }
                if t > 0 {
                    rows.push((
                        "Move Take Up".into(),
                        SongCommand::MoveTake {
                            group,
                            take: t,
                            to: t - 1,
                        },
                    ));
                }
                if t + 1 < n {
                    rows.push((
                        "Move Take Down".into(),
                        SongCommand::MoveTake {
                            group,
                            take: t,
                            to: t + 1,
                        },
                    ));
                }
                if let Some(Clip {
                    content: ClipContent::Audio(a),
                    ..
                }) = g.takes.get(t)
                {
                    rows.extend(self.take_level_rows(group, t, a));
                }
                rows.push((
                    "Delete Take".into(),
                    SongCommand::DeleteTake { group, take: t },
                ));
            }
            None => {
                let x = pos.x;
                if let Some((_, at, _, _)) = self.nearest_cut(g, x)
                    && Self::audio_group(g)
                {
                    for (label, ticks) in XFADES {
                        let xfade = ticks.map_or(0, |d| (PPQ * 4 / d) as u64);
                        let current = g.comp.iter().find(|s| s.start == at).map_or(0, |s| s.xfade);
                        if xfade != current {
                            rows.push((
                                (*label).into(),
                                SongCommand::SetCompXfade { group, at, xfade },
                            ));
                        }
                    }
                }
                let tick = self.axis.x_to_tick(x);
                if let Some(s) = g.comp.iter().find(|s| s.start <= tick && tick < s.end) {
                    rows.push((
                        "Silence Segment".into(),
                        SongCommand::SetCompSilence {
                            group,
                            start: s.start,
                            end: s.end,
                        },
                    ));
                }
            }
        }
        let used = |t: usize| g.comp.iter().any(|s| s.take == t);
        let unused = (0..n).filter(|t| !used(*t)).count();
        if unused > 0 && unused < n {
            rows.push((
                "Delete Unused Takes".into(),
                SongCommand::DeleteUnusedTakes { group },
            ));
        }
        if rows.is_empty() {
            return;
        }
        let items = rows
            .iter()
            .enumerate()
            .map(|(i, (label, _))| (i.to_string(), label.clone()))
            .collect();
        let commands = rows.into_iter().map(|(_, c)| c).collect();
        let menu = super::Menu {
            device,
            kind: super::MenuKind::Takes(commands),
            items,
            origin: pos,
            scroll: 0,
            hover: None,
            current: None,
            checked: Vec::new(),
        };
        self.open_menu(menu);
    }

    /// The cut nearest to x `x` within a few pixels: `(index, tick, left take, right take)`.
    fn nearest_cut(&self, g: &TakeGroup, x: f64) -> Option<(usize, i64, usize, usize)> {
        Self::cuts(g)
            .into_iter()
            .enumerate()
            .map(|(i, c)| (i, c, (x - self.axis.tick_to_x(c.2)).abs()))
            .filter(|(_, _, d)| *d <= CUT_MENU_PX)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, c, _)| (i, c.2, c.0, c.1))
    }

    /// The level and offset rows of an audio take's menu: the steps the asset allows.
    fn take_level_rows(
        &self,
        group: u32,
        take: usize,
        a: &atomik_model::AudioClipContent,
    ) -> Vec<(String, SongCommand)> {
        let mut rows = Vec::new();
        for (label, db) in [("Take Level -3 dB", -3.0f32), ("Take Level +3 dB", 3.0)] {
            let gain_db = a.gain_db + db;
            if (AUDIO_GAIN_DB_MIN..=AUDIO_GAIN_DB_MAX).contains(&gain_db) {
                rows.push((
                    label.into(),
                    SongCommand::SetTake {
                        group,
                        take,
                        gain_db,
                        source_offset_delta: 0,
                    },
                ));
            }
        }
        if let Some(meta) = self.assets.get(&a.asset) {
            let step = i64::from(meta.sample_rate / 100);
            for (label, delta) in [("Take Earlier 10 ms", step), ("Take Later 10 ms", -step)] {
                // The audio moves with the offset: more offset plays later audio sooner.
                let offset = a.source_offset.checked_add_signed(delta);
                if offset.is_some_and(|o| o <= meta.frames) {
                    rows.push((
                        label.into(),
                        SongCommand::SetTake {
                            group,
                            take,
                            gain_db: a.gain_db,
                            source_offset_delta: delta,
                        },
                    ));
                }
            }
        }
        rows
    }

    // ------------------------------------------------------------------ painting

    /// Paints the timeline part of a group row or a take row.
    pub(super) fn paint_take_lane(&self, c: &mut dyn Canvas, kind: &RowKind, lr: Rect) {
        let st = &self.st;
        sp::rect(c, lr, st.lane_auto);
        match kind {
            RowKind::TakeGroup { group, .. } => {
                let Some(g) = self.group(*group) else { return };
                for s in &g.comp {
                    let r = self.span_rect(lr, s.start, s.end);
                    c.fill_with(r, take_color(s.take).with_alpha(0.85));
                    if r.width() > 14.0 {
                        sp::label(
                            c,
                            &(s.take + 1).to_string(),
                            r.x0 + 4.0,
                            r.center().y + 3.5,
                            9.0,
                            st.base.plate,
                            Align::Start,
                        );
                    }
                }
                let (rs, re) = g.range();
                c.stroke_with(
                    Rect::new(
                        self.axis.tick_to_x(rs),
                        lr.y0 + 3.0,
                        self.axis.tick_to_x(re),
                        lr.y1 - 3.0,
                    ),
                    1.0,
                    st.base.text_muted.with_alpha(0.6),
                );
                self.paint_cuts(c, lr, g);
            }
            RowKind::Take { group, take, .. } => {
                let Some(g) = self.group(*group) else { return };
                let Some(clip) = g.takes.get(*take) else {
                    return;
                };
                let col = take_color(*take);
                let body = self.span_rect(lr, clip.start, clip.end());
                c.fill_with(RoundedRect::from_rect(body, 2.0), col.with_alpha(0.16));
                c.stroke_with(RoundedRect::from_rect(body, 2.0), 1.0, col.with_alpha(0.55));
                let audio = match &clip.content {
                    ClipContent::Audio(a) => Some(a),
                    ClipContent::Notes(_) | ClipContent::Automation(_) => None,
                };
                // Under a waveform the played parts are a lighter wash so the wave stays readable.
                let played = if audio.is_some() { 0.3 } else { 0.7 };
                for s in g.comp.iter().filter(|s| s.take == *take) {
                    let r = self
                        .span_rect(lr, s.start.max(clip.start), s.end.min(clip.end()))
                        .intersect(body);
                    c.fill_with(r, col.with_alpha(played));
                }
                if let Some(a) = audio {
                    let tl = self.timeline_rect();
                    let vis = Rect::new(
                        body.x0.max(tl.x0 - 4.0),
                        body.y0,
                        body.x1.min(tl.x1 + 4.0),
                        body.y1,
                    );
                    if vis.width() > 0.0 {
                        let geo = self.audio_geo(body, clip, a, tl);
                        self.paint_audio_wave(
                            c,
                            vis,
                            body,
                            clip,
                            a,
                            mix(col, st.base.text, 0.35),
                            false,
                            tl,
                            &geo,
                        );
                    }
                } else {
                    self.paint_take_notes(c, lr, clip, mix(col, st.base.text, 0.5));
                }
                if let Drag::Comp {
                    group: dg,
                    take: dt,
                    t0,
                    t1,
                    moved: true,
                    mode: CompDrag::Swipe,
                    ..
                } = &self.drag
                    && dg == group
                    && dt == take
                {
                    let r = self.span_rect(lr, *t0.min(t1), *t0.max(t1));
                    c.fill_with(r, st.selection.with_alpha(0.25));
                    c.stroke_with(r, 1.0, st.selection);
                }
            }
            RowKind::Lane { .. }
            | RowKind::Folder { .. }
            | RowKind::AddTrack(_)
            | RowKind::AddAudio => {}
        }
    }

    /// The cut handles and crossfade tails of a group's comp strip, and the preview of the
    /// drag in progress (a silent range, a moving cut, a crossfade being set).
    fn paint_cuts(&self, c: &mut dyn Canvas, lr: Rect, g: &TakeGroup) {
        let st = &self.st;
        let (y0, y1) = (lr.y0 + 3.0, lr.y1 - 3.0);
        let audio = Self::audio_group(g);
        let drag = match &self.drag {
            Drag::Comp {
                group,
                t0,
                t1,
                moved: true,
                mode,
                ..
            } if *group == g.id => Some((*mode, *t0, *t1)),
            _ => None,
        };
        for (_, _, at, _, hi) in Self::cuts(g) {
            let Some(seg) = g.comp.iter().find(|s| s.start == at) else {
                continue;
            };
            let mut at_x = self.axis.tick_to_x(at);
            let mut xfade = seg.xfade;
            match drag {
                Some((CompDrag::Cut { at: a, .. }, _, t1)) if a == at => {
                    at_x = self.axis.tick_to_x(t1)
                }
                Some((CompDrag::Xfade { at: a }, _, t1)) if a == at => {
                    xfade = (t1 - at).max(0) as u64;
                }
                _ => {}
            }
            if audio && xfade > 0 {
                let end = at.saturating_add(xfade as i64).min(hi);
                let tail = Rect::new(at_x, y0, self.axis.tick_to_x(end).max(at_x), y1);
                c.fill_with(tail, st.base.plate.with_alpha(0.55));
                sp::line(c, tail.x1, y0, tail.x1, y1, st.base.text.with_alpha(0.9));
            }
            sp::line(c, at_x, y0, at_x, y1, st.base.text.with_alpha(0.9));
        }
        if let Some((CompDrag::Silence, t0, t1)) = drag {
            let r = self.span_rect(lr, t0.min(t1), t0.max(t1));
            c.fill_with(r, st.base.plate.with_alpha(0.6));
            c.stroke_with(r, 1.0, st.selection);
        }
    }

    /// The rectangle of the ticks `[a, b)` in a take or group row.
    fn span_rect(&self, lr: Rect, a: i64, b: i64) -> Rect {
        Rect::new(
            self.axis.tick_to_x(a),
            lr.y0 + 3.0,
            self.axis.tick_to_x(b),
            lr.y1 - 3.0,
        )
    }

    /// A mini view of the notes of a take: one tick mark per note at its key's height.
    fn paint_take_notes(&self, c: &mut dyn Canvas, lr: Rect, clip: &Clip, col: Color) {
        let ClipContent::Notes(notes) = &clip.content else {
            return;
        };
        let (lo, hi) = notes
            .iter()
            .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
        let span = f64::from(hi.saturating_sub(lo)).max(11.0);
        let (y0, y1) = (lr.y0 + 5.0, lr.y1 - 5.0);
        for n in notes {
            let s = clip.start + n.tick - clip.offset;
            let x0 = self.axis.tick_to_x(s);
            let x1 = self.axis.tick_to_x(s + n.length).max(x0 + 1.5);
            if x1 < self.axis.x0 || x0 > self.bounds.x1 {
                continue;
            }
            let y = y1 - (f64::from(n.key.saturating_sub(lo)) / span) * (y1 - y0);
            c.fill_with(Rect::new(x0, y - 0.75, x1, y + 0.75), col);
        }
    }

    /// Paints the header (track list side) of a group row or a take row.
    pub(super) fn paint_take_header(&self, c: &mut dyn Canvas, kind: &RowKind, hr: Rect) {
        let st = &self.st;
        let t = &st.base;
        sp::rect(c, hr, st.header_lane);
        match kind {
            RowKind::TakeGroup { group, .. } => {
                let Some(g) = self.group(*group) else { return };
                let open = self.tk.open.contains(group);
                let cx = hr.x0 + 16.0;
                let cy = hr.center().y;
                let mut tri = BezPath::new();
                if open {
                    tri.move_to(pt(cx - 4.0, cy - 2.5));
                    tri.line_to(pt(cx + 4.0, cy - 2.5));
                    tri.line_to(pt(cx, cy + 3.5));
                } else {
                    tri.move_to(pt(cx - 2.5, cy - 4.0));
                    tri.line_to(pt(cx + 3.5, cy));
                    tri.line_to(pt(cx - 2.5, cy + 4.0));
                }
                tri.close_path();
                let hovered = self.hover == Some(Hit::TakeToggle(*group));
                c.fill_with(tri, if hovered { t.text } else { t.text_muted });
                sp::label(
                    c,
                    &format!("Takes ({})", g.takes.len()),
                    hr.x0 + 28.0,
                    cy + 3.5,
                    9.5,
                    t.text,
                    Align::Start,
                );
                let hovered = self.hover == Some(Hit::TakeFlatten(*group));
                sp::button(c, st, self.flatten_rect(hr), "FLATTEN", None, hovered);
            }
            RowKind::Take { group, take, .. } => {
                let Some(g) = self.group(*group) else { return };
                let col = take_color(*take);
                let playing = g.comp.iter().any(|s| s.take == *take);
                sp::rect(
                    c,
                    Rect::new(hr.x0 + 16.0, hr.y0 + 5.0, hr.x0 + 21.0, hr.y1 - 5.0),
                    col,
                );
                let hovered = self.hover == Some(Hit::TakePick(*group, *take));
                let text_col = if playing || hovered {
                    t.text
                } else {
                    t.text_muted
                };
                let label = if g.picked() == Some(*take) {
                    format!("Take {} (active)", take + 1)
                } else if playing {
                    format!("Take {} (in comp)", take + 1)
                } else {
                    format!("Take {}", take + 1)
                };
                sp::label(
                    c,
                    &label,
                    hr.x0 + 28.0,
                    hr.center().y + 3.5,
                    9.5,
                    text_col,
                    Align::Start,
                );
                let r = self.take_delete_rect(hr);
                let hovered = self.hover == Some(Hit::TakeDelete(*group, *take));
                sp::button(c, st, r, "x", None, hovered);
            }
            RowKind::Lane { .. }
            | RowKind::Folder { .. }
            | RowKind::AddTrack(_)
            | RowKind::AddAudio => {}
        }
    }
}
