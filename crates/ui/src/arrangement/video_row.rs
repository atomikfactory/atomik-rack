//! The video row of the [`ArrangementView`]: one row above the track lanes that shows the song's
//! film as a clip bar with frame thumbnails, and edits it like a clip.
//!
//! * **Place.** Between the energy lane and the track lanes, full width (it does not scroll with
//!   the tracks), only when the song has a film and the row is not hidden. It can be collapsed to
//!   a thin bar (no thumbnails, same editing) with the chevron in its header cell; the host hides
//!   it with a command ([`ArrangementView::set_video_row`]) and keeps both choices in the
//!   interface state.
//! * **Thumbnails.** The bar is cut into slots as wide as a picture of the row's height; slot
//!   `k` starts `k` slots after the film's left edge and shows the frame that plays at its
//!   start. Only the slots in view are asked for ([`ArrangementView::video_wanted`]): the host
//!   decodes them on a worker and hands each back with [`ArrangementView::add_video_thumb`].
//!   The pictures are kept by frame number under a byte budget (least recently wanted out first),
//!   so scrolling back or zooming back finds them again; a slot waits for its picture as a plain
//!   placeholder. The view decodes and reads nothing.
//! * **States.** No file: a red labelled bar ("missing"). No preview yet (not built, being built,
//!   FFmpeg missing): a labelled bar with the host's note. Both still move and trim.
//! * **Editing.** A press selects the film (Escape or any other press lets go); a drag of the
//!   body moves it and a drag of either end trims it, each step one `Command::Video` with one
//!   merge id (one undo step) that Escape puts back. Moves snap to the song's grid and then to a
//!   whole frame of the song's clock, trims snap their edge to the grid and the cut to a whole
//!   frame of the film ([`atomik_model::video_snap`]); the no-snap modifier switches both off.
//!   With the film selected Delete removes it (one undo step).
//! * **Timecode.** The header shows the film's timecode at the playhead and the bar the timecode
//!   of its first and last frame, as the host worked them out ([`VideoFeed`]).
//!
//! The row is its own band: no lane or clip lies under it, so it takes no press a track would.

use super::{ArrangementView, DRAG_THRESHOLD, Drag};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::event::{Modifiers, PointerButton};
use crate::keymap::CommandOutcome;
use crate::peniko::{
    Blob, Brush, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality, ImageSampler,
};
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::snap_round;
use atomik_model::video_snap::{self, Snap};
use atomik_model::{Command, VideoClip, VideoCommand};
use std::collections::BTreeMap;
use std::sync::Arc;
use vello::kurbo::{Affine, BezPath, Point, Rect, RoundedRect};

/// Height of the video row.
pub const VIDEO_ROW_HEIGHT: f64 = 54.0;
/// Height of the collapsed video row.
pub const VIDEO_ROW_COLLAPSED: f64 = 18.0;
/// The height in pixels the host decodes thumbnails at (twice the picture's height on screen, so
/// they stay sharp on a dense display).
pub const VIDEO_THUMB_HEIGHT: u32 = 92;
/// Space above and below the pictures.
const PAD: f64 = 4.0;
/// Width of the grab zone at each end of the bar.
const EDGE: f64 = 7.0;
/// The narrowest and widest a thumbnail slot gets.
const SLOT_MIN: f64 = 24.0;
const SLOT_MAX: f64 = 170.0;
/// The most slots one paint walks (a very long film at a wide view).
const MAX_SLOTS: usize = 600;
/// What the thumbnails may hold, in bytes of pixels.
const THUMB_BUDGET: usize = 10 * 1024 * 1024;

/// A part of the film's bar under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VideoPart {
    /// The middle: a drag moves the film.
    Body,
    /// The left end: a drag trims the start.
    Start,
    /// The right end: a drag trims the end.
    End,
}

/// What the host tells the row about the film beyond the document: whether its file is there,
/// whether a preview exists, a line for when it does not, and the timecodes to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoFeed {
    /// The film's file is missing.
    pub missing: bool,
    /// How many frames the preview has; `None`: there is no preview to take pictures from.
    pub preview_frames: Option<u32>,
    /// Why there is no preview ("Preview not built", "Preparing 40%", ...).
    pub note: String,
    /// The film's timecode at the playhead (`HH:MM:SS:FF`).
    pub playhead_timecode: String,
    /// The timecode of the film's first frame in the bar.
    pub start_timecode: String,
    /// The timecode just after the film's last frame in the bar.
    pub end_timecode: String,
}

/// One thumbnail slot of the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoSlot {
    /// Left edge.
    pub x0: f64,
    /// Right edge (the last slot of a film may reach past the film's end: it is cut off).
    pub x1: f64,
    /// The frame of the preview the slot shows.
    pub frame: u32,
}

struct Thumb {
    width: u32,
    height: u32,
    brush: Brush,
    used: u64,
    bytes: usize,
}

impl std::fmt::Debug for Thumb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Thumb({}x{})", self.width, self.height)
    }
}

/// A drag of the film.
#[derive(Debug, Clone)]
pub(super) struct VideoDrag {
    part: VideoPart,
    origin: VideoClip,
    press: Point,
    /// The song time under the pointer at the press, in microseconds.
    press_us: i64,
    merge: u64,
    moved: bool,
    emitted: bool,
    /// What the last step set: `(start, in, out)`.
    last: (i64, u64, u64),
}

/// What the video row keeps.
#[derive(Debug)]
pub(super) struct VideoUi {
    /// The song's film (a copy of the document's, synced with it).
    pub(super) clip: Option<VideoClip>,
    /// The row is shown (when there is a film).
    shown: bool,
    collapsed: bool,
    selected: bool,
    feed: VideoFeed,
    thumbs: BTreeMap<u32, Thumb>,
    bytes: usize,
    clock: u64,
    drag: Option<VideoDrag>,
}

impl Default for VideoUi {
    fn default() -> Self {
        Self {
            clip: None,
            shown: true,
            collapsed: false,
            selected: false,
            feed: VideoFeed::default(),
            thumbs: BTreeMap::new(),
            bytes: 0,
            clock: 0,
            drag: None,
        }
    }
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the row is switched on (it only shows while the song has a film).
    pub fn video_row_visible(&self) -> bool {
        self.vid.shown
    }

    /// Switches the row on or off.
    pub fn set_video_row(&mut self, on: bool) {
        if on != self.vid.shown {
            self.vid.shown = on;
            self.video_row_changed();
        }
    }

    /// Whether the row is collapsed to a thin bar.
    pub fn video_row_collapsed(&self) -> bool {
        self.vid.collapsed
    }

    /// Collapses the row or opens it again.
    pub fn set_video_row_collapsed(&mut self, collapsed: bool) {
        if collapsed != self.vid.collapsed {
            self.vid.collapsed = collapsed;
            self.video_row_changed();
        }
    }

    fn video_row_changed(&mut self) {
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The row's rectangle over the whole view width (empty when there is no film or the row is
    /// hidden).
    pub fn video_row_rect(&self) -> Rect {
        Rect::new(
            self.bounds.x0,
            self.harmony_bottom(),
            self.bounds.x1.max(self.axis.x0),
            self.lanes_top(),
        )
    }

    /// The film is selected in the row (Delete removes it).
    pub fn video_selected(&self) -> bool {
        self.vid.selected
    }

    /// Hands the row what the host knows about the film.
    pub fn set_video_feed(&mut self, feed: VideoFeed) {
        if self.vid.feed != feed {
            self.vid.feed = feed;
            // The row is drawn live, over the recorded lanes: it never needs them re-recorded
            // (a timecode that changes with every frame of playback would).
            self.playhead_dirty = true;
        }
    }

    /// What the host last told the row.
    pub fn video_feed(&self) -> &VideoFeed {
        &self.vid.feed
    }

    /// Keeps the picture of frame `frame` (`width` by `height` RGBA pixels) for the slots that
    /// show it.
    pub fn add_video_thumb(&mut self, frame: u32, width: u32, height: u32, rgba: Arc<Vec<u8>>) {
        if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
            return;
        }
        let bytes = rgba.len();
        let brush = Brush::Image(ImageBrush {
            image: ImageData {
                data: Blob::new(rgba),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width,
                height,
            },
            sampler: ImageSampler::new().with_quality(ImageQuality::Medium),
        });
        let ui = &mut self.vid;
        ui.clock += 1;
        let thumb = Thumb {
            width,
            height,
            brush,
            used: ui.clock,
            bytes,
        };
        if let Some(old) = ui.thumbs.insert(frame, thumb) {
            ui.bytes -= old.bytes;
        }
        ui.bytes += bytes;
        while ui.bytes > THUMB_BUDGET && ui.thumbs.len() > 1 {
            let oldest = ui
                .thumbs
                .iter()
                .filter(|(k, _)| **k != frame)
                .min_by_key(|(_, t)| t.used)
                .map(|(k, _)| *k);
            let Some(oldest) = oldest else { break };
            if let Some(gone) = ui.thumbs.remove(&oldest) {
                ui.bytes -= gone.bytes;
            }
        }
        self.playhead_dirty = true;
    }

    /// Forgets every thumbnail (the host started on another film or preview).
    pub fn clear_video_thumbs(&mut self) {
        if !self.vid.thumbs.is_empty() {
            self.vid.thumbs.clear();
            self.vid.bytes = 0;
            self.playhead_dirty = true;
        }
    }

    /// Whether frame `frame` has its thumbnail.
    pub fn video_has_thumb(&self, frame: u32) -> bool {
        self.vid.thumbs.contains_key(&frame)
    }

    /// How many thumbnails are kept, and their bytes.
    pub fn video_thumbs(&self) -> (usize, usize) {
        (self.vid.thumbs.len(), self.vid.bytes)
    }

    /// The frames the slots in view show that have no thumbnail yet, the ones nearest the middle
    /// of the view first. The thumbnails the slots do have count as used now (they leave last).
    /// Empty while the row is collapsed or hidden, the film is missing or there is no preview.
    pub fn video_wanted(&mut self) -> Vec<u32> {
        let slots = self.video_slots();
        let middle = self.timeline_rect().center().x;
        self.vid.clock += 1;
        let clock = self.vid.clock;
        let mut missing: Vec<(f64, u32)> = Vec::new();
        for s in &slots {
            if let Some(t) = self.vid.thumbs.get_mut(&s.frame) {
                t.used = clock;
            } else if !missing.iter().any(|(_, f)| *f == s.frame) {
                missing.push((((s.x0 + s.x1) / 2.0 - middle).abs(), s.frame));
            }
        }
        missing.sort_by(|a, b| a.0.total_cmp(&b.0));
        missing.into_iter().map(|(_, f)| f).collect()
    }

    // ------------------------------------------------------------------ geometry

    /// Whether the row takes room: the song has a film and the row is on.
    pub(super) fn video_row_on(&self) -> bool {
        self.vid.shown && self.vid.clip.is_some()
    }

    /// The height of the row (0 when it takes no room).
    pub(super) fn video_row_height(&self) -> f64 {
        match (self.video_row_on(), self.vid.collapsed) {
            (false, _) => 0.0,
            (true, true) => VIDEO_ROW_COLLAPSED,
            (true, false) => VIDEO_ROW_HEIGHT,
        }
    }

    /// The film's bar: from its start to its end, as high as the row less the padding. `None`
    /// without a film. It may reach past the view on either side.
    pub fn video_bar_rect(&self) -> Option<Rect> {
        let clip = self.vid.clip.as_ref().filter(|_| self.video_row_on())?;
        let row = self.video_row_rect();
        let pad = if self.vid.collapsed { 2.0 } else { PAD };
        let (x0, x1) = self.video_x_range(clip);
        Some(Rect::new(x0, row.y0 + pad, x1.max(x0 + 1.0), row.y1 - pad))
    }

    /// The x of the film's start and end. A start before the song's first tick is drawn at that
    /// tick (nothing of the timeline lies left of it).
    fn video_x_range(&self, clip: &VideoClip) -> (f64, f64) {
        let start = clip.start_tick(&self.song);
        let end = clip.end_tick(&self.song);
        (self.axis.tick_to_x(start), self.axis.tick_to_x(end))
    }

    /// The width of one thumbnail slot: a picture of the film's shape as high as the bar.
    fn video_slot_width(&self, clip: &VideoClip) -> f64 {
        let h = f64::from(clip.facts.height.max(1));
        let aspect = f64::from(clip.facts.width.max(1)) / h;
        ((VIDEO_ROW_HEIGHT - 2.0 * PAD) * aspect).clamp(SLOT_MIN, SLOT_MAX)
    }

    /// The song time at logical `x`, in microseconds (negative times read as 0: the tempo map
    /// starts at the first tick).
    fn video_us_at(&self, x: f64) -> i64 {
        let tick = self.axis.scroll + (x - self.axis.x0) / self.axis.px_per_tick;
        (self.tempo.seconds_at_tick(tick) * 1e6).round() as i64
    }

    /// The thumbnail slots that are in view, left to right (none while collapsed, missing or
    /// without a preview).
    pub fn video_slots(&self) -> Vec<VideoSlot> {
        let Some(clip) = self.vid.clip.as_ref().filter(|_| self.video_row_on()) else {
            return Vec::new();
        };
        let Some(frames) = self.vid.feed.preview_frames.filter(|f| *f > 0) else {
            return Vec::new();
        };
        if self.vid.collapsed || self.vid.feed.missing {
            return Vec::new();
        }
        let tl = self.timeline_rect();
        let (x0, x1) = self.video_x_range(clip);
        let slot = self.video_slot_width(clip);
        let (from, to) = (tl.x0.max(x0), tl.x1.min(x1));
        if to <= from {
            return Vec::new();
        }
        let first = ((from - x0) / slot).floor().max(0.0) as usize;
        let last = ((to - x0) / slot).ceil().max(0.0) as usize;
        let rate = (clip.facts.fps_num.max(1), clip.facts.fps_den.max(1));
        (first..last.min(first + MAX_SLOTS))
            .map(|k| {
                let sx = x0 + k as f64 * slot;
                // The first slot shows the film's first frame whatever the rounding of its
                // edge; the others the picture that plays at their start.
                let at_us = if k == 0 {
                    clip.start_us.max(0)
                } else {
                    self.video_us_at(sx)
                };
                let into = (at_us - clip.start_us).max(0);
                let source = clip.in_us.saturating_add(into as u64).min(clip.out_us);
                let number = frame_of(source, rate);
                VideoSlot {
                    x0: sx,
                    x1: sx + slot,
                    frame: number.min(i64::from(frames - 1)) as u32,
                }
            })
            .collect()
    }

    /// What lies at `pos` in the row's band (the caller checked the y range).
    pub(super) fn video_hit(&self, pos: Point) -> super::Hit {
        let tl = self.timeline_rect();
        if pos.x < tl.x0 {
            return if self.video_toggle_rect().contains(pos) {
                super::Hit::VideoToggle
            } else {
                super::Hit::VideoHeader
            };
        }
        let Some(bar) = self.video_bar_rect() else {
            return super::Hit::Empty;
        };
        if pos.x < bar.x0 || pos.x > bar.x1 || pos.y < bar.y0 || pos.y > bar.y1 {
            return super::Hit::Empty;
        }
        // An end can only be grabbed where it is in view.
        let zone = EDGE.min(bar.width() / 3.0);
        let in_view = |x: f64| x >= tl.x0 && x <= tl.x1;
        let part = if pos.x <= bar.x0 + zone && in_view(bar.x0) {
            VideoPart::Start
        } else if pos.x >= bar.x1 - zone && in_view(bar.x1) {
            VideoPart::End
        } else {
            VideoPart::Body
        };
        super::Hit::VideoBar(part)
    }

    /// The chevron button of the header cell.
    fn video_toggle_rect(&self) -> Rect {
        let row = self.video_row_rect();
        Rect::new(
            row.x0 + 4.0,
            row.y0 + 2.0,
            row.x0 + 22.0,
            (row.y0 + 20.0).min(row.y1),
        )
    }

    // ------------------------------------------------------------------ gestures

    /// A press on the row.
    pub(super) fn video_down(
        &mut self,
        part: Option<VideoPart>,
        pos: Point,
        button: PointerButton,
    ) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.og.group = None;
        self.vid.selected = true;
        let Some(origin) = self.vid.clip.clone() else {
            return;
        };
        let (Some(part), PointerButton::Primary) = (part, button) else {
            return;
        };
        let merge = self.merge_id();
        self.vid.drag = Some(VideoDrag {
            part,
            press: pos,
            press_us: self.video_us_at(pos.x),
            merge,
            moved: false,
            emitted: false,
            last: (origin.start_us, origin.in_us, origin.out_us),
            origin,
        });
        self.drag = Drag::Video;
    }

    /// The part of the film under a running drag.
    pub(super) fn video_drag_part(&self) -> Option<VideoPart> {
        self.vid.drag.as_ref().map(|d| d.part)
    }

    /// The chevron: collapses or opens the row.
    pub(super) fn video_toggle(&mut self) {
        self.set_video_row_collapsed(!self.vid.collapsed);
    }

    /// Pointer motion during a drag of the film: one command per step that changes something,
    /// all with the drag's merge id.
    pub(super) fn video_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let off = self.vp.snap_disabled(modifiers);
        let Some(d) = self.vid.drag.clone() else {
            return;
        };
        if !d.moved && pos.distance(d.press) <= DRAG_THRESHOLD {
            return;
        }
        let rate = (d.origin.facts.fps_num, d.origin.facts.fps_den);
        let delta = self.video_us_at(pos.x) - d.press_us;
        let grid = self.grid(modifiers);
        let tempo = &self.tempo;
        let on_grid = move |us: i64| -> i64 {
            if us < 0 {
                return us;
            }
            let tick = tempo.tick_at_seconds(us as f64 / 1e6).round() as i64;
            let snapped = snap_round(tick, grid.clone());
            (tempo.seconds_at_tick(snapped as f64) * 1e6).round() as i64
        };
        let snap = if off {
            Snap::OFF
        } else {
            Snap {
                grid: Some(&on_grid),
                frame: true,
            }
        };
        let clip = &d.origin;
        let (start, in_us, out_us) = match d.part {
            VideoPart::Body => (
                video_snap::moved_start(clip.start_us.saturating_add(delta), rate.0, rate.1, snap),
                clip.in_us,
                clip.out_us,
            ),
            VideoPart::Start => {
                let edge = clip.start_us.saturating_add(delta);
                let (i, o) = video_snap::trimmed_in(clip, edge, snap);
                let start = i128::from(clip.start_us) + i128::from(i) - i128::from(clip.in_us);
                (i64::try_from(start).unwrap_or(clip.start_us), i, o)
            }
            VideoPart::End => {
                let edge = clip.end_us().saturating_add(delta);
                let (i, o) = video_snap::trimmed_out(clip, edge, snap);
                (clip.start_us, i, o)
            }
        };
        let Some(live) = &mut self.vid.drag else {
            return;
        };
        live.moved = true;
        self.repaint = true;
        if (start, in_us, out_us) == live.last {
            return;
        }
        live.last = (start, in_us, out_us);
        live.emitted = true;
        let merge = Some(live.merge);
        out.push(UiAction::Command(Command::Video(match d.part {
            VideoPart::Body => VideoCommand::Move {
                start_us: start,
                merge,
            },
            VideoPart::Start | VideoPart::End => VideoCommand::Trim {
                in_us,
                out_us,
                merge,
            },
        })));
    }

    /// The release: the steps of the drag become one undo step.
    pub(super) fn video_up(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.vid.drag.take()
            && d.emitted
        {
            out.push(UiAction::EndMerge);
        }
    }

    /// Escape during a drag: the film goes back to where it was.
    pub(super) fn video_cancel(&mut self, out: &mut Vec<UiAction>) {
        self.drag = Drag::None;
        let Some(d) = self.vid.drag.take() else {
            return;
        };
        if !d.emitted {
            return;
        }
        let merge = Some(d.merge);
        let o = &d.origin;
        out.push(UiAction::Command(Command::Video(match d.part {
            VideoPart::Body => VideoCommand::Move {
                start_us: o.start_us,
                merge,
            },
            VideoPart::Start | VideoPart::End => VideoCommand::Trim {
                in_us: o.in_us,
                out_us: o.out_us,
                merge,
            },
        })));
        out.push(UiAction::EndMerge);
    }

    /// Escape without a drag: the film is no longer selected.
    pub(super) fn video_deselect(&mut self) {
        self.vid.selected = false;
    }

    /// `edit.delete` with the film selected removes it.
    pub(super) fn video_command(&mut self, id: &str) -> Option<CommandOutcome> {
        if id == "edit.delete" && self.vid.selected && self.vid.clip.is_some() {
            self.vid.selected = false;
            return Some(CommandOutcome::Handled(vec![UiAction::Command(
                Command::Video(VideoCommand::Remove),
            )]));
        }
        None
    }

    /// Forgets the selection of a film that is gone.
    pub(super) fn video_synced(&mut self, clip: Option<&VideoClip>) {
        if clip.is_none() {
            self.vid.selected = false;
            self.vid.drag = None;
            if matches!(self.drag, Drag::Video) {
                self.drag = Drag::None;
            }
        }
        self.clamp_scroll();
    }

    // ------------------------------------------------------------------ painting

    /// The row: header cell, bar with its thumbnails or its message, playhead.
    pub(super) fn paint_video_row(&self, c: &mut dyn Canvas) {
        let Some(clip) = self.vid.clip.as_ref().filter(|_| self.video_row_on()) else {
            return;
        };
        let st = &self.st;
        let t = &st.base;
        let row = self.video_row_rect();
        let tl = self.timeline_rect();
        let feed = &self.vid.feed;
        // Header cell.
        let header = Rect::new(row.x0, row.y0, tl.x0, row.y1);
        sp::rect(c, header, st.header_lane);
        let chevron = self.video_toggle_rect();
        let hot = matches!(self.hover, Some(super::Hit::VideoToggle));
        let tint = if hot { t.text } else { t.text_muted };
        let mut tri = BezPath::new();
        let (cx, cy) = (chevron.center().x, chevron.center().y);
        if self.vid.collapsed {
            tri.move_to(pt(cx - 2.5, cy - 4.0));
            tri.line_to(pt(cx + 3.0, cy));
            tri.line_to(pt(cx - 2.5, cy + 4.0));
        } else {
            tri.move_to(pt(cx - 4.0, cy - 2.5));
            tri.line_to(pt(cx + 4.0, cy - 2.5));
            tri.line_to(pt(cx, cy + 3.0));
        }
        tri.close_path();
        c.fill_with(tri, tint);
        sp::label(
            c,
            "VIDEO",
            header.x0 + 28.0,
            row.y0 + if self.vid.collapsed { 13.0 } else { 17.0 },
            8.0,
            t.text_muted,
            Align::Start,
        );
        if !feed.playhead_timecode.is_empty() {
            if self.vid.collapsed {
                sp::label(
                    c,
                    &feed.playhead_timecode,
                    header.x1 - 8.0,
                    row.y0 + 13.0,
                    10.0,
                    t.highlight,
                    Align::End,
                );
            } else {
                sp::label(
                    c,
                    &feed.playhead_timecode,
                    header.x0 + 12.0,
                    row.y0 + 36.0,
                    12.0,
                    t.highlight,
                    Align::Start,
                );
            }
        }
        // Timeline part.
        let timeline = Rect::new(tl.x0, row.y0, row.x1, row.y1);
        sp::rect(c, timeline, st.lane_auto);
        sp::line(c, row.x0, row.y1 - 0.5, row.x1, row.y1 - 0.5, st.separator);
        c.push_clip(&timeline.into());
        if let Some(bar) = self.video_bar_rect() {
            self.paint_video_bar(c, clip, bar, tl);
        }
        // The playhead runs through the row like it does through the lanes.
        let x = self.axis.tick_to_x(self.playhead);
        if x >= tl.x0 - 1.0 && x <= tl.x1 + 1.0 {
            let col = if self.recording {
                st.record
            } else {
                st.playhead
            };
            let x = crate::paint::snap_hairline(x, self.scale);
            c.stroke_with(
                vello::kurbo::Line::new(pt(x, row.y0), pt(x, row.y1)),
                1.5,
                col,
            );
        }
        c.pop_clip();
    }

    fn paint_video_bar(&self, c: &mut dyn Canvas, clip: &VideoClip, bar: Rect, tl: Rect) {
        let st = &self.st;
        let t = &st.base;
        let feed = &self.vid.feed;
        let shape = RoundedRect::from_rect(bar, 3.0);
        let ready = !feed.missing && feed.preview_frames.is_some_and(|f| f > 0);
        let body = if feed.missing {
            mix(st.background, st.record, 0.35)
        } else {
            mix(st.lane_auto, t.ink, 0.1)
        };
        c.fill_with(shape, body);
        c.push_clip(&Rect::new(bar.x0.max(tl.x0), bar.y0, bar.x1.min(tl.x1), bar.y1).into());
        if ready && !self.vid.collapsed {
            for s in self.video_slots() {
                let r = Rect::new(s.x0, bar.y0, s.x1, bar.y1);
                if let Some(th) = self.vid.thumbs.get(&s.frame) {
                    let sx = (s.x1 - s.x0) / f64::from(th.width);
                    let sy = bar.height() / f64::from(th.height);
                    c.push_transform(
                        Affine::translate((r.x0, r.y0)) * Affine::scale_non_uniform(sx, sy),
                    );
                    c.fill_with(
                        Rect::new(0.0, 0.0, f64::from(th.width), f64::from(th.height)),
                        th.brush.clone(),
                    );
                    c.pop_transform();
                }
                sp::line(
                    c,
                    s.x0 + 0.5,
                    bar.y0,
                    s.x0 + 0.5,
                    bar.y1,
                    st.separator.with_alpha(0.7),
                );
            }
        }
        // The words: the file, then why there is no picture.
        let say = |c: &mut dyn Canvas, text: &str, x: f64, align: Align| {
            let h = bar.height();
            let w = text.chars().count() as f64 * 6.2 + 10.0;
            let (x0, x1) = match align {
                Align::End => (x - w, x),
                _ => (x, x + w),
            };
            let plate = Rect::new(
                x0,
                bar.center().y - 7.0_f64.min(h / 2.0),
                x1,
                bar.center().y + 7.0_f64.min(h / 2.0),
            );
            c.fill_with(
                RoundedRect::from_rect(plate, 2.0),
                st.background.with_alpha(0.72),
            );
            let tx = if align == Align::End {
                x - 5.0
            } else {
                x + 5.0
            };
            sp::label(c, text, tx, bar.center().y + 3.5, 10.0, t.text, align);
        };
        let left = bar.x0.max(tl.x0) + 4.0;
        let name = clip.name();
        if feed.missing {
            say(c, &format!("MISSING  {name}"), left, Align::Start);
        } else if !ready {
            let note = if feed.note.is_empty() {
                "No preview"
            } else {
                &feed.note
            };
            say(c, &format!("{name}  {note}"), left, Align::Start);
        } else if !self.vid.collapsed || bar.width() > 160.0 {
            let text = if feed.start_timecode.is_empty() {
                name.to_owned()
            } else {
                format!("{name}  {}", feed.start_timecode)
            };
            say(c, &text, left, Align::Start);
        }
        if !feed.end_timecode.is_empty() && bar.x1 <= tl.x1 && bar.width() > 320.0 {
            say(c, &feed.end_timecode, bar.x1 - 4.0, Align::End);
        }
        c.pop_clip();
        // Outline, and the ends when they can be grabbed.
        let lit = self.vid.selected;
        let edge = if lit { st.selection } else { st.separator };
        c.stroke_with(shape, if lit { 2.0 } else { 1.0 }, edge);
        let hover = match (&self.vid.drag, &self.hover) {
            (Some(d), _) => Some(d.part),
            (None, Some(super::Hit::VideoBar(p))) => Some(*p),
            _ => None,
        };
        let zone = EDGE.min(bar.width() / 3.0);
        for (part, r) in [
            (
                VideoPart::Start,
                Rect::new(bar.x0, bar.y0, bar.x0 + zone, bar.y1),
            ),
            (
                VideoPart::End,
                Rect::new(bar.x1 - zone, bar.y0, bar.x1, bar.y1),
            ),
        ] {
            if hover == Some(part) || lit {
                let a = if hover == Some(part) { 0.55 } else { 0.25 };
                c.fill_with(r, t.ink.with_alpha(a));
            }
        }
    }
}

/// The frame number at `source_us` microseconds into the film at the rate `(num, den)`.
fn frame_of(source_us: u64, rate: (u32, u32)) -> i64 {
    let micros = i128::from(source_us.min(i64::MAX as u64));
    let n = micros * i128::from(rate.0) / (i128::from(rate.1) * 1_000_000);
    n.min(i128::from(i64::MAX)) as i64
}
