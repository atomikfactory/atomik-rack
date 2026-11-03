//! Pattern lanes: the automation lane of a pattern device's `pattern` param, drawn as blocks of
//! pattern numbers instead of a curve.
//!
//! A clip of the lane holds stepped points (a number held until the next point); its first point
//! switches the device's pattern at the clip start and the lane's static value (if it has one)
//! takes over at the clip end. Drawing, resizing, moving, splitting and copying are the
//! automation clip gestures; what is special here is
//!
//! * the **blocks**: each held number is a labelled block over its stretch of the clip,
//! * the **picker**: a right click on a clip opens the list of the device's patterns (and Delete
//!   Clip) and the row sets the number of the block under the pointer,
//! * the **seed** of a new clip: a clip drawn on the lane starts with one point, the number under
//!   the pointer, so it plays at once.

use super::*;
use atomik_model::PATTERN_PARAM;

/// The most patterns any device has (the picker never lists more).
const MAX_PICKER_ROWS: usize = 64;

/// True for the descriptor of a `pattern` param.
pub(super) fn is_pattern(param: &ParamDescriptor) -> bool {
    param.key == PATTERN_PARAM
}

/// The pattern number a value reads as, within the param's range.
fn number(param: &ParamDescriptor, value: f32) -> usize {
    (value.round().clamp(param.min, param.max) as i64).max(1) as usize
}

impl ArrangementView {
    /// True when lane `lane` of track `track` is a pattern lane.
    pub(super) fn is_pattern_lane(&self, track: usize, lane: usize) -> bool {
        self.is_automation_lane(track, lane) && is_pattern(&self.param_of(track, lane))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the paint inputs of one clip are passed through"
    )]
    /// Paints the held numbers of an automation clip as blocks: `windows` are the clip's
    /// content windows, `r` its rectangle.
    pub(super) fn paint_pattern_blocks(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        points: &[AutomationPoint],
        windows: &[seq_edit::ContentWindow],
        param: &ParamDescriptor,
        (accent, text): (Color, Color),
        tl: Rect,
    ) {
        let (y0, y1) = (r.y0 + CLIP_HEADER + 2.0, r.y1 - 3.0);
        if y1 - y0 < 4.0 {
            return;
        }
        for w in windows {
            let len = w.song_end - w.song_start;
            for (i, p) in points.iter().enumerate() {
                // A number is held from its point (the first one from the window start) to the
                // next point or the window end.
                let from = if i == 0 { 0 } else { p.tick - w.content_start };
                let to = points
                    .get(i + 1)
                    .map_or(len, |n| (n.tick - w.content_start).min(len));
                if to <= from.max(0) || from >= len {
                    continue;
                }
                let (x0, x1) = (
                    self.axis.tick_to_x(w.song_start + from.max(0)),
                    self.axis.tick_to_x(w.song_start + to),
                );
                if x1 < tl.x0 || x0 > tl.x1 || x1 - x0 < 1.0 {
                    continue;
                }
                let block = RoundedRect::from_rect(Rect::new(x0 + 0.5, y0, x1 - 0.5, y1), 2.0);
                c.fill_with(block, accent.with_alpha(0.3));
                c.stroke_with(block, 1.0, accent.with_alpha(0.8));
                if x1 - x0 >= 16.0 {
                    let label = number(param, p.value).to_string();
                    sp::label(
                        c,
                        &label,
                        x0.max(tl.x0) + 5.0,
                        (y0 + y1) / 2.0 + 3.5,
                        10.0,
                        text,
                        Align::Start,
                    );
                }
            }
        }
    }

    /// Opens the picker for a press at `pos` on `clip` of a pattern lane. The rows set the number
    /// of the block under the pointer (the first one for a clip with no point yet); the last row
    /// deletes the clip.
    pub(super) fn open_pattern_menu(&mut self, track: usize, lane: usize, clip: &Clip, pos: Point) {
        let ClipContent::Automation(points) = &clip.content else {
            return;
        };
        let param = self.param_of(track, lane);
        let count = (param.max.round().max(1.0) as usize).min(MAX_PICKER_ROWS);
        // The block under the pointer: the last point at or before it.
        let raw = self.axis.x_to_tick(pos.x);
        let content_tick = seq_edit::content_windows(clip)
            .into_iter()
            .find(|w| (w.song_start..w.song_end).contains(&raw))
            .map_or(0, |w| raw - w.song_start + w.content_start);
        let index = points
            .partition_point(|p| p.tick <= content_tick)
            .saturating_sub(1)
            .min(points.len().saturating_sub(1));
        let set = |value: f32| {
            let mut list = points.clone();
            match list.get_mut(index) {
                Some(p) => {
                    p.value = value;
                    p.curve = Curve::Step;
                }
                None => list.push(AutomationPoint {
                    tick: 0,
                    value,
                    curve: Curve::Step,
                }),
            }
            SongCommand::SetClipContent {
                id: clip.id,
                content: ClipContent::Automation(list),
                merge: None,
            }
        };
        let mut items: Vec<(String, String)> = Vec::new();
        let mut commands: Vec<SongCommand> = Vec::new();
        for n in 1..=count {
            items.push((n.to_string(), format!("Pattern {n}")));
            commands.push(set(n as f32));
        }
        items.push(("delete".into(), "Delete Clip".into()));
        commands.push(SongCommand::RemoveClip { id: clip.id });
        let current = points.get(index).map(|p| number(&param, p.value) - 1);
        let device = self.song.tracks[track].device.clone();
        let scroll = current
            .unwrap_or(0)
            .saturating_sub(MENU_ROWS / 2)
            .min(items.len().saturating_sub(MENU_ROWS));
        let mut menu = Menu::new(device, MenuKind::Takes(commands), items, pos, scroll);
        menu.current = current;
        self.open_menu(menu);
    }

    /// A clip drawn on a lane, as the `AddClip` of [`new_clip_cmd`]; a clip of a pattern lane
    /// starts with one `Step` point, the number the press height shows.
    pub(super) fn drawn_clip(
        &self,
        (device, lane): (DeviceKey, usize),
        automation: bool,
        (start, length): (i64, i64),
        press: Point,
    ) -> UiAction {
        let action = new_clip_cmd(device.clone(), lane, automation, start, length);
        let Some(track) = self.song.tracks.iter().position(|t| t.device == device) else {
            return action;
        };
        let Some(row) = self.row_of(track, lane) else {
            return action;
        };
        if !(automation && self.is_pattern_lane(track, lane)) {
            return action;
        }
        let UiAction::Command(Command::Song(SongCommand::AddClip { mut clip, .. })) = action else {
            return action;
        };
        let (top, bottom) = value_range(self.clip_rect_in(self.row_rect(row), &clip));
        let param = self.param_of(track, lane);
        clip.content = ClipContent::Automation(vec![AutomationPoint {
            tick: 0,
            value: seq_edit::value_from_y(&param, press.y, top, bottom - top),
            curve: Curve::Step,
        }]);
        song_cmd(SongCommand::AddClip { device, lane, clip })
    }
}
