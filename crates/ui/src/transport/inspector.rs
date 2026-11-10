//! The clip inspector at the right of the transport bar's tempo and signature readouts: the
//! properties of the selected audio clips (fade in, fade out, transpose, gain, reverse, warp and
//! the source tempo), edited in place.
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
//!   length minus the other fade. The small unit tag at the top right of a fade box switches
//!   both fades between milliseconds and ticks (the clip's own tick span over its frames).
//! * Transpose is in semitones with two decimals (the model keeps cents): a drag or the wheel on
//!   the whole part steps a semitone, on the decimals a cent. The Repitch warp mode does not
//!   transpose, so a clip in it keeps its settings, and when no selected clip can be transposed
//!   the field is disabled and its tooltip says why.
//! * Reverse plays the clip backwards; a warped clip keeps its warp (the stretched span plays
//!   backwards).
//! * WARP switches the clip's tempo following on and off (its mode, tempo and markers stay when
//!   it goes off).
//! * TEMPO is the tempo the clip's audio was recorded at, in BPM with two decimals (the model's
//!   source tempo). It is edited like the other values and works on the clips that are warped:
//!   with warp off the field is dimmed and its tooltip says to switch warp on first.
//!
//! When the bar is too narrow the clip name goes first, then fields from the right (tempo,
//! warp, gain, reverse, transpose, fade out), so nothing overlaps the readouts. The warp and
//! tempo boxes come after the five older ones and only use room the rest leaves free: the
//! transport cluster does not give up space for them.

use super::*;
use atomik_model::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN};
use atomik_model::{
    AudioClipContent, ClipId, DeviceKey, WARP_BPM_MAX, WARP_BPM_MIN, WARP_TRANSPOSE_MAX_CENTS,
    WarpMode,
};

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
/// Ticks of a fade step when the fades are shown in ticks (Shift: one): a 16th note.
const FADE_STEP_TICKS: i64 = 960;
/// Width of the unit tag at the top right of a fade box.
const UNIT_TAG_W: f64 = 30.0;
/// Height of the label row of a box, which holds the unit tag.
const LABEL_ROW_H: f64 = 13.0;

/// The unit the inspector shows the fades in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FadeUnit {
    /// Milliseconds.
    #[default]
    Ms,
    /// Ticks of the clip's own span.
    Ticks,
}

impl FadeUnit {
    fn other(self) -> Self {
        match self {
            Self::Ms => Self::Ticks,
            Self::Ticks => Self::Ms,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Ms => "MS",
            Self::Ticks => "TICKS",
        }
    }
}
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

    /// Ticks of `frames`: the clip's tick length spread over its frames, rounded.
    fn ticks(&self, frames: u64) -> i64 {
        let total = u128::from(self.frames.max(1));
        ((u128::from(frames) * self.length.max(0) as u128 + total / 2) / total) as i64
    }

    /// Frames of `ticks` ticks of the clip.
    fn frames_of_ticks(&self, ticks: i64) -> u64 {
        let length = u128::from(self.length.max(1) as u64);
        ((ticks.max(0) as u128 * u128::from(self.frames) + length / 2) / length) as u64
    }

    /// `frames` in `unit`.
    fn fade_in_unit(&self, frames: u64, unit: FadeUnit) -> i64 {
        match unit {
            FadeUnit::Ms => self.ms(frames),
            FadeUnit::Ticks => self.ticks(frames),
        }
    }

    /// Frames of `value` in `unit`.
    fn fade_frames(&self, value: i64, unit: FadeUnit) -> u64 {
        match unit {
            FadeUnit::Ms => self.frames_of(value),
            FadeUnit::Ticks => self.frames_of_ticks(value),
        }
    }

    /// The clip can be transposed: it is not in the Repitch mode.
    fn can_transpose(&self) -> bool {
        self.content.warp.mode != WarpMode::Repitch
    }

    /// The most a fade can be, in `unit`: the clip less the other fade.
    fn fade_max(&self, out: bool, unit: FadeUnit) -> i64 {
        let other = if out {
            self.content.fade_in
        } else {
            self.content.fade_out
        };
        self.fade_in_unit(self.frames.saturating_sub(other), unit)
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
    /// The warp button.
    Warp,
    /// The source tempo of a warped clip.
    Tempo,
}

/// A box of the inspector (the transposition is one box with two zones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    FadeIn,
    FadeOut,
    Transpose,
    Gain,
    Reverse,
    Warp,
    Tempo,
}

/// The boxes the bar always keeps room for (the transport cluster and the clip name give way
/// to them); the boxes after them only appear when the window is wide enough.
const CORE_SLOTS: usize = 5;

const SLOTS: [Slot; 7] = [
    Slot::FadeIn,
    Slot::FadeOut,
    Slot::Transpose,
    Slot::Gain,
    Slot::Reverse,
    Slot::Warp,
    Slot::Tempo,
];

impl Slot {
    fn width(self) -> f64 {
        match self {
            Slot::FadeIn | Slot::FadeOut => 68.0,
            Slot::Transpose => 78.0,
            Slot::Gain => 72.0,
            Slot::Reverse => 40.0,
            Slot::Warp => 44.0,
            Slot::Tempo => 70.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Slot::FadeIn => "FADE IN",
            Slot::FadeOut => "FADE OUT",
            Slot::Transpose => "TRANSPOSE",
            Slot::Gain => "GAIN",
            Slot::Reverse => "REV",
            Slot::Warp => "WARP",
            Slot::Tempo => "TEMPO",
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
            InspectorField::Warp => Slot::Warp,
            InspectorField::Tempo => Slot::Tempo,
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
    /// The unit the fades are shown, dragged and typed in.
    unit: FadeUnit,
    /// The field being typed into, its text and whether the text is still the untouched original.
    editing: Option<(InspectorField, TextField)>,
}

/// Where the inspector's boxes are.
struct Layout {
    name: Option<Rect>,
    slots: Vec<(Slot, Rect)>,
}

/// The value of `slot` on a clip (ms or ticks, cents, tenths of a dB, 0 or 1).
fn slot_value(c: &InspClip, slot: Slot, unit: FadeUnit) -> i64 {
    match slot {
        Slot::FadeIn => c.fade_in_unit(c.content.fade_in, unit),
        Slot::FadeOut => c.fade_in_unit(c.content.fade_out, unit),
        Slot::Transpose => i64::from(c.content.warp.transpose_cents),
        Slot::Gain => (f64::from(c.content.gain_db) * 10.0).round() as i64,
        Slot::Reverse => i64::from(c.content.reverse),
        Slot::Warp => i64::from(c.content.warp.enabled),
        Slot::Tempo => (c.content.warp.source_bpm * 100.0).round() as i64,
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
        let unit = self.unit;
        let Some(first) = clips.first().map(|c| slot_value(c, slot, unit)) else {
            return Shown::Empty;
        };
        if clips.iter().all(|c| slot_value(c, slot, unit) == first) {
            Shown::Value(first)
        } else {
            Shown::Mixed
        }
    }

    /// The value a drag, the wheel or typing starts from: the shared value, or the first clip's.
    fn base(&self, slot: Slot) -> i64 {
        self.clips()
            .first()
            .map_or(0, |c| slot_value(c, slot, self.unit))
    }

    /// Why a slot cannot be edited, `None` when it can.
    fn disabled(&self, slot: Slot) -> Option<&'static str> {
        let clips = self.clips();
        if clips.is_empty() {
            return Some("select an audio clip to edit it");
        }
        if slot == Slot::Transpose && !clips.iter().any(InspClip::can_transpose) {
            return Some("the Repitch warp mode does not transpose: pick another mode");
        }
        if slot == Slot::Tempo && !clips.iter().any(|c| c.content.warp.enabled) {
            return Some("switch warp on to set the tempo the audio was recorded at");
        }
        None
    }

    fn range(&self, slot: Slot) -> (i64, i64) {
        match slot {
            Slot::FadeIn | Slot::FadeOut => (
                0,
                self.clips()
                    .iter()
                    .map(|c| c.fade_max(slot == Slot::FadeOut, self.unit))
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
            Slot::Reverse | Slot::Warp => (0, 1),
            Slot::Tempo => (
                (WARP_BPM_MIN * 100.0).ceil() as i64,
                (WARP_BPM_MAX * 100.0).floor() as i64,
            ),
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
                            .fade_frames(value, self.unit)
                            .min(c.frames.saturating_sub(c.content.fade_out));
                    }
                    Slot::FadeOut => {
                        a.fade_out = c
                            .fade_frames(value, self.unit)
                            .min(c.frames.saturating_sub(c.content.fade_in));
                    }
                    Slot::Transpose => {
                        if !c.can_transpose() {
                            return None;
                        }
                        a.warp.transpose_cents = value as i32;
                    }
                    Slot::Gain => a.gain_db = value as f32 / 10.0,
                    Slot::Reverse => a.reverse = value != 0,
                    Slot::Warp => a.warp.enabled = value != 0,
                    Slot::Tempo => {
                        if !c.content.warp.enabled {
                            return None;
                        }
                        a.warp.source_bpm = value as f64 / 100.0;
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
fn step_value(field: InspectorField, v: i64, notches: i32, fine: bool, unit: FadeUnit) -> i64 {
    let step = match field {
        InspectorField::FadeIn | InspectorField::FadeOut => {
            if fine {
                1
            } else if unit == FadeUnit::Ticks {
                FADE_STEP_TICKS
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
        InspectorField::Reverse | InspectorField::Warp => 0,
        InspectorField::Tempo => {
            if fine {
                1
            } else {
                100
            }
        }
    };
    v + i64::from(notches) * step
}

/// The text of a value, as the field shows it.
fn value_text(slot: Slot, v: i64, unit: FadeUnit) -> String {
    match slot {
        Slot::FadeIn | Slot::FadeOut if unit == FadeUnit::Ticks => format!("{v} t"),
        Slot::FadeIn | Slot::FadeOut => format!("{v} ms"),
        Slot::Transpose => format!("{:+.2}", v as f64 / 100.0),
        Slot::Gain => format!("{:+.1} dB", v as f64 / 10.0),
        Slot::Tempo => format!("{:.2}", v as f64 / 100.0),
        Slot::Reverse | Slot::Warp => String::new(),
    }
}

/// The text typing starts from.
fn edit_text(slot: Slot, v: i64) -> String {
    match slot {
        Slot::FadeIn | Slot::FadeOut => v.to_string(),
        Slot::Transpose => format!("{:.2}", v as f64 / 100.0),
        Slot::Gain => format!("{:.1}", v as f64 / 10.0),
        Slot::Tempo => format!("{:.2}", v as f64 / 100.0),
        Slot::Reverse | Slot::Warp => String::new(),
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
        Slot::Tempo => (v * 100.0).round() as i64,
        Slot::Reverse | Slot::Warp => return None,
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
        self.insp.editing.as_ref().map(|(_, f)| f.text())
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
                Slot::Warp => vec![InspectorField::Warp],
                Slot::Tempo => vec![InspectorField::Tempo],
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

    /// The width the whole clip inspector wants after the readouts and its lead (0 when no clip
    /// is selected): the transport bar's cluster leaves it free.
    pub(super) fn inspector_demand(&self) -> f64 {
        if self.insp.source == InspectorSource::None {
            return 0.0;
        }
        let all: f64 = SLOTS[..CORE_SLOTS].iter().map(|s| s.width() + GAP).sum();
        LEAD + 118.0 + GAP + all - GAP
    }

    fn inspector_layout(&self) -> Layout {
        let mut out = Layout {
            name: None,
            slots: Vec::new(),
        };
        if self.insp.source == InspectorSource::None {
            return out;
        }
        let x0 = self.cluster_end() + LEAD;
        let limit = self.bounds.x1 - TAIL - self.rec_reserved();
        let y0 = self.bounds.center().y - BOX_H / 2.0;
        let all: f64 = SLOTS[..CORE_SLOTS].iter().map(|s| s.width() + GAP).sum();
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

    /// The unit shown for the fades.
    pub fn fade_unit(&self) -> FadeUnit {
        self.insp.unit
    }

    /// Shows the fades in `unit`.
    pub fn set_fade_unit(&mut self, unit: FadeUnit) {
        if self.insp.unit != unit {
            self.insp.unit = unit;
            self.insp.editing = None;
            self.insp.drag = None;
            self.repaint = true;
        }
    }

    /// The centre of the unit tag of the fade-in box (`None` when the box is not shown).
    pub fn fade_unit_tag_point(&self) -> Option<Point> {
        let (_, r) = self
            .inspector_layout()
            .slots
            .into_iter()
            .find(|(s, _)| *s == Slot::FadeIn)?;
        Some(pt(r.x1 - UNIT_TAG_W / 2.0, r.y0 + LABEL_ROW_H / 2.0))
    }

    /// True when `pos` is on the unit tag of a fade box.
    fn unit_tag_at(&self, pos: Point) -> bool {
        self.inspector_layout().slots.into_iter().any(|(s, r)| {
            matches!(s, Slot::FadeIn | Slot::FadeOut)
                && pos.x >= r.x1 - UNIT_TAG_W
                && pos.x < r.x1
                && pos.y >= r.y0
                && pos.y < r.y0 + LABEL_ROW_H
        })
    }

    /// A press on the unit tag switches the fades' unit; true when it was one.
    pub(super) fn inspector_unit_press(&mut self, pos: Point) -> bool {
        if self.insp.clips().is_empty() || !self.unit_tag_at(pos) {
            return false;
        }
        let unit = self.insp.unit.other();
        self.set_fade_unit(unit);
        true
    }

    fn inspector_hit(&self, pos: Point) -> Option<InspectorField> {
        for (s, r) in self.inspector_layout().slots {
            if r.contains(pos) {
                return Some(match s {
                    Slot::FadeIn => InspectorField::FadeIn,
                    Slot::FadeOut => InspectorField::FadeOut,
                    Slot::Gain => InspectorField::Gain,
                    Slot::Reverse => InspectorField::Reverse,
                    Slot::Warp => InspectorField::Warp,
                    Slot::Tempo => InspectorField::Tempo,
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
        if matches!(field, InspectorField::Reverse | InspectorField::Warp) {
            let to = i64::from(self.insp.shown(slot) != Shown::Value(1));
            return self.insp.one_step(slot, to).into_iter().collect();
        }
        if clicks >= 2 {
            let text = match self.insp.shown(slot) {
                Shown::Value(v) => edit_text(slot, v),
                _ => String::new(),
            };
            self.insp.drag = None;
            let signed = matches!(
                field,
                InspectorField::TransposeWhole
                    | InspectorField::TransposeCents
                    | InspectorField::Gain
            );
            let accept: fn(char) -> bool = if signed {
                |c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')
            } else {
                |c| c.is_ascii_digit() || c == '.'
            };
            self.insp.editing = Some((field, TextField::entry(&text, TEXT_MAX, accept)));
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
            d.value = step_value(d.field, d.value, n, fine, self.insp.unit).clamp(lo, hi);
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
        if notches == 0
            || matches!(field, InspectorField::Reverse | InspectorField::Warp)
            || self.insp.disabled(slot).is_some()
        {
            return;
        }
        let (lo, hi) = self.insp.range(slot);
        let v =
            step_value(field, self.insp.base(slot), notches, fine, self.insp.unit).clamp(lo, hi);
        out.extend(self.insp.one_step(slot, v));
    }

    /// The inspector field under `pos`, for the events that need it.
    pub(super) fn inspector_field_at(&self, pos: Point) -> Option<InspectorField> {
        self.inspector_hit(pos)
    }

    /// Commits the typed value.
    pub(super) fn inspector_commit(&mut self, out: &mut Vec<UiAction>) {
        if let Some((field, text)) = self.insp.editing.take() {
            let slot = field.slot();
            if let Some(v) = parse_text(slot, text.text()) {
                out.extend(self.insp.one_step(slot, v));
            }
        }
        self.repaint = true;
    }

    /// A key while a value is being typed.
    pub(super) fn inspector_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some((_, field)) = &mut self.insp.editing else {
            return;
        };
        match key {
            Key::Enter => return self.inspector_commit(out),
            Key::Escape => self.insp.editing = None,
            _ => {
                field.handle_key(key, modifiers);
            }
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
                Slot::Warp => "Warp",
                Slot::Tempo => "Tempo",
            };
            return Some(Help::new(r, format!("{what}: {why}")));
        }
        let text = match slot {
            Slot::FadeIn | Slot::FadeOut => {
                let unit = if self.insp.unit == FadeUnit::Ticks {
                    "ticks"
                } else {
                    "ms"
                };
                let what = if slot == Slot::FadeIn { "in" } else { "out" };
                return Some(Help::new(
                    r,
                    format!(
                        "Fade {what} time in {unit}: drag or scroll to change (Shift: fine), double-click to type; click the unit tag to switch between ms and ticks"
                    ),
                ));
            }
            Slot::Transpose => {
                "Transpose in semitones: drag or scroll the whole part, or the cents after the point; double-click to type. A clip in the Repitch mode is left alone"
            }
            Slot::Gain => {
                "Clip gain in dB: drag or scroll to change (Shift: fine), double-click to type"
            }
            Slot::Reverse => {
                "Play the clip backwards (a warped clip keeps its warp: the stretched audio plays backwards)"
            }
            Slot::Warp => {
                "Warp: follow the song tempo, keeping the pitch (the mode, tempo and markers stay when it is switched off)"
            }
            Slot::Tempo => {
                "The tempo the audio was recorded at, in BPM: drag or scroll to change (Shift: fine), double-click to type. The clip plays faster or slower to follow the song"
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
            if matches!(slot, Slot::Reverse | Slot::Warp) {
                let lit = match shown {
                    Shown::Value(1) => Some(t.focus),
                    Shown::Mixed => Some(mix(t.focus, t.button, 0.55)),
                    _ => None,
                };
                let r = Rect::from_center_size(r.center(), (r.width(), BUTTON_H));
                sp::button(c, st, r, slot.label(), lit.filter(|_| !off), false);
                continue;
            }
            let editing = self.insp.editing.as_ref().filter(|(f, _)| f.slot() == slot);
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
            if matches!(slot, Slot::FadeIn | Slot::FadeOut) {
                sp::label(
                    c,
                    self.insp.unit.tag(),
                    r.x1 - LCD_PAD,
                    r.y0 + 10.0,
                    6.5,
                    t.focus,
                    Align::End,
                );
            }
            if let Some((_, field)) = editing {
                field.paint_lcd(c, r, VALUE_SIZE, t);
                continue;
            }
            let text = match shown {
                Shown::Value(v) => value_text(slot, v, self.insp.unit),
                Shown::Mixed => MIXED.to_owned(),
                Shown::Empty => String::new(),
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
