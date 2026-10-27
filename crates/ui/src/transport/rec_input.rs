//! The recording input at the right end of the transport bar: the input gain, the clip-safe
//! switch and the input level meter of one armed audio track.
//!
//! **Why here.** The bar is on screen in every view and while recording, and the track whose
//! input you are setting is rarely the one whose header is in view. The track shown (the
//! *focus*) is the selected track when it is armed, else the first armed audio track; the app
//! decides and hands it in with [`TransportBar::set_rec_input`], and the section is not drawn
//! when no audio track is armed or the bar is too narrow to hold it (the clip inspector gives
//! way first).
//!
//! * **Gain** (`INPUT GAIN`): dB, `-60` (off) to `+24`, 0.5 dB a step (Shift: 0.1 dB) by drag or
//!   wheel (the shared step drag, see [`crate::step_drag`]); a double-click sets 0 dB. A drag is
//!   one merge group, so it is one undo step. The edit is a plain `SetParam` of the track's
//!   `rec_gain`, so it is stored, undoable and heard in the monitor and in the take.
//! * **SAFE**: the take's clip-safe limiter on or off (`rec_clip_safe`).
//! * **Meter**: the post-gain peak of the track's input, in dBFS on the console's scale, with a
//!   hold tick (the console's ballistics), and a clip lamp that **latches** when the post-gain
//!   peak reaches 0 dBFS and stays lit until the meter is clicked. The latch is kept per track,
//!   so a track that clipped while another was shown still shows it when it comes back. With
//!   SAFE on the lamp still lights: it says the input is hot, while the take is protected.
//!
//! The app feeds the peaks once per frame with a monotonic time ([`TransportBar::feed_rec_peaks`]);
//! the bar has no clock of its own.

use super::*;
use crate::console_meter::{
    Ballistic, METER_FLOOR_DB, PEAK_FALL_DB_PER_S, amp_to_meter_db, meter_norm,
};
use atomik_core::{REC_CLIP_SAFE_PARAM, REC_GAIN_MAX_DB, REC_GAIN_MIN_DB, REC_GAIN_PARAM};
use atomik_model::DeviceKey;
use std::collections::BTreeMap;

/// Width of the gain box.
const GAIN_W: f64 = 84.0;
/// Width of the SAFE button.
const SAFE_W: f64 = 46.0;
/// Width of the meter box.
const METER_W: f64 = 140.0;
/// Gap between the boxes.
const GAP: f64 = 8.0;
/// Gap between the clip inspector (or the readouts) and the section.
const LEAD: f64 = 18.0;
/// Right margin of the bar.
const TAIL: f64 = 12.0;
/// Height of a box.
const BOX_H: f64 = 28.0;
/// Font size of the gain value.
const VALUE_SIZE: f32 = 12.0;
/// Tenths of a dB of a gain step (Shift: one tenth).
const STEP: i64 = 5;
/// The largest frame step the ballistics take at once.
const MAX_DT: f64 = 0.25;

/// What the section shows: the focus track and its settings (from the document).
#[derive(Debug, Clone, PartialEq)]
pub struct RecInput {
    /// The audio track's device.
    pub device: DeviceKey,
    /// Its name.
    pub name: String,
    /// Input gain, dB.
    pub gain_db: f32,
    /// The take is limited to 0 dBFS.
    pub clip_safe: bool,
}

/// A part of the section a press, a drag or the wheel acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecInputPart {
    /// The gain readout.
    Gain,
    /// The SAFE switch.
    Safe,
    /// The meter, with its clip lamp (a click clears the latch).
    Meter,
}

/// The displayed level of one track.
#[derive(Debug, Clone, Copy, Default)]
struct Level {
    bar: Ballistic,
    latched: bool,
}

/// A drag on the gain: the value is tracked here so it never depends on the document catching
/// up between two moves.
#[derive(Debug, Clone, Copy)]
struct GainDrag {
    steps: StepDrag,
    /// Tenths of a dB.
    value: i64,
    /// The last value sent, so a drag held against the end of the range sends nothing.
    sent: i64,
    merge: u64,
    changed: bool,
}

/// The section's state inside the bar.
#[derive(Debug, Default)]
pub(super) struct RecInputState {
    source: Option<RecInput>,
    levels: BTreeMap<DeviceKey, Level>,
    last_time: Option<f64>,
    drag: Option<GainDrag>,
}

/// Where the section's boxes are.
struct Layout {
    gain: Rect,
    safe: Rect,
    meter: Rect,
}

fn tenths(db: f32) -> i64 {
    (f64::from(db) * 10.0).round() as i64
}

fn range() -> (i64, i64) {
    (tenths(REC_GAIN_MIN_DB), tenths(REC_GAIN_MAX_DB))
}

/// The gain as the readout shows it.
fn gain_text(t: i64) -> String {
    if t <= range().0 {
        "OFF".to_owned()
    } else {
        format!("{:+.1} dB", t as f64 / 10.0)
    }
}

impl TransportBar {
    /// Hands the bar the track whose input it shows (`None`: no section).
    pub fn set_rec_input(&mut self, source: Option<RecInput>) {
        if self.rec.source != source {
            if self.rec.drag.is_some()
                && self.rec.source.as_ref().map(|s| &s.device) != source.as_ref().map(|s| &s.device)
            {
                self.rec.drag = None;
            }
            self.rec.source = source;
            self.repaint = true;
        }
    }

    /// The track the section shows now.
    pub fn rec_input(&self) -> Option<&RecInput> {
        self.rec.source.as_ref()
    }

    /// The post-gain peaks (linear) of the armed audio tracks for this frame, at the monotonic
    /// time `now_seconds`. A track not listed loses its level and its latch.
    pub fn feed_rec_peaks(&mut self, peaks: &[(DeviceKey, f32)], now_seconds: f64) {
        let dt = self
            .rec
            .last_time
            .map_or(0.0, |t| (now_seconds - t).clamp(0.0, MAX_DT)) as f32;
        self.rec.last_time = Some(now_seconds);
        let before = self.rec_shown();
        self.rec
            .levels
            .retain(|d, _| peaks.iter().any(|(k, _)| k == d));
        for (device, peak) in peaks {
            let level = self.rec.levels.entry(device.clone()).or_default();
            let peak = if peak.is_nan() { 0.0 } else { *peak };
            level
                .bar
                .step(amp_to_meter_db(peak), dt, PEAK_FALL_DB_PER_S);
            if peak >= 1.0 {
                level.latched = true;
            }
        }
        if self.rec_shown() != before {
            self.repaint = true;
        }
    }

    /// What the meter of the focus track shows: the level and the hold in dBFS, and the latch
    /// (rounded, so a change too small to see does not repaint).
    fn rec_shown(&self) -> (i32, i32, bool) {
        let level = self
            .rec
            .source
            .as_ref()
            .and_then(|s| self.rec.levels.get(&s.device));
        let floor = (METER_FLOOR_DB * 4.0).round() as i32;
        level.map_or((floor, floor, false), |l| {
            (
                (l.bar.level_db * 4.0).round() as i32,
                (l.bar.hold_db * 4.0).round() as i32,
                l.latched,
            )
        })
    }

    /// The level of the track shown, in dBFS (the floor when silent), and its hold.
    pub fn rec_level_db(&self) -> (f32, f32) {
        self.rec
            .source
            .as_ref()
            .and_then(|s| self.rec.levels.get(&s.device))
            .map_or((METER_FLOOR_DB, METER_FLOOR_DB), |l| {
                (l.bar.level_db, l.bar.hold_db)
            })
    }

    /// Whether the clip lamp of the track shown is lit.
    pub fn rec_clip_latched(&self) -> bool {
        self.rec
            .source
            .as_ref()
            .and_then(|s| self.rec.levels.get(&s.device))
            .is_some_and(|l| l.latched)
    }

    /// Whether the section is on screen now.
    pub fn rec_input_visible(&self) -> bool {
        self.rec_layout().is_some()
    }

    /// A point on `part`, `None` when the section is not drawn.
    pub fn rec_input_point(&self, part: RecInputPart) -> Option<Point> {
        let l = self.rec_layout()?;
        Some(match part {
            RecInputPart::Gain => l.gain.center(),
            RecInputPart::Safe => l.safe.center(),
            RecInputPart::Meter => l.meter.center(),
        })
    }

    /// True while a drag on the gain is held.
    pub fn is_dragging_rec_input(&self) -> bool {
        self.rec.drag.is_some()
    }

    fn rec_width() -> f64 {
        GAIN_W + SAFE_W + METER_W + 2.0 * GAP
    }

    /// The width the clip inspector must leave free at the right.
    pub(super) fn rec_reserved(&self) -> f64 {
        if self.rec_layout().is_some() {
            Self::rec_width() + LEAD
        } else {
            0.0
        }
    }

    fn rec_layout(&self) -> Option<Layout> {
        self.rec.source.as_ref()?;
        let x1 = self.bounds.x1 - TAIL;
        let x0 = x1 - Self::rec_width();
        if x0 < self.lcd_rects()[3].x1 + LEAD {
            return None;
        }
        let y0 = self.bounds.center().y - BOX_H / 2.0;
        let boxed = |a: f64, w: f64| Rect::new(a, y0, a + w, y0 + BOX_H);
        Some(Layout {
            gain: boxed(x0, GAIN_W),
            safe: boxed(x0 + GAIN_W + GAP, SAFE_W),
            meter: boxed(x0 + GAIN_W + SAFE_W + 2.0 * GAP, METER_W),
        })
    }

    pub(super) fn rec_hit(&self, pos: Point) -> Option<RecInputPart> {
        let l = self.rec_layout()?;
        if l.gain.contains(pos) {
            Some(RecInputPart::Gain)
        } else if l.safe.contains(pos) {
            Some(RecInputPart::Safe)
        } else if l.meter.contains(pos) {
            Some(RecInputPart::Meter)
        } else {
            None
        }
    }

    fn rec_set(&self, device: &DeviceKey, param: &str, value: f32, merge: Option<u64>) -> UiAction {
        UiAction::Command(Command::SetParam {
            key: device.clone(),
            param: param.to_owned(),
            value,
            merge,
        })
    }

    /// A primary press on `part`.
    pub(super) fn rec_press(&mut self, part: RecInputPart, clicks: u8, y: f64) -> Vec<UiAction> {
        let Some(src) = self.rec.source.clone() else {
            return Vec::new();
        };
        match part {
            RecInputPart::Safe => {
                let to = if src.clip_safe { 0.0 } else { 1.0 };
                vec![self.rec_set(&src.device, REC_CLIP_SAFE_PARAM, to, None)]
            }
            RecInputPart::Meter => {
                if let Some(l) = self.rec.levels.get_mut(&src.device) {
                    l.latched = false;
                    l.bar = Ballistic::new();
                }
                self.repaint = true;
                Vec::new()
            }
            RecInputPart::Gain => {
                self.rec.drag = None;
                if clicks >= 2 {
                    // Back to 0 dB.
                    return if tenths(src.gain_db) == 0 {
                        Vec::new()
                    } else {
                        vec![self.rec_set(&src.device, REC_GAIN_PARAM, 0.0, None)]
                    };
                }
                let value = tenths(src.gain_db);
                self.rec.drag = Some(GainDrag {
                    steps: StepDrag::new(y),
                    value,
                    sent: value,
                    merge: self.next_merge,
                    changed: false,
                });
                self.next_merge += 1;
                Vec::new()
            }
        }
    }

    /// The pointer moved with a gain drag held.
    pub(super) fn rec_drag_to(&mut self, y: f64, fine: bool, out: &mut Vec<UiAction>) {
        let (Some(mut d), Some(src)) = (self.rec.drag, self.rec.source.clone()) else {
            return;
        };
        let n = d.steps.update(y, PX_PER_STEP);
        if n != 0 {
            let (lo, hi) = range();
            d.value = (d.value + i64::from(n) * if fine { 1 } else { STEP }).clamp(lo, hi);
            if d.value != d.sent {
                d.sent = d.value;
                d.changed = true;
                out.push(self.rec_set(
                    &src.device,
                    REC_GAIN_PARAM,
                    d.value as f32 / 10.0,
                    Some(d.merge),
                ));
            }
            self.repaint = true;
        }
        self.rec.drag = Some(d);
    }

    /// The release of a gain drag.
    pub(super) fn rec_release(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.rec.drag.take() {
            if d.changed {
                out.push(UiAction::EndMerge);
            }
            self.repaint = true;
        }
    }

    /// One wheel event over `part`: one notch is one step of the gain.
    pub(super) fn rec_wheel(
        &self,
        part: RecInputPart,
        notches: i32,
        fine: bool,
        out: &mut Vec<UiAction>,
    ) {
        let (RecInputPart::Gain, Some(src)) = (part, &self.rec.source) else {
            return;
        };
        if notches == 0 {
            return;
        }
        let (lo, hi) = range();
        let v =
            (tenths(src.gain_db) + i64::from(notches) * if fine { 1 } else { STEP }).clamp(lo, hi);
        if v != tenths(src.gain_db) {
            out.push(self.rec_set(&src.device, REC_GAIN_PARAM, v as f32 / 10.0, None));
        }
    }

    /// The tooltip of a box of the section under `pos`.
    pub(super) fn rec_help(&self, pos: Point) -> Option<Help> {
        let l = self.rec_layout()?;
        let src = self.rec.source.as_ref()?;
        if l.gain.contains(pos) {
            return Some(Help::new(
                l.gain,
                format!(
                    "Input gain of {} in dB, applied to the recording and the monitor: drag or scroll to change (Shift: fine), double-click for 0 dB",
                    src.name
                ),
            ));
        }
        if l.safe.contains(pos) {
            return Some(Help::new(
                l.safe,
                "Clip safe: the recording is limited so it never goes above 0 dBFS (the monitor is not)",
            ));
        }
        l.meter.contains(pos).then(|| {
            Help::new(
                l.meter,
                "Input level after the gain; the lamp lights when it reaches 0 dBFS and stays lit until you click here",
            )
        })
    }

    /// Paints the section.
    pub(super) fn paint_rec_input(&self, c: &mut dyn Canvas) {
        let (Some(l), Some(src)) = (self.rec_layout(), &self.rec.source) else {
            return;
        };
        let st = &self.st;
        let t = &st.base;
        // Gain.
        sp::lcd(c, st, l.gain, self.rec.drag.map(|_| t.focus));
        sp::label(
            c,
            "INPUT GAIN",
            l.gain.x0 + 6.0,
            l.gain.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        let shown = self
            .rec
            .drag
            .map_or_else(|| tenths(src.gain_db), |d| d.value);
        c.text(
            &TextRun::new(
                gain_text(shown),
                pt(l.gain.x1 - LCD_PAD, l.gain.y1 - 7.0),
                VALUE_SIZE,
                t.highlight,
            )
            .weight(600)
            .align(Align::End),
        );
        // Safe.
        let r = Rect::from_center_size(l.safe.center(), (l.safe.width(), BUTTON_H));
        sp::button(c, st, r, "SAFE", src.clip_safe.then_some(t.focus), false);
        // Meter.
        sp::lcd(c, st, l.meter, None);
        let name = crate::paint::fit_label(&src.name, l.meter.width() - 30.0, 6.5);
        sp::label(
            c,
            &name.to_uppercase(),
            l.meter.x0 + 6.0,
            l.meter.y0 + 10.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        let level = self
            .rec
            .levels
            .get(&src.device)
            .copied()
            .unwrap_or_default();
        let bar = Rect::new(
            l.meter.x0 + 6.0,
            l.meter.y1 - 12.0,
            l.meter.x1 - 26.0,
            l.meter.y1 - 6.0,
        );
        c.fill_with(
            RoundedRect::from_rect(bar, 1.5),
            t.bevel_dark.with_alpha(0.55),
        );
        let x_of = |db: f32| bar.x0 + bar.width() * meter_norm(db);
        for (lo, hi, color) in [
            (METER_FLOOR_DB, -9.0, st.solo),
            (-9.0, -0.5, st.mute),
            (-0.5, 6.0, st.record),
        ] {
            let top = level.bar.level_db.min(hi);
            if top > lo {
                c.fill_with(Rect::new(x_of(lo), bar.y0, x_of(top), bar.y1), color);
            }
        }
        if level.bar.hold_db > METER_FLOOR_DB + 0.5 {
            let x = x_of(level.bar.hold_db).min(bar.x1 - 1.0);
            sp::line(c, x, bar.y0 - 1.0, x, bar.y1 + 1.0, t.highlight);
        }
        // The 0 dBFS mark.
        sp::line(
            c,
            x_of(0.0),
            bar.y0 - 2.0,
            x_of(0.0),
            bar.y1 + 2.0,
            t.text_muted,
        );
        // The clip lamp.
        let lamp = Circle::new(pt(l.meter.x1 - 12.0, l.meter.center().y), 4.5);
        c.fill_with(
            lamp,
            if level.latched {
                st.record
            } else {
                t.bevel_dark.with_alpha(0.7)
            },
        );
    }
}
