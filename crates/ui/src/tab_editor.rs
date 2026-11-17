//! The Tab Editor: one note clip's content as tablature (one line per string), with the rhythm
//! under the staff, chord names and diagrams above it and lyrics below.
//!
//! The view follows the piano roll's contract: `new(theme)`, `set_bounds`, `sync`, `handle`,
//! `command`, `paint`, `needs_repaint`, `set_scale`, `set_playhead`, `set_focused`; open a clip
//! with [`TabEditorView::open`]. The x axis is in content ticks (bar 1 = content tick 0).
//!
//! # The clip and its sidecar
//!
//! The notes stay the clip's sound; where each is played lives in the clip's tab sidecar
//! (`MidiData::tab`). A clip without a sidecar is shown fretted by the automatic fretter on the
//! instrument its track's name suggests; its sidecar is made by the first edit, which commits
//! the automatic positions too. Every edit is one [`atomik_model::tab_edit`] operation turned
//! into one `Command::Batch` (notes and sidecar together: one undo step), so the pitch of a note
//! is always the string's open pitch plus the fret plus the capo.
//!
//! # Typing
//!
//! The cursor sits on a string at a tick. Digit keys set the fret there (two digits typed
//! within [`entry::FRET_TIMEOUT`] are one fret and one undo step); the note gets the entry
//! length ([`NoteValue`], also the step of Left and Right). The commands of the `tab` keymap
//! context move the cursor and set the value; Delete removes the note under the cursor. A
//! typed note is auditioned. Every digit carries the time it arrived at (the view's clock is
//! aligned by [`TabEditorView::tick`] and [`TabEditorView::stamp`]), so a digit typed after an
//! idle stretch never joins an old one.
//!
//! # Selecting and editing
//!
//! A **range** is a rectangle of cells (ticks times strings): Shift+arrows grow it from where
//! the cursor was, a drag on the staff draws it, Select All takes the clip. The notes inside are
//! [`TabEditorView::selected`]. Techniques, Alt+Up/Down (change string, same pitch), transpose,
//! re-fret, copy, cut and delete work on the selection, or on the note under the cursor when
//! nothing is selected. Copy keeps strings, frets and techniques in the view's own tab clipboard
//! (a note-only clipboard cannot carry them); paste puts them down at the cursor. The technique
//! toolbar sits in the top bar's free space, the right-click menu is the app's
//! [`ContextTarget::TabStaff`], and two small modal panels (instrument, tuning and capo;
//! re-fret at position) are drawn over the pane (see `dialog`). Dragging on the ruler sets the
//! song loop to the bars dragged over; a click still locates the playhead.

mod dialog;
mod entry;
mod paint;
mod tech;

pub use entry::{DIVISIONS, FRET_TIMEOUT, FretBuffer, NoteValue, Typed};

use crate::action::{ContextTarget, TransportAction, UiAction};
use crate::cursor::CursorHint;
use crate::event::{Key, NumpadKey, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::seq_edit;
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::timeline::{Bars, Meter, PPQ, TimeAxis};
use crate::tooltip::Help;
use atomik_device_sdk::Registry;
use atomik_model::tab_edit::{self, RefretTarget, TabEdit, TabEditError, TabSnippet};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, MidiData, Note, RackDoc, SongCommand,
};
use atomik_tab::fretting::FretOptions;
use atomik_tab::score::Techniques;
use dialog::{Modal, Outcome, PickHit, PositionPick, TuningDialog};
use tech::Tech;
use vello::kurbo::{Point, Rect};

/// Height of the top bar (title, instrument, note values).
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the ruler (bar numbers, tempo, section markers).
pub const RULER_HEIGHT: f64 = 22.0;
/// Width of the gutter left of the staff (string names, the clef, the time signature).
pub const GUTTER_WIDTH: f64 = 52.0;
/// The widest spacing of two staff lines.
pub const LINE_HEIGHT: f64 = 16.0;
/// The width of a chord diagram above the staff.
pub const DIAGRAM_WIDTH: f64 = 44.0;
const MIN_LINE_HEIGHT: f64 = 9.0;
const STAFF_PAD_TOP: f64 = 26.0;
const RHYTHM_HEIGHT: f64 = 36.0;
const LYRICS_HEIGHT: f64 = 22.0;
const ZOOM_STEP: f64 = 1.15;
const ZOOM_KEY: f64 = 1.25;
const SCROLL_STEP: f64 = 36.0;
const MERGE_BASE: u64 = 15 << 40;
const DEFAULT_VELOCITY: f32 = 0.8;
const STEP_AUDITION_SECS: f64 = 0.25;
/// A click this close (logical pixels) to a note picks it.
const PICK_PX: f64 = 9.0;

/// A note of the clip with the place it is played.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placed {
    pub tick: i64,
    pub length: i64,
    pub key: u8,
    pub string: u8,
    pub fret: u8,
    pub tech: Techniques,
}

/// The clip as the editor works on it: notes and sidecar with every note placed.
#[derive(Debug, Clone)]
pub(crate) struct Work {
    /// The notes the edits start from (sorted by `(tick, key)`).
    pub notes: Vec<Note>,
    /// The clip's data with the sidecar present (made by the fretter when the clip had none).
    pub midi: MidiData,
    /// The notes with their strings, sorted by `(tick, key)`.
    pub placed: Vec<Placed>,
    /// Notes no string of the instrument can play.
    pub unplaced: Vec<(i64, u8)>,
    /// Whether the clip had no sidecar (or lacked entries) and the positions are automatic.
    pub automatic: bool,
}

impl Work {
    fn build(clip: &Clip, instrument: &str) -> Option<Work> {
        let ClipContent::Notes(list) = &clip.content else {
            return None;
        };
        let notes: &[Note] = list;
        let midi: MidiData = (*clip.midi).clone();
        let complete = midi
            .tab
            .as_ref()
            .is_some_and(|t| notes.iter().all(|n| t.note_of(n.tick, n.key).is_some()));
        let (notes, midi, unplaced) = if complete {
            let mut sorted = notes.to_vec();
            sorted.sort_by_key(|n| (n.tick, n.key));
            (sorted, midi, Vec::new())
        } else {
            let edit = if midi.tab.is_some() {
                tab_edit::auto_fret_clip(notes, &midi, &FretOptions::default())
            } else {
                tab_edit::start_tab(notes, &midi, instrument, true)
            }
            .ok()?;
            (edit.notes, edit.midi, edit.unplayable)
        };
        let tab = midi.tab.as_ref()?;
        let mut placed = Vec::with_capacity(notes.len());
        let mut lost = Vec::new();
        for n in &notes {
            match tab.note_of(n.tick, n.key) {
                Some(r) => placed.push(Placed {
                    tick: n.tick,
                    length: n.length,
                    key: n.key,
                    string: r.string,
                    fret: r.fret,
                    tech: r.tech.clone(),
                }),
                None => lost.push((n.tick, n.key)),
            }
        }
        let mut unplaced = unplaced;
        unplaced.extend(lost);
        unplaced.sort_unstable();
        unplaced.dedup();
        Some(Work {
            notes,
            midi,
            placed,
            unplaced,
            automatic: !complete,
        })
    }

    pub(crate) fn tab(&self) -> &atomik_model::TabData {
        // `build` returned `None` without a sidecar.
        self.midi.tab.as_ref().unwrap_or_else(|| unreachable_tab())
    }

    fn strings(&self) -> usize {
        self.tab().tuning.len().max(1)
    }
}

fn unreachable_tab() -> &'static atomik_model::TabData {
    static EMPTY: std::sync::OnceLock<atomik_model::TabData> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| {
        atomik_model::TabData::for_instrument("guitar6").unwrap_or_else(|| {
            atomik_model::TabData::with_tuning(
                "",
                atomik_tab::Tuning::from_opens(&[64], 24),
                atomik_tab::Capo::NONE,
            )
        })
    })
}

/// The note a typed fret is sounding until its release.
#[derive(Debug, Clone)]
struct StepNote {
    device: DeviceKey,
    key: u8,
    off_at: Option<f64>,
}

/// The tab editor view.
pub struct TabEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    axis: TimeAxis,
    clip_id: Option<ClipId>,
    clip: Option<Clip>,
    clip_rev: Option<(u64, Option<ClipId>)>,
    doc_rev: Option<u64>,
    device: Option<DeviceKey>,
    title: String,
    instrument: &'static str,
    work: Option<Work>,
    meter: Meter,
    bars: Bars,
    /// Tempo changes inside the clip, in content ticks (the first is the tempo at its start).
    tempo: Vec<(i64, f64)>,
    /// Markers inside the clip: content tick and name.
    markers: Vec<(i64, String)>,
    /// The cursor: content tick and string (tab order).
    cursor: (i64, u8),
    value: NoteValue,
    digits: FretBuffer,
    next_merge: u64,
    merge_open: Option<u64>,
    step_note: Option<StepNote>,
    status: Option<String>,
    follow: bool,
    playing: bool,
    playhead: i64,
    focused: bool,
    hover: Option<Point>,
    repaint: bool,
    /// The song's bar lines (the loop snaps to these).
    song_bars: Bars,
    /// The selected cells, when a range is selected.
    range: Option<Range>,
    /// The fixed corner of the range while it grows.
    anchor: (i64, u8),
    /// Whether the keyboard range is a rectangle (Shift+Up/Down was used) or whole columns.
    rect_mode: bool,
    /// The notes inside the range (tick and key).
    sel: Vec<(i64, u8)>,
    drag: Drag,
    /// The tab clipboard: strings, frets and techniques of the last copy.
    clipboard: Option<TabSnippet>,
    /// The amount of the bend button, in cents.
    bend_cents: i32,
    modal: Option<Modal>,
    /// The time of the last tick or stamp, in seconds.
    clock: f64,
}

/// A rectangle of cells: ticks `t0..=t1` and strings `s0..=s1`. `all` also takes the notes no
/// string plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Range {
    pub t0: i64,
    pub t1: i64,
    pub s0: u8,
    pub s1: u8,
    pub all: bool,
}

impl Range {
    /// The rectangle between two cells.
    fn between(a: (i64, u8), b: (i64, u8)) -> Self {
        Self {
            t0: a.0.min(b.0),
            t1: a.0.max(b.0),
            s0: a.1.min(b.1),
            s1: a.1.max(b.1),
            all: false,
        }
    }

    fn holds(&self, p: &Placed) -> bool {
        self.all
            || ((self.t0..=self.t1).contains(&p.tick) && (self.s0..=self.s1).contains(&p.string))
    }
}

/// A pointer gesture in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// Drawing a range on the staff from `anchor`.
    Select {
        anchor: (i64, u8),
    },
    /// Dragging over the ruler: content ticks of the press and of the pointer, and the x of the
    /// press (a click that never leaves it only locates).
    Loop {
        from: i64,
        to: i64,
        press_x: f64,
        moved: bool,
    },
}

impl TabEditorView {
    /// A closed editor.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, 400.0),
            scale: 1.0,
            axis: TimeAxis::new(GUTTER_WIDTH, 260.0),
            clip_id: None,
            clip: None,
            clip_rev: None,
            doc_rev: None,
            device: None,
            title: String::new(),
            instrument: "guitar6",
            work: None,
            meter: Meter::default(),
            bars: Bars::default(),
            tempo: Vec::new(),
            markers: Vec::new(),
            cursor: (0, 0),
            value: NoteValue::QUARTER,
            digits: FretBuffer::default(),
            next_merge: MERGE_BASE,
            merge_open: None,
            step_note: None,
            status: None,
            follow: true,
            playing: false,
            playhead: 0,
            focused: false,
            hover: None,
            repaint: true,
            song_bars: Bars::default(),
            range: None,
            anchor: (0, 0),
            rect_mode: false,
            sel: Vec::new(),
            drag: Drag::None,
            clipboard: None,
            bend_cents: tech::DEFAULT_BEND,
            modal: None,
            clock: 0.0,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Switches the colours; the view repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Opens a note clip (ignored by [`Self::sync`] when it is not one).
    pub fn open(&mut self, id: ClipId) {
        if self.clip_id != Some(id) {
            self.cursor = (0, 0);
            self.digits.clear();
            self.axis.scroll = 0.0;
            self.clear_range();
            self.modal = None;
            self.drag = Drag::None;
        }
        self.clip_id = Some(id);
        self.clip = None;
        self.clip_rev = None;
        self.work = None;
        self.repaint = true;
    }

    /// Closes the editor (it then paints an empty state).
    pub fn close(&mut self) {
        self.clip_id = None;
        self.clip = None;
        self.clip_rev = None;
        self.work = None;
        self.device = None;
        self.digits.clear();
        self.merge_open = None;
        self.clear_range();
        self.modal = None;
        self.drag = Drag::None;
        self.repaint = true;
    }

    /// The open clip.
    pub fn clip(&self) -> Option<ClipId> {
        self.clip_id
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, outer: Rect) {
        if outer != self.bounds {
            self.bounds = outer;
            self.axis.x0 = outer.x0 + GUTTER_WIDTH;
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the view as focused (thin top line).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// Whether the editor follows the playhead while playing.
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Sets follow (the Workflow preference).
    pub fn set_follow(&mut self, on: bool) {
        self.follow = on;
    }

    /// Tells the view whether the transport plays.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Sets the transport position (song ticks). While playing and following, the view pages to
    /// keep the playhead inside.
    pub fn set_playhead(&mut self, tick: i64) {
        if tick == self.playhead {
            return;
        }
        let before = self.playhead_content();
        self.playhead = tick;
        let after = self.playhead_content();
        let px = |t: Option<i64>| t.map(|t| self.axis.tick_to_x(t).round() as i64);
        if px(before) != px(after) {
            self.repaint = true;
        }
        if self.follow
            && self.playing
            && let Some(c) = after
            && self.axis.page_to(c, self.bounds.x1)
        {
            self.repaint = true;
        }
    }

    /// True while a pointer gesture is in progress (a range or a loop being dragged).
    pub fn is_dragging(&self) -> bool {
        self.drag != Drag::None
    }

    /// True while a modal panel is open: it takes every key (Enter applies, Esc closes), so the
    /// shell resolves no command for them.
    pub fn is_text_owner(&self) -> bool {
        self.modal.is_some()
    }

    /// Whether a panel (instrument, tuning and capo; re-fret at position) is open.
    pub fn modal_open(&self) -> bool {
        self.modal.is_some()
    }

    /// Whether the edits (copy, cut, delete, techniques, transpose ...) have notes to work on:
    /// a selection, or a note under the cursor.
    pub fn has_targets(&self) -> bool {
        !self.targets().is_empty()
    }

    /// Whether the tab clipboard holds notes to paste.
    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// The song loop range the pointer is dragging over the ruler, in song ticks.
    pub fn loop_preview(&self) -> Option<(i64, i64)> {
        let Drag::Loop {
            from,
            to,
            moved: true,
            ..
        } = self.drag
        else {
            return None;
        };
        self.loop_range(from, to)
    }

    /// The bars between two content ticks as a song range (both bars included).
    fn loop_range(&self, a: i64, b: i64) -> Option<(i64, i64)> {
        let c = self.clip.as_ref()?;
        let to_song = |t: i64| (c.start + t - c.offset).max(0);
        let (lo, hi) = (to_song(a.min(b)), to_song(a.max(b)));
        let map = self.song_bars.map();
        let start = map.bar_start(map.bar_at(lo));
        let end = map.bar_start(map.bar_at(hi) + 1);
        (end > start).then_some((start, end))
    }

    /// The view's clock in seconds: the time of the last [`Self::tick`] or [`Self::stamp`].
    fn now(&self) -> f64 {
        self.clock
    }

    /// Sets the view's clock to `now` (seconds on a monotonic clock, the one [`Self::tick`] gets).
    /// A host stamps each event with the time it arrived at before handing it over, so a digit
    /// typed after an idle stretch (no frame ran) is not mistaken for a quick second digit.
    pub fn stamp(&mut self, now: f64) {
        self.clock = now;
    }

    /// True when something changed since the last paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The mouse cursor for the pointer's position.
    pub fn cursor(&self) -> CursorHint {
        CursorHint::Default
    }

    /// A tooltip for the top bar's buttons.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.modal.is_some() {
            return None;
        }
        let (rect, b) = self
            .top_buttons()
            .into_iter()
            .find(|(r, _)| r.contains(pos))?;
        let text: &'static str = match b {
            TopButton::Value(_) => "Note value of the next typed note",
            TopButton::Dotted => "Dotted note value",
            TopButton::Triplet => "Triplet note value",
            TopButton::Tool(i) => {
                let item = tech::TOOLBAR[i];
                return Some(Help::new(rect, item.tip).with_arg("editor.tab_technique", item.arg));
            }
            TopButton::BendAmount => "Bend amount in tones (click to change)",
            TopButton::Instrument => {
                return Some(
                    Help::new(rect, "Instrument, tuning and capo")
                        .with_command("editor.tab_tuning"),
                );
            }
        };
        Some(Help::new(rect, text))
    }

    /// The cursor: content tick and string (tab order, 0 = top line).
    pub fn cursor_cell(&self) -> (i64, u8) {
        self.cursor
    }

    /// Moves the cursor (clamped to the clip and the strings).
    pub fn set_cursor(&mut self, tick: i64, string: u8) {
        let end = self.end_tick();
        let last = u8::try_from(self.strings().saturating_sub(1)).unwrap_or(u8::MAX);
        self.cursor = (tick.clamp(0, (end - 1).max(0)), string.min(last));
        self.clear_range();
        self.repaint = true;
    }

    /// The note value of the entry.
    pub fn note_value(&self) -> NoteValue {
        self.value
    }

    /// Sets the note value of the entry.
    pub fn set_note_value(&mut self, value: NoteValue) {
        self.value = value;
        self.repaint = true;
    }

    /// The selected notes (tick and key): the notes inside the selected range, in time order.
    pub fn selected(&self) -> &[(i64, u8)] {
        &self.sel
    }

    /// The selected range as `(first tick, last tick, first string, last string)`, when there
    /// is one (it may hold no note).
    pub fn selected_range(&self) -> Option<(i64, i64, u8, u8)> {
        self.range.map(|r| (r.t0, r.t1, r.s0, r.s1))
    }

    /// Selects the cells between two cells (tick and string), as a drag does.
    pub fn select_between(&mut self, a: (i64, u8), b: (i64, u8)) {
        self.anchor = a;
        self.range = Some(Range::between(a, b));
        self.cursor = b;
        self.refresh_sel();
        self.repaint = true;
    }

    /// Selects every note of the clip.
    pub fn select_all(&mut self) {
        let last = u8::try_from(self.strings().saturating_sub(1)).unwrap_or(0);
        self.range = Some(Range {
            t0: 0,
            t1: self.end_tick(),
            s0: 0,
            s1: last,
            all: true,
        });
        self.refresh_sel();
        self.repaint = true;
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self) {
        self.clear_range();
        self.repaint = true;
    }

    fn clear_range(&mut self) {
        self.range = None;
        self.sel.clear();
    }

    /// Recomputes the notes inside the range from the clip.
    fn refresh_sel(&mut self) {
        self.sel = match (&self.range, &self.work) {
            (Some(r), Some(w)) => {
                let mut v: Vec<(i64, u8)> = w
                    .placed
                    .iter()
                    .filter(|p| r.holds(p))
                    .map(|p| (p.tick, p.key))
                    .collect();
                if r.all {
                    v.extend(w.unplaced.iter().copied());
                    v.sort_unstable();
                }
                v
            }
            _ => Vec::new(),
        };
    }

    /// What an edit works on: the selection, else the note under the cursor.
    fn targets(&self) -> Vec<(i64, u8)> {
        if !self.sel.is_empty() {
            return self.sel.clone();
        }
        let (tick, string) = self.cursor;
        self.work
            .as_ref()
            .and_then(|w| w.tab().note_on_string(tick, string))
            .map(|r| vec![(r.tick, r.key)])
            .unwrap_or_default()
    }

    /// The text of the last refused edit (cleared by the next accepted one).
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// The strings of the instrument (1 when no clip is open).
    pub fn strings(&self) -> usize {
        self.work.as_ref().map_or(1, Work::strings)
    }

    /// The fret and the string at a note, when the clip has one there.
    pub fn note_at(&self, tick: i64, string: u8) -> Option<(u8, i64)> {
        self.work
            .as_ref()?
            .placed
            .iter()
            .find(|p| p.tick == tick && p.string == string)
            .map(|p| (p.fret, p.length))
    }

    /// Every note as `(tick, string, fret, length)`, in time order.
    pub fn notes(&self) -> Vec<(i64, u8, u8, i64)> {
        self.work.as_ref().map_or_else(Vec::new, |w| {
            w.placed
                .iter()
                .map(|p| (p.tick, p.string, p.fret, p.length))
                .collect()
        })
    }

    /// Whether the clip has no sidecar yet (the positions shown are the automatic ones).
    pub fn is_automatic(&self) -> bool {
        self.work.as_ref().is_some_and(|w| w.automatic)
    }

    /// The tick the clip content ends at (the cursor never goes past it).
    pub fn end_tick(&self) -> i64 {
        match &self.clip {
            Some(c) => match c.loop_length {
                Some(n) if n > 0 => n,
                _ => c.offset + c.length,
            },
            None => 4 * PPQ,
        }
    }

    /// Logical x of a content tick.
    pub fn tick_to_x(&self, tick: i64) -> f64 {
        self.axis.tick_to_x(tick)
    }

    /// Logical y of a string's line.
    pub fn string_y(&self, string: u8) -> f64 {
        self.staff_top() + f64::from(string) * self.line_h()
    }

    /// Zoom: pixels per tick.
    pub fn px_per_tick(&self) -> f64 {
        self.axis.px_per_tick
    }

    /// Releases the note a typed fret is sounding, at once (before the editor closes or its clip
    /// changes, so no note hangs).
    pub fn release_audition(&mut self) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.release_audition_into(&mut out);
        out
    }

    fn release_audition_into(&mut self, out: &mut Vec<UiAction>) {
        if let Some(s) = self.step_note.take() {
            out.push(UiAction::Audition {
                device: s.device,
                key: s.key,
                on: false,
            });
        }
    }

    /// Per-frame clock (`now` in seconds, any monotonic clock): releases the audition note and
    /// ends the fret being typed once its digits ran out.
    pub fn tick(&mut self, now: f64) -> Vec<UiAction> {
        self.stamp(now);
        let mut out = Vec::new();
        if self.digits.tick(now) {
            self.end_typing(&mut out);
        }
        if let Some(s) = &mut self.step_note {
            let due = *s.off_at.get_or_insert(now + STEP_AUDITION_SECS);
            if now >= due {
                self.release_audition_into(&mut out);
            }
        }
        out
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open clip from the document.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        self.clip_rev = None;
        self.sync_from(doc, registry, None);
    }

    /// Tells the view the revision of the document the next `handle`, `command` and `paint`
    /// calls get (see [`crate::PianoRollView::set_doc_revision`]).
    pub fn set_doc_revision(&mut self, revision: Option<u64>) {
        self.doc_rev = revision;
    }

    /// [`Self::sync`] for a document known by its revision.
    pub fn sync_at(&mut self, doc: &RackDoc, registry: &Registry, revision: u64) {
        self.sync_from(doc, registry, Some(revision));
    }

    fn sync_current(&mut self, doc: &RackDoc, registry: &Registry) {
        match self.doc_rev {
            Some(r) => self.sync_at(doc, registry, r),
            None => self.sync(doc, registry),
        }
    }

    fn sync_from(&mut self, doc: &RackDoc, _registry: &Registry, revision: Option<u64>) {
        let Some(id) = self.clip_id else { return };
        if let Some(rev) = revision
            && self.clip_rev == Some((rev, Some(id)))
        {
            return;
        }
        self.clip_rev = revision.map(|r| (r, Some(id)));
        let found = doc.song.clip_or_take(id);
        let same = match found {
            Some((t, c)) => {
                self.clip.as_ref() == Some(c) && self.device.as_ref() == Some(&t.device)
            }
            None => self.clip.is_none(),
        };
        let meter = Meter::at(&doc.song, found.map_or(0, |(_, c)| c.start));
        let song_bars = Bars::of(&doc.song);
        if same && meter == self.meter && song_bars == self.song_bars {
            return;
        }
        match found {
            Some((t, c)) => {
                self.device = Some(t.device.clone());
                self.title = crate::seq_paint::track_name(doc, &t.device);
                self.instrument = atomik_tab::instrument::instrument_for_name(&self.title);
                self.clip = Some(c.clone());
                self.meter = meter;
                self.bars = Bars::single(meter);
                self.song_bars = song_bars;
                self.read_song_marks(doc, c);
                self.work = Work::build(c, self.instrument);
            }
            None => {
                self.clip = None;
                self.device = None;
                self.work = None;
            }
        }
        if self.work.is_none() {
            self.clip = None;
        }
        let last = u8::try_from(self.strings().saturating_sub(1)).unwrap_or(u8::MAX);
        self.cursor.1 = self.cursor.1.min(last);
        self.cursor.0 = self.cursor.0.clamp(0, (self.end_tick() - 1).max(0));
        self.refresh_sel();
        self.repaint = true;
    }

    /// Tempo changes and markers of the song that fall inside the clip, in content ticks.
    fn read_song_marks(&mut self, doc: &RackDoc, clip: &Clip) {
        let to_content = |t: i64| t - clip.start + clip.offset;
        let (from, to) = (clip.start, clip.start + clip.length);
        let mut tempo = Vec::new();
        if let Some(e) = doc.song.tempo.iter().rev().find(|e| e.tick <= from) {
            tempo.push((to_content(from), e.bpm));
        }
        for e in doc
            .song
            .tempo
            .iter()
            .filter(|e| e.tick > from && e.tick < to)
        {
            tempo.push((to_content(e.tick), e.bpm));
        }
        self.tempo = tempo;
        self.markers = doc
            .song
            .markers
            .iter()
            .filter(|m| m.tick >= from && m.tick < to)
            .map(|m| (to_content(m.tick), m.name.clone()))
            .collect();
    }

    fn playhead_content(&self) -> Option<i64> {
        self.clip
            .as_ref()
            .and_then(|c| seq_edit::song_to_content(c, self.playhead))
    }

    // ------------------------------------------------------------------ geometry

    fn diagrams_shown(&self) -> bool {
        self.work
            .as_ref()
            .is_some_and(|w| !w.tab().diagrams.is_empty())
    }

    fn chord_height(&self) -> f64 {
        if self.diagrams_shown() {
            crate::chord_diagram::diagram_height(DIAGRAM_WIDTH, true) + 6.0
        } else {
            0.0
        }
    }

    fn staff_top(&self) -> f64 {
        self.bounds.y0 + TOP_BAR_HEIGHT + RULER_HEIGHT + self.chord_height() + STAFF_PAD_TOP
    }

    fn line_h(&self) -> f64 {
        let n = self.strings() as f64;
        let fixed = TOP_BAR_HEIGHT
            + RULER_HEIGHT
            + self.chord_height()
            + STAFF_PAD_TOP
            + RHYTHM_HEIGHT
            + LYRICS_HEIGHT;
        ((self.bounds.height() - fixed) / n).clamp(MIN_LINE_HEIGHT, LINE_HEIGHT)
    }

    fn staff_bottom(&self) -> f64 {
        self.string_y(u8::try_from(self.strings().saturating_sub(1)).unwrap_or(0))
    }

    fn ruler_rect(&self) -> Rect {
        let y = self.bounds.y0 + TOP_BAR_HEIGHT;
        Rect::new(self.axis.x0, y, self.bounds.x1, y + RULER_HEIGHT)
    }

    fn staff_rect(&self) -> Rect {
        let lh = self.line_h();
        Rect::new(
            self.axis.x0,
            self.staff_top() - lh * 0.6,
            self.bounds.x1,
            self.staff_bottom() + lh * 0.6,
        )
    }

    /// The label of the top bar's left end: the track, the instrument and the capo.
    pub(super) fn instrument_label(&self) -> Option<String> {
        let w = self.work.as_ref()?;
        let tab = w.tab();
        let name = atomik_tab::instrument::find_instrument(&tab.instrument_id)
            .map_or("Custom", |i| i.name);
        Some(format!("{}  -  {}", self.title, name))
    }

    /// The clickable rectangle of the instrument label (it opens the tuning dialog), including
    /// the capo text that follows it.
    fn instrument_rect(&self) -> Option<Rect> {
        let label = self.instrument_label()?;
        let capo = self.work.as_ref()?.tab().capo.fret > 0;
        let w = label.chars().count() as f64 * 6.6 + 8.0 + if capo { 62.0 } else { 0.0 };
        Some(Rect::new(
            self.bounds.x0 + 6.0,
            self.bounds.y0 + 3.0,
            self.bounds.x0 + 10.0 + w,
            self.bounds.y0 + TOP_BAR_HEIGHT - 3.0,
        ))
    }

    /// The buttons of the top bar: the instrument label, the technique toolbar (as many buttons
    /// as fit before the note values), then the seven note values, the dot and the triplet.
    fn top_buttons(&self) -> Vec<(Rect, TopButton)> {
        let y0 = self.bounds.y0 + 4.0;
        let (w, h, gap) = (30.0, TOP_BAR_HEIGHT - 8.0, 3.0);
        let mut values: Vec<TopButton> = DIVISIONS.iter().map(|d| TopButton::Value(*d)).collect();
        values.push(TopButton::Dotted);
        values.push(TopButton::Triplet);
        let total = values.len() as f64 * (w + gap) - gap;
        let x0 = (self.bounds.x1 - total - 8.0).max(self.bounds.x0);
        let mut out: Vec<(Rect, TopButton)> = values
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let x = x0 + i as f64 * (w + gap);
                (Rect::new(x, y0, x + w, y0 + h), b)
            })
            .collect();
        let mut x = self.bounds.x0 + 8.0;
        if let Some(r) = self.instrument_rect() {
            x = r.x1 + 8.0;
            out.push((r, TopButton::Instrument));
        }
        if self.work.is_some() {
            let mut items: Vec<(TopButton, f64)> = Vec::new();
            for (i, item) in tech::TOOLBAR.iter().enumerate() {
                items.push((TopButton::Tool(i), 24.0));
                if item.arg == "bend" {
                    items.push((TopButton::BendAmount, 34.0));
                }
            }
            for (b, bw) in items {
                if x + bw > x0 - 8.0 {
                    break;
                }
                out.push((Rect::new(x, y0, x + bw, y0 + h), b));
                x += bw + 2.0;
            }
        }
        out
    }

    // ------------------------------------------------------------------ events

    /// Handles a window event; returns what the host must carry out.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync_current(doc, registry);
        let mut out = Vec::new();
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return out;
        }
        if self.work.is_none() {
            return out;
        }
        if self.modal.is_some() {
            self.modal_event(event, &mut out);
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, .. } => self.pointer_move(*pos),
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => self.pointer_down(*pos, modifiers.shift, &mut out),
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Secondary,
                ..
            } => self.context_click(*pos, &mut out),
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.pointer_up(&mut out),
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                if modifiers.command() {
                    self.axis
                        .zoom_at(ZOOM_STEP.powf(delta.y), pos.x.max(self.axis.x0));
                } else {
                    let d = if delta.x.abs() > 1e-9 {
                        delta.x
                    } else {
                        delta.y
                    };
                    self.axis.scroll_px(-d * SCROLL_STEP);
                }
                self.follow_pause();
                self.repaint = true;
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if !modifiers.command() && !modifiers.alt => match key {
                Key::Char(c) if c.is_ascii_digit() => {
                    self.type_digit(*c as u8 - b'0', &mut out);
                }
                Key::Numpad(NumpadKey::Digit(d)) => self.type_digit(*d, &mut out),
                Key::Escape => {
                    self.digits.clear();
                    self.end_typing(&mut out);
                    if self.range.is_some() {
                        self.clear_range();
                        self.repaint = true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        out
    }

    /// A manual scroll stops following until the next jump (like the piano roll).
    fn follow_pause(&mut self) {}

    /// The cell (tick and string) a point on the staff means; a point beside the staff is
    /// clamped to its nearest string and to the clip.
    fn cell_at(&self, pos: Point) -> (i64, u8) {
        let lh = self.line_h();
        let n = self.strings();
        let string = (((pos.y - self.staff_top()) / lh).round().max(0.0) as usize).min(n - 1);
        let string = u8::try_from(string).unwrap_or(0);
        let tick = self
            .pick_tick(pos.x, string)
            .clamp(0, (self.end_tick() - 1).max(0));
        (tick, string)
    }

    fn pointer_move(&mut self, pos: Point) {
        let over = |v: &Self, p: Option<Point>| {
            p.and_then(|p| v.top_buttons().into_iter().find(|(r, _)| r.contains(p)))
                .map(|(r, _)| r)
        };
        if over(self, self.hover) != over(self, Some(pos)) {
            self.repaint = true;
        }
        self.hover = Some(pos);
        match self.drag {
            Drag::Select { anchor } => {
                let cell = self.cell_at(pos);
                self.cursor = cell;
                self.anchor = anchor;
                self.range = (cell != anchor).then(|| Range::between(anchor, cell));
                self.refresh_sel();
                self.repaint = true;
            }
            Drag::Loop {
                from,
                press_x,
                moved,
                ..
            } => {
                self.drag = Drag::Loop {
                    from,
                    to: self.axis.x_to_tick(pos.x).max(0),
                    press_x,
                    moved: moved || (pos.x - press_x).abs() >= 3.0,
                };
                self.repaint = true;
            }
            Drag::None => {}
        }
    }

    fn pointer_down(&mut self, pos: Point, shift: bool, out: &mut Vec<UiAction>) {
        self.repaint = true;
        self.end_typing(out);
        self.release_audition_into(out);
        if let Some((_, b)) = self
            .top_buttons()
            .into_iter()
            .find(|(r, _)| r.contains(pos))
        {
            self.top_button(b, out);
            return;
        }
        if self.ruler_rect().contains(pos) {
            if let Some(c) = &self.clip {
                let tick = self.axis.x_to_tick(pos.x).max(0);
                out.push(UiAction::Transport(TransportAction::Locate(
                    (c.start + tick - c.offset).max(0),
                )));
                self.drag = Drag::Loop {
                    from: tick,
                    to: tick,
                    press_x: pos.x,
                    moved: false,
                };
            }
            return;
        }
        if !self.staff_rect().contains(pos) {
            return;
        }
        let cell = self.cell_at(pos);
        if shift {
            if self.range.is_none_or(|r| r.all) {
                self.anchor = self.cursor;
            }
            self.cursor = cell;
            self.range = Some(Range::between(self.anchor, cell));
            self.refresh_sel();
            return;
        }
        self.clear_range();
        self.cursor = cell;
        self.anchor = cell;
        self.drag = Drag::Select { anchor: cell };
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        if let Drag::Loop {
            from,
            to,
            moved: true,
            ..
        } = drag
            && let Some(range) = self.loop_range(from, to)
        {
            out.push(UiAction::Command(Command::Song(SongCommand::SetLoop {
                range,
                enabled: true,
            })));
        }
        self.repaint = true;
    }

    /// A right click on the staff: the note under it joins the selection (else becomes it) and
    /// the app opens the staff menu.
    fn context_click(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        self.end_typing(out);
        if !self.staff_rect().contains(pos) {
            return;
        }
        let cell = self.cell_at(pos);
        let inside = self.range.is_some_and(|r| {
            r.all || ((r.t0..=r.t1).contains(&cell.0) && (r.s0..=r.s1).contains(&cell.1))
        });
        if !inside {
            self.cursor = cell;
            self.anchor = cell;
            let on_note = self.work.as_ref().is_some_and(|w| {
                w.placed
                    .iter()
                    .any(|p| p.tick == cell.0 && p.string == cell.1)
            });
            if on_note {
                self.range = Some(Range::between(cell, cell));
                self.refresh_sel();
            } else {
                self.clear_range();
            }
        }
        self.repaint = true;
        out.push(UiAction::ContextMenu {
            target: ContextTarget::TabStaff,
            at: pos,
        });
    }

    fn top_button(&mut self, b: TopButton, out: &mut Vec<UiAction>) {
        match b {
            TopButton::Value(_) | TopButton::Dotted | TopButton::Triplet => {
                self.set_value_from(b, out);
            }
            TopButton::Tool(i) => {
                self.technique(tech::TOOLBAR[i].arg, out);
            }
            TopButton::BendAmount => {
                let at = tech::BEND_CENTS
                    .iter()
                    .position(|c| *c == self.bend_cents)
                    .unwrap_or(0);
                self.bend_cents = tech::BEND_CENTS[(at + 1) % tech::BEND_CENTS.len()];
            }
            TopButton::Instrument => self.open_tuning(),
        }
    }

    /// The tick a click at `x` on `string` means: a note close enough, else the value grid.
    fn pick_tick(&self, x: f64, string: u8) -> i64 {
        if let Some(w) = &self.work {
            let near = w
                .placed
                .iter()
                .filter(|p| p.string == string)
                .map(|p| (p.tick, (self.axis.tick_to_x(p.tick) - x).abs()))
                .filter(|(_, d)| *d <= PICK_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((tick, _)) = near {
                return tick;
            }
        }
        entry::snap_to_grid(self.axis.x_to_tick(x), self.value.ticks())
    }

    fn set_value_from(&mut self, b: TopButton, out: &mut Vec<UiAction>) {
        let v = match b {
            TopButton::Value(d) => NoteValue {
                division: d,
                ..self.value
            },
            TopButton::Dotted => self.value.toggle_dotted(),
            TopButton::Triplet => self.value.toggle_triplet(),
            _ => return,
        };
        self.apply_value(v, out);
    }

    /// Sets the entry value and resizes the notes under the cursor (the selection, or the note
    /// the cursor is on) to it.
    fn apply_value(&mut self, v: NoteValue, out: &mut Vec<UiAction>) {
        self.value = v;
        self.repaint = true;
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let prepared = self
            .work
            .as_ref()
            .map(|w| tab_edit::set_length(&w.notes, &w.midi, &targets, v.ticks()));
        if let (Some(r), Some(clip)) = (prepared, self.clip_id) {
            self.emit(r, clip, None, out);
        }
    }

    fn new_merge(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    /// Ends the merged undo step of the digits typed so far.
    fn end_typing(&mut self, out: &mut Vec<UiAction>) {
        if self.merge_open.take().is_some() {
            out.push(UiAction::EndMerge);
        }
        self.digits.clear();
    }

    /// Turns an edit into the command (or the refusal into the status line).
    fn emit(
        &mut self,
        r: Result<TabEdit, TabEditError>,
        clip: ClipId,
        merge: Option<u64>,
        out: &mut Vec<UiAction>,
    ) -> Option<TabEdit> {
        match r {
            Ok(edit) => {
                self.status = None;
                out.push(UiAction::Command(edit.clone().into_command(clip, merge)));
                self.repaint = true;
                Some(edit)
            }
            Err(e) => {
                self.status = Some(e.to_string());
                self.repaint = true;
                None
            }
        }
    }

    fn type_digit(&mut self, digit: u8, out: &mut Vec<UiAction>) {
        let (Some(w), Some(clip), Some(device)) = (&self.work, self.clip_id, self.device.clone())
        else {
            return;
        };
        let (tick, string) = self.cursor;
        let tab = w.tab();
        let s = usize::from(string);
        let max = tab.tuning.max_fret(s, tab.capo);
        let min = tab.tuning.min_fret(s, tab.capo);
        let typed = self.digits.push(digit, self.cursor, max, self.now());
        let fret = typed.fret;
        let valid = fret == 0 || (min..=max).contains(&fret);
        let prepared = if valid {
            tab.tuning.pitch(s, fret, tab.capo).map(|key| {
                let r = tab_edit::set_fret(
                    &w.notes,
                    &w.midi,
                    (tick, string, fret),
                    (self.value.ticks(), DEFAULT_VELOCITY),
                );
                (key, r)
            })
        } else {
            None
        };
        let merge = if typed.joined && self.merge_open.is_some() {
            self.merge_open
        } else {
            if self.merge_open.take().is_some() {
                out.push(UiAction::EndMerge);
            }
            let m = self.new_merge();
            self.merge_open = Some(m);
            Some(m)
        };
        let Some((key, r)) = prepared else {
            self.status = Some(format!("fret {fret} is not on this string"));
            self.repaint = true;
            return;
        };
        if self.emit(r, clip, merge, out).is_some() {
            self.release_audition_into(out);
            out.push(UiAction::Audition {
                device: device.clone(),
                key,
                on: true,
            });
            self.step_note = Some(StepNote {
                device,
                key,
                off_at: None,
            });
        }
    }

    // ------------------------------------------------------------------ commands

    /// Runs a registry command of the editor. `Declined`: not the editor's.
    pub fn command(
        &mut self,
        id: &str,
        arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync_current(doc, registry);
        if self.work.is_none() {
            return CommandOutcome::Declined;
        }
        let mut out = Vec::new();
        let taken = self.run_command(id, arg, &mut out);
        if taken {
            self.repaint = true;
            CommandOutcome::Handled(out)
        } else {
            CommandOutcome::Declined
        }
    }

    fn run_command(&mut self, id: &str, arg: Option<&str>, out: &mut Vec<UiAction>) -> bool {
        if self.modal.is_some() {
            return false;
        }
        if !matches!(
            id,
            "editor.tab_zoom_in" | "editor.tab_zoom_out" | "sequencer.follow"
        ) {
            self.end_typing(out);
        }
        let end = self.end_tick();
        let (tick, string) = self.cursor;
        match id {
            "editor.tab_cursor_left" | "editor.tab_cursor_right" => {
                let dir = if id.ends_with("left") { -1 } else { 1 };
                let t = entry::step_on_grid(tick, self.value.ticks(), dir, end);
                self.cursor.0 = t;
                self.clear_range();
                self.reveal();
            }
            "editor.tab_cursor_up" => {
                self.cursor.1 = string.saturating_sub(1);
                self.clear_range();
            }
            "editor.tab_cursor_down" => {
                let last = u8::try_from(self.strings().saturating_sub(1)).unwrap_or(0);
                self.cursor.1 = (string + 1).min(last);
                self.clear_range();
            }
            "editor.tab_bar_next" | "editor.tab_bar_prev" => {
                let dir = if id.ends_with("next") { 1 } else { -1 };
                self.cursor.0 = entry::step_bar(tick, dir, &self.bars, end);
                self.clear_range();
                self.reveal();
            }
            "editor.tab_duration" => {
                let Some(v) = arg.and_then(NoteValue::from_name) else {
                    return false;
                };
                self.apply_value(
                    NoteValue {
                        dotted: self.value.dotted,
                        triplet: self.value.triplet,
                        ..v
                    },
                    out,
                );
            }
            "editor.tab_dotted" => self.apply_value(self.value.toggle_dotted(), out),
            "editor.tab_triplet" => self.apply_value(self.value.toggle_triplet(), out),
            "editor.tab_extend" => {
                let Some(dir) = arg else { return false };
                return self.extend(dir);
            }
            "editor.tab_technique" => {
                let Some(name) = arg else { return false };
                if Tech::parse(name, self.bend_cents).is_none() {
                    return false;
                }
                self.technique(name, out);
            }
            "editor.tab_bend_amount" => {
                let Some(c) = arg.and_then(tech::bend_from_name) else {
                    return false;
                };
                self.bend_cents = c;
            }
            "editor.tab_string_up" => self.step_string(false, out),
            "editor.tab_string_down" => self.step_string(true, out),
            "editor.tab_transpose" => {
                let semis = match arg {
                    Some("up") => 1,
                    Some("down") => -1,
                    Some("octave_up") => 12,
                    Some("octave_down") => -12,
                    _ => return false,
                };
                self.transpose(semis, out);
            }
            "editor.tab_refret" => match arg {
                Some("choose") => self.open_position_pick(),
                Some("automatic") => self.refret(RefretTarget::Automatic, out),
                Some("lowest") => self.refret(RefretTarget::Lowest, out),
                _ => return false,
            },
            "editor.tab_tuning" => self.open_tuning(),
            "edit.select_all" => self.select_all(),
            "edit.copy" => {
                self.copy();
            }
            "edit.cut" => {
                if self.copy() {
                    self.delete_targets(out);
                }
            }
            "edit.paste" => self.paste(out),
            "edit.delete" => self.delete_targets(out),
            "editor.tab_zoom_in" | "editor.tab_zoom_out" => {
                let f = if id == "editor.tab_zoom_in" {
                    ZOOM_KEY
                } else {
                    1.0 / ZOOM_KEY
                };
                self.axis.zoom_at(f, (self.axis.x0 + self.bounds.x1) / 2.0);
            }
            "sequencer.follow" => self.follow = !self.follow,
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------ edits

    /// Reports that an edit had nothing to work on.
    fn nothing_selected(&mut self) {
        self.status = Some("Select notes, or put the cursor on a note, first".into());
        self.repaint = true;
    }

    /// Grows (or starts) the range one step toward `dir`.
    fn extend(&mut self, dir: &str) -> bool {
        let last = u8::try_from(self.strings().saturating_sub(1)).unwrap_or(0);
        let end = self.end_tick();
        if self.range.is_none_or(|r| r.all) {
            self.anchor = self.cursor;
            self.rect_mode = false;
        }
        let (tick, string) = self.cursor;
        match dir {
            "left" => self.cursor.0 = entry::step_on_grid(tick, self.value.ticks(), -1, end),
            "right" => self.cursor.0 = entry::step_on_grid(tick, self.value.ticks(), 1, end),
            "up" => {
                self.cursor.1 = string.saturating_sub(1);
                self.rect_mode = true;
            }
            "down" => {
                self.cursor.1 = (string + 1).min(last);
                self.rect_mode = true;
            }
            _ => return false,
        }
        // Left and Right select whole columns (every string); Up and Down narrow it to the
        // rectangle between the anchor and the cursor.
        self.range = Some(if self.rect_mode {
            Range::between(self.anchor, self.cursor)
        } else {
            Range::between((self.anchor.0, 0), (self.cursor.0, last))
        });
        self.refresh_sel();
        self.reveal();
        true
    }

    /// Toggles the technique `name` on the targets: set when any lacks it, cleared when all
    /// have it.
    fn technique(&mut self, name: &str, out: &mut Vec<UiAction>) {
        if let Some(c) = tech::explicit_bend(name) {
            self.bend_cents = c;
        }
        let Some(tech) = Tech::parse(name, self.bend_cents) else {
            return;
        };
        let targets = self.targets();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        if targets.is_empty() {
            self.nothing_selected();
            return;
        }
        let plain = Techniques::default();
        let on = tech.toggle_on(
            targets
                .iter()
                .map(|(t, k)| w.tab().note_of(*t, *k).map_or(&plain, |r| &r.tech)),
        );
        let r = tab_edit::update_techniques(&w.notes, &w.midi, &targets, move |t| {
            tech.apply(t, on);
        });
        self.finish_edit(r, clip, out);
    }

    /// Emits an edit and reports the notes the neck could not place.
    fn finish_edit(
        &mut self,
        r: Result<TabEdit, TabEditError>,
        clip: ClipId,
        out: &mut Vec<UiAction>,
    ) -> Option<TabEdit> {
        let e = self.emit(r, clip, None, out)?;
        if !e.unplayable.is_empty() {
            self.status = Some(format!(
                "{} notes cannot be played on this neck",
                e.unplayable.len()
            ));
        }
        Some(e)
    }

    /// Moves the targets to the next string (down: toward the bass) that plays their pitch. The
    /// cursor and the range follow the notes to their new strings.
    fn step_string(&mut self, down: bool, out: &mut Vec<UiAction>) {
        let targets = self.targets();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        if targets.is_empty() {
            self.nothing_selected();
            return;
        }
        let r = tab_edit::step_string(&w.notes, &w.midi, &targets, down);
        if matches!(r, Err(TabEditError::NotPlayable { .. })) {
            self.status = Some("No other string plays that pitch there".into());
            self.repaint = true;
            return;
        }
        let Some(e) = self.finish_edit(r, clip, out) else {
            return;
        };
        let Some(tab) = e.midi.tab.as_ref() else {
            return;
        };
        let strings: Vec<u8> = targets
            .iter()
            .filter_map(|(t, k)| tab.note_of(*t, *k).map(|r| r.string))
            .collect();
        let (Some(&lo), Some(&hi)) = (strings.iter().min(), strings.iter().max()) else {
            return;
        };
        match &mut self.range {
            Some(r) if !r.all => {
                r.s0 = lo;
                r.s1 = hi;
                self.cursor.1 = self.cursor.1.clamp(lo, hi);
            }
            Some(_) => {}
            None => self.cursor.1 = lo,
        }
    }

    fn transpose(&mut self, semitones: i32, out: &mut Vec<UiAction>) {
        let targets = self.targets();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        if targets.is_empty() {
            self.nothing_selected();
            return;
        }
        let r = tab_edit::transpose(&w.notes, &w.midi, &targets, semitones);
        self.finish_edit(r, clip, out);
    }

    fn refret(&mut self, target: RefretTarget, out: &mut Vec<UiAction>) {
        let targets = self.targets();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        if targets.is_empty() {
            self.nothing_selected();
            return;
        }
        let r = tab_edit::refret_at(&w.notes, &w.midi, &targets, target, &FretOptions::default());
        self.finish_edit(r, clip, out);
    }

    fn open_position_pick(&mut self) {
        if self.targets().is_empty() {
            self.nothing_selected();
            return;
        }
        let frets = self.work.as_ref().map_or(12, |w| w.tab().tuning.frets);
        self.modal = Some(Modal::Position(PositionPick {
            max_position: frets.saturating_sub(3).clamp(1, 15),
        }));
        self.repaint = true;
    }

    fn open_tuning(&mut self) {
        if let Some(w) = &self.work {
            self.modal = Some(Modal::Tuning(Box::new(TuningDialog::new(w.tab()))));
            self.repaint = true;
        }
    }

    /// Deletes the selection, else the note under the cursor.
    fn delete_targets(&mut self, out: &mut Vec<UiAction>) {
        let targets = self.targets();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        if targets.is_empty() {
            return;
        }
        let r = tab_edit::delete_notes(&w.notes, &w.midi, &targets);
        self.emit(r, clip, None, out);
        self.clear_range();
    }

    /// Copies the targets into the tab clipboard. False when there was nothing to copy.
    fn copy(&mut self) -> bool {
        let targets = self.targets();
        let Some(w) = &self.work else { return false };
        if targets.is_empty() {
            self.nothing_selected();
            return false;
        }
        match tab_edit::copy_snippet(&w.notes, &w.midi, &targets) {
            Ok(s) => {
                self.clipboard = Some(s);
                self.status = None;
                true
            }
            Err(e) => {
                self.status = Some(e.to_string());
                self.repaint = true;
                false
            }
        }
    }

    /// Puts the clipboard down with its first note at the cursor and selects what was pasted.
    fn paste(&mut self, out: &mut Vec<UiAction>) {
        let (Some(w), Some(clip), Some(snip)) = (&self.work, self.clip_id, &self.clipboard) else {
            self.status = Some("The tab clipboard is empty: copy notes first".into());
            self.repaint = true;
            return;
        };
        let at = self.cursor.0;
        let fits = snip.clipped(self.end_tick() - at);
        if fits.is_empty() {
            self.status = Some("The cursor is too near the end of the clip to paste there".into());
            self.repaint = true;
            return;
        }
        let strings = fits.notes.iter().map(|n| n.string);
        let (s0, s1) = (
            strings.clone().min().unwrap_or(0),
            strings.max().unwrap_or(0),
        );
        let t1 = at + fits.notes.iter().map(|n| n.dt).max().unwrap_or(0);
        let r = tab_edit::paste_snippet(&w.notes, &w.midi, &fits, at);
        if self.emit(r, clip, None, out).is_some() {
            self.anchor = (at, s0);
            self.range = Some(Range {
                t0: at,
                t1,
                s0,
                s1,
                all: false,
            });
        }
    }

    // ------------------------------------------------------------------ modal panels

    fn modal_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) {
        let bounds = self.bounds;
        self.repaint = true;
        let outcome = match event {
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                None
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                None
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                ..
            } => match &mut self.modal {
                Some(Modal::Tuning(d)) => match d.hit(bounds, *pos) {
                    Some(h) => d.click(h, modifiers.shift),
                    None if !d.contains(bounds, *pos) => Some(Outcome::Cancel),
                    None => None,
                },
                Some(Modal::Position(p)) => match p.hit(bounds, *pos) {
                    Some(PickHit::Automatic) => Some(Outcome::Refret(RefretTarget::Automatic)),
                    Some(PickHit::Lowest) => Some(Outcome::Refret(RefretTarget::Lowest)),
                    Some(PickHit::Position(n)) => Some(Outcome::Refret(RefretTarget::Position(n))),
                    None if !p.contains(bounds, *pos) => Some(Outcome::Cancel),
                    None => None,
                },
                None => None,
            },
            UiEvent::Wheel { delta, pos, .. } => {
                if let Some(Modal::Tuning(d)) = &mut self.modal {
                    let rows = if delta.y > 0.0 { -1 } else { 1 };
                    d.wheel(bounds, *pos, rows);
                }
                None
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => Some(Outcome::Cancel),
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if matches!(self.modal, Some(Modal::Tuning(_))) => Some(Outcome::Apply),
            _ => None,
        };
        match outcome {
            Some(Outcome::Cancel) => self.modal = None,
            Some(Outcome::Apply) => {
                if let Some(Modal::Tuning(d)) = self.modal.take() {
                    self.apply_tuning(&d, out);
                }
            }
            Some(Outcome::Refret(t)) => {
                self.modal = None;
                self.refret(t, out);
            }
            None => {}
        }
    }

    /// Applies the tuning dialog: the instrument, tuning and capo of the clip change, as one
    /// undo step.
    fn apply_tuning(&mut self, d: &TuningDialog, out: &mut Vec<UiAction>) {
        let (id, tuning, capo, keep) = d.result();
        let (Some(w), Some(clip)) = (&self.work, self.clip_id) else {
            return;
        };
        let r = if keep {
            tab_edit::set_tuning(&w.notes, &w.midi, &id, tuning, capo)
        } else {
            tab_edit::set_tuning_keep_frets(&w.notes, &w.midi, &id, tuning, capo)
        };
        self.finish_edit(r, clip, out);
    }

    /// Scrolls so the cursor is inside the view.
    fn reveal(&mut self) {
        self.axis.page_to(self.cursor.0, self.bounds.x1);
    }

    /// Paints the editor (see the module docs for the layout).
    pub fn paint(
        &mut self,
        canvas: &mut dyn crate::canvas::Canvas,
        doc: &RackDoc,
        registry: &Registry,
    ) {
        self.sync_current(doc, registry);
        self.paint_all(canvas);
        self.repaint = false;
    }
}

/// A button of the top bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopButton {
    Value(u8),
    Dotted,
    Triplet,
    /// A technique button: an index into [`tech::TOOLBAR`].
    Tool(usize),
    /// The bend amount chip.
    BendAmount,
    /// The instrument label (opens the tuning dialog).
    Instrument,
}
