//! The clip inspector at the right of the transport bar's tempo and signature readouts: the
//! properties of the selected audio clips (fade in, fade out, transpose, gain, reverse), edited
//! in place.
//!
//! The app hands the bar what is selected ([`InspectorSource`], from
//! [`ArrangementView::audio_inspector`](crate::ArrangementView::audio_inspector)); with nothing
//! applicable the inspector is not drawn. With an audio track selected and no clip it shows the
//! track's name and the fields dimmed (there is nothing to edit yet). With several clips a
//! field shows its value when every clip has it and a dash when they differ; an edit sets the
//! value on all of them, as **one undo step** (one batch for a click, a wheel notch or typed
//! text, one merge group for a drag).
//!
//! * Fades are in milliseconds (the model keeps frames); a fade never grows past the clip's
//!   length minus the other fade.
//! * Transpose is in semitones with two decimals (the model keeps cents): a drag or the wheel on
//!   the whole part steps a semitone, on the decimals a cent. A reversed clip cannot be
//!   transposed and the Repitch warp mode does not transpose, so a clip of either kind keeps its
//!   settings, and when no selected clip can be transposed the field is disabled and its tooltip
//!   says why.
//! * Reverse switches a warped clip's warp and transposition off, as the Reverse Audio command
//!   does.
//!
//! When the bar is too narrow the clip name goes first, then fields from the right (gain,
//! reverse, transpose, fade out), so nothing overlaps the readouts.

use super::*;
use atomik_model::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN};
use atomik_model::{AudioClipContent, ClipId, DeviceKey, WARP_TRANSPOSE_MAX_CENTS, WarpMode};

/// Gap between the inspector's boxes.
const GAP: f64 = 8.0;
/// Gap between the signature readout and the inspector.
const LEAD: f64 = 18.0;
/// Right margin of the bar.
const TAIL: f64 = 12.0;
/// Height of a box (the readouts' height).
const BOX_H: f64 = 28.0;
/// Font size of a value.
const VALUE_SIZE: f32 = 12.0;
/// Characters of a typed value.
const TEXT_MAX: usize = 8;
/// Milliseconds of a fade step (Shift: one).
const FADE_STEP_MS: i64 = 10;
/// Cents of a semitone step (Shift: ten cents).
const SEMITONE_CENTS: i64 = 100;
/// Tenths of a dB of a gain step (Shift: one tenth).
const GAIN_STEP: i64 = 10;
/// What a value shows when the selected clips differ.
const MIXED: &str = "\u{2014}";

/// One selected audio clip as the inspector needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct InspClip {
    /// The clip.
    pub id: ClipId,
    /// Start in ticks.
    pub start: i64,
    /// Length in ticks.
    pub length: i64,
    /// The clip is muted.
    pub muted: bool,
    /// The audio content.
    pub content: AudioClipContent,
    /// Sample rate of the clip's asset, Hz (the unit of its fades).
    pub rate: u32,
    /// The clip's length in frames at that rate.
    pub frames: u64,
}

impl InspClip {
    /// Milliseconds of `frames` at the clip's rate, rounded.
    fn ms(&self, frames: u64) -> i64 {
        let rate = u64::from(self.rate.max(1));
        ((frames * 1000 + rate / 2) / rate) as i64
    }

    /// Frames of `ms` milliseconds at the clip's rate.
    fn frames_of(&self, ms: i64) -> u64 {
        (ms.max(0) as u64 * u64::from(self.rate) + 500) / 1000
    }

    /// The clip can be transposed: not reversed and not in the Repitch mode.
    fn can_transpose(&self) -> bool {
        !self.content.reverse && self.content.warp.mode != WarpMode::Repitch
    }

    /// The most a fade can be, in milliseconds: the clip less the other fade.
    fn fade_max_ms(&self, out: bool) -> i64 {
        let other = if out {
            self.content.fade_in
        } else {
            self.content.fade_out
        };
        self.ms(self.frames.saturating_sub(other))
    }
}

/// What the inspector shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum InspectorSource {
    /// Nothing applicable is selected: the inspector is not drawn.
    #[default]
    None,
    /// An audio track is selected and no clip: its name, the fields dimmed.
    Track(DeviceKey),
    /// The selected audio clips, in song order.
    Clips(Vec<InspClip>),
}

/// A part of the inspector a drag, the wheel or a click acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorField {
    /// Fade-in time.
    FadeIn,
    /// Fade-out time.
    FadeOut,
    /// The whole semitones of the transposition.
    TransposeWhole,
    /// The cents of the transposition (the two decimals).
    TransposeCents,
    /// Clip gain.
    Gain,
    /// The reverse button.
    Reverse,
}

/// A box of the inspector (the transposition is one box with two zones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    FadeIn,
    FadeOut,
    Transpose,
    Gain,
    Reverse,
}

const SLOTS: [Slot; 5] = [
    Slot::FadeIn,
    Slot::FadeOut,
    Slot::Transpose,
    Slot::Gain,
    Slot::Reverse,
];

impl Slot {
    fn width(self) -> f64 {
        match self {
            Slot::FadeIn | Slot::FadeOut => 68.0,
            Slot::Transpose => 78.0,
            Slot::Gain => 72.0,
            Slot::Reverse => 40.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Slot::FadeIn => "FADE IN",
            Slot::FadeOut => "FADE OUT",
            Slot::Transpose => "TRANSPOSE",
            Slot::Gain => "GAIN",
            Slot::Reverse => "REV",
        }
    }
}

impl InspectorField {
    fn slot(self) -> Slot {
        match self {
            InspectorField::FadeIn => Slot::FadeIn,
            InspectorField::FadeOut => Slot::FadeOut,
            InspectorField::TransposeWhole | InspectorField::TransposeCents => Slot::Transpose,
            InspectorField::Gain => Slot::Gain,
            InspectorField::Reverse => Slot::Reverse,
        }
    }
}

/// What a slot shows over the selected clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// No clip.
    Empty,
    /// The clips differ.
    Mixed,
    /// The same value on every clip (ms, cents, tenths of a dB, 0 or 1).
    Value(i64),
}

/// A drag on a field: the value is tracked here so it never depends on the document catching up
/// between two moves.
#[derive(Debug, Clone, Copy)]
struct InspDrag {
    field: InspectorField,
    steps: StepDrag,
    value: i64,
    /// The last value sent, so a drag held against the end of the range sends nothing.
    sent: i64,
    merge: u64,
    changed: bool,
}

/// The inspector's state inside the bar.
#[derive(Debug, Default)]
pub(super) struct Inspector {
    source: InspectorSource,
    drag: Option<InspDrag>,
    /// The field being typed into, its text and whether the text is still the untouched original.
    editing: Option<(InspectorField, String, bool)>,
}

/// Where the inspector's boxes are.
struct Layout {
    name: Option<Rect>,
    slots: Vec<(Slot, Rect)>,
}

/// The value of `slot` on a clip (ms, cents, tenths of a dB, 0 or 1).
fn slot_value(c: &InspClip, slot: Slot) -> i64 {
    match slot {
        Slot::FadeIn => c.ms(c.content.fade_in),
        Slot::FadeOut => c.ms(c.content.fade_out),
        Slot::Transpose => i64::from(c.content.warp.transpose_cents),
        Slot::Gain => (f64::from(c.content.gain_db) * 10.0).round() as i64,
        Slot::Reverse => i64::from(c.content.reverse),
    }
}

fn decimals_w() -> f64 {
    crate::paint::estimate_width(".00", VALUE_SIZE)
}

impl Inspector {
    pub(super) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    fn clips(&self) -> &[InspClip] {
        match &self.source {
            InspectorSource::Clips(c) => c,
            _ => &[],
        }
    }

    fn shown(&self, slot: Slot) -> Shown {
        let clips = self.clips();
        let Some(first) = clips.first().map(|c| slot_value(c, slot)) else {
            return Shown::Empty;
        };
        if clips.iter().all(|c| slot_value(c, slot) == first) {
            Shown::Value(first)
        } else {
            Shown::Mixed
        }
    }

    /// The value a drag, the wheel or typing starts from: the shared value, or the first clip's.
    fn base(&self, slot: Slot) -> i64 {
        self.clips().first().map_or(0, |c| slot_value(c, slot))
    }

    /// Why a slot cannot be edited, `None` when it can.
    fn disabled(&self, slot: Slot) -> Option<&'static str> {
        let clips = self.clips();
        if clips.is_empty() {
            return Some("select an audio clip to edit it");
        }
        if slot == Slot::Transpose && !clips.iter().any(InspClip::can_transpose) {
            return Some(if clips.iter().all(|c| c.content.reverse) {
                "a reversed clip cannot be transposed"
            } else {
                "the Repitch warp mode does not transpose: pick another mode"
            });
        }
        None
    }

    fn range(&self, slot: Slot) -> (i64, i64) {
        match slot {
            Slot::FadeIn | Slot::FadeOut => (
                0,
                self.clips()
                    .iter()
                    .map(|c| c.fade_max_ms(slot == Slot::FadeOut))
                    .max()
                    .unwrap_or(0),
            ),
            Slot::Transpose => (
                -i64::from(WARP_TRANSPOSE_MAX_CENTS),
                i64::from(WARP_TRANSPOSE_MAX_CENTS),
            ),
            Slot::Gain => (
                (f64::from(AUDIO_GAIN_DB_MIN) * 10.0).ceil() as i64,
                (f64::from(AUDIO_GAIN_DB_MAX) * 10.0).floor() as i64,
            ),
            Slot::Reverse => (0, 1),
        }
    }

    /// The new content of every clip the edit changes, with the clip's index.
    fn contents(&self, slot: Slot, value: i64) -> Vec<(usize, AudioClipContent)> {
        let (lo, hi) = self.range(slot);
        let value = value.clamp(lo, hi);
        self.clips()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let mut a = c.content.clone();
                match slot {
                    Slot::FadeIn => {
                        a.fade_in = c
                            .frames_of(value)
                            .min(c.frames.saturating_sub(c.content.fade_out));
                    }
                    Slot::FadeOut => {
                        a.fade_out = c
                            .frames_of(value)
                            .min(c.frames.saturating_sub(c.content.fade_in));
                    }
                    Slot::Transpose => {
                        if !c.can_transpose() {
                            return None;
                        }
                        a.warp.transpose_cents = value as i32;
                    }
                    Slot::Gain => a.gain_db = value as f32 / 10.0,
                    Slot::Reverse => {
                        a.reverse = value != 0;
                        if a.reverse {
                            a.warp.enabled = false;
                            a.warp.transpose_cents = 0;
                        }
                    }
                }
                (a != c.content).then_some((i, a))
            })
            .collect()
    }

    fn set_commands(&self, slot: Slot, value: i64) -> Vec<Command> {
        self.contents(slot, value)
            .into_iter()
            .map(|(i, content)| {
                let c = &self.clips()[i];
                Command::Song(SongCommand::SetAudioClip {
                    id: c.id,
                    start: c.start,
                    length: c.length,
                    muted: c.muted,
                    content,
                    merge: None,
                })
            })
            .collect()
    }

    /// One undo step: the command itself for one clip, a batch for several.
    fn one_step(&self, slot: Slot, value: i64) -> Option<UiAction> {
        let mut cmds = self.set_commands(slot, value);
        match cmds.len() {
            0 => None,
            1 => Some(UiAction::Command(cmds.remove(0))),
            _ => Some(UiAction::Command(Command::Batch(cmds))),
        }
    }

    /// The same as separate commands of one merge group (a drag).
    fn merged(&self, slot: Slot, value: i64, merge: u64) -> Vec<UiAction> {
        self.set_commands(slot, value)
            .into_iter()
            .map(|c| {
                UiAction::Command(match c {
                    Command::Song(SongCommand::SetAudioClip {
                        id,
                        start,
                        length,
                        muted,
                        content,
                        ..
                    }) => Command::Song(SongCommand::SetAudioClip {
                        id,
                        start,
                        length,
                        muted,
                        content,
                        merge: Some(merge),
                    }),
                    other => other,
                })
            })
            .collect()
    }
}

/// The value `notches` steps from `v` on `field`, in the units of its slot, clamped.
fn step_value(field: InspectorField, v: i64, notches: i32, fine: bool) -> i64 {
    let step = match field {
        InspectorField::FadeIn | InspectorField::FadeOut => {
            if fine {
                1
            } else {
                FADE_STEP_MS
            }
        }
        InspectorField::TransposeWhole => {
            if fine {
                SEMITONE_CENTS / 10
            } else {
                SEMITONE_CENTS
            }
        }
        InspectorField::TransposeCents => 1,
        InspectorField::Gain => {
            if fine {
                1
            } else {
                GAIN_STEP
            }
        }
        InspectorField::Reverse => 0,
    };
    v + i64::from(notches) * step
}

/// The text of a value, as the field shows it.
fn value_text(slot: Slot, v: i64) -> String {
    match slot {
        Slot::FadeIn | Slot::FadeOut => format!("{v} ms"),
        Slot::Transpose => format!("{:+.2}", v as f64 / 100.0),
        Slot::Gain => format!("{:+.1} dB", v as f64 / 10.0),
        Slot::Reverse => String::new(),
    }
}

/// The text typing starts from.
fn edit_text(slot: Slot, v: i64) -> String {
    match slot {
        Slot::FadeIn | Slot::FadeOut => v.to_string(),
        Slot::Transpose => format!("{:.2}", v as f64 / 100.0),
        Slot::Gain => format!("{:.1}", v as f64 / 10.0),
        Slot::Reverse => String::new(),
    }
}

/// The value typed `text` means in the units of `slot`; `None` when it does not parse.
fn parse_text(slot: Slot, text: &str) -> Option<i64> {
    let v: f64 = text.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(match slot {
        Slot::FadeIn | Slot::FadeOut => v.round() as i64,
        Slot::Transpose => (v * 100.0).round() as i64,
        Slot::Gain => (v * 10.0).round() as i64,
        Slot::Reverse => return None,
    })
}

impl TransportBar {
    /// Hands the bar what is selected in the arrangement (the clip inspector's content).
    pub fn set_inspector(&mut self, source: InspectorSource) {
        if self.insp.source != source {
            if matches!(source, InspectorSource::None) || self.insp.clips().is_empty() {
                self.insp.editing = None;
            }
            self.insp.source = source;
            self.repaint = true;
        }
    }

    /// True while a value of the inspector is being typed: route every key event here.
    pub fn is_editing_inspector(&self) -> bool {
        self.insp.editing.is_some()
    }

    /// The text being typed into the inspector.
    pub fn inspector_text(&self) -> Option<&str> {
        self.insp.editing.as_ref().map(|(_, s, _)| s.as_str())
    }

    /// The fields the inspector shows now (fewer on a narrow bar), left to right.
    pub fn inspector_fields(&self) -> Vec<InspectorField> {
        self.inspector_layout()
            .slots
            .into_iter()
            .flat_map(|(s, _)| match s {
                Slot::FadeIn => vec![InspectorField::FadeIn],
                Slot::FadeOut => vec![InspectorField::FadeOut],
                Slot::Transpose => vec![
                    InspectorField::TransposeWhole,
                    InspectorField::TransposeCents,
                ],
                Slot::Gain => vec![InspectorField::Gain],
                Slot::Reverse => vec![InspectorField::Reverse],
            })
            .collect()
    }

    /// A point on `field`, `None` when the inspector does not show it.
    pub fn inspector_point(&self, field: InspectorField) -> Option<Point> {
        let (_, r) = self
            .inspector_layout()
            .slots
            .into_iter()
            .find(|(s, _)| *s == field.slot())?;
        Some(match field {
            InspectorField::TransposeWhole => pt(r.x0 + 12.0, r.center().y),
            InspectorField::TransposeCents => pt(r.x1 - LCD_PAD - decimals_w() / 2.0, r.center().y),
            _ => r.center(),
        })
    }

    /// True while a drag on an inspector field is held.
    pub fn is_dragging_inspector(&self) -> bool {
        self.insp.drag.is_some()
    }

    fn inspector_layout(&self) -> Layout {
        let mut out = Layout {
            name: None,
            slots: Vec::new(),
        };
        if self.insp.source == InspectorSource::None {
            return out;
        }
        let x0 = self.lcd_rects()[3].x1 + LEAD;
        let limit = self.bounds.x1 - TAIL - self.rec_reserved();
        let y0 = self.bounds.center().y - BOX_H / 2.0;
        let all: f64 = SLOTS.iter().map(|s| s.width() + GAP).sum();
        let name_w = 118.0;
        let mut x = x0;
        if x0 + name_w + GAP + all - GAP <= limit {
            out.name = Some(Rect::new(x, y0, x + name_w, y0 + BOX_H));
            x += name_w + GAP;
        }
        for s in SLOTS {
            let w = s.width();
            if x + w > limit {
                break;
            }
            out.slots.push((s, Rect::new(x, y0, x + w, y0 + BOX_H)));
            x += w + GAP;
        }
        out
    }

    fn inspector_hit(&self, pos: Point) -> Option<InspectorField> {
        for (s, r) in self.inspector_layout().slots {
            if r.contains(pos) {
                return Some(match s {
                    Slot::FadeIn => InspectorField::FadeIn,
                    Slot::FadeOut => InspectorField::FadeOut,
                    Slot::Gain => InspectorField::Gain,
                    Slot::Reverse => InspectorField::Reverse,
                    Slot::Transpose => {
                        if pos.x >= r.x1 - LCD_PAD - decimals_w() {
                            InspectorField::TransposeCents
                        } else {
                            InspectorField::TransposeWhole
                        }
                    }
                });
            }
        }
        None
    }

    /// A primary press on an inspector field.
    pub(super) fn inspector_press(
        &mut self,
        field: InspectorField,
        clicks: u8,
        y: f64,
    ) -> Vec<UiAction> {
        let slot = field.slot();
        if self.insp.disabled(slot).is_some() {
            return Vec::new();
        }
        if field == InspectorField::Reverse {
            let to = i64::from(self.insp.shown(slot) != Shown::Value(1));
            return self.insp.one_step(slot, to).into_iter().collect();
        }
        if clicks >= 2 {
            let text = match self.insp.shown(slot) {
                Shown::Value(v) => edit_text(slot, v),
                _ => String::new(),
            };
            self.insp.drag = None;
            self.insp.editing = Some((field, text, true));
            return Vec::new();
        }
        let value = self.insp.base(slot);
        let sent = match self.insp.shown(slot) {
            Shown::Value(v) => v,
            _ => i64::MIN,
        };
        self.insp.drag = Some(InspDrag {
            field,
            steps: StepDrag::new(y),
            value,
            sent,
            merge: self.next_merge,
            changed: false,
        });
        self.next_merge += 1;
        Vec::new()
    }

    /// The pointer moved with an inspector drag held.
    pub(super) fn inspector_drag_to(&mut self, y: f64, fine: bool, out: &mut Vec<UiAction>) {
        let Some(mut d) = self.insp.drag else { return };
        let n = d.steps.update(y, PX_PER_STEP);
        if n != 0 {
            let (lo, hi) = self.insp.range(d.field.slot());
            d.value = step_value(d.field, d.value, n, fine).clamp(lo, hi);
            if d.value != d.sent {
                d.sent = d.value;
                let actions = self.insp.merged(d.field.slot(), d.value, d.merge);
                d.changed |= !actions.is_empty();
                out.extend(actions);
            }
            self.repaint = true;
        }
        self.insp.drag = Some(d);
    }

    /// The release of an inspector drag.
    pub(super) fn inspector_release(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.insp.drag.take() {
            if d.changed {
                out.push(UiAction::EndMerge);
            }
            self.repaint = true;
        }
    }

    /// One wheel event over `field`: one notch is one step.
    pub(super) fn inspector_wheel(
        &self,
        field: InspectorField,
        notches: i32,
        fine: bool,
        out: &mut Vec<UiAction>,
    ) {
        let slot = field.slot();
        if notches == 0 || field == InspectorField::Reverse || self.insp.disabled(slot).is_some() {
            return;
        }
        let (lo, hi) = self.insp.range(slot);
        let v = step_value(field, self.insp.base(slot), notches, fine).clamp(lo, hi);
        out.extend(self.insp.one_step(slot, v));
    }

    /// The inspector field under `pos`, for the events that need it.
    pub(super) fn inspector_field_at(&self, pos: Point) -> Option<InspectorField> {
        self.inspector_hit(pos)
    }

    /// Commits the typed value.
    pub(super) fn inspector_commit(&mut self, out: &mut Vec<UiAction>) {
        if let Some((field, text, _)) = self.insp.editing.take() {
            let slot = field.slot();
            if let Some(v) = parse_text(slot, &text) {
                out.extend(self.insp.one_step(slot, v));
            }
        }
        self.repaint = true;
    }

    /// A key while a value is being typed.
    pub(super) fn inspector_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some((field, text, fresh)) = &mut self.insp.editing else {
            return;
        };
        let signed = matches!(
            field,
            InspectorField::TransposeWhole | InspectorField::TransposeCents | InspectorField::Gain
        );
        match key {
            Key::Char(c)
                if c.is_ascii_digit() || *c == '.' || (signed && matches!(c, '-' | '+')) =>
            {
                if *fresh {
                    text.clear();
                    *fresh = false;
                }
                if text.len() < TEXT_MAX {
                    text.push(*c);
                }
            }
            Key::Backspace => {
                *fresh = false;
                text.pop();
            }
            Key::Enter => return self.inspector_commit(out),
            Key::Escape => self.insp.editing = None,
            _ => {}
        }
        self.repaint = true;
    }

    /// The tooltip of an inspector box under `pos`.
    pub(super) fn inspector_help(&self, pos: Point) -> Option<Help> {
        let layout = self.inspector_layout();
        if let Some(r) = layout.name.filter(|r| r.contains(pos)) {
            return Some(Help::new(
                r,
                "The selected audio clips, or the audio track when no clip is selected",
            ));
        }
        let (slot, r) = layout.slots.into_iter().find(|(_, r)| r.contains(pos))?;
        if let Some(why) = self.insp.disabled(slot) {
            let what = match slot {
                Slot::FadeIn => "Fade in",
                Slot::FadeOut => "Fade out",
                Slot::Transpose => "Transpose",
                Slot::Gain => "Clip gain",
                Slot::Reverse => "Reverse",
            };
            return Some(Help::new(r, format!("{what}: {why}")));
        }
        let text = match slot {
            Slot::FadeIn => {
                "Fade in time in ms: drag or scroll to change (Shift: fine), double-click to type"
            }
            Slot::FadeOut => {
                "Fade out time in ms: drag or scroll to change (Shift: fine), double-click to type"
            }
            Slot::Transpose => {
                "Transpose in semitones: drag or scroll the whole part, or the cents after the point; double-click to type. Reversed clips and the Repitch mode are left alone"
            }
            Slot::Gain => {
                "Clip gain in dB: drag or scroll to change (Shift: fine), double-click to type"
            }
            Slot::Reverse => {
                "Play the clip backwards (a warped clip stops following the tempo and transposing)"
            }
        };
        Some(Help::new(r, text))
    }

    /// Paints the inspector.
    pub(super) fn paint_inspector(&self, c: &mut dyn Canvas, doc: &RackDoc) {
        let layout = self.inspector_layout();
        let st = &self.st;
        let t = &st.base;
        if let Some(r) = layout.name {
            let name = match &self.insp.source {
                InspectorSource::Track(d) => sp::track_name(doc, d),
                InspectorSource::Clips(clips) => match clips.as_slice() {
                    [one] => doc
                        .assets
                        .get(&one.content.asset)
                        .map_or_else(|| "Audio clip".to_owned(), |m| m.original_name.clone()),
                    many => format!("{} audio clips", many.len()),
                },
                InspectorSource::None => String::new(),
            };
            let tag = match self.insp.source {
                InspectorSource::Track(_) => "AUDIO TRACK",
                _ => "AUDIO CLIP",
            };
            sp::lcd(c, st, r, None);
            sp::label(
                c,
                tag,
                r.x0 + 6.0,
                r.y0 + 10.0,
                6.5,
                t.text_muted,
                Align::Start,
            );
            let text = crate::paint::fit_label(&name, r.width() - 12.0, 11.0);
            c.text(&TextRun::new(text, pt(r.x0 + 6.0, r.y1 - 7.0), 11.0, t.highlight).weight(600));
        }
        for (slot, r) in layout.slots {
            let off = self.insp.disabled(slot).is_some();
            let shown = self.insp.shown(slot);
            if slot == Slot::Reverse {
                let lit = match shown {
                    Shown::Value(1) => Some(t.focus),
                    Shown::Mixed => Some(mix(t.focus, t.button, 0.55)),
                    _ => None,
                };
                let r = Rect::from_center_size(r.center(), (r.width(), BUTTON_H));
                sp::button(c, st, r, slot.label(), lit.filter(|_| !off), false);
                continue;
            }
            let editing = self
                .insp
                .editing
                .as_ref()
                .filter(|(f, _, _)| f.slot() == slot);
            sp::lcd(c, st, r, editing.map(|_| t.focus));
            sp::label(
                c,
                slot.label(),
                r.x0 + 6.0,
                r.y0 + 10.0,
                6.5,
                t.text_muted,
                Align::Start,
            );
            let text = match (editing, shown) {
                (Some((_, s, _)), _) => format!("{s}_"),
                (None, Shown::Value(v)) => value_text(slot, v),
                (None, Shown::Mixed) => MIXED.to_owned(),
                (None, Shown::Empty) => String::new(),
            };
            let color = if off { t.text_muted } else { t.highlight };
            c.text(
                &TextRun::new(text, pt(r.x1 - LCD_PAD, r.y1 - 7.0), VALUE_SIZE, color)
                    .weight(600)
                    .align(Align::End),
            );
        }
    }
}
