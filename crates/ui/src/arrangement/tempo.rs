//! The tempo lane and the meter changes of the [`ArrangementView`]: the gestures (add, drag,
//! delete, ramp), the commands behind the context menus, the painting and the accessors the app
//! and the tests use. The pure edits are in [`crate::tempo_lane`].
//!
//! A drag sends `DragTempoMap` / `DragMeterMap` with one merge id (one undo step) and ends with
//! [`UiAction::EndMerge`]; Escape puts the map back.

use super::{ArrangementView, Drag, Hit, LABEL_BAND, song_cmd};
use crate::action::{ContextTarget, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Modifiers, PointerButton};
use crate::keymap::CommandOutcome;
use crate::seq_paint as sp;
use crate::tempo_lane::{self as lane, LanePaint};
use crate::theme::mix;
use crate::timeline::{Bars, Quant};
use crate::viewport::Tool;
use atomik_model::{MeterEvent, SongCommand, TempoEvent};
use vello::kurbo::{Point, Rect, RoundedRect};

/// Width of a meter tag in the ruler.
const TAG_W: f64 = 26.0;

/// What the context menu of a tempo point can do (the app enables its items from this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TempoMenu {
    /// "Ramp to next" can be switched (the point is not the last).
    pub can_ramp: bool,
    /// The point ramps to the next one.
    pub ramped: bool,
    /// The point can be deleted (it is not the first).
    pub can_delete: bool,
}

fn set_tempo(map: Vec<TempoEvent>) -> UiAction {
    song_cmd(SongCommand::SetTempoMap(map))
}

fn set_meter_map(map: Vec<MeterEvent>) -> UiAction {
    song_cmd(SongCommand::SetMeterMap(map))
}

/// `"3/4"` as `(3, 4)`.
fn parse_meter(arg: &str) -> Option<(u8, u8)> {
    let (n, d) = arg.split_once('/')?;
    Some((n.trim().parse().ok()?, d.trim().parse().ok()?))
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the tempo lane is shown between the ruler and the track lanes.
    pub fn tempo_lane_visible(&self) -> bool {
        self.tempo_lane
    }

    /// Shows or hides the tempo lane.
    pub fn set_tempo_lane(&mut self, on: bool) {
        if on != self.tempo_lane {
            self.tempo_lane = on;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The tempo lane's rectangle (empty when it is hidden).
    pub fn tempo_lane_rect(&self) -> Rect {
        self.tempo_rect()
    }

    /// Centre of tempo point `i` in the lane.
    pub fn tempo_point_center(&self, i: usize) -> Option<Point> {
        let e = self.song.tempo.get(i)?;
        let range = lane::bpm_range(&self.song.tempo);
        Some(lane::point_pos(e, self.tempo_rect(), &self.axis, range))
    }

    /// Centre of the meter tag of change `i` in the ruler.
    pub fn meter_tag_center(&self, i: usize) -> Option<Point> {
        self.meter_tag_rect(i).map(|r| r.center())
    }

    /// The selected tempo point.
    pub fn selected_tempo_point(&self) -> Option<usize> {
        self.tempo_sel
    }

    /// The selected meter change.
    pub fn selected_meter_change(&self) -> Option<usize> {
        self.meter_sel
    }

    /// The bar the last ruler context menu opened on.
    pub fn context_bar(&self) -> u32 {
        self.ctx_bar
    }

    /// The state of the tempo point the context menu is on (or the selected one).
    pub fn tempo_menu(&self) -> TempoMenu {
        let Some(i) = self.ctx_tempo.or(self.tempo_sel) else {
            return TempoMenu::default();
        };
        TempoMenu {
            can_ramp: i + 1 < self.song.tempo.len(),
            ramped: self.song.tempo.get(i).is_some_and(|e| e.ramp),
            can_delete: i > 0 && i < self.song.tempo.len(),
        }
    }

    /// Whether the meter change the context menu is on (or the selected one) can be deleted
    /// (every change but the first can).
    pub fn meter_change_deletable(&self) -> bool {
        self.ctx_meter
            .or(self.meter_sel)
            .is_some_and(|i| i > 0 && i < self.song.meter.len())
    }

    // ------------------------------------------------------------------ geometry

    /// The tag of meter change `i` in the ruler: right of the bar number of its bar.
    pub(super) fn meter_tag_rect(&self, i: usize) -> Option<Rect> {
        let seg = self.bars.segments().get(i)?;
        // The 4/4 every song starts in needs no tag; every other signature has one.
        if i == 0 && (seg.num, seg.den) == (4, 4) {
            return None;
        }
        let x = self.axis.tick_to_x(seg.tick);
        let digits = seg.bar.to_string().len() as f64;
        let x0 = x + 3.0 + digits * 5.6 + 4.0;
        let r = Rect::new(
            x0,
            self.bounds.y0 + 3.0,
            x0 + TAG_W,
            self.bounds.y0 + LABEL_BAND - 3.0,
        );
        (r.x1 > self.axis.x0 && r.x0 < self.bounds.x1).then_some(r)
    }

    // ------------------------------------------------------------------ gestures

    /// A press on a tempo point: select it and drag it (the eraser deletes it).
    pub(super) fn tempo_point_down(
        &mut self,
        i: usize,
        button: PointerButton,
        pos: Point,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.meter_sel = None;
        self.tempo_sel = Some(i);
        match button {
            PointerButton::Secondary => {
                self.ctx_tempo = Some(i);
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::TempoPoint,
                    at: pos,
                });
            }
            PointerButton::Primary if tool == Tool::Eraser => {
                if let Some(map) = lane::delete_point(&self.song.tempo, i) {
                    self.tempo_sel = None;
                    out.push(set_tempo(map));
                }
            }
            PointerButton::Primary => {
                let merge = self.merge_id();
                self.drag = Drag::TempoPoint {
                    start: pos,
                    index: i,
                    original: self.song.tempo.clone(),
                    range: lane::bpm_range(&self.song.tempo),
                    merge,
                    moved: false,
                    emitted: false,
                    last: self.song.tempo.clone(),
                };
            }
            PointerButton::Middle => {}
        }
    }

    /// A press on the empty tempo lane: the pencil or a double-click adds a point at the
    /// snapped tick and the pointer's BPM; the right button opens the lane's menu.
    pub(super) fn tempo_lane_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.meter_sel = None;
        self.tempo_sel = None;
        let tick = self
            .grid(modifiers)
            .round_to(self.axis.x_to_tick(pos.x))
            .max(0);
        match button {
            PointerButton::Secondary => {
                self.ctx_tick = tick;
                self.ctx_tempo = None;
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::TempoLane,
                    at: pos,
                });
            }
            PointerButton::Primary if clicks >= 2 || tool == Tool::Pencil => {
                let range = lane::bpm_range(&self.song.tempo);
                let bpm = lane::round_bpm(
                    lane::y_to_bpm(pos.y, self.tempo_rect(), range),
                    modifiers.shift,
                );
                if let Some((map, i)) = lane::add_point(&self.song.tempo, tick, bpm) {
                    self.tempo_sel = Some(i);
                    out.push(set_tempo(map));
                }
            }
            _ => {}
        }
    }

    /// A press on a meter tag: select the change and drag it to another bar (the eraser
    /// deletes it, the right button opens its menu).
    pub(super) fn meter_tag_down(
        &mut self,
        i: usize,
        button: PointerButton,
        pos: Point,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.tempo_sel = None;
        self.meter_sel = Some(i);
        match button {
            PointerButton::Secondary => {
                self.ctx_meter = Some(i);
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::MeterChange,
                    at: pos,
                });
            }
            PointerButton::Primary if tool == Tool::Eraser => {
                if let Some(map) = lane::delete_meter(&self.song.meter, i) {
                    self.meter_sel = None;
                    out.push(set_meter_map(map));
                }
            }
            PointerButton::Primary if i > 0 => {
                let merge = self.merge_id();
                self.drag = Drag::Meter {
                    start: pos,
                    index: i,
                    original: self.song.meter.clone(),
                    merge,
                    moved: false,
                    emitted: false,
                    last: self.song.meter.clone(),
                };
            }
            _ => {}
        }
    }

    /// The pointer moved during a tempo point or meter change drag.
    pub(super) fn tempo_drag_move(
        &mut self,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let (axis, lane_rect) = (self.axis, self.tempo_rect());
        let grid = self.grid(modifiers);
        match &mut self.drag {
            Drag::TempoPoint {
                start,
                index,
                original,
                range,
                merge,
                moved,
                emitted,
                last,
            } => {
                *moved |= pos.distance(*start) > super::DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let tick = grid.round_to(axis.x_to_tick(pos.x)).max(0);
                let bpm =
                    lane::round_bpm(lane::y_to_bpm(pos.y, lane_rect, *range), modifiers.shift);
                let new = lane::move_point(original, *index, tick, bpm);
                if new != *last {
                    out.push(song_cmd(SongCommand::DragTempoMap {
                        map: new.clone(),
                        merge: *merge,
                    }));
                    *last = new;
                    *emitted = true;
                }
            }
            Drag::Meter {
                start,
                index,
                original,
                merge,
                moved,
                emitted,
                last,
            } => {
                *moved |= pos.distance(*start) > super::DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                // The bars the change can take are those of the map without it: the bar lines
                // under the pointer do not move with the change itself.
                let without = lane::delete_meter(original, *index).unwrap_or_default();
                let bars = Bars::from_events(&without);
                let bar = bars.bar_of(bars.nearest_bar_line(axis.x_to_tick(pos.x)));
                let bar = u32::try_from(bar.max(1)).unwrap_or(1);
                let new = lane::move_meter(original, *index, bar);
                if new != *last {
                    out.push(song_cmd(SongCommand::DragMeterMap {
                        map: new.clone(),
                        merge: *merge,
                    }));
                    *last = new;
                    *emitted = true;
                }
            }
            _ => {}
        }
        self.repaint = true;
    }

    /// Escape during a tempo or meter drag: the original map goes back (one merged step, then
    /// the end of the drag).
    pub(super) fn cancel_tempo_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::TempoPoint {
                original,
                merge,
                emitted: true,
                ..
            } => {
                out.push(song_cmd(SongCommand::DragTempoMap {
                    map: original,
                    merge,
                }));
                out.push(UiAction::EndMerge);
            }
            Drag::Meter {
                original,
                merge,
                emitted: true,
                ..
            } => {
                out.push(song_cmd(SongCommand::DragMeterMap {
                    map: original,
                    merge,
                }));
                out.push(UiAction::EndMerge);
            }
            other => self.drag = other,
        }
    }

    // ------------------------------------------------------------------ commands

    /// The tempo and meter commands: the context menus' items (on the point or bar the menu
    /// opened on, else the selection) and Delete on a selected tempo point or meter change.
    /// `None` when `id` is not one of them.
    pub(super) fn tempo_command(&mut self, id: &str, arg: Option<&str>) -> Option<CommandOutcome> {
        let handled = |a: Vec<UiAction>| Some(CommandOutcome::Handled(a));
        let point = self.ctx_tempo.or(self.tempo_sel);
        let change = self.ctx_meter.or(self.meter_sel);
        match id {
            "sequencer.tempo_add_point" => {
                let tick = self.ctx_tick;
                let bpm = lane::round_bpm(lane::tempo_at(&self.song.tempo, tick), true);
                let Some((map, i)) = lane::add_point(&self.song.tempo, tick, bpm) else {
                    return Some(CommandOutcome::NotApplicable(
                        "There is a tempo point here already".into(),
                    ));
                };
                self.tempo_sel = Some(i);
                handled(vec![set_tempo(map)])
            }
            "sequencer.tempo_toggle_ramp" => {
                let map = point.and_then(|i| lane::toggle_ramp(&self.song.tempo, i));
                self.ctx_tempo = None;
                match map {
                    Some(map) => handled(vec![set_tempo(map)]),
                    None => Some(CommandOutcome::NotApplicable(
                        "The last tempo point has no next point to ramp to".into(),
                    )),
                }
            }
            "sequencer.tempo_set_value" => {
                let i = point?;
                self.ctx_tempo = None;
                handled(vec![UiAction::EditTempoPoint(i)])
            }
            "sequencer.tempo_delete_point" => {
                let map = point.and_then(|i| lane::delete_point(&self.song.tempo, i));
                self.ctx_tempo = None;
                self.tempo_sel = None;
                match map {
                    Some(map) => handled(vec![set_tempo(map)]),
                    None => Some(CommandOutcome::NotApplicable(
                        "The first tempo point cannot be deleted".into(),
                    )),
                }
            }
            "sequencer.meter_insert" => {
                let (num, den) = arg.and_then(parse_meter)?;
                let map = lane::insert_meter(&self.song.meter, self.ctx_bar.max(1), num, den)?;
                handled(vec![set_meter_map(map)])
            }
            "sequencer.meter_set" => {
                let (num, den) = arg.and_then(parse_meter)?;
                let map = lane::set_meter(&self.song.meter, change.unwrap_or(0), num, den)?;
                self.ctx_meter = None;
                handled(vec![set_meter_map(map)])
            }
            "sequencer.meter_delete" => {
                let map = change.and_then(|i| lane::delete_meter(&self.song.meter, i));
                self.ctx_meter = None;
                self.meter_sel = None;
                match map {
                    Some(map) => handled(vec![set_meter_map(map)]),
                    None => Some(CommandOutcome::NotApplicable(
                        "The first meter cannot be deleted".into(),
                    )),
                }
            }
            "edit.delete" if self.tempo_sel.is_some() || self.meter_sel.is_some() => {
                if let Some(i) = self.tempo_sel.take() {
                    return handled(
                        lane::delete_point(&self.song.tempo, i)
                            .map(set_tempo)
                            .into_iter()
                            .collect(),
                    );
                }
                let i = self.meter_sel.take()?;
                handled(
                    lane::delete_meter(&self.song.meter, i)
                        .map(set_meter_map)
                        .into_iter()
                        .collect(),
                )
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ painting

    /// The tempo lane and its header cell.
    pub(super) fn paint_tempo_lane(&self, c: &mut dyn Canvas) {
        if !self.tempo_lane {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let rect = self.tempo_rect();
        let header = Rect::new(self.bounds.x0, rect.y0, rect.x0, rect.y1);
        sp::rect(c, header, st.header_lane);
        sp::label(
            c,
            "TEMPO",
            header.x0 + 12.0,
            header.y0 + 17.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        let playhead_bpm = lane::tempo_at(&self.song.tempo, self.playhead);
        sp::label(
            c,
            &format!("{playhead_bpm:.2} BPM"),
            header.x0 + 12.0,
            header.y0 + 34.0,
            11.0,
            t.highlight,
            Align::Start,
        );
        let range = match &self.drag {
            Drag::TempoPoint { range, .. } => *range,
            _ => lane::bpm_range(&self.song.tempo),
        };
        let hover = match &self.hover {
            Some(Hit::TempoPoint(i)) => Some(*i),
            _ => None,
        };
        c.push_clip(&rect.into());
        lane::paint(
            c,
            st,
            &self.axis,
            rect,
            &LanePaint {
                events: &self.song.tempo,
                range,
                selected: self.tempo_sel,
                hover,
                bars: &self.bars,
                playhead: (
                    self.playhead,
                    if self.recording {
                        st.record
                    } else {
                        st.playhead
                    },
                ),
            },
        );
        c.pop_clip();
        sp::line(
            c,
            header.x0,
            rect.y1 - 0.5,
            rect.x1,
            rect.y1 - 0.5,
            st.separator,
        );
        sp::line(
            c,
            rect.x0 - 0.5,
            rect.y0,
            rect.x0 - 0.5,
            rect.y1,
            st.separator,
        );
    }

    /// The meter tags of the ruler: the signature of every change next to its bar number.
    pub(super) fn paint_meter_tags(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        for (i, seg) in self.bars.segments().iter().enumerate() {
            let Some(r) = self.meter_tag_rect(i) else {
                continue;
            };
            let lit = self.meter_sel == Some(i) || self.hover == Some(Hit::MeterTag(i));
            let fill = if self.meter_sel == Some(i) {
                mix(t.highlight, t.plate, 0.55)
            } else if lit {
                mix(t.focus, t.plate, 0.7)
            } else {
                mix(t.text_muted, t.plate, 0.78)
            };
            c.fill_with(RoundedRect::from_rect(r, 3.0), fill);
            sp::label(
                c,
                &format!("{}/{}", seg.num, seg.den),
                r.center().x,
                r.center().y + 3.0,
                8.5,
                t.text,
                Align::Center,
            );
        }
    }
}
