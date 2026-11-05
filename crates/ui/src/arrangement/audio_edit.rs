//! Audio editing in the [`ArrangementView`]: the gain envelope of a clip, the crossfade zones,
//! the reverse mark and the audio clip commands.
//!
//! * The **gain envelope** is drawn over the waveform as a line with a dot on every point, on
//!   the same dB scale as the clip gain handle ([`audio::GAIN_MIN_DB`] at the bottom of the
//!   clip, [`audio::GAIN_MAX_DB`] at the top). The pencil adds a point where it is clicked on a
//!   clip and drags it at once; a point of any clip can be dragged with the select tool, and a
//!   right click or the eraser removes it. Points sit at frames from the clip start (a time, like
//!   a fade), so they follow the clip when it moves and the audio when its left edge is trimmed.
//!   A point being placed or dragged snaps to the grid (the disable-snap role turns it off).
//!   Shift-click or Ctrl-click (Cmd on macOS) adds a point to the selection or takes it out;
//!   dragging a selected point moves every selected point of the clip together, and Delete
//!   removes them together (see [`ArrangementView::selected_envelope_points`]).
//! * A **crossfade zone** (where a clip starts over the end of an earlier one, see
//!   [`atomik_model::song::crossfades`]) is shaded at the start of the later clip with the two
//!   gain curves crossing.
//! * A **reversed clip** draws its waveform right to left and carries an `R` mark in its header.
//! * The commands (`sequencer.audio_*`) change the selected audio clips in one undo step:
//!   reverse, clip gain, fade shape, crossfade shape, fades and clearing the envelope.
//!   Normalise and Strip Silence need the samples and live in the app.

use super::{
    ArrangementView, AudioGeo, AudioGrab, CLIP_HEADER, DEFAULT_RATE, Drag, Menu, MenuKind,
    POINT_RADIUS, audio_clip_cmd, find_clip, song_cmd, value_range,
};
use crate::action::UiAction;
use crate::audio::{self, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::canvas::{Canvas, CanvasExt, pt};
use crate::event::Modifiers;
use crate::keymap::CommandOutcome;
use crate::seq_paint as sp;
use crate::timeline::{Quant, snap_round};
use atomik_model::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN};
use atomik_model::song::{Crossfade, crossfades};
use atomik_model::{AudioClipContent, Clip, ClipContent, Command};
use atomik_model::{ClipId, FadeCurve, SongCommand};
use std::collections::{BTreeMap, BTreeSet};
use vello::kurbo::{BezPath, Circle, Point, Rect, RoundedRect, Stroke};

/// Frames of a declick fade as milliseconds.
const DECLICK_MS: u64 = 5;
/// Gain step of `sequencer.audio_gain` up and down, in dB.
const GAIN_STEP_DB: f32 = 1.0;
/// Pixels between the samples of the envelope curve.
const ENVELOPE_STEP: f64 = 2.0;

/// An edit of one audio clip: the clip, its content and its asset's rate in, the new content
/// out (`None`: nothing to change).
type ClipEdit = Box<dyn Fn(&Clip, &AudioClipContent, u32) -> Option<AudioClipContent>>;

/// What the selected audio clips allow, for the menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioMenu {
    /// At least one audio clip is selected.
    pub can_edit: bool,
    /// Every selected audio clip is reversed (the Reverse item is checked).
    pub reversed: bool,
    /// A selected audio clip has a gain envelope.
    pub envelope: bool,
    /// Every selected audio clip is warped (the Warp item is checked).
    pub warped: bool,
    /// An audio clip is selected, so it can be warped or transposed.
    pub can_warp: bool,
}

/// The view state of audio editing.
#[derive(Debug, Default)]
pub(super) struct AudioUi {
    /// The crossfade zones by the clip that fades in.
    pub(super) zones: BTreeMap<ClipId, Crossfade>,
    /// The selected points of one clip's gain envelope (positions in its point list).
    pub(super) env_sel: Option<(ClipId, BTreeSet<usize>)>,
}

impl ArrangementView {
    /// The crossfade zones of the song again (after the song changed).
    pub(super) fn rebuild_audio_ui(&mut self) {
        self.au.zones.clear();
        for track in self.song.tracks.iter().filter(|t| t.is_audio()) {
            for lane in &track.lanes {
                for z in crossfades(lane) {
                    self.au.zones.insert(z.later, z);
                }
            }
        }
    }

    /// The selected points of an audio clip's gain envelope: the clip and the positions in its
    /// point list, ascending.
    pub fn selected_envelope_points(&self) -> Option<(ClipId, Vec<usize>)> {
        self.au
            .env_sel
            .as_ref()
            .map(|(id, s)| (*id, s.iter().copied().collect()))
    }

    /// Selects points of an audio clip's gain envelope (the clip selection becomes that clip);
    /// an empty list clears the selection.
    pub fn set_selected_envelope_points(&mut self, clip: ClipId, indices: &[usize]) {
        if indices.is_empty() {
            self.au.env_sel = None;
        } else {
            self.au.env_sel = Some((clip, indices.iter().copied().collect()));
            self.selection.select_only(clip);
            self.selected_track = None;
            self.point_sel = None;
        }
        self.repaint = true;
    }

    /// Whether point `index` of clip `id`'s envelope is selected.
    pub(super) fn envelope_point_selected(&self, id: ClipId, index: usize) -> bool {
        matches!(&self.au.env_sel, Some((c, s)) if *c == id && s.contains(&index))
    }

    /// Adds point `index` of clip `id`'s envelope to the selection, or takes it out again.
    pub(super) fn toggle_envelope_point(&mut self, id: ClipId, index: usize) {
        let mut set = match &self.au.env_sel {
            Some((c, s)) if *c == id => s.clone(),
            _ => BTreeSet::new(),
        };
        if !set.remove(&index) {
            set.insert(index);
        }
        self.au.env_sel = (!set.is_empty()).then_some((id, set));
        self.repaint = true;
    }

    /// Drops selected envelope points that no longer exist (after the song changed).
    pub(super) fn retain_envelope_selection(&mut self) {
        let Some((id, set)) = &mut self.au.env_sel else {
            return;
        };
        let len = match find_clip(&self.song, *id) {
            Some((_, _, c)) if self.selection.contains(id) => match &c.content {
                ClipContent::Audio(a) => a.envelope.len(),
                _ => 0,
            },
            _ => 0,
        };
        set.retain(|i| *i < len);
        if set.is_empty() {
            self.au.env_sel = None;
        }
    }

    /// The command that removes the selected envelope points together, and forgets them.
    pub(super) fn delete_envelope_points(&mut self) -> Option<UiAction> {
        let (id, set) = self.au.env_sel.take()?;
        let (_, _, clip) = self.find_clip(id)?;
        let ClipContent::Audio(a) = &clip.content else {
            return None;
        };
        let cmd = self.envelope_edit_cmd(clip, a, |p| audio::envelope_remove_many(p, &set));
        self.repaint = true;
        Some(cmd)
    }

    /// What the selected audio clips allow.
    pub fn audio_menu(&self) -> AudioMenu {
        let clips = self.selected_audio();
        AudioMenu {
            can_edit: !clips.is_empty(),
            reversed: !clips.is_empty() && clips.iter().all(|(_, a)| a.reverse),
            envelope: clips.iter().any(|(_, a)| !a.envelope.is_empty()),
            warped: !clips.is_empty() && clips.iter().all(|(_, a)| a.warp.enabled),
            can_warp: !clips.is_empty(),
        }
    }

    /// What the transport bar's clip inspector shows: the selected audio clips (with the rate
    /// and the length in frames of each), or the selected audio track when no clip is, or
    /// nothing.
    pub fn audio_inspector(&self) -> crate::InspectorSource {
        let clips: Vec<crate::InspClip> = self
            .selected_audio()
            .into_iter()
            .map(|(c, a)| {
                let rate = self
                    .assets
                    .get(&a.asset)
                    .map_or(DEFAULT_RATE, |m| m.sample_rate);
                crate::InspClip {
                    id: c.id,
                    start: c.start,
                    length: c.length,
                    muted: c.muted,
                    content: a.clone(),
                    rate,
                    frames: self
                        .tempo
                        .frame_at(c.start + c.length, rate)
                        .saturating_sub(self.tempo.frame_at(c.start, rate)),
                }
            })
            .collect();
        if !clips.is_empty() {
            return crate::InspectorSource::Clips(clips);
        }
        match self.selected_track() {
            Some(d) if self.song.track(d).is_some_and(|t| t.is_audio()) => {
                crate::InspectorSource::Track(d.clone())
            }
            _ => crate::InspectorSource::None,
        }
    }

    /// The selected audio clips with their content, in song order.
    fn selected_audio(&self) -> Vec<(&Clip, &AudioClipContent)> {
        self.selected_in_order()
            .into_iter()
            .filter_map(|c| match &c.content {
                ClipContent::Audio(a) => Some((c, a)),
                _ => None,
            })
            .collect()
    }

    /// The pixel of every envelope point of `clip`, with its index in the list.
    pub(super) fn envelope_points(
        &self,
        r: Rect,
        a: &AudioClipContent,
        g: &AudioGeo,
    ) -> Vec<(usize, Point)> {
        let (top, bottom) = value_range(r);
        a.envelope
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let x = self.x_of_tick(
                    self.tempo
                        .tick_at_frame(g.start_frame + p.frame as f64, g.rate),
                );
                (i, pt(x, audio::gain_to_y(p.gain_db, top, bottom)))
            })
            .collect()
    }

    /// Centre of point `index` of the gain envelope of audio clip `id` (logical pixels), if the
    /// clip is an audio clip in a shown lane and has that point.
    pub fn envelope_point_center(&self, id: ClipId, index: usize) -> Option<Point> {
        let (track, lane, clip) = self.find_clip(id)?;
        let ClipContent::Audio(a) = &clip.content else {
            return None;
        };
        let r = self.clip_rect_in(self.row_rect(self.row_of(track, lane)?), clip);
        let g = self.audio_geo(r, clip, a, self.timeline_rect());
        self.envelope_points(r, a, &g)
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, p)| p)
    }

    /// The crossfade zone at the start of audio clip `id` as `(start tick, end tick)`: where it
    /// starts over the end of an earlier clip and fades in over it.
    pub fn crossfade_zone(&self, id: ClipId) -> Option<(i64, i64)> {
        self.au.zones.get(&id).map(|z| (z.start, z.end))
    }

    /// The envelope point of `clip` under `pos`, if any.
    pub(super) fn envelope_point_at(
        &self,
        pos: Point,
        r: Rect,
        clip: &Clip,
        a: &AudioClipContent,
        tl: Rect,
    ) -> Option<usize> {
        if a.envelope.is_empty() {
            return None;
        }
        let g = self.audio_geo(r, clip, a, tl);
        self.envelope_points(r, a, &g)
            .into_iter()
            .map(|(i, p)| (i, p.distance(pos)))
            .filter(|(_, d)| *d <= POINT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The clip frame under a logical x (not below 0).
    fn frame_under(&self, x: f64, g: &AudioGeo) -> u64 {
        let f = self.tempo.frame_at_tick(self.tick_of_x(x), g.rate) - g.start_frame;
        f.round().max(0.0) as u64
    }

    /// Starts an envelope drag on `clip` with the point list `list` and `index` the point
    /// being dragged; emits the command that makes the list.
    pub(super) fn start_envelope_drag(
        &mut self,
        clip: &Clip,
        content: &AudioClipContent,
        list: Vec<atomik_model::GainPoint>,
        index: usize,
        pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        let Some((track, lane, _)) = self.find_clip(clip.id) else {
            return;
        };
        let Some(row) = self.row_of(track, lane).map(|r| self.row_rect(r)) else {
            return;
        };
        let (top, bottom) = value_range(self.clip_rect_in(row, clip));
        let merge = self.merge_id();
        let mut edited = content.clone();
        edited.envelope = list;
        let added = edited.envelope != content.envelope;
        if added {
            out.push(audio_clip_cmd(
                clip,
                clip.start,
                clip.length,
                edited.clone(),
                merge,
            ));
        }
        let rate = self.asset_rate(&content.asset);
        self.drag = Drag::Audio {
            start: pos,
            rate,
            asset_frames: u64::MAX >> 2,
            content: content.clone(),
            clip: clip.clone(),
            grab: AudioGrab::EnvPoint(index, top, bottom - top),
            merge,
            moved: false,
            emitted: added,
            last: (clip.start, clip.length, edited),
            others: Vec::new(),
        };
    }

    /// The command that replaces an audio clip's envelope with `f(points)`, one undo step.
    pub(super) fn envelope_edit_cmd(
        &self,
        clip: &Clip,
        a: &AudioClipContent,
        f: impl FnOnce(&[atomik_model::GainPoint]) -> Vec<atomik_model::GainPoint>,
    ) -> UiAction {
        song_cmd(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content: AudioClipContent {
                envelope: f(&a.envelope),
                ..a.clone()
            },
            merge: None,
        })
    }

    /// A pencil click on an audio clip: a new envelope point where it landed, dragged at once.
    pub(super) fn pencil_envelope(
        &mut self,
        clip: &Clip,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let ClipContent::Audio(a) = &clip.content else {
            return false;
        };
        let Some((track, lane, _)) = self.find_clip(clip.id) else {
            return false;
        };
        let Some(row) = self.row_of(track, lane).map(|r| self.row_rect(r)) else {
            return false;
        };
        let r = self.clip_rect_in(row, clip);
        let g = self.audio_geo(r, clip, a, self.timeline_rect());
        let (top, bottom) = value_range(r);
        let step = self.step(modifiers);
        // With a grid the new point lands on its lines.
        let frame = if step.unit().is_some() {
            let tick = snap_round(self.axis.x_to_tick(pos.x), &step);
            let f = self.tempo.frame_at_tick(tick as f64, g.rate) - g.start_frame;
            f.round().max(0.0) as u64
        } else {
            self.frame_under(pos.x, &g)
        };
        let db = audio::y_to_gain(pos.y, top, bottom);
        let (list, index) = audio::envelope_insert(&a.envelope, frame, db);
        self.selection.select_only(clip.id);
        self.au.env_sel = Some((clip.id, BTreeSet::from([index])));
        self.start_envelope_drag(clip, a, list, index, pos, out);
        true
    }

    /// Paints the gain envelope, the crossfade zone and the reverse mark of an audio clip.
    pub(super) fn paint_audio_edits(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        vis: Rect,
        clip: &Clip,
        a: &AudioClipContent,
        g: &AudioGeo,
    ) {
        let st = &self.st;
        let (top, bottom) = value_range(r);
        if let Some(z) = self.au.zones.get(&clip.id) {
            let (x0, x1) = (
                self.x_of_tick(z.start as f64).max(vis.x0),
                self.x_of_tick(z.end as f64).min(vis.x1),
            );
            if x1 > x0 {
                c.fill_with(Rect::new(x0, r.y0 + CLIP_HEADER, x1, r.y1), st.fade_shade);
                for rising in [true, false] {
                    let mut line = BezPath::new();
                    for i in 0..=12 {
                        let t = f64::from(i) / 12.0;
                        let v = z.curve.eval(if rising { t } else { 1.0 - t });
                        let p = pt(x0 + t * (x1 - x0), bottom - v * (bottom - top));
                        if i == 0 {
                            line.move_to(p);
                        } else {
                            line.line_to(p);
                        }
                    }
                    c.stroke(
                        &line.into(),
                        &Stroke::new(1.0),
                        &st.base.text.with_alpha(0.7).into(),
                    );
                }
            }
            // The length handle at the zone's end, where the zone can change length.
            let xe = self.x_of_tick(z.end as f64);
            if xe >= vis.x0
                && xe <= vis.x1
                && self.x_of_tick(z.end as f64) - self.x_of_tick(z.start as f64)
                    >= super::crossfade_drag::MIN_ZONE_PX
                && self.crossfade_handle_at(pt(xe, r.y1 - 1.0), r, clip)
            {
                let mid = (r.y0 + CLIP_HEADER + r.y1) / 2.0;
                let grip = RoundedRect::new(xe - 2.0, mid - 7.0, xe + 2.0, mid + 7.0, 1.5);
                c.fill_with(grip, st.handle.with_alpha(0.9));
            }
        }
        if !a.envelope.is_empty() {
            let x_start = vis.x0.max(self.axis.x0).floor();
            let x_end = vis.x1.ceil();
            let mut line = BezPath::new();
            let mut x = x_start;
            let mut first = true;
            let mut steps = 0;
            while x <= x_end && steps < 4096 {
                let frame = self.frame_under(x, g);
                let amp = a.envelope_gain_at(frame).unwrap_or(1.0);
                let db = atomik_core::gain_to_db(amp).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
                let p = pt(x, audio::gain_to_y(db, top, bottom));
                if first {
                    line.move_to(p);
                    first = false;
                } else {
                    line.line_to(p);
                }
                x += ENVELOPE_STEP;
                steps += 1;
            }
            let ink = st.handle.with_alpha(0.95);
            c.stroke(&line.into(), &Stroke::new(1.5), &ink.into());
            for (i, p) in self.envelope_points(r, a, g) {
                if p.x >= vis.x0 - 4.0 && p.x <= vis.x1 + 4.0 {
                    if self.envelope_point_selected(clip.id, i) {
                        c.fill_with(Circle::new(p, 5.0), st.selection);
                        c.fill_with(Circle::new(p, 3.0), st.base.plate);
                    } else {
                        c.fill_with(Circle::new(p, 3.5), ink);
                        c.stroke_with(Circle::new(p, 3.5), 1.0, st.base.plate);
                    }
                }
            }
        }
        for m in &a.warp.markers {
            let x = self.x_of_tick((clip.start + m.tick) as f64);
            if x > vis.x0 && x < vis.x1 {
                sp::line(c, x, r.y0 + CLIP_HEADER, x, r.y1, st.handle.with_alpha(0.6));
            }
        }
        if a.warp.active() {
            let mut mark = if a.warp.enabled {
                "W".to_owned()
            } else {
                String::new()
            };
            let cents = a.warp.effective_cents();
            if cents != 0 {
                if !mark.is_empty() {
                    mark.push(' ');
                }
                if cents % 100 == 0 {
                    mark.push_str(&format!("{:+}", cents / 100));
                } else {
                    mark.push_str(&format!("{:+.1}", f64::from(cents) / 100.0));
                }
            }
            sp::label(
                c,
                &mark,
                (vis.x1 - super::AUDIO_HANDLE_INSET - 22.0 - 6.0 * (mark.len() as f64 - 1.0))
                    .max(vis.x0 + 4.0),
                r.y0 + CLIP_HEADER - 3.0,
                8.5,
                st.base.text.with_alpha(0.85),
                crate::canvas::Align::Start,
            );
        }
        if a.reverse {
            sp::label(
                c,
                "R",
                (vis.x1 - super::AUDIO_HANDLE_INSET - 22.0).max(vis.x0 + 4.0),
                r.y0 + CLIP_HEADER - 3.0,
                8.5,
                st.base.text.with_alpha(0.85),
                crate::canvas::Align::Start,
            );
        }
    }

    /// Opens the shape menu of one fade of `clip` at `pos`: a click on a row shapes that fade
    /// alone (the other keeps its shape). `track` is the clip's track.
    pub(super) fn open_fade_menu(
        &mut self,
        track: usize,
        clip: &Clip,
        a: &AudioClipContent,
        out: bool,
        pos: Point,
    ) {
        let Some(device) = self.song.tracks.get(track).map(|t| t.device.clone()) else {
            return;
        };
        let current = if out { a.out_curve() } else { a.fade_curve };
        let (items, commands): (Vec<_>, Vec<_>) = FadeCurve::ALL
            .iter()
            .map(|&curve| {
                let item = (curve.id().to_owned(), curve.label().to_owned());
                let shaped = a.clone().with_fade_shape(out, curve);
                (item, self.audio_content_cmd(clip, shaped))
            })
            .unzip();
        let mut menu = Menu::new(device, MenuKind::Takes(commands), items, pos, 0);
        menu.current = FadeCurve::ALL.iter().position(|&c| c == current);
        self.open_menu(menu);
    }

    /// The command that replaces the content of audio clip `clip` with `content`.
    pub(super) fn audio_content_cmd(&self, clip: &Clip, content: AudioClipContent) -> UiAction {
        song_cmd(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content,
            merge: None,
        })
    }

    /// The `sequencer.audio_*` commands that need no samples. `None` for a command that is not
    /// one of them.
    pub(super) fn audio_command(&mut self, id: &str, arg: Option<&str>) -> Option<CommandOutcome> {
        let edit: ClipEdit = match id {
            "sequencer.audio_reverse" => {
                let to = !self.audio_menu().reversed;
                Box::new(move |_, a, _| {
                    // A warped clip keeps its warp: the stretched span plays backwards.
                    (a.reverse != to).then(|| AudioClipContent {
                        reverse: to,
                        ..a.clone()
                    })
                })
            }
            "sequencer.audio_gain" => {
                let step = match arg? {
                    "up" => Some(GAIN_STEP_DB),
                    "down" => Some(-GAIN_STEP_DB),
                    "reset" => None,
                    _ => return Some(CommandOutcome::Declined),
                };
                Box::new(move |_, a, _| {
                    let db = step.map_or(0.0, |s| {
                        (((a.gain_db + s) * 10.0).round() / 10.0)
                            .clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX)
                    });
                    ((db - a.gain_db).abs() > f32::EPSILON).then(|| AudioClipContent {
                        gain_db: db,
                        ..a.clone()
                    })
                })
            }
            "sequencer.audio_fade_shape" => {
                let curve = FadeCurve::from_id(arg?)?;
                // Both fades take the shape (a fade-out of its own goes with it).
                Box::new(move |_, a, _| {
                    (a.fade_curve != curve || a.fade_out_curve.is_some())
                        .then(|| a.clone().with_fade_curve(curve))
                })
            }
            "sequencer.audio_crossfade" => {
                // `toggle`: each clip's crossfade goes off when on, to equal power when off.
                let fixed = match arg? {
                    "toggle" => None,
                    "off" => Some(None),
                    other => Some(Some(FadeCurve::from_id(other)?)),
                };
                Box::new(move |_, a, _| {
                    let xfade = fixed.unwrap_or(match a.xfade {
                        Some(_) => None,
                        None => Some(FadeCurve::EqualPower),
                    });
                    (a.xfade != xfade).then(|| AudioClipContent { xfade, ..a.clone() })
                })
            }
            "sequencer.audio_fades" => {
                let declick = match arg? {
                    "declick" => true,
                    "clear" => false,
                    _ => return Some(CommandOutcome::Declined),
                };
                let tempo = self.tempo.clone();
                Box::new(move |clip, a, rate| {
                    let frames = tempo
                        .frame_at(clip.start + clip.length, rate)
                        .saturating_sub(tempo.frame_at(clip.start, rate));
                    let n = if declick {
                        (u64::from(rate) * DECLICK_MS / 1000).min(frames / 2)
                    } else {
                        0
                    };
                    ((a.fade_in, a.fade_out) != (n, n)).then(|| AudioClipContent {
                        fade_in: n,
                        fade_out: n,
                        ..a.clone()
                    })
                })
            }
            "sequencer.audio_warp" => {
                let to = !self.audio_menu().warped;
                Box::new(move |_, a, _| {
                    (a.warp.enabled != to).then(|| {
                        let mut c = a.clone();
                        c.warp.enabled = to;
                        c
                    })
                })
            }
            "sequencer.audio_warp_mode" => {
                let mode = atomik_model::WarpMode::from_id(arg?)?;
                Box::new(move |_, a, _| {
                    (a.warp.mode != mode).then(|| {
                        let mut c = a.clone();
                        c.warp.mode = mode;
                        c
                    })
                })
            }
            "sequencer.audio_transpose" => {
                let step = match arg? {
                    "up" => Some(100),
                    "down" => Some(-100),
                    "octave_up" => Some(1200),
                    "octave_down" => Some(-1200),
                    "reset" => None,
                    _ => return Some(CommandOutcome::Declined),
                };
                Box::new(move |_, a, _| {
                    let cents = step.map_or(0, |s| {
                        (a.warp.transpose_cents + s).clamp(
                            -atomik_model::WARP_TRANSPOSE_MAX_CENTS,
                            atomik_model::WARP_TRANSPOSE_MAX_CENTS,
                        )
                    });
                    (cents != a.warp.transpose_cents).then(|| {
                        let mut c = a.clone();
                        c.warp.transpose_cents = cents;
                        c
                    })
                })
            }
            "sequencer.audio_envelope_clear" => Box::new(|_, a, _| {
                (!a.envelope.is_empty()).then(|| AudioClipContent {
                    envelope: Vec::new(),
                    ..a.clone()
                })
            }),
            _ => return None,
        };
        let clips: Vec<(Clip, AudioClipContent)> = self
            .selected_audio()
            .into_iter()
            .map(|(c, a)| (c.clone(), a.clone()))
            .collect();
        if clips.is_empty() {
            return Some(CommandOutcome::NotApplicable(
                "Select an audio clip first".into(),
            ));
        }
        let mut commands = Vec::new();
        for (clip, a) in &clips {
            let rate = self
                .assets
                .get(&a.asset)
                .map_or(DEFAULT_RATE, |m| m.sample_rate);
            if let Some(content) = edit(clip, a, rate) {
                commands.push(Command::Song(SongCommand::SetAudioClip {
                    id: clip.id,
                    start: clip.start,
                    length: clip.length,
                    muted: clip.muted,
                    content,
                    merge: None,
                }));
            }
        }
        // Several clips are one undo step.
        let action = match commands.len() {
            0 => return Some(CommandOutcome::Handled(Vec::new())),
            1 => commands.remove(0),
            _ => Command::Batch(commands),
        };
        Some(CommandOutcome::Handled(vec![UiAction::Command(action)]))
    }
}
