//! A segmented time field: a tick shown as `bar.beat.16th.tick` (a position) or as the same
//! four parts counted from zero (a length), with each part edited on its own.
//!
//! The field is plain data plus pure functions, so a host with several of them (the arrangement
//! inspector's Position, Length and Loop fields) keeps one [`SegmentDrag`] or [`SegmentEdit`]
//! for the gesture in progress and asks the field what a step or a typed text means:
//!
//! * a press on a part and a vertical drag steps that part (Shift: the next finer unit), the
//!   wheel steps it by one notch, Up and Down step it while typing;
//! * a double-click opens typing on a part: digits replace the part, `+n` and `-n` add or
//!   subtract `n` ticks, and a text with dots (`3.2.1.0`) sets the whole field;
//! * the parts follow the song's meter map ([`Bars`]): a bar is as long as the bar at the
//!   value, a length counts bars and beats of the meter where it starts.
//!
//! The field never clamps: the host applies its own range (a clip starts at zero or later, a
//! length is at least one tick).

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::Key;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag};
use crate::theme::mix;
use crate::timeline::{Bars, PPQ};
use vello::kurbo::Rect;

/// Parts of a field: bar, beat, sixteenth, tick.
pub const SEGMENTS: usize = 4;
/// Ticks of a sixteenth note.
pub const SIXTEENTH: i64 = PPQ / 4;
/// Ticks one step of the tick part moves (Shift: one).
const TICK_STEP: i64 = 10;
/// Pixel widths of the four parts.
const SEG_WIDTHS: [f64; SEGMENTS] = [30.0, 18.0, 18.0, 38.0];
/// Pixels between two parts (where the dot is drawn).
const SEG_GAP: f64 = 7.0;
/// Pixels between the field's edge and its first part.
const PAD: f64 = 6.0;
/// Pixels from the top of a field to its parts (the label sits above).
const LABEL_BAND: f64 = 12.0;
/// The most digits a typed part takes.
const TEXT_MAX: usize = 8;
/// Parts above this read as this when typed (keeps the maths far from overflow).
const PART_MAX: i64 = 1_000_000;

/// The width of one field in logical pixels.
pub const FIELD_WIDTH: f64 =
    PAD * 2.0 + SEG_WIDTHS[0] + SEG_WIDTHS[1] + SEG_WIDTHS[2] + SEG_WIDTHS[3] + SEG_GAP * 3.0;

/// What the field counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentMode {
    /// A song position: bar, beat and sixteenth count from 1, the tick from 0.
    Position,
    /// A length (or an offset) from `anchor`: every part counts from 0, in the meter at
    /// `anchor`.
    Length {
        /// The tick the length starts at.
        anchor: i64,
    },
}

/// What a field shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValue {
    /// Nothing to show.
    Empty,
    /// The things edited differ.
    Mixed,
    /// One value, in ticks.
    Value(i64),
}

/// A segmented field over a meter map.
#[derive(Debug, Clone)]
pub struct SegmentField {
    bars: Bars,
    mode: SegmentMode,
}

impl SegmentField {
    /// A field of `mode` over `bars`.
    pub fn new(bars: &Bars, mode: SegmentMode) -> Self {
        Self {
            bars: bars.clone(),
            mode,
        }
    }

    /// What the field counts.
    pub fn mode(&self) -> SegmentMode {
        self.mode
    }

    /// The four parts of `tick`.
    pub fn split(&self, tick: i64) -> [i64; SEGMENTS] {
        match self.mode {
            SegmentMode::Position => {
                let (bar, beat, rem) = self.bars.map().bbt(tick);
                [
                    i64::from(bar),
                    i64::from(beat),
                    i64::from(rem) / SIXTEENTH + 1,
                    i64::from(rem) % SIXTEENTH,
                ]
            }
            SegmentMode::Length { anchor } => {
                let t = tick.max(0);
                let map = self.bars.map();
                let (bar_t, beat_t) = (
                    map.bar_ticks_at(anchor.max(0)),
                    map.beat_ticks_at(anchor.max(0)),
                );
                let rem = t % bar_t;
                let in_beat = rem % beat_t;
                [
                    t / bar_t,
                    rem / beat_t,
                    in_beat / SIXTEENTH,
                    in_beat % SIXTEENTH,
                ]
            }
        }
    }

    /// The tick of four parts (the inverse of [`Self::split`]; a part past its range rolls
    /// into the next one).
    pub fn join(&self, parts: [i64; SEGMENTS]) -> i64 {
        let p = parts.map(|x| x.clamp(0, PART_MAX));
        match self.mode {
            SegmentMode::Position => {
                let rem = (p[2] - 1).max(0) * SIXTEENTH + p[3];
                self.bars.map().from_bbt((
                    p[0] as u32,
                    p[1] as u32,
                    u32::try_from(rem).unwrap_or(u32::MAX),
                ))
            }
            SegmentMode::Length { anchor } => {
                let map = self.bars.map();
                p[0] * map.bar_ticks_at(anchor.max(0))
                    + p[1] * map.beat_ticks_at(anchor.max(0))
                    + p[2] * SIXTEENTH
                    + p[3]
            }
        }
    }

    /// Ticks one step of `part` moves from `tick` (`fine`: the next smaller part's step).
    pub fn unit(&self, tick: i64, part: usize, fine: bool) -> i64 {
        let at = match self.mode {
            SegmentMode::Position => tick,
            SegmentMode::Length { anchor } => anchor,
        }
        .max(0);
        let map = self.bars.map();
        let part = if fine { (part + 1).min(SEGMENTS) } else { part };
        match part {
            0 => map.bar_ticks_at(at),
            1 => map.beat_ticks_at(at),
            2 => SIXTEENTH,
            3 => TICK_STEP,
            _ => 1,
        }
    }

    /// `tick` moved `notches` steps of `part` (not clamped).
    pub fn step(&self, tick: i64, part: usize, notches: i32, fine: bool) -> i64 {
        tick.saturating_add(i64::from(notches) * self.unit(tick, part, fine))
    }

    /// The text of each part.
    pub fn texts(&self, tick: i64) -> [String; SEGMENTS] {
        let p = self.split(tick);
        [
            p[0].to_string(),
            p[1].to_string(),
            p[2].to_string(),
            format!("{:04}", p[3]),
        ]
    }

    /// The tick a typed `text` means while `part` of `tick` is being edited, `None` when it does
    /// not parse: `+n` and `-n` add or subtract `n` ticks, a text with dots sets the whole field
    /// (missing parts read as the first position or as zero), and plain digits replace `part`.
    pub fn parse(&self, tick: i64, part: usize, text: &str) -> Option<i64> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(n) = t.strip_prefix('+') {
            return Some(tick.saturating_add(n.parse::<i64>().ok()?.clamp(0, i64::from(i32::MAX))));
        }
        if let Some(n) = t.strip_prefix('-') {
            return Some(tick.saturating_sub(n.parse::<i64>().ok()?.clamp(0, i64::from(i32::MAX))));
        }
        if t.contains('.') {
            let mut parts = match self.mode {
                SegmentMode::Position => [1, 1, 1, 0],
                SegmentMode::Length { .. } => [0; SEGMENTS],
            };
            for (i, s) in t.split('.').enumerate() {
                if i >= SEGMENTS {
                    return None;
                }
                if !s.is_empty() {
                    parts[i] = s.parse::<i64>().ok()?;
                }
            }
            return Some(self.join(parts));
        }
        let n: i64 = t.parse().ok()?;
        let mut parts = self.split(tick);
        parts[part.min(SEGMENTS - 1)] = n;
        Some(self.join(parts))
    }
}

/// The rectangles of the four parts inside the field `r`.
pub fn segment_rects(r: Rect) -> [Rect; SEGMENTS] {
    let mut x = r.x0 + PAD;
    let y0 = r.y0 + LABEL_BAND;
    SEG_WIDTHS.map(|w| {
        let seg = Rect::new(x, y0, x + w, r.y1 - 1.0);
        x += w + SEG_GAP;
        seg
    })
}

/// The part under `pos` in the field `r`: the nearest part when the pointer is anywhere in the
/// field, `None` outside it.
pub fn hit(r: Rect, pos: vello::kurbo::Point) -> Option<usize> {
    if !r.contains(pos) {
        return None;
    }
    let rects = segment_rects(r);
    Some(
        (0..SEGMENTS)
            .find(|&i| pos.x < rects[i].x1 + SEG_GAP / 2.0)
            .unwrap_or(SEGMENTS - 1),
    )
}

/// A vertical drag on one part: the value is tracked here so it never depends on the document
/// catching up between two moves.
#[derive(Debug, Clone, Copy)]
pub struct SegmentDrag {
    /// The part dragged.
    pub part: usize,
    steps: StepDrag,
    /// The value now, ticks (not clamped).
    pub value: i64,
    /// The last value reported, so a drag held against a limit reports nothing.
    sent: i64,
}

impl SegmentDrag {
    /// A drag on `part` that starts at `y` from `value`.
    pub fn new(part: usize, y: f64, value: i64) -> Self {
        Self {
            part,
            steps: StepDrag::new(y),
            value,
            sent: i64::MIN,
        }
    }

    /// The pointer is now at `y`: the new value when it changed, clamped by `clamp`.
    pub fn update(
        &mut self,
        field: &SegmentField,
        y: f64,
        fine: bool,
        clamp: impl Fn(i64) -> i64,
    ) -> Option<i64> {
        let n = self.steps.update(y, PX_PER_STEP);
        if n == 0 {
            return None;
        }
        self.value = clamp(field.step(self.value, self.part, n, fine));
        if self.value == self.sent {
            return None;
        }
        self.sent = self.value;
        Some(self.value)
    }

    /// The pointer travelled far enough to be a drag.
    pub fn moved(&self) -> bool {
        self.steps.moved()
    }
}

/// What a key means while a part is being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    /// Nothing for the field.
    Ignored,
    /// The text changed.
    Typed,
    /// Enter: commit the text.
    Commit,
    /// Escape: drop the text.
    Cancel,
    /// Up or Down: step the part (`+1` up).
    Step(i32),
    /// Left or Right: move to the neighbouring part (`+1` right).
    Move(i32),
}

/// Typing into one part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentEdit {
    /// The part typed into.
    pub part: usize,
    /// The text so far.
    pub text: String,
    /// The text is still the untouched original: the next character replaces it.
    pub fresh: bool,
}

impl SegmentEdit {
    /// Typing starts on `part` showing `text`.
    pub fn new(part: usize, text: String) -> Self {
        Self {
            part,
            text,
            fresh: true,
        }
    }

    /// Applies a key.
    pub fn key(&mut self, key: &Key) -> EditKey {
        self.key_with(key, |_| false)
    }

    /// Applies a key, taking the characters `extra` accepts besides digits, dots and signs (a
    /// note name takes letters and `#`).
    pub fn key_with(&mut self, key: &Key, extra: impl Fn(char) -> bool) -> EditKey {
        match key {
            Key::Char(c) if c.is_ascii_digit() || matches!(c, '.' | '+' | '-') || extra(*c) => {
                if self.fresh {
                    self.text.clear();
                    self.fresh = false;
                }
                if self.text.len() < TEXT_MAX {
                    self.text.push(*c);
                }
                EditKey::Typed
            }
            Key::Backspace => {
                self.fresh = false;
                self.text.pop();
                EditKey::Typed
            }
            Key::Enter => EditKey::Commit,
            Key::Escape => EditKey::Cancel,
            Key::Up => EditKey::Step(1),
            Key::Down => EditKey::Step(-1),
            Key::Left => EditKey::Move(-1),
            Key::Right => EditKey::Move(1),
            _ => EditKey::Ignored,
        }
    }
}

/// What a field draws besides its value.
#[derive(Debug, Clone, Copy)]
pub struct FieldLook<'a> {
    /// The small label above the parts.
    pub label: &'a str,
    /// The part highlighted (pressed or typed into).
    pub part: Option<usize>,
    /// The text being typed into `part`.
    pub typing: Option<&'a str>,
    /// The field can be edited (otherwise it is dimmed).
    pub enabled: bool,
}

/// Paints a field in the rectangle `r`.
pub fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    field: &SegmentField,
    r: Rect,
    value: FieldValue,
    look: FieldLook<'_>,
) {
    let t = &st.base;
    sp::lcd(c, st, r, look.typing.map(|_| t.focus));
    sp::label(
        c,
        look.label,
        r.x0 + PAD,
        r.y0 + 10.0,
        6.5,
        t.text_muted,
        Align::Start,
    );
    let rects = segment_rects(r);
    let color = if look.enabled {
        t.highlight
    } else {
        t.text_muted
    };
    let texts = match value {
        FieldValue::Value(v) => Some(field.texts(v)),
        _ => None,
    };
    for (i, seg) in rects.iter().enumerate() {
        if look.part == Some(i) {
            c.fill_with(seg.inset((-1.0, 0.0, -1.0, 0.0)), mix(t.lcd, t.focus, 0.3));
        }
        let text = match (look.typing.filter(|_| look.part == Some(i)), &texts, value) {
            (Some(s), _, _) => format!("{s}_"),
            (None, Some(texts), _) => texts[i].clone(),
            (None, None, FieldValue::Mixed) => "\u{2014}".to_owned(),
            (None, None, _) => String::new(),
        };
        c.text(
            &TextRun::new(text, pt(seg.x1, seg.y1 - 6.0), 12.0, color)
                .weight(600)
                .align(Align::End),
        );
        if i + 1 < SEGMENTS {
            c.text(
                &TextRun::new(
                    ".",
                    pt(seg.x1 + SEG_GAP / 2.0, seg.y1 - 6.0),
                    12.0,
                    t.text_muted,
                )
                .align(Align::Center),
            );
        }
    }
}

/// Paints a one-value field (a pitch, a velocity, a parameter value) in the rectangle `r`: the
/// label above, `text` at the right (`typing` replaces it with the text being typed).
pub fn paint_scalar<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    r: Rect,
    label: &str,
    text: &str,
    typing: Option<&str>,
    enabled: bool,
) {
    let t = &st.base;
    sp::lcd(c, st, r, typing.map(|_| t.focus));
    sp::label(
        c,
        label,
        r.x0 + PAD,
        r.y0 + 10.0,
        6.5,
        t.text_muted,
        Align::Start,
    );
    let color = if enabled { t.highlight } else { t.text_muted };
    let shown = typing.map_or_else(|| text.to_owned(), |s| format!("{s}_"));
    c.text(
        &TextRun::new(shown, pt(r.x1 - PAD, r.y1 - 7.0), 12.0, color)
            .weight(600)
            .align(Align::End),
    );
}
