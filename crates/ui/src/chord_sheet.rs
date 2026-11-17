//! The Chord Sheet pane: a song's chord sheets (chords over lyrics) to read, play along with and
//! edit.
//!
//! The view follows the editors' contract (`new(theme)`, `set_bounds`, `sync`/`sync_at`, `handle`,
//! `command`, `paint`, `needs_repaint`, `set_scale`, `set_focused`, `set_playhead`,
//! `set_playing`, `help_at`, `cursor`, `is_text_owner`, `tick`). It shows one sheet of
//! `Song::sheets` at a time (a selector when there are several) and keeps no copy of the truth:
//! every change of the stored sheets is one `SongCommand::SetSheets`, one undo step.
//!
//! # What is shown, and what is only view state
//!
//! * **Rendering** (`layout`, `content`, `paint`): the title and key, section headers, comments,
//!   chord names over the lyric characters they are anchored to (pushed apart when they would
//!   touch, the lyric gaps with them), chord-only lines and tab text, wrapped at the pane width
//!   with every chord kept over its syllable. The font size is a control.
//! * **Display controls** change what is drawn, never the song: transpose by semitones (the
//!   spelling follows the key, or the sharps/flats preference), a capo (the chords are shown as
//!   the shapes to play with it), and the instrument and tuning the diagrams are drawn for.
//!   "Apply Transpose to Sheet" writes what is shown into the sheet (`SetSheets`, undoable).
//! * **Chord diagrams**: a strip at the top holds every distinct chord of the sheet with its
//!   diagram (it scrolls sideways); clicking a chord of the text or of the strip opens a larger
//!   popup that cycles through the generator's voicings. The voicing picked for a chord is
//!   remembered for the strip while the pane lives.
//! * **Autoscroll**: when lines are linked to bars of the song (`SheetBlock::Line::bar`) and the
//!   transport plays, the sheet scrolls with the playhead; otherwise a play/pause button and a
//!   speed in lines per minute scroll it. [`ChordSheetView::tick`] advances it by the time
//!   between two calls, so a test drives it with chosen times.
//! * **Edit as text**: the sheet becomes a multi-line text field (the chords-over-lyrics form).
//!   Ctrl+Enter or Apply parses it back (one undo step, the bar links of the old lines kept by
//!   line position), Esc or Cancel leaves it. While the field, the rename box or the instrument
//!   list is open the view is a text owner: the shell routes the keys to it.

mod content;
mod layout;
mod paint;

pub use layout::{CHORD_ADVANCE, CHORD_SCALE, LYRIC_ADVANCE, chord_width, layout_line, text_width};

use crate::action::UiAction;
use crate::cursor::CursorHint;
use crate::event::{ImeEvent, Key, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::practice_bar::{PRACTICE_BAR_HEIGHT, PracticeBar, PracticeItem, PracticeState};
use crate::seq_paint::SeqTheme;
use crate::text_field::TextField;
use crate::theme::Theme;
use crate::timeline::Bars;
use crate::tooltip::Help;
use atomik_device_sdk::Registry;
use atomik_model::chord_sheets::{check_sheets, with_sheet, with_sheet_replaced, without_sheet};
use atomik_model::{Command, RackDoc, SongCommand};
use atomik_tab::instrument::{Family, Instrument, catalogue, find_instrument};
use atomik_tab::sheet::{ChordSheet, SheetBlock, parse_sheet};
use atomik_tab::transpose::Spelling;
use atomik_tab::voicing::{Voicing, VoicingCache, VoicingOptions};
use atomik_tab::{Capo, ChordSymbol, Tuning};
use content::Content;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use vello::kurbo::{Point, Rect};

/// The height of one row of the toolbar.
pub const BAR_ROW_HEIGHT: f64 = 24.0;
/// The width of a diagram of the strip.
pub const STRIP_DIAGRAM_WIDTH: f64 = 52.0;
/// The first font size.
pub const DEFAULT_SIZE: f32 = 14.0;
/// The smallest and the largest font size.
pub const SIZE_RANGE: (f32, f32) = (9.0, 30.0);
/// The first autoscroll speed in lines per minute.
pub const DEFAULT_SPEED: f64 = 24.0;
/// The slowest and the fastest autoscroll, in lines per minute.
pub const SPEED_RANGE: (f64, f64) = (4.0, 120.0);
/// The step of the speed buttons.
pub const SPEED_STEP: f64 = 4.0;
/// The most semitones the view transposes by, either way.
pub const MAX_TRANSPOSE: i32 = 11;
/// The highest capo the view offers.
pub const MAX_CAPO: u8 = 12;
/// The longest text the edit field takes, in characters.
const EDIT_LIMIT: usize = 200_000;
const STRIP_GAP: f64 = 10.0;
const STRIP_PAD: f64 = 6.0;
const POPUP_DIAGRAM_WIDTH: f64 = 120.0;
const SCROLL_STEP: f64 = 48.0;
/// A gap longer than this between two ticks is a pause of the host, not time to scroll by.
const MAX_TICK_GAP: f64 = 0.5;

/// How the chord roots are spelled when the view transposes or re-spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpellPref {
    /// The sheet's key decides, else its own habit.
    #[default]
    Auto,
    /// Sharps for every black key.
    Sharps,
    /// Flats for every black key.
    Flats,
}

impl SpellPref {
    fn next(self) -> Self {
        match self {
            SpellPref::Auto => SpellPref::Sharps,
            SpellPref::Sharps => SpellPref::Flats,
            SpellPref::Flats => SpellPref::Auto,
        }
    }

    fn spelling(self) -> Option<Spelling> {
        match self {
            SpellPref::Auto => None,
            SpellPref::Sharps => Some(Spelling::Sharps),
            SpellPref::Flats => Some(Spelling::Flats),
        }
    }
}

/// A button of the toolbar (also the key of a test helper that finds it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetButton {
    /// The previous sheet.
    Prev,
    /// The next sheet.
    Next,
    /// A new sheet.
    New,
    /// Import a sheet file.
    Import,
    /// Rename the sheet.
    Rename,
    /// Delete the sheet.
    Delete,
    /// Edit the sheet as text.
    Edit,
    /// Transpose a semitone down.
    TransposeDown,
    /// Transpose a semitone up.
    TransposeUp,
    /// Back to the written key.
    TransposeReset,
    /// Capo one fret lower.
    CapoDown,
    /// Capo one fret higher.
    CapoUp,
    /// Cycle the sharps/flats preference.
    Spelling,
    /// Smaller text.
    FontDown,
    /// Larger text.
    FontUp,
    /// Open the instrument list.
    Instrument,
    /// The next tuning of the instrument.
    Tuning,
    /// Write the shown chords into the sheet.
    Apply,
    /// Start or stop the speed autoscroll.
    AutoToggle,
    /// Autoscroll slower.
    AutoSlower,
    /// Autoscroll faster.
    AutoFaster,
    /// Follow the transport, or scroll by speed.
    AutoSync,
    /// Apply the edited text.
    ApplyText,
    /// Leave the edit as it was.
    CancelText,
}

/// One item of the toolbar: a button, or a label when `btn` is `None`.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub btn: Option<SheetButton>,
    pub rect: Rect,
    pub text: String,
    pub lit: bool,
    pub on: bool,
    /// The text is the user's (a sheet name), not a phrase of the program.
    pub verbatim: bool,
}

/// What the pane is doing besides showing the sheet.
enum Mode {
    View,
    /// The sheet's text in a field.
    Edit(Box<EditState>),
    /// A one-line field for the sheet's title.
    Rename(Box<RenameState>),
}

struct EditState {
    field: TextField,
    dragging: bool,
    /// The sheet being edited.
    sheet: usize,
    /// The text the field started with.
    original: String,
    error: Option<String>,
}

struct RenameState {
    field: TextField,
    dragging: bool,
    sheet: usize,
}

/// The diagram popup of one chord.
#[derive(Debug, Clone)]
struct Popup {
    chord: ChordSymbol,
    /// Where it hangs from, in window coordinates.
    at: Point,
}

/// What the cache of the laid-out sheet was built for.
#[derive(Debug, Clone, PartialEq)]
struct ContentKey {
    rev: u64,
    sheet: usize,
    transpose: i32,
    capo: Option<u8>,
    pref: SpellPref,
    size: u32,
    width: i64,
}

/// The chord sheet view.
pub struct ChordSheetView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    practice: PracticeBar,
    focused: bool,
    hover: Option<Point>,
    repaint: bool,
    doc_rev: Option<u64>,
    /// The song's sheets as last read.
    sheets: Vec<ChordSheet>,
    /// Counts the changes of `sheets` (the layout cache key).
    sheets_rev: u64,
    bars: Bars,
    selected: usize,
    size: f32,
    transpose: i32,
    capo: Option<u8>,
    pref: SpellPref,
    instrument: &'static str,
    tuning_idx: usize,
    voicings: VoicingCache,
    /// The voicing picked per chord (by name).
    choice: BTreeMap<String, usize>,
    popup: Option<Popup>,
    picker: bool,
    scroll: f64,
    strip_scroll: f64,
    mode: Mode,
    status: Option<String>,
    content: Option<(ContentKey, Rc<Content>)>,
    // Autoscroll.
    running: bool,
    speed: f64,
    sync_pref: bool,
    last_tick: Option<f64>,
    playing: bool,
    playhead: i64,
    /// A sheet was added by this pane (New): select it when it shows up and open the editor.
    expect_new: Option<usize>,
}

impl ChordSheetView {
    /// A view with no sheet.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, 400.0),
            scale: 1.0,
            practice: PracticeBar::new(),
            focused: false,
            hover: None,
            repaint: true,
            doc_rev: None,
            sheets: Vec::new(),
            sheets_rev: 0,
            bars: Bars::default(),
            selected: 0,
            size: DEFAULT_SIZE,
            transpose: 0,
            capo: None,
            pref: SpellPref::Auto,
            instrument: "guitar6",
            tuning_idx: 0,
            voicings: VoicingCache::new(),
            choice: BTreeMap::new(),
            popup: None,
            picker: false,
            scroll: 0.0,
            strip_scroll: 0.0,
            mode: Mode::View,
            status: None,
            content: None,
            running: false,
            speed: DEFAULT_SPEED,
            sync_pref: true,
            last_tick: None,
            playing: false,
            playhead: 0,
            expect_new: None,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Switches the colours; the view repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, outer: Rect) {
        if outer != self.bounds {
            self.bounds = outer;
            self.clamp_scroll();
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

    /// Tells the view whether the transport plays.
    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.repaint = true;
        }
    }

    /// Sets the transport position (song ticks). While the transport plays and the sheet is
    /// linked to bars, the sheet scrolls to the line the playhead is in.
    pub fn set_playhead(&mut self, tick: i64) {
        if tick == self.playhead {
            return;
        }
        self.playhead = tick;
        if self.playing && self.synced() {
            self.follow_playhead();
        }
    }

    /// True while a text field or the instrument list is open: they take every key (Enter,
    /// Esc, arrows), so the shell resolves no command for them.
    pub fn is_text_owner(&self) -> bool {
        self.picker || !matches!(self.mode, Mode::View)
    }

    /// Drops what is open over the sheet (a text being edited, the rename box, the instrument
    /// list, the diagram popup); the host calls it when the pane closes.
    pub fn leave_text(&mut self) {
        self.mode = Mode::View;
        self.picker = false;
        self.popup = None;
        self.running = false;
        self.repaint = true;
    }

    /// Whether the sheet is being edited as text.
    pub fn editing(&self) -> bool {
        matches!(self.mode, Mode::Edit(_))
    }

    /// Whether the rename box is open.
    pub fn renaming(&self) -> bool {
        matches!(self.mode, Mode::Rename(_))
    }

    /// Whether the instrument list is open.
    pub fn picking_instrument(&self) -> bool {
        self.picker
    }

    /// Whether the view wants to be drawn again (autoscroll runs, or something changed).
    pub fn needs_repaint(&self) -> bool {
        self.repaint || (self.running && !self.synced() && self.has_sheet())
    }

    /// The mouse cursor.
    pub fn cursor(&self) -> CursorHint {
        CursorHint::Default
    }

    /// The status line (a refusal or a parse warning), if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    // ------------------------------------------------------------------ state for tests and host

    /// The sheets the view holds (the song's).
    pub fn sheets(&self) -> &[ChordSheet] {
        &self.sheets
    }

    /// The index of the sheet shown.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Selects a sheet by index (clamped); the display controls go back to the sheet's own.
    pub fn select(&mut self, index: usize) {
        let index = index.min(self.sheets.len().saturating_sub(1));
        if index != self.selected {
            self.selected = index;
            self.reset_display();
        }
        self.repaint = true;
    }

    /// The transposition shown, in semitones.
    pub fn transpose_semitones(&self) -> i32 {
        self.transpose
    }

    /// The capo the shapes are shown for (the sheet's own until changed).
    pub fn capo_shown(&self) -> u8 {
        self.capo
            .unwrap_or_else(|| self.sheets.get(self.selected).map_or(0, |s| s.capo))
    }

    /// The spelling preference.
    pub fn spelling_pref(&self) -> SpellPref {
        self.pref
    }

    /// The font size.
    pub fn font_size(&self) -> f32 {
        self.size
    }

    /// The instrument the diagrams are drawn for.
    pub fn instrument_id(&self) -> &'static str {
        self.instrument
    }

    /// The tuning preset the diagrams are drawn for.
    pub fn tuning_name(&self) -> &'static str {
        self.tuning_preset().name
    }

    /// Chooses the instrument of the diagrams (ignored for an id the catalogue lacks).
    pub fn set_instrument(&mut self, id: &str) {
        if let Some(i) = find_instrument(id) {
            self.instrument = i.id;
            self.tuning_idx = 0;
            self.choice.clear();
            self.repaint = true;
        }
    }

    /// The sheet as shown: transposed, with the capo's shapes.
    pub fn shown_sheet(&self) -> Option<ChordSheet> {
        let stored = self.sheets.get(self.selected)?;
        let capo = self.capo_shown();
        let net = self.transpose + i32::from(stored.capo) - i32::from(capo);
        let mut s = stored.clone();
        if net != 0 || self.pref != SpellPref::Auto {
            s.transpose(net, self.pref.spelling());
        }
        s.capo = capo;
        Some(s)
    }

    /// The names of the distinct chords shown, in order of first appearance.
    pub fn chord_names(&self) -> Vec<String> {
        self.shown_sheet()
            .map(|s| s.chords_used().iter().map(ChordSymbol::name).collect())
            .unwrap_or_default()
    }

    /// The scroll offset of the text, in pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Scrolls the text to `y` pixels (clamped).
    pub fn set_scroll(&mut self, y: f64) {
        self.scroll = y;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The most the text can scroll.
    pub fn max_scroll(&mut self) -> f64 {
        let h = self.content().height;
        (h - self.frame().body.height()).max(0.0)
    }

    /// Whether the autoscroll runs (manual speed).
    pub fn autoscroll_running(&self) -> bool {
        self.running
    }

    /// The autoscroll speed in lines per minute.
    pub fn autoscroll_speed(&self) -> f64 {
        self.speed
    }

    /// Whether the sheet scrolls with the transport: some line is linked to a bar and the
    /// preference allows it.
    pub fn synced(&self) -> bool {
        self.sync_pref && self.linked()
    }

    /// Whether some line of the sheet is linked to a bar.
    pub fn linked(&self) -> bool {
        self.sheets.get(self.selected).is_some_and(|s| {
            s.blocks
                .iter()
                .any(|b| matches!(b, SheetBlock::Line { bar: Some(_), .. }))
        })
    }

    /// The chord whose diagram popup is open.
    pub fn popup_chord(&self) -> Option<String> {
        self.popup.as_ref().map(|p| p.chord.name())
    }

    /// The voicing the popup (or the strip) shows for `chord`: index and count.
    pub fn voicing_choice(&mut self, chord: &str) -> Option<(usize, usize)> {
        let c = ChordSymbol::parse(chord).ok()?;
        let n = self.voicings_of(&c).len();
        (n > 0).then(|| (self.choice.get(&c.name()).copied().unwrap_or(0) % n, n))
    }

    /// The centre of a chord name in the text, in window coordinates (for tests and hosts):
    /// the first chord of the shown sheet called `name` that is on screen's layout.
    pub fn chord_position(&mut self, name: &str) -> Option<Point> {
        let content = self.content();
        let f = self.frame();
        let (r, _) = content.chord_hits.iter().find(|(_, c)| c.name() == name)?;
        Some(Point::new(
            f.body.x0 + content::MARGIN + (r.x0 + r.x1) / 2.0,
            f.body.y0 + (r.y0 + r.y1) / 2.0 - self.scroll,
        ))
    }

    /// The centre of a diagram of the strip, in window coordinates.
    pub fn strip_position(&mut self, index: usize) -> Option<Point> {
        let f = self.frame();
        let strip = f.strip?;
        let r = self.strip_cell(strip, index);
        Some(r.center())
    }

    /// The centre of a toolbar button (for tests and hosts).
    pub fn button_center(&self, btn: SheetButton) -> Option<Point> {
        self.frame()
            .items
            .iter()
            .find(|i| i.btn == Some(btn))
            .map(|i| i.rect.center())
    }

    /// Whether a toolbar button exists and is enabled.
    pub fn button_on(&self, btn: SheetButton) -> bool {
        self.frame()
            .items
            .iter()
            .any(|i| i.btn == Some(btn) && i.on)
    }

    // ------------------------------------------------------------------ sync

    /// Reads the sheets from the document.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        self.sync_from(doc, registry, None);
    }

    /// Tells the view the revision of the document the next `handle`, `command` and `paint`
    /// calls get (the editors' contract).
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

    fn sync_from(&mut self, doc: &RackDoc, _registry: &Registry, _revision: Option<u64>) {
        let bars = Bars::of(&doc.song);
        if bars != self.bars {
            self.bars = bars;
            self.repaint = true;
        }
        if doc.song.sheets == self.sheets {
            return;
        }
        let grew = doc.song.sheets.len() > self.sheets.len();
        // The first sheets of a song are not an import: the pane starts on the first.
        let had_some = !self.sheets.is_empty();
        self.sheets = doc.song.sheets.clone();
        self.sheets_rev += 1;
        self.content = None;
        self.repaint = true;
        let last = self.sheets.len().saturating_sub(1);
        if grew && self.expect_new.take().is_some() {
            self.selected = last;
            self.reset_display();
            self.open_edit();
        } else if grew && had_some {
            // An import: show what came in.
            self.selected = last;
            self.reset_display();
        }
        if self.selected > last {
            self.selected = last;
            self.reset_display();
        }
        // An edit in progress on a sheet that is gone or changed under it is dropped.
        let gone = match &self.mode {
            Mode::Edit(e) => e.sheet >= self.sheets.len(),
            Mode::Rename(r) => r.sheet >= self.sheets.len(),
            Mode::View => false,
        };
        if gone {
            self.mode = Mode::View;
        }
        self.clamp_scroll();
    }

    fn reset_display(&mut self) {
        self.transpose = 0;
        self.capo = None;
        self.popup = None;
        self.scroll = 0.0;
        self.strip_scroll = 0.0;
        self.content = None;
        self.status = None;
    }

    // ------------------------------------------------------------------ geometry

    fn has_sheet(&self) -> bool {
        self.selected < self.sheets.len()
    }

    fn instrument_def(&self) -> &'static Instrument {
        find_instrument(self.instrument).unwrap_or(&catalogue()[0])
    }

    fn tuning_preset(&self) -> &'static atomik_tab::NamedTuning {
        let i = self.instrument_def();
        &i.tunings[self.tuning_idx % i.tunings.len()]
    }

    fn tuning(&self) -> &'static Tuning {
        &self.tuning_preset().tuning
    }

    fn display_name(&self, i: usize) -> String {
        match self.sheets.get(i) {
            Some(s) if !s.title.trim().is_empty() => s.title.clone(),
            _ => format!("Sheet {}", i + 1),
        }
    }

    /// The laid-out sheet (cached).
    fn content(&mut self) -> Rc<Content> {
        let width = (self.bounds.width() - 2.0 * content::MARGIN).max(40.0);
        let key = ContentKey {
            rev: self.sheets_rev,
            sheet: self.selected,
            transpose: self.transpose,
            capo: self.capo,
            pref: self.pref,
            size: self.size.to_bits(),
            width: (width * 8.0) as i64,
        };
        if let Some((k, c)) = &self.content
            && *k == key
        {
            return Rc::clone(c);
        }
        let built = Rc::new(
            self.shown_sheet()
                .map(|s| content::build(&s, self.size, width))
                .unwrap_or_default(),
        );
        self.content = Some((key, Rc::clone(&built)));
        built
    }

    fn clamp_scroll(&mut self) {
        let max = if self.has_sheet() && !self.editing() {
            let h = self.content().height;
            (h - self.frame().body.height()).max(0.0)
        } else {
            0.0
        };
        self.scroll = self.scroll.clamp(0.0, max);
    }

    // ------------------------------------------------------------------ voicings

    fn voicing_options(&self) -> VoicingOptions {
        if self.instrument_def().family == Family::Bass {
            VoicingOptions::bass()
        } else {
            VoicingOptions::default()
        }
    }

    fn voicings_of(&mut self, chord: &ChordSymbol) -> Arc<Vec<Voicing>> {
        let opts = self.voicing_options();
        let tuning = self.tuning();
        self.voicings.get(tuning, Capo::NONE, chord, &opts)
    }

    /// The voicing picked for `chord`.
    fn voicing_of(&mut self, chord: &ChordSymbol) -> Option<(Voicing, usize, usize)> {
        let all = self.voicings_of(chord);
        if all.is_empty() {
            return None;
        }
        let i = self.choice.get(&chord.name()).copied().unwrap_or(0) % all.len();
        Some((all[i].clone(), i, all.len()))
    }

    fn cycle_voicing(&mut self, delta: isize) {
        let Some(p) = &self.popup else { return };
        let chord = p.chord.clone();
        let n = self.voicings_of(&chord).len();
        if n == 0 {
            return;
        }
        let cur = self.choice.get(&chord.name()).copied().unwrap_or(0) % n;
        let next = (cur as isize + delta).rem_euclid(n as isize) as usize;
        self.choice.insert(chord.name(), next);
        self.repaint = true;
    }

    // ------------------------------------------------------------------ autoscroll

    /// Advances the manual autoscroll to `now` (seconds, the frame clock).
    pub fn tick(&mut self, now: f64) {
        let last = self.last_tick.replace(now);
        if !self.running || self.synced() || !self.has_sheet() || self.editing() {
            return;
        }
        let Some(last) = last else { return };
        let dt = now - last;
        if dt <= 0.0 || dt > MAX_TICK_GAP {
            return;
        }
        let unit = self.content().line_unit;
        let max = self.max_scroll();
        self.scroll = (self.scroll + self.speed / 60.0 * dt * unit).min(max);
        if self.scroll >= max {
            self.running = false;
        }
        self.repaint = true;
    }

    /// Scrolls so the line the playhead is in sits a third down the pane.
    fn follow_playhead(&mut self) {
        let content = self.content();
        let mut pts: Vec<(i64, f64)> = content
            .anchors
            .iter()
            .map(|(y, bar)| (self.bars.bar_start(i64::from(*bar)), *y))
            .collect();
        if pts.is_empty() {
            return;
        }
        pts.sort_by_key(|p| p.0);
        let t = self.playhead;
        let y = if t <= pts[0].0 {
            pts[0].1
        } else if let Some(w) = pts.windows(2).find(|w| t < w[1].0) {
            let span = (w[1].0 - w[0].0).max(1) as f64;
            w[0].1 + (w[1].1 - w[0].1) * ((t - w[0].0) as f64 / span)
        } else {
            pts[pts.len() - 1].1
        };
        let lead = self.frame().body.height() * 0.3;
        let max = (content.height - self.frame().body.height()).max(0.0);
        let to = (y - lead).clamp(0.0, max);
        if (to - self.scroll).abs() > 0.5 {
            self.scroll = to;
            self.repaint = true;
        }
    }

    /// The index of the anchored line the playhead is in, among the anchors in order.
    pub(crate) fn active_anchor(&mut self) -> Option<usize> {
        let content = self.content();
        let mut best: Option<(usize, i64)> = None;
        for (i, (_, bar)) in content.anchors.iter().enumerate() {
            let start = self.bars.bar_start(i64::from(*bar));
            if start <= self.playhead && best.is_none_or(|(_, s)| start >= s) {
                best = Some((i, start));
            }
        }
        best.map(|(i, _)| i)
    }

    // ------------------------------------------------------------------ toolbar

    /// The toolbar's items placed in rows, and its height.
    pub(crate) fn frame(&self) -> Frame {
        let b = self.bounds;
        // (button, text, lit, enabled, the user's own words)
        type Entry = (Option<SheetButton>, String, bool, bool, bool);
        let mut entries: Vec<Entry> = Vec::new();
        fn add(v: &mut Vec<Entry>, btn: Option<SheetButton>, text: &str, lit: bool, on: bool) {
            v.push((btn, text.to_owned(), lit, on, false));
        }
        let has = self.has_sheet();
        match &self.mode {
            Mode::Edit(_) => {
                add(&mut entries, None, "Editing the sheet as text", false, true);
                add(
                    &mut entries,
                    Some(SheetButton::ApplyText),
                    "Apply",
                    false,
                    true,
                );
                add(
                    &mut entries,
                    Some(SheetButton::CancelText),
                    "Cancel",
                    false,
                    true,
                );
            }
            Mode::Rename(_) => add(&mut entries, None, "Renaming the sheet", false, true),
            Mode::View => {
                let many = self.sheets.len() > 1;
                add(&mut entries, Some(SheetButton::Prev), "<", false, many);
                if has {
                    let name = format!(
                        "{} ({}/{})",
                        self.display_name(self.selected),
                        self.selected + 1,
                        self.sheets.len()
                    );
                    entries.push((None, name, false, true, true));
                } else {
                    add(&mut entries, None, "No chord sheet", false, true);
                }
                let e = &mut entries;
                add(e, Some(SheetButton::Next), ">", false, many);
                add(e, Some(SheetButton::New), "New Sheet", false, true);
                add(e, Some(SheetButton::Import), "Import...", false, true);
                add(e, Some(SheetButton::Rename), "Rename", false, has);
                add(e, Some(SheetButton::Delete), "Delete", false, has);
                add(e, Some(SheetButton::Edit), "Edit Text", false, has);
                add(e, None, "Transpose", false, true);
                add(e, Some(SheetButton::TransposeDown), "-", false, has);
                let t = format!("{:+}", self.transpose);
                add(
                    e,
                    Some(SheetButton::TransposeReset),
                    &t,
                    self.transpose != 0,
                    has,
                );
                add(e, Some(SheetButton::TransposeUp), "+", false, has);
                add(e, None, "Capo", false, true);
                add(e, Some(SheetButton::CapoDown), "-", false, has);
                let c = match self.capo_shown() {
                    0 => "off".to_owned(),
                    n => n.to_string(),
                };
                add(e, None, &c, self.capo_shown() != 0, true);
                add(e, Some(SheetButton::CapoUp), "+", false, has);
                let pref = match self.pref {
                    SpellPref::Auto => "Auto #/b",
                    SpellPref::Sharps => "Sharps",
                    SpellPref::Flats => "Flats",
                };
                add(
                    e,
                    Some(SheetButton::Spelling),
                    pref,
                    self.pref != SpellPref::Auto,
                    has,
                );
                add(e, Some(SheetButton::FontDown), "A-", false, has);
                add(e, Some(SheetButton::FontUp), "A+", false, has);
                let dirty = has && self.display_differs();
                add(e, Some(SheetButton::Apply), "Apply to Sheet", false, dirty);
                let name = self.instrument_def().name;
                e.push((
                    Some(SheetButton::Instrument),
                    name.to_owned(),
                    self.picker,
                    true,
                    true,
                ));
                let tuning = self.tuning_preset().name;
                e.push((
                    Some(SheetButton::Tuning),
                    tuning.to_owned(),
                    false,
                    true,
                    true,
                ));
                let linked = self.linked();
                if self.synced() {
                    add(e, Some(SheetButton::AutoSync), "Synced", true, has);
                } else {
                    if linked {
                        add(e, Some(SheetButton::AutoSync), "Manual", false, has);
                    }
                    let label = if self.running { "Pause" } else { "Scroll" };
                    add(e, Some(SheetButton::AutoToggle), label, self.running, has);
                    add(e, Some(SheetButton::AutoSlower), "-", false, has);
                    let s = format!("{} lines/min", self.speed.round() as i64);
                    add(e, None, &s, false, true);
                    add(e, Some(SheetButton::AutoFaster), "+", false, has);
                }
            }
        }
        // Flow.
        let pad = 8.0;
        let gap = 4.0;
        let mut x = b.x0 + pad;
        let mut row = 0usize;
        let mut items = Vec::new();
        for (btn, text, lit, on, verbatim) in entries {
            let chars = text.chars().count() as f64;
            let w = if btn.is_some() {
                (chars * 6.0 + 14.0).max(22.0)
            } else {
                chars * 6.0 + 4.0
            };
            if x + w > b.x1 - pad && x > b.x0 + pad {
                x = b.x0 + pad;
                row += 1;
            }
            let y0 = b.y0 + 3.0 + row as f64 * BAR_ROW_HEIGHT;
            let rect = Rect::new(x, y0, x + w, y0 + BAR_ROW_HEIGHT - 4.0);
            items.push(Item {
                btn,
                rect,
                text,
                lit,
                on,
                verbatim,
            });
            x += w + if btn.is_some() { gap } else { 2.0 };
        }
        let bar_h = (row + 1) as f64 * BAR_ROW_HEIGHT + 4.0;
        let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + bar_h);
        // The practice bar sits under the toolbar while the sheet is shown (not while it is edited).
        let practice = matches!(self.mode, Mode::View).then(|| {
            Rect::new(
                b.x0,
                bar.y1,
                b.x1,
                (bar.y1 + PRACTICE_BAR_HEIGHT).min(b.y1.max(bar.y1)),
            )
        });
        let top = practice.map_or(bar.y1, |p| p.y1);
        let strip = (self.has_sheet() && !self.editing() && self.chord_count() > 0).then(|| {
            let h =
                crate::chord_diagram::diagram_height(STRIP_DIAGRAM_WIDTH, true) + 2.0 * STRIP_PAD;
            Rect::new(b.x0, top, b.x1, (top + h).min(b.y1.max(top)))
        });
        let body_top = strip.map_or(top, |s| s.y1);
        let body = Rect::new(b.x0, body_top, b.x1, b.y1.max(body_top));
        Frame {
            bar,
            items,
            practice,
            strip,
            body,
        }
    }

    fn chord_count(&self) -> usize {
        self.sheets
            .get(self.selected)
            .map_or(0, |s| s.chords_used().len())
    }

    /// Whether what is shown differs from what is stored.
    fn display_differs(&self) -> bool {
        match (self.shown_sheet(), self.sheets.get(self.selected)) {
            (Some(a), Some(b)) => a != *b,
            _ => false,
        }
    }

    /// The rectangle of diagram `index` of the strip, in window coordinates.
    fn strip_cell(&self, strip: Rect, index: usize) -> Rect {
        let h = crate::chord_diagram::diagram_height(STRIP_DIAGRAM_WIDTH, true);
        let x =
            strip.x0 + 8.0 - self.strip_scroll + index as f64 * (STRIP_DIAGRAM_WIDTH + STRIP_GAP);
        Rect::new(
            x,
            strip.y0 + STRIP_PAD,
            x + STRIP_DIAGRAM_WIDTH,
            strip.y0 + STRIP_PAD + h,
        )
    }

    fn max_strip_scroll(&self, strip: Rect) -> f64 {
        let n = self.chord_count() as f64;
        let w = 16.0 + n * (STRIP_DIAGRAM_WIDTH + STRIP_GAP) - STRIP_GAP;
        (w - strip.width()).max(0.0)
    }

    // ------------------------------------------------------------------ help

    /// The centre of a practice bar control (for tests and hosts).
    pub fn practice_item_center(&self, item: PracticeItem) -> Option<Point> {
        let bar = self.frame().practice?;
        self.practice
            .items(bar)
            .into_iter()
            .find(|(_, i)| *i == item)
            .map(|(r, _)| r.center())
    }

    /// Shows the practice state (speed, count-in, trainer ...) in the practice bar.
    pub fn set_practice(&mut self, state: PracticeState) {
        if self.practice.set_state(state) {
            self.repaint = true;
        }
    }

    /// The song range of the bars the sheet's linked lines cover: from the first linked bar to
    /// the end of the last linked line, which is as long as the gap before it (a bar when only
    /// one line is linked). `None` when no line is linked.
    fn linked_loop(&self) -> Option<(i64, i64)> {
        let sheet = self.sheets.get(self.selected)?;
        let mut bars: Vec<u32> = sheet
            .blocks
            .iter()
            .filter_map(|b| match b {
                SheetBlock::Line { bar: Some(bar), .. } => Some(*bar),
                _ => None,
            })
            .collect();
        bars.sort_unstable();
        bars.dedup();
        let (first, last) = (*bars.first()?, *bars.last()?);
        let span = match bars.len() {
            0 | 1 => 1,
            n => (last - bars[n - 2]).max(1),
        };
        let start = self.bars.bar_start(i64::from(first));
        let end = self.bars.bar_start(i64::from(last + span));
        (end > start).then_some((start, end))
    }

    /// The tooltip of the button under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let f = self.frame();
        if let Some(bar) = f.practice
            && let Some(help) = self.practice.help_at(bar, pos)
        {
            return Some(help);
        }
        let item = f
            .items
            .iter()
            .find(|i| i.btn.is_some() && i.rect.contains(pos))?;
        let btn = item.btn?;
        let (text, cmd, arg): (&'static str, Option<&'static str>, Option<&'static str>) = match btn
        {
            SheetButton::Prev => (
                "Previous chord sheet",
                Some("editor.sheet_select"),
                Some("previous"),
            ),
            SheetButton::Next => (
                "Next chord sheet",
                Some("editor.sheet_select"),
                Some("next"),
            ),
            SheetButton::New => ("Create a new chord sheet", Some("editor.sheet_new"), None),
            SheetButton::Import => ("Import a chord sheet file", Some("file.import_sheet"), None),
            SheetButton::Rename => ("Rename this chord sheet", Some("editor.sheet_rename"), None),
            SheetButton::Delete => (
                "Delete this chord sheet (Undo brings it back)",
                Some("editor.sheet_delete"),
                None,
            ),
            SheetButton::Edit => (
                "Edit the sheet as text",
                Some("editor.sheet_edit_text"),
                None,
            ),
            SheetButton::TransposeDown => (
                "Show the chords a semitone lower",
                Some("editor.sheet_transpose"),
                Some("down"),
            ),
            SheetButton::TransposeUp => (
                "Show the chords a semitone higher",
                Some("editor.sheet_transpose"),
                Some("up"),
            ),
            SheetButton::TransposeReset => (
                "Back to the written key",
                Some("editor.sheet_transpose"),
                Some("reset"),
            ),
            SheetButton::CapoDown => (
                "Capo one fret lower: shapes shown for it",
                Some("editor.sheet_capo"),
                Some("down"),
            ),
            SheetButton::CapoUp => (
                "Capo one fret higher: shapes shown for it",
                Some("editor.sheet_capo"),
                Some("up"),
            ),
            SheetButton::Spelling => (
                "Sharps or flats for the chord names",
                Some("editor.sheet_spelling"),
                Some("next"),
            ),
            SheetButton::FontDown => ("Smaller text", Some("editor.sheet_font"), Some("smaller")),
            SheetButton::FontUp => ("Larger text", Some("editor.sheet_font"), Some("larger")),
            SheetButton::Instrument => ("Instrument the diagrams are drawn for", None, None),
            SheetButton::Tuning => ("Tuning of the diagrams (click for the next)", None, None),
            SheetButton::Apply => (
                "Write the shown chords into the sheet",
                Some("editor.sheet_apply_transpose"),
                None,
            ),
            SheetButton::AutoToggle => (
                "Scroll the sheet at the speed shown",
                Some("editor.sheet_autoscroll"),
                Some("toggle"),
            ),
            SheetButton::AutoSlower => (
                "Scroll slower",
                Some("editor.sheet_autoscroll"),
                Some("slower"),
            ),
            SheetButton::AutoFaster => (
                "Scroll faster",
                Some("editor.sheet_autoscroll"),
                Some("faster"),
            ),
            SheetButton::AutoSync => (
                "Follow the transport or scroll by speed",
                Some("editor.sheet_autoscroll"),
                Some("sync"),
            ),
            SheetButton::ApplyText => ("Make the text the sheet (Ctrl+Enter)", None, None),
            SheetButton::CancelText => ("Leave the text as it was (Esc)", None, None),
        };
        let h = Help::new(item.rect, text);
        Some(match (cmd, arg) {
            (Some(c), Some(a)) => h.with_arg(c, a),
            (Some(c), None) => h.with_command(c),
            _ => h,
        })
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
        if self.picker {
            self.picker_event(event);
            return out;
        }
        match &self.mode {
            Mode::Edit(_) | Mode::Rename(_) => {
                self.text_event(event, &mut out);
                return out;
            }
            Mode::View => {}
        }
        match event {
            UiEvent::PointerMove { pos, .. } => {
                if let Some(bar) = self.frame().practice {
                    self.practice.pointer_move(bar, *pos, &mut out);
                }
                self.hover = Some(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(bar) = self.frame().practice.filter(|b| b.contains(*pos)) {
                    self.popup = None;
                    self.practice.pointer_down(bar, *pos, &mut out);
                    self.repaint = true;
                } else {
                    self.pointer_down(*pos, &mut out);
                }
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } if self.practice.pointer_up() => self.repaint = true,
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                let f = self.frame();
                let sideways = f.strip.is_some_and(|s| s.contains(*pos)) || modifiers.shift;
                if sideways && let Some(strip) = f.strip {
                    let d = if delta.x.abs() > 1e-9 {
                        delta.x
                    } else {
                        delta.y
                    };
                    let max = self.max_strip_scroll(strip);
                    self.strip_scroll = (self.strip_scroll - d * SCROLL_STEP).clamp(0.0, max);
                } else {
                    self.scroll_by(-delta.y * SCROLL_STEP);
                    self.popup = None;
                }
                self.repaint = true;
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if !modifiers.command() && !modifiers.alt => self.key_down(key),
            _ => {}
        }
        out
    }

    fn scroll_by(&mut self, dy: f64) {
        self.scroll += dy;
        let max = self.max_scroll();
        self.scroll = self.scroll.clamp(0.0, max);
    }

    fn key_down(&mut self, key: &Key) {
        let page = (self.frame().body.height() * 0.9).max(40.0);
        match key {
            Key::Escape => {
                if self.popup.take().is_some() {
                    self.repaint = true;
                }
            }
            Key::Down => self.scroll_by(SCROLL_STEP),
            Key::Up => self.scroll_by(-SCROLL_STEP),
            Key::PageDown => self.scroll_by(page),
            Key::PageUp => self.scroll_by(-page),
            Key::Home => self.scroll = 0.0,
            Key::End => {
                self.scroll = self.max_scroll();
            }
            Key::Right if self.popup.is_some() => self.cycle_voicing(1),
            Key::Left if self.popup.is_some() => self.cycle_voicing(-1),
            _ => return,
        }
        self.repaint = true;
    }

    fn pointer_down(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        let f = self.frame();
        // The popup first: its buttons, else it closes and the click goes on.
        let mut closed: Option<ChordSymbol> = None;
        if let Some(p) = self.popup.clone() {
            let r = self.popup_rect(&p);
            if r.contains(pos) {
                self.popup_click(r, pos);
                return;
            }
            closed = Some(p.chord);
            self.popup = None;
            self.repaint = true;
        }
        if f.bar.contains(pos) {
            if let Some(item) = f
                .items
                .iter()
                .find(|i| i.btn.is_some() && i.rect.contains(pos))
                && item.on
                && let Some(btn) = item.btn
            {
                self.press(btn, out);
            }
            return;
        }
        if let Some(strip) = f.strip
            && strip.contains(pos)
        {
            let names = self.chord_symbols();
            for (i, c) in names.into_iter().enumerate() {
                let cell = self.strip_cell(strip, i);
                if cell.contains(pos) {
                    // A click on the chord whose popup it just closed leaves it closed.
                    if closed.as_ref() != Some(&c) {
                        self.toggle_popup(c, Point::new(cell.center().x, cell.y1));
                    }
                    return;
                }
            }
            return;
        }
        if f.body.contains(pos) && self.has_sheet() {
            let content = self.content();
            let local = Point::new(
                pos.x - f.body.x0 - content::MARGIN,
                pos.y - f.body.y0 + self.scroll,
            );
            if let Some((r, c)) = content.chord_hits.iter().find(|(r, _)| r.contains(local)) {
                let at = Point::new(
                    f.body.x0 + content::MARGIN + (r.x0 + r.x1) / 2.0,
                    f.body.y0 + r.y1 - self.scroll,
                );
                if closed.as_ref() != Some(c) {
                    self.toggle_popup(c.clone(), at);
                }
            }
        }
    }

    fn chord_symbols(&mut self) -> Vec<ChordSymbol> {
        self.content().chords.clone()
    }

    fn toggle_popup(&mut self, chord: ChordSymbol, at: Point) {
        if self.popup.as_ref().is_some_and(|p| p.chord == chord) {
            self.popup = None;
        } else {
            self.popup = Some(Popup { chord, at });
        }
        self.repaint = true;
    }

    fn popup_click(&mut self, r: Rect, pos: Point) {
        let (prev, next, close) = paint::popup_buttons(r);
        if close.contains(pos) {
            self.popup = None;
        } else if prev.contains(pos) {
            self.cycle_voicing(-1);
        } else if next.contains(pos) {
            self.cycle_voicing(1);
        }
        self.repaint = true;
    }

    /// The popup's rectangle: below its anchor, or above it near the bottom of the pane.
    fn popup_rect(&self, p: &Popup) -> Rect {
        let (w, h) = paint::popup_size();
        let b = self.bounds;
        let x = (p.at.x - w / 2.0).clamp(b.x0 + 4.0, (b.x1 - w - 4.0).max(b.x0 + 4.0));
        let mut y = p.at.y + 4.0;
        if y + h > b.y1 - 4.0 {
            y = (p.at.y - h - 24.0).max(b.y0 + 4.0);
        }
        Rect::new(x, y, x + w, y + h)
    }

    // ------------------------------------------------------------------ buttons and commands

    fn press(&mut self, btn: SheetButton, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match btn {
            SheetButton::Prev => self.select(self.selected.saturating_sub(1)),
            SheetButton::Next => self.select(self.selected + 1),
            SheetButton::New => self.new_sheet(out),
            SheetButton::Import => out.push(UiAction::RunCommand {
                id: "file.import_sheet".to_owned(),
                arg: None,
            }),
            SheetButton::Rename => self.open_rename(),
            SheetButton::Delete => self.delete_sheet(out),
            SheetButton::Edit => self.open_edit(),
            SheetButton::TransposeDown => self.transpose = (self.transpose - 1).max(-MAX_TRANSPOSE),
            SheetButton::TransposeUp => self.transpose = (self.transpose + 1).min(MAX_TRANSPOSE),
            SheetButton::TransposeReset => self.transpose = 0,
            SheetButton::CapoDown => self.capo = Some(self.capo_shown().saturating_sub(1)),
            SheetButton::CapoUp => self.capo = Some((self.capo_shown() + 1).min(MAX_CAPO)),
            SheetButton::Spelling => self.pref = self.pref.next(),
            SheetButton::FontDown => self.size = (self.size - 1.0).max(SIZE_RANGE.0),
            SheetButton::FontUp => self.size = (self.size + 1.0).min(SIZE_RANGE.1),
            SheetButton::Instrument => self.picker = true,
            SheetButton::Tuning => {
                let n = self.instrument_def().tunings.len();
                self.tuning_idx = (self.tuning_idx + 1) % n;
                self.choice.clear();
            }
            SheetButton::Apply => self.apply_display(out),
            SheetButton::AutoToggle => {
                self.running = !self.running;
                self.last_tick = None;
            }
            SheetButton::AutoSlower => self.speed = (self.speed - SPEED_STEP).max(SPEED_RANGE.0),
            SheetButton::AutoFaster => self.speed = (self.speed + SPEED_STEP).min(SPEED_RANGE.1),
            SheetButton::AutoSync => self.sync_pref = !self.sync_pref,
            SheetButton::ApplyText => self.apply_edit(out),
            SheetButton::CancelText => self.mode = Mode::View,
        }
        self.clamp_scroll();
    }

    /// Runs a command of the pane.
    pub fn command(
        &mut self,
        id: &str,
        arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        if id == "practice.loop_selection" {
            self.sync_current(doc, registry);
            return match self.linked_loop() {
                Some(range) => CommandOutcome::Handled(vec![UiAction::Command(Command::Song(
                    SongCommand::SetLoop {
                        range,
                        enabled: true,
                    },
                ))]),
                None => CommandOutcome::NotApplicable(
                    "No line of this sheet is linked to a bar of the song".to_owned(),
                ),
            };
        }
        if !id.starts_with("editor.sheet_") {
            return CommandOutcome::Declined;
        }
        self.sync_current(doc, registry);
        if self.is_text_owner() {
            return CommandOutcome::Declined;
        }
        let needs_sheet = !matches!(id, "editor.sheet_new");
        if needs_sheet && !self.has_sheet() {
            return CommandOutcome::NotApplicable(
                "There is no chord sheet: create or import one".to_owned(),
            );
        }
        let btn = match (id, arg) {
            ("editor.sheet_select", Some("next")) => SheetButton::Next,
            ("editor.sheet_select", Some("previous")) => SheetButton::Prev,
            ("editor.sheet_new", _) => SheetButton::New,
            ("editor.sheet_rename", _) => SheetButton::Rename,
            ("editor.sheet_delete", _) => SheetButton::Delete,
            ("editor.sheet_edit_text", _) => SheetButton::Edit,
            ("editor.sheet_apply_transpose", _) => {
                if !self.display_differs() {
                    return CommandOutcome::NotApplicable(
                        "Nothing to apply: the sheet is shown as written".to_owned(),
                    );
                }
                SheetButton::Apply
            }
            ("editor.sheet_transpose", Some("up")) => SheetButton::TransposeUp,
            ("editor.sheet_transpose", Some("down")) => SheetButton::TransposeDown,
            ("editor.sheet_transpose", Some("reset")) => SheetButton::TransposeReset,
            ("editor.sheet_capo", Some("up")) => SheetButton::CapoUp,
            ("editor.sheet_capo", Some("down")) => SheetButton::CapoDown,
            ("editor.sheet_capo", Some("off")) => {
                self.capo = Some(0);
                self.repaint = true;
                return CommandOutcome::Handled(Vec::new());
            }
            ("editor.sheet_spelling", Some("next")) => SheetButton::Spelling,
            ("editor.sheet_spelling", Some(a)) => {
                self.pref = match a {
                    "sharps" => SpellPref::Sharps,
                    "flats" => SpellPref::Flats,
                    "auto" => SpellPref::Auto,
                    _ => return CommandOutcome::Declined,
                };
                self.repaint = true;
                return CommandOutcome::Handled(Vec::new());
            }
            ("editor.sheet_font", Some("larger")) => SheetButton::FontUp,
            ("editor.sheet_font", Some("smaller")) => SheetButton::FontDown,
            ("editor.sheet_autoscroll", Some("toggle")) => SheetButton::AutoToggle,
            ("editor.sheet_autoscroll", Some("faster")) => SheetButton::AutoFaster,
            ("editor.sheet_autoscroll", Some("slower")) => SheetButton::AutoSlower,
            ("editor.sheet_autoscroll", Some("sync")) => SheetButton::AutoSync,
            _ => return CommandOutcome::Declined,
        };
        let mut out = Vec::new();
        self.press(btn, &mut out);
        CommandOutcome::Handled(out)
    }

    // ------------------------------------------------------------------ edits of the song

    fn emit(&mut self, sheets: Vec<ChordSheet>, out: &mut Vec<UiAction>) {
        out.push(UiAction::Command(Command::Song(SongCommand::SetSheets(
            sheets,
        ))));
    }

    fn new_sheet(&mut self, out: &mut Vec<UiAction>) {
        let mut n = self.sheets.len() + 1;
        let title = loop {
            let t = format!("New Sheet {n}");
            if !self.sheets.iter().any(|s| s.title == t) {
                break t;
            }
            n += 1;
        };
        let sheet = ChordSheet {
            title,
            blocks: vec![
                SheetBlock::Section("Verse".to_owned()),
                SheetBlock::lyric("Type the lyrics here"),
            ],
            ..ChordSheet::default()
        };
        match with_sheet(&self.sheets, sheet) {
            Some(list) => {
                self.expect_new = Some(list.len() - 1);
                self.emit(list, out);
            }
            None => {
                self.status =
                    Some("The song already holds as many chord sheets as it can".to_owned())
            }
        }
    }

    fn delete_sheet(&mut self, out: &mut Vec<UiAction>) {
        if let Some(list) = without_sheet(&self.sheets, self.selected) {
            self.emit(list, out);
        }
    }

    /// Writes the shown (transposed, capo) sheet into the song.
    fn apply_display(&mut self, out: &mut Vec<UiAction>) {
        let Some(shown) = self.shown_sheet() else {
            return;
        };
        if let Some(list) = with_sheet_replaced(&self.sheets, self.selected, shown) {
            self.emit(list, out);
            self.transpose = 0;
            self.capo = None;
            self.pref = SpellPref::Auto;
        }
    }

    fn open_rename(&mut self) {
        if let Some(s) = self.sheets.get(self.selected) {
            self.mode = Mode::Rename(Box::new(RenameState {
                field: TextField::new(&s.title, atomik_model::chord_sheets::MAX_SHEET_FIELD, false),
                dragging: false,
                sheet: self.selected,
            }));
        }
    }

    fn open_edit(&mut self) {
        if let Some(s) = self.sheets.get(self.selected) {
            let text = s.to_over_lyrics();
            self.mode = Mode::Edit(Box::new(EditState {
                field: TextField::new(&text, EDIT_LIMIT, true),
                dragging: false,
                sheet: self.selected,
                original: text,
                error: None,
            }));
            self.popup = None;
        }
    }

    fn apply_edit(&mut self, out: &mut Vec<UiAction>) {
        let Mode::Edit(e) = &mut self.mode else {
            return;
        };
        let text = e.field.text().to_owned();
        let index = e.sheet;
        if text == e.original {
            self.mode = Mode::View;
            return;
        }
        let parsed = match parse_sheet(&text) {
            Ok(p) => p,
            Err(err) => {
                e.error = Some(err.to_string());
                return;
            }
        };
        let mut sheet = parsed.sheet;
        // The text forms carry no bar links: keep them by the position of the lyric line.
        if let Some(old) = self.sheets.get(index) {
            keep_links(old, &mut sheet);
        }
        if let Err(msg) = check_sheets(std::slice::from_ref(&sheet)) {
            e.error = Some(msg);
            return;
        }
        let Some(list) = with_sheet_replaced(&self.sheets, index, sheet) else {
            self.mode = Mode::View;
            return;
        };
        self.status = parsed.warnings.first().cloned();
        self.mode = Mode::View;
        self.emit(list, out);
        self.transpose = 0;
        self.capo = None;
        self.scroll = 0.0;
    }

    fn apply_rename(&mut self, out: &mut Vec<UiAction>) {
        let Mode::Rename(r) = &self.mode else { return };
        let title = r.field.text().trim().to_owned();
        let index = r.sheet;
        self.mode = Mode::View;
        let Some(old) = self.sheets.get(index) else {
            return;
        };
        if title.is_empty() || title == old.title {
            return;
        }
        let mut sheet = old.clone();
        sheet.title = title;
        if let Some(list) = with_sheet_replaced(&self.sheets, index, sheet) {
            self.emit(list, out);
        }
    }

    // ------------------------------------------------------------------ text modes

    fn text_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) {
        self.repaint = true;
        let rename = self.renaming();
        match event {
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Ime(ime) => self.field_ime(ime),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => match key {
                Key::Escape => self.mode = Mode::View,
                Key::Enter if rename || modifiers.command() => {
                    if rename {
                        self.apply_rename(out);
                    } else {
                        self.apply_edit(out);
                    }
                }
                _ => self.field_key(key, *modifiers),
            },
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } if !rename => {
                let f = self.frame();
                if let Some(item) = f
                    .items
                    .iter()
                    .find(|i| i.btn.is_some() && i.rect.contains(*pos))
                    && let Some(btn) = item.btn
                {
                    self.press(btn, out);
                    return;
                }
                self.field_pointer(event);
            }
            _ => self.field_pointer(event),
        }
    }

    fn field_key(&mut self, key: &Key, m: crate::event::Modifiers) {
        match &mut self.mode {
            Mode::Edit(e) => {
                e.error = None;
                e.field.handle_key(key, m);
            }
            Mode::Rename(r) => {
                r.field.handle_key(key, m);
            }
            Mode::View => {}
        }
    }

    fn field_ime(&mut self, ime: &ImeEvent) {
        match &mut self.mode {
            Mode::Edit(e) => {
                e.field.handle_ime(ime);
            }
            Mode::Rename(r) => {
                r.field.handle_ime(ime);
            }
            Mode::View => {}
        }
    }

    fn field_pointer(&mut self, event: &UiEvent) {
        match &mut self.mode {
            Mode::Edit(e) => {
                e.field.pointer_event(&mut e.dragging, event);
            }
            Mode::Rename(r) => {
                r.field.pointer_event(&mut r.dragging, event);
            }
            Mode::View => {}
        }
    }

    /// The text of the edit field (for tests): `None` outside the edit mode.
    pub fn edit_text(&self) -> Option<&str> {
        match &self.mode {
            Mode::Edit(e) => Some(e.field.text()),
            _ => None,
        }
    }

    /// Replaces the text of the edit field (for tests and hosts).
    pub fn set_edit_text(&mut self, text: &str) {
        if let Mode::Edit(e) = &mut self.mode {
            e.field.set_text(text);
            e.error = None;
            self.repaint = true;
        }
    }

    /// The message of the last refused apply, while editing.
    pub fn edit_error(&self) -> Option<&str> {
        match &self.mode {
            Mode::Edit(e) => e.error.as_deref(),
            _ => None,
        }
    }

    /// The title typed in the rename box (for tests): `None` outside it.
    pub fn rename_text(&self) -> Option<&str> {
        match &self.mode {
            Mode::Rename(r) => Some(r.field.text()),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ instrument list

    /// The rectangle of the instrument list and of each row.
    pub(crate) fn picker_layout(&self) -> (Rect, Vec<(Rect, &'static Instrument)>) {
        paint::picker_layout(self.bounds)
    }

    fn picker_event(&mut self, event: &UiEvent) {
        self.repaint = true;
        match event {
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let (panel, rows) = self.picker_layout();
                if let Some((_, inst)) = rows.iter().find(|(r, _)| r.contains(*pos)) {
                    self.set_instrument(inst.id);
                    self.picker = false;
                } else if !panel.contains(*pos) {
                    self.picker = false;
                }
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => self.picker = false,
            _ => {}
        }
    }

    /// The centre of the row of instrument `id` in the open list (for tests and hosts).
    pub fn instrument_row_center(&self, id: &str) -> Option<Point> {
        self.picker_layout()
            .1
            .into_iter()
            .find(|(_, i)| i.id == id)
            .map(|(r, _)| r.center())
    }

    /// The centre of the popup's next-voicing button (for tests and hosts).
    pub fn popup_next_center(&self) -> Option<Point> {
        let r = self.popup_rect(self.popup.as_ref()?);
        Some(paint::popup_buttons(r).1.center())
    }

    /// Opens the instrument list (for tests and hosts).
    pub fn open_instrument_list(&mut self) {
        self.picker = true;
        self.repaint = true;
    }

    // ------------------------------------------------------------------ paint

    /// Paints the pane.
    pub fn paint(
        &mut self,
        canvas: &mut dyn crate::canvas::Canvas,
        doc: &RackDoc,
        registry: &Registry,
    ) {
        self.sync_current(doc, registry);
        self.clamp_scroll();
        self.paint_all(canvas);
        self.repaint = false;
    }
}

/// The toolbar, the strip and the text area.
pub(crate) struct Frame {
    pub bar: Rect,
    pub items: Vec<Item>,
    pub practice: Option<Rect>,
    pub strip: Option<Rect>,
    pub body: Rect,
}

/// Copies the bar links of `old` to the lyric lines of `new` that sit at the same position among
/// the lines (the text forms carry no links); nothing is kept when the line counts differ.
fn keep_links(old: &ChordSheet, new: &mut ChordSheet) {
    let lines = |s: &ChordSheet| {
        s.blocks
            .iter()
            .filter(|b| matches!(b, SheetBlock::Line { .. }))
            .count()
    };
    if lines(old) != lines(new) {
        return;
    }
    let links: Vec<Option<u32>> = old
        .blocks
        .iter()
        .filter_map(|b| match b {
            SheetBlock::Line { bar, .. } => Some(*bar),
            _ => None,
        })
        .collect();
    let mut it = links.into_iter();
    for b in &mut new.blocks {
        if let SheetBlock::Line { bar, .. } = b {
            *bar = it.next().flatten();
        }
    }
}
