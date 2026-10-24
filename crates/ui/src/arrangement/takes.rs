//! Take groups in the [`ArrangementView`]: a thin comp strip under the lane of a track that has
//! a group, and, when it is opened, one row per take (stacked sub-lanes).
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
//!
//! Which groups are open is saved with the project (`SetTakesOpen`; the view shows a click at
//! once). The rows are laid out below the lane row (lane 0, the notes or audio lane) of their
//! track, before its automation lanes.

use super::{ArrangementView, Drag, Hit, Rect, Row, RowKind, TRACK_LIST_WIDTH, song_cmd};
use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::event::Modifiers;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::snap_round;
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

    /// The rows that follow lane 0 of track `ti`: per take group its strip and, when open, the
    /// take rows. Heights are fixed (they do not follow the vertical zoom).
    pub(super) fn take_rows(&self, ti: usize) -> Vec<(RowKind, f64)> {
        let mut rows = Vec::new();
        let Some(lane) = self.song.tracks.get(ti).and_then(|t| t.lanes.first()) else {
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
    fn group_toggle_rect(&self, row: Rect) -> Rect {
        Rect::new(row.x0, row.y0, row.x0 + 132.0, row.y1)
    }

    /// The FLATTEN button of a group row's header.
    fn flatten_rect(&self, row: Rect) -> Rect {
        Rect::new(
            row.x0 + TRACK_LIST_WIDTH - 66.0,
            row.y0 + 2.0,
            row.x0 + TRACK_LIST_WIDTH - 8.0,
            row.y1 - 2.0,
        )
    }

    /// The cross that deletes a take, in a take row's header.
    fn take_delete_rect(&self, row: Rect) -> Rect {
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
                };
            }
            Hit::TakeStrip(_) => {}
            _ => return false,
        }
        true
    }

    /// Pointer motion during a comp drag.
    pub(super) fn comp_move(&mut self, pos: Point, modifiers: Modifiers) {
        let t = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
        if let Drag::Comp {
            start, t1, moved, ..
        } = &mut self.drag
        {
            *moved |= pos.distance(*start) > super::DRAG_THRESHOLD;
            *t1 = t;
            self.repaint = true;
        }
    }

    /// The end of a comp drag: the range is the take's part of the comp, or, with no drag, the
    /// whole take is picked.
    pub(super) fn comp_up(
        &mut self,
        group: u32,
        take: usize,
        (t0, t1): (i64, i64),
        moved: bool,
        out: &mut Vec<crate::action::UiAction>,
    ) {
        let Some(g) = self.group(group) else {
            return;
        };
        if !moved {
            out.push(song_cmd(SongCommand::PickTake { group, take }));
            return;
        }
        let (rs, re) = g.range();
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
                for s in g.comp.iter().filter(|s| s.take == *take) {
                    let r = self
                        .span_rect(lr, s.start.max(clip.start), s.end.min(clip.end()))
                        .intersect(body);
                    c.fill_with(r, col.with_alpha(0.7));
                }
                self.paint_take_notes(c, lr, clip, mix(col, st.base.text, 0.5));
                if let Drag::Comp {
                    group: dg,
                    take: dt,
                    t0,
                    t1,
                    moved: true,
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
