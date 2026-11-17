//! The Chord Trainer pane: drills for changing between chords.
//!
//! The view follows the editors' contract (`new(theme)`, `set_bounds`, `sync`/`sync_at`, `handle`,
//! `command`, `paint`, `needs_repaint`, `set_scale`, `set_focused`, `help_at`, `cursor`,
//! `is_text_owner`, `tick`). It is not tied to a clip and changes nothing in the project: the set
//! of chords, the instrument and the drill are view state, and the best scores (kept in the
//! user's settings by the host) are handed in and out ([`ChordTrainerView::set_scores`],
//! [`ChordTrainerView::take_scores_change`]).
//!
//! # Setup
//!
//! * **Instrument, tuning and capo** from the catalogue (the same list as the chord sheet's). The
//!   voicings are generated for them with the capo, so a diagram shows frets relative to the capo
//!   and the chord that sounds is the chord named.
//! * **The chords** come from a key (the diatonic triads or sevenths), from a typed list (names
//!   the chord grammar reads; the words that are not chords are flagged), or from the song (the
//!   chord lane and the chord sheets). Clicking a diagram of the strip selects the chord; clicking
//!   it again, or the arrows, cycles through the generator's fingerings for it.
//!
//! # The drill
//!
//! The logic is [`atomik_tab::trainer::Drill`]; this view owns it and feeds it a clock. Starting a
//! drill is a request the next [`ChordTrainerView::tick`] carries out, so the drill starts at a
//! frame's time rather than at a stale one. The host takes the clicks and chord changes with
//! [`ChordTrainerView::take_events`] (it sounds the click), and feeds what the input heard to
//! [`ChordTrainerView::hear`]. Changing a setting while a drill runs ends the session (its scores
//! are kept) and starts a new one. A session's results update the best scores of its chord pairs
//! when it ends.

mod paint;

use crate::action::UiAction;
use crate::cursor::CursorHint;
use crate::event::{ImeEvent, Key, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::seq_paint::SeqTheme;
use crate::text_field::TextField;
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_device_sdk::Registry;
use atomik_model::RackDoc;
use atomik_model::chord_symbol::format_chord;
use atomik_tab::instrument::{Family, Instrument, NamedTuning, catalogue, find_instrument};
use atomik_tab::trainer::{
    self, ChordList, Drill, DrillConfig, DrillEvent, DrillMode, Heard, MAX_BEATS, MAX_BPM,
    MAX_CHORDS, MIN_BEATS, MIN_BPM, Order, Scores,
};
use atomik_tab::voicing::{Voicing, VoicingCache, VoicingOptions};
use atomik_tab::{Capo, ChordSymbol, Tuning, grade};
use std::collections::BTreeMap;
use std::sync::Arc;
use vello::kurbo::{Point, Rect};

/// Height of one row of the toolbar.
pub const BAR_ROW: f64 = 24.0;
/// The width of a diagram of the chord strip.
pub const STRIP_DIAGRAM_WIDTH: f64 = 48.0;
/// The most a capo can be.
pub const MAX_CAPO: u8 = 12;
/// How much a tempo step changes the tempo.
pub const BPM_STEP: u16 = 5;
const STRIP_GAP: f64 = 8.0;
const STRIP_PAD: f64 = 6.0;
const SCROLL_STEP: f64 = 48.0;
const LIST_LIMIT: usize = 400;

/// Where the chords of the drill come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The diatonic chords of a key.
    #[default]
    Key,
    /// A list typed by the user.
    List,
    /// The chords of the song (chord lane and chord sheets).
    Song,
}

impl Source {
    /// The argument of `editor.trainer_source`.
    pub fn arg(self) -> &'static str {
        match self {
            Source::Key => "key",
            Source::List => "list",
            Source::Song => "song",
        }
    }

    fn next(self) -> Source {
        match self {
            Source::Key => Source::List,
            Source::List => Source::Song,
            Source::Song => Source::Key,
        }
    }

    fn from_arg(a: &str) -> Option<Source> {
        [Source::Key, Source::List, Source::Song]
            .into_iter()
            .find(|s| s.arg() == a)
    }
}

fn mode_arg(m: DrillMode) -> &'static str {
    match m {
        DrillMode::Timed => "timed",
        DrillMode::Verified => "verified",
        DrillMode::Listen => "listen",
    }
}

fn next_mode(m: DrillMode) -> DrillMode {
    match m {
        DrillMode::Timed => DrillMode::Verified,
        DrillMode::Verified => DrillMode::Listen,
        DrillMode::Listen => DrillMode::Timed,
    }
}

fn order_arg(o: Order) -> &'static str {
    match o {
        Order::Sequential => "sequential",
        Order::Random => "random",
        Order::Pair => "pair",
    }
}

fn next_order(o: Order) -> Order {
    match o {
        Order::Sequential => Order::Random,
        Order::Random => Order::Pair,
        Order::Pair => Order::Sequential,
    }
}

/// A button of the toolbar and the panels (also the key of the test helper that finds it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainerButton {
    /// Opens the instrument list.
    Instrument,
    /// The next tuning of the instrument.
    Tuning,
    /// Capo one fret lower.
    CapoDown,
    /// Capo one fret higher.
    CapoUp,
    /// The next source of chords.
    Source,
    /// The key one semitone down.
    KeyDown,
    /// The key one semitone up.
    KeyUp,
    /// Major or minor.
    KeyMode,
    /// Triads or sevenths.
    Sevenths,
    /// Opens the list editor.
    EditList,
    /// The next drill mode.
    Mode,
    /// The next order.
    Order,
    /// Fewer beats per chord.
    BeatsDown,
    /// More beats per chord.
    BeatsUp,
    /// A slower tempo.
    BpmDown,
    /// A faster tempo.
    BpmUp,
    /// The first chord of the pair.
    PairA,
    /// The second chord of the pair.
    PairB,
    /// Start or stop the drill.
    Start,
    /// Skip the chord (listen mode).
    Skip,
    /// The next listening input.
    Input,
    /// Forget the best scores.
    ResetScores,
    /// The previous fingering of the selected chord.
    VoicingPrev,
    /// The next fingering of the selected chord.
    VoicingNext,
    /// Apply the typed list.
    ApplyList,
    /// Leave the list editor.
    CancelList,
}

/// One item of the toolbar: a button, or a label when `btn` is `None`.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub btn: Option<TrainerButton>,
    pub rect: Rect,
    pub text: String,
    pub lit: bool,
    pub on: bool,
    /// The text is the user's or the catalogue's own, not a phrase of the program.
    pub verbatim: bool,
}

/// The rectangles of the pane.
pub(crate) struct Frame {
    pub bar: Rect,
    pub items: Vec<Item>,
    pub strip: Rect,
    pub stage: Rect,
    pub scores: Rect,
}

/// The list editor.
struct ListEdit {
    field: TextField,
    dragging: bool,
}

/// The chord trainer view.
pub struct ChordTrainerView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    focused: bool,
    hover: Option<Point>,
    repaint: bool,
    // Setup.
    instrument: &'static str,
    tuning_idx: usize,
    capo: u8,
    source: Source,
    key_root: u8,
    minor: bool,
    sevenths: bool,
    list_text: String,
    list_edit: Option<ListEdit>,
    song_chords: Vec<ChordSymbol>,
    set: Vec<ChordSymbol>,
    invalid: Vec<String>,
    voicings: VoicingCache,
    choice: BTreeMap<String, usize>,
    selected: usize,
    pair: (usize, usize),
    strip_scroll: f64,
    cfg: DrillConfig,
    picker: bool,
    // Running.
    drill: Option<Drill>,
    pending_start: bool,
    session: u64,
    now: f64,
    seed: u64,
    events: Vec<DrillEvent>,
    input: Option<String>,
    // Results.
    scores: Scores,
    scores_changed: bool,
    status: Option<String>,
}

impl ChordTrainerView {
    /// A view with the key of C major, the guitar and a drill of four beats a chord at 60 BPM.
    pub fn new(theme: Theme) -> Self {
        let mut v = Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 520.0),
            scale: 1.0,
            focused: false,
            hover: None,
            repaint: true,
            instrument: "guitar6",
            tuning_idx: 0,
            capo: 0,
            source: Source::Key,
            key_root: 0,
            minor: false,
            sevenths: false,
            list_text: String::new(),
            list_edit: None,
            song_chords: Vec::new(),
            set: Vec::new(),
            invalid: Vec::new(),
            voicings: VoicingCache::new(),
            choice: BTreeMap::new(),
            selected: 0,
            pair: (0, 1),
            strip_scroll: 0.0,
            cfg: DrillConfig::default(),
            picker: false,
            drill: None,
            pending_start: false,
            session: 0,
            now: 0.0,
            seed: 0x9E37_79B9_7F4A_7C15,
            events: Vec::new(),
            input: None,
            scores: Scores::new(),
            scores_changed: false,
            status: None,
        };
        v.refresh_set();
        v
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
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale (the window's pixels per logical pixel).
    pub fn set_scale(&mut self, scale: f64) {
        if (scale - self.scale).abs() > f64::EPSILON {
            self.scale = scale;
            self.repaint = true;
        }
    }

    /// Marks the pane focused or not.
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// Whether the keyboard goes to a text field (the list editor) or the instrument list.
    pub fn is_text_owner(&self) -> bool {
        self.picker || self.list_edit.is_some()
    }

    /// Whether the clock must keep ticking: a drill runs, or a start waits for the next frame.
    pub fn wants_frames(&self) -> bool {
        self.running() || self.pending_start
    }

    /// Whether the view wants another frame: while a drill runs (its timer and beats move), or
    /// after a change.
    pub fn needs_repaint(&self) -> bool {
        self.repaint || self.running()
    }

    /// The pointer shape.
    pub fn cursor(&self) -> CursorHint {
        CursorHint::Default
    }

    /// The status line (a refusal, a new best), if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Ends what is open over the pane (the list editor, the instrument list) and stops the drill;
    /// the host calls it when the pane closes.
    pub fn leave(&mut self) {
        self.list_edit = None;
        self.picker = false;
        self.pending_start = false;
        self.stop_now();
    }

    /// The seed of the random order (the host picks one per run; tests fix it).
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed | 1;
    }

    /// Names the input the listening modes use (`None`: the interface has none).
    pub fn set_input(&mut self, label: Option<String>) {
        if self.input != label {
            self.input = label;
            self.repaint = true;
        }
    }

    /// The input the listening modes use.
    pub fn input(&self) -> Option<&str> {
        self.input.as_deref()
    }

    /// Reads the song's chords (the chord lane and the sheets) from the document.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let mut names: Vec<String> = Vec::new();
        for c in &doc.song.harmony.chords {
            names.push(format_chord(c.root, c.quality, c.bass));
        }
        for sheet in &doc.song.sheets {
            names.extend(sheet.chords_used().iter().map(ChordSymbol::name));
        }
        let mut chords: Vec<ChordSymbol> = Vec::new();
        for n in names {
            if chords.len() >= MAX_CHORDS {
                break;
            }
            if let Ok(c) = ChordSymbol::parse(&n)
                && !chords.iter().any(|k| k.name() == c.name())
            {
                chords.push(c);
            }
        }
        if chords != self.song_chords {
            self.song_chords = chords;
            if self.source == Source::Song {
                self.chords_changed();
            }
            self.repaint = true;
        }
    }

    /// Tells the view the revision of the document the next calls get (the editors' contract; the
    /// view reads the song's chords on every sync, so it keeps nothing).
    pub fn set_doc_revision(&mut self, _revision: Option<u64>) {}

    /// [`Self::sync`] for a document known by its revision.
    pub fn sync_at(&mut self, doc: &RackDoc, registry: &Registry, _revision: u64) {
        self.sync(doc, registry);
    }

    // ------------------------------------------------------------------ state for tests and host

    /// The chords of the set, in order.
    pub fn chords(&self) -> &[ChordSymbol] {
        &self.set
    }

    /// The names of the chords of the set.
    pub fn chord_names(&self) -> Vec<String> {
        self.set.iter().map(ChordSymbol::name).collect()
    }

    /// The words of the typed list that are not chords.
    pub fn invalid_words(&self) -> &[String] {
        &self.invalid
    }

    /// The chord source.
    pub fn source(&self) -> Source {
        self.source
    }

    /// The key's tonic (pitch class) and whether it is minor.
    pub fn key(&self) -> (u8, bool) {
        (self.key_root, self.minor)
    }

    /// Whether the key's chords are sevenths.
    pub fn sevenths(&self) -> bool {
        self.sevenths
    }

    /// The instrument the diagrams are drawn for.
    pub fn instrument_id(&self) -> &'static str {
        self.instrument
    }

    /// The name of the tuning preset.
    pub fn tuning_name(&self) -> &'static str {
        self.tuning_preset().name
    }

    /// The capo, in frets (0 = none).
    pub fn capo(&self) -> u8 {
        self.capo
    }

    /// The drill settings.
    pub fn config(&self) -> &DrillConfig {
        &self.cfg
    }

    /// The two chords of the pair drill (indexes into the set).
    pub fn pair(&self) -> (usize, usize) {
        self.pair
    }

    /// The selected chord of the strip.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The drill, running or finished (`None` before the first and after a setting changed).
    pub fn drill(&self) -> Option<&Drill> {
        self.drill.as_ref()
    }

    /// Whether a drill is running.
    pub fn running(&self) -> bool {
        self.drill.as_ref().is_some_and(Drill::running)
    }

    /// Counts the drills started (a restart is a new one): the host tells a click it started for
    /// an old session from the one that runs now.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Whether the running drill sounds a click (the timed modes).
    pub fn wants_click(&self) -> bool {
        self.running() && self.cfg.mode != DrillMode::Listen
    }

    /// Whether the running drill listens to the input.
    pub fn wants_input(&self) -> bool {
        self.running() && self.cfg.mode != DrillMode::Timed
    }

    /// Whether the listening modes are chosen (a start needs an input then).
    pub fn listens(&self) -> bool {
        self.cfg.mode != DrillMode::Timed
    }

    /// The best scores.
    pub fn scores(&self) -> &Scores {
        &self.scores
    }

    /// Sets the best scores (the host, from the user's saved state); not a change to save.
    pub fn set_scores(&mut self, scores: Scores) {
        self.scores = scores;
        self.scores_changed = false;
        self.repaint = true;
    }

    /// The scores when a session or a reset changed them since the last call (the host saves them).
    pub fn take_scores_change(&mut self) -> Option<Scores> {
        std::mem::take(&mut self.scores_changed).then(|| self.scores.clone())
    }

    /// The clicks and chord changes since the last call.
    pub fn take_events(&mut self) -> Vec<DrillEvent> {
        std::mem::take(&mut self.events)
    }

    /// The text of the typed list.
    pub fn list_text(&self) -> &str {
        &self.list_text
    }

    /// Sets the typed list and makes it the source (for tests and hosts).
    pub fn set_list_text(&mut self, text: &str) {
        self.list_text = text.chars().take(LIST_LIMIT).collect();
        self.source = Source::List;
        self.chords_changed();
    }

    /// The text of the list field while it is open.
    pub fn edit_text(&self) -> Option<&str> {
        self.list_edit.as_ref().map(|e| e.field.text())
    }

    /// Replaces the text of the list field (for tests and hosts).
    pub fn set_edit_text(&mut self, text: &str) {
        if let Some(e) = &mut self.list_edit {
            e.field.set_text(text);
            self.repaint = true;
        }
    }

    /// The voicing chosen for chord `index` of the set, with its place among the alternatives.
    pub fn voicing_choice(&mut self, index: usize) -> Option<(usize, usize)> {
        let chord = self.set.get(index)?.clone();
        let (_, i, n) = self.voicing_of(&chord)?;
        Some((i, n))
    }

    /// Opens the instrument list (for tests and hosts).
    pub fn open_instrument_list(&mut self) {
        self.picker = true;
        self.repaint = true;
    }

    /// Chooses the instrument (ignored for an id the catalogue lacks).
    pub fn set_instrument(&mut self, id: &str) {
        if let Some(i) = find_instrument(id) {
            self.instrument = i.id;
            self.tuning_idx = 0;
            self.choice.clear();
            self.setup_changed();
        }
    }

    /// The centre of the row of instrument `id` in the open list (for tests and hosts).
    pub fn instrument_row_center(&self, id: &str) -> Option<Point> {
        crate::chord_sheet::paint::picker_layout(self.bounds)
            .1
            .into_iter()
            .find(|(_, i)| i.id == id)
            .map(|(r, _)| r.center())
    }

    /// The centre of a toolbar button (for tests and hosts).
    pub fn button_center(&self, btn: TrainerButton) -> Option<Point> {
        self.frame()
            .items
            .iter()
            .find(|i| i.btn == Some(btn))
            .map(|i| i.rect.center())
    }

    /// Whether a toolbar button is enabled.
    pub fn button_on(&self, btn: TrainerButton) -> bool {
        self.frame()
            .items
            .iter()
            .find(|i| i.btn == Some(btn))
            .is_some_and(|i| i.on)
    }

    /// The centre of diagram `index` of the chord strip (for tests and hosts).
    pub fn strip_center(&self, index: usize) -> Option<Point> {
        (index < self.set.len()).then(|| self.strip_cell(self.frame().strip, index).center())
    }

    /// The centre of the Reset button of the scores (for tests and hosts).
    pub fn reset_center(&self) -> Point {
        paint::reset_rect(self.frame().scores).center()
    }

    // ------------------------------------------------------------------ instrument, voicings

    fn instrument_def(&self) -> &'static Instrument {
        find_instrument(self.instrument).unwrap_or(&catalogue()[0])
    }

    fn tuning_preset(&self) -> &'static NamedTuning {
        let i = self.instrument_def();
        &i.tunings[self.tuning_idx % i.tunings.len()]
    }

    fn tuning(&self) -> &'static Tuning {
        &self.tuning_preset().tuning
    }

    fn capo_of(&self) -> Capo {
        if self.capo == 0 {
            Capo::NONE
        } else {
            Capo::at(self.capo)
        }
    }

    fn voicing_options(&self) -> VoicingOptions {
        if self.instrument_def().family == Family::Bass {
            VoicingOptions::bass()
        } else {
            VoicingOptions::default()
        }
    }

    fn voicings_of(&mut self, chord: &ChordSymbol) -> Arc<Vec<Voicing>> {
        let opts = self.voicing_options();
        let (tuning, capo) = (self.tuning(), self.capo_of());
        self.voicings.get(tuning, capo, chord, &opts)
    }

    /// The voicing picked for `chord` with its place among the alternatives.
    fn voicing_of(&mut self, chord: &ChordSymbol) -> Option<(Voicing, usize, usize)> {
        let all = self.voicings_of(chord);
        if all.is_empty() {
            return None;
        }
        let i = self.choice.get(&chord.name()).copied().unwrap_or(0) % all.len();
        Some((all[i].clone(), i, all.len()))
    }

    /// The pitch classes each chord of the set is heard as: those of its chosen voicing.
    fn masks(&mut self) -> Vec<u16> {
        let (tuning, capo) = (self.tuning(), self.capo_of());
        let set = self.set.clone();
        set.iter()
            .map(|c| match self.voicing_of(c) {
                Some((v, ..)) => v.pc_mask(tuning, capo),
                None => grade::pc_mask(&c.pitch_classes()),
            })
            .collect()
    }

    fn cycle_voicing(&mut self, delta: isize) -> bool {
        let Some(chord) = self.set.get(self.selected).cloned() else {
            return false;
        };
        let n = self.voicings_of(&chord).len();
        if n == 0 {
            return false;
        }
        let cur = self.choice.get(&chord.name()).copied().unwrap_or(0) % n;
        let next = (cur as isize + delta).rem_euclid(n as isize) as usize;
        self.choice.insert(chord.name(), next);
        self.repaint = true;
        // What the input must hear changes with the fingering.
        self.restart_if_running();
        true
    }

    // ------------------------------------------------------------------ the set

    fn refresh_set(&mut self) {
        let (set, invalid) = match self.source {
            Source::Key => (
                trainer::diatonic(trainer::key_of(self.key_root, self.minor), self.sevenths),
                Vec::new(),
            ),
            Source::List => {
                let ChordList { chords, invalid } = trainer::parse_chord_list(&self.list_text);
                (chords, invalid)
            }
            Source::Song => (self.song_chords.clone(), Vec::new()),
        };
        self.set = set;
        self.invalid = invalid;
        let n = self.set.len();
        self.selected = self.selected.min(n.saturating_sub(1));
        self.pair.0 = self.pair.0.min(n.saturating_sub(1));
        self.pair.1 = self.pair.1.min(n.saturating_sub(1));
        if self.pair.0 == self.pair.1 && n > 1 {
            self.pair.1 = (self.pair.0 + 1) % n;
        }
        self.strip_scroll = self.strip_scroll.clamp(0.0, self.max_strip_scroll());
        self.repaint = true;
    }

    /// The set changed: the diagrams, the drill and the stats start over.
    fn chords_changed(&mut self) {
        self.refresh_set();
        self.setup_changed();
    }

    /// A setting the drill depends on changed.
    fn setup_changed(&mut self) {
        self.refresh_set();
        self.restart_if_running();
        if !self.running() {
            self.drill = None;
        }
        self.repaint = true;
    }

    fn restart_if_running(&mut self) {
        if self.running() {
            self.stop_now();
            self.pending_start = true;
        }
    }

    // ------------------------------------------------------------------ the drill

    /// Moves the clock to `now` (seconds, the frame clock): starts a requested drill and advances a
    /// running one. The beats and changes are taken with [`Self::take_events`].
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        if self.pending_start {
            self.pending_start = false;
            self.begin(now);
        }
        if let Some(d) = &mut self.drill
            && d.running()
        {
            let ev = d.advance(now);
            self.events.extend(ev);
            self.repaint = true;
        }
    }

    fn begin(&mut self, now: f64) {
        if self.set.len() < 2 {
            self.status = Some("Pick at least two chords for the drill".to_owned());
            return;
        }
        let masks = self.masks();
        let names = self.chord_names();
        // splitmix-style step so every start differs.
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut d = Drill::new(names, masks, self.cfg, self.pair, self.seed);
        d.start(now);
        let ev = d.advance(now);
        self.events.extend(ev);
        self.drill = Some(d);
        self.session += 1;
        self.status = None;
        self.repaint = true;
    }

    /// Ends the running drill at the last clock time and counts its results.
    fn stop_now(&mut self) {
        let Some(d) = &mut self.drill else { return };
        if !d.running() {
            return;
        }
        d.stop(self.now);
        let results = d.pair_results();
        if self.scores.record_all(&results) {
            self.scores_changed = true;
        }
        self.repaint = true;
    }

    /// Takes what the input heard: a time on the clock of [`Self::tick`] and the chroma there.
    pub fn hear(&mut self, time: f64, chroma: [f32; 12]) {
        let Some(d) = &mut self.drill else { return };
        let ev = d.hear(&Heard { time, chroma });
        self.events.extend(ev);
        self.repaint = true;
    }

    // ------------------------------------------------------------------ commands

    fn button_command(
        btn: TrainerButton,
        view: &Self,
    ) -> Option<(&'static str, Option<&'static str>)> {
        use TrainerButton as B;
        let up = |b: bool| Some(if b { "up" } else { "down" });
        Some(match btn {
            B::Instrument => ("editor.trainer_instrument", None),
            B::Tuning => ("editor.trainer_tuning", None),
            B::CapoDown => ("editor.trainer_capo", up(false)),
            B::CapoUp => ("editor.trainer_capo", up(true)),
            B::Source => ("editor.trainer_source", Some(view.source.next().arg())),
            B::KeyDown => ("editor.trainer_key", up(false)),
            B::KeyUp => ("editor.trainer_key", up(true)),
            B::KeyMode => (
                "editor.trainer_key",
                Some(if view.minor { "major" } else { "minor" }),
            ),
            B::Sevenths => ("editor.trainer_sevenths", None),
            B::EditList => ("editor.trainer_edit_list", None),
            B::Mode => (
                "editor.trainer_mode",
                Some(mode_arg(next_mode(view.cfg.mode))),
            ),
            B::Order => (
                "editor.trainer_order",
                Some(order_arg(next_order(view.cfg.order))),
            ),
            B::BeatsDown => ("editor.trainer_beats", up(false)),
            B::BeatsUp => ("editor.trainer_beats", up(true)),
            B::BpmDown => ("editor.trainer_bpm", up(false)),
            B::BpmUp => ("editor.trainer_bpm", up(true)),
            B::PairA => ("editor.trainer_pair", Some("a")),
            B::PairB => ("editor.trainer_pair", Some("b")),
            B::Start => ("editor.trainer_start", None),
            B::Skip => ("editor.trainer_skip", None),
            B::ResetScores => ("editor.trainer_reset_scores", None),
            B::VoicingPrev => ("editor.trainer_voicing", Some("prev")),
            B::VoicingNext => ("editor.trainer_voicing", Some("next")),
            B::Input | B::ApplyList | B::CancelList => return None,
        })
    }

    fn press(&mut self, btn: TrainerButton, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match btn {
            TrainerButton::Input => out.push(UiAction::RunCommand {
                id: "practice.listen_input".to_owned(),
                arg: None,
            }),
            TrainerButton::ApplyList => self.apply_list(),
            TrainerButton::CancelList => self.list_edit = None,
            _ => {
                if let Some((id, arg)) = Self::button_command(btn, self)
                    && let CommandOutcome::NotApplicable(msg) = self.run(id, arg)
                {
                    self.status = Some(msg);
                }
            }
        }
    }

    /// Runs one of the trainer's commands (`editor.trainer_*`).
    pub fn command(
        &mut self,
        id: &str,
        arg: Option<&str>,
        _doc: &RackDoc,
        _registry: &Registry,
    ) -> CommandOutcome {
        if !id.starts_with("editor.trainer_") {
            return CommandOutcome::Declined;
        }
        if self.list_edit.is_some() {
            return CommandOutcome::Declined;
        }
        self.run(id, arg)
    }

    fn handled() -> CommandOutcome {
        CommandOutcome::Handled(Vec::new())
    }

    fn refuse(text: &str) -> CommandOutcome {
        CommandOutcome::NotApplicable(text.to_owned())
    }

    fn run(&mut self, id: &str, arg: Option<&str>) -> CommandOutcome {
        self.repaint = true;
        self.status = None;
        let up = arg == Some("up");
        match id {
            "editor.trainer_start" => {
                if self.running() || self.pending_start {
                    self.pending_start = false;
                    self.stop_now();
                    return Self::handled();
                }
                if self.set.len() < 2 {
                    return Self::refuse("Pick at least two chords for the drill");
                }
                if self.listens() && self.input.is_none() {
                    return Self::refuse("The audio interface has no inputs to listen to");
                }
                self.pending_start = true;
                Self::handled()
            }
            "editor.trainer_skip" => {
                let now = self.now;
                let skipped = self.drill.as_mut().is_some_and(|d| d.skip(now));
                if skipped {
                    Self::handled()
                } else {
                    Self::refuse("Skip works while a listen drill runs")
                }
            }
            "editor.trainer_source" => match arg.and_then(Source::from_arg) {
                Some(s) => {
                    self.source = s;
                    self.chords_changed();
                    Self::handled()
                }
                None => Self::refuse("Unknown chord source"),
            },
            "editor.trainer_key" => {
                match arg {
                    Some("up") => self.key_root = (self.key_root + 1) % 12,
                    Some("down") => self.key_root = (self.key_root + 11) % 12,
                    Some("minor") => self.minor = true,
                    Some("major") => self.minor = false,
                    _ => return Self::refuse("Unknown key change"),
                }
                self.source = Source::Key;
                self.chords_changed();
                Self::handled()
            }
            "editor.trainer_sevenths" => {
                self.sevenths = !self.sevenths;
                self.source = Source::Key;
                self.chords_changed();
                Self::handled()
            }
            "editor.trainer_edit_list" => {
                self.open_list_editor();
                Self::handled()
            }
            "editor.trainer_instrument" => {
                self.picker = true;
                Self::handled()
            }
            "editor.trainer_tuning" => {
                self.tuning_idx = (self.tuning_idx + 1) % self.instrument_def().tunings.len();
                self.choice.clear();
                self.setup_changed();
                Self::handled()
            }
            "editor.trainer_capo" => {
                let capo = if up {
                    (self.capo + 1).min(MAX_CAPO)
                } else {
                    self.capo.saturating_sub(1)
                };
                if capo != self.capo {
                    self.capo = capo;
                    self.choice.clear();
                    self.setup_changed();
                }
                Self::handled()
            }
            "editor.trainer_mode" => {
                let mode = [DrillMode::Timed, DrillMode::Verified, DrillMode::Listen]
                    .into_iter()
                    .find(|m| Some(mode_arg(*m)) == arg);
                match mode {
                    Some(m) => {
                        self.cfg.mode = m;
                        self.setup_changed();
                        Self::handled()
                    }
                    None => Self::refuse("Unknown drill mode"),
                }
            }
            "editor.trainer_order" => {
                let order = [Order::Sequential, Order::Random, Order::Pair]
                    .into_iter()
                    .find(|o| Some(order_arg(*o)) == arg);
                match order {
                    Some(o) => {
                        self.cfg.order = o;
                        self.setup_changed();
                        Self::handled()
                    }
                    None => Self::refuse("Unknown drill order"),
                }
            }
            "editor.trainer_beats" => {
                let b = if up {
                    self.cfg.beats.saturating_add(1)
                } else {
                    self.cfg.beats.saturating_sub(1)
                };
                self.cfg.beats = b.clamp(MIN_BEATS, MAX_BEATS);
                self.setup_changed();
                Self::handled()
            }
            "editor.trainer_bpm" => {
                let b = if up {
                    self.cfg.bpm.saturating_add(BPM_STEP)
                } else {
                    self.cfg.bpm.saturating_sub(BPM_STEP)
                };
                self.cfg.bpm = b.clamp(MIN_BPM, MAX_BPM);
                self.setup_changed();
                Self::handled()
            }
            "editor.trainer_pair" => {
                let n = self.set.len();
                if n < 2 {
                    return Self::refuse("Pick at least two chords for the drill");
                }
                let (a, b) = self.pair;
                self.pair = match arg {
                    Some("a") => {
                        let mut a = (a + 1) % n;
                        if a == b {
                            a = (a + 1) % n;
                        }
                        (a, b)
                    }
                    Some("b") => {
                        let mut b = (b + 1) % n;
                        if b == a {
                            b = (b + 1) % n;
                        }
                        (a, b)
                    }
                    _ => return Self::refuse("Unknown pair change"),
                };
                self.setup_changed();
                Self::handled()
            }
            "editor.trainer_voicing" => {
                let delta = match arg {
                    Some("next") => 1,
                    Some("prev") => -1,
                    _ => return Self::refuse("Unknown fingering change"),
                };
                if self.cycle_voicing(delta) {
                    Self::handled()
                } else {
                    Self::refuse("No fingering found for this chord")
                }
            }
            "editor.trainer_reset_scores" => {
                if !self.scores.is_empty() {
                    self.scores.clear();
                    self.scores_changed = true;
                }
                Self::handled()
            }
            _ => CommandOutcome::Declined,
        }
    }

    // ------------------------------------------------------------------ list editor

    fn open_list_editor(&mut self) {
        let mut field = TextField::new(&self.list_text, LIST_LIMIT, false);
        field.select_all();
        self.list_edit = Some(ListEdit {
            field,
            dragging: false,
        });
        self.picker = false;
    }

    fn apply_list(&mut self) {
        let Some(e) = self.list_edit.take() else {
            return;
        };
        self.list_text = e.field.text().to_owned();
        self.source = Source::List;
        self.chords_changed();
        if !self.invalid.is_empty() {
            self.status = Some(format!("Not chords: {}", self.invalid.join(" ")));
        }
    }

    /// The words of the text in the field that are not chords (live, while typing).
    pub(crate) fn typed_invalid(&self) -> Vec<String> {
        self.list_edit
            .as_ref()
            .map(|e| trainer::parse_chord_list(e.field.text()).invalid)
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------ layout

    /// The toolbar's items placed in rows, and the panels below it.
    pub(crate) fn frame(&self) -> Frame {
        let b = self.bounds;
        // (button, text, lit, enabled, the user's own words)
        type Entry = (Option<TrainerButton>, String, bool, bool, bool);
        let mut e: Vec<Entry> = Vec::new();
        fn add(v: &mut Vec<Entry>, btn: Option<TrainerButton>, text: &str, lit: bool, on: bool) {
            v.push((btn, text.to_owned(), lit, on, false));
        }
        fn chip(v: &mut Vec<Entry>, btn: TrainerButton, text: &str, lit: bool) {
            v.push((Some(btn), text.to_owned(), lit, true, true));
        }
        use TrainerButton as B;
        let running = self.running();
        if self.list_edit.is_some() {
            add(&mut e, None, "Typing a list of chords", false, true);
            add(&mut e, Some(B::ApplyList), "Apply", false, true);
            add(&mut e, Some(B::CancelList), "Cancel", false, true);
        } else {
            chip(
                &mut e,
                B::Instrument,
                self.instrument_def().name,
                self.picker,
            );
            chip(&mut e, B::Tuning, self.tuning_preset().name, false);
            add(&mut e, None, "Capo", false, true);
            add(&mut e, Some(B::CapoDown), "-", false, self.capo > 0);
            let capo = match self.capo {
                0 => "off".to_owned(),
                n => n.to_string(),
            };
            add(&mut e, None, &capo, self.capo != 0, true);
            add(&mut e, Some(B::CapoUp), "+", false, self.capo < MAX_CAPO);
            add(&mut e, None, "Chords", false, true);
            let source = match self.source {
                Source::Key => "By Key",
                Source::List => "Typed List",
                Source::Song => "The Song",
            };
            add(&mut e, Some(B::Source), source, true, true);
            match self.source {
                Source::Key => {
                    add(&mut e, Some(B::KeyDown), "<", false, true);
                    let key = trainer::key_of(self.key_root, self.minor).name();
                    e.push((None, key, false, true, true));
                    add(&mut e, Some(B::KeyUp), ">", false, true);
                    let mode = if self.minor { "minor" } else { "major" };
                    add(&mut e, Some(B::KeyMode), mode, false, true);
                    add(&mut e, Some(B::Sevenths), "7ths", self.sevenths, true);
                }
                Source::List => {
                    add(&mut e, Some(B::EditList), "Edit List...", false, true);
                    if !self.invalid.is_empty() {
                        let t = crate::i18n::tf(
                            "Not chords: {words}",
                            &[("words", &self.invalid.join(" "))],
                        );
                        e.push((None, t, true, true, true));
                    }
                }
                Source::Song => {
                    if self.set.is_empty() {
                        add(&mut e, None, "The song has no chords yet", false, true);
                    }
                }
            }
            add(&mut e, None, "Drill", false, true);
            let mode = match self.cfg.mode {
                DrillMode::Timed => "Timed",
                DrillMode::Verified => "Timed + Listen",
                DrillMode::Listen => "Listen",
            };
            add(&mut e, Some(B::Mode), mode, true, true);
            let order = match self.cfg.order {
                Order::Sequential => "In Order",
                Order::Random => "Random",
                Order::Pair => "One Pair",
            };
            add(&mut e, Some(B::Order), order, false, true);
            if self.cfg.order == Order::Pair && self.set.len() > 1 {
                let name = |i: usize| self.set.get(i).map(ChordSymbol::name).unwrap_or_default();
                chip(&mut e, B::PairA, &name(self.pair.0), false);
                add(&mut e, None, "and", false, true);
                chip(&mut e, B::PairB, &name(self.pair.1), false);
            }
            if self.cfg.mode != DrillMode::Listen {
                add(&mut e, None, "Beats", false, true);
                add(
                    &mut e,
                    Some(B::BeatsDown),
                    "-",
                    false,
                    self.cfg.beats > MIN_BEATS,
                );
                add(&mut e, None, &self.cfg.beats.to_string(), false, true);
                add(
                    &mut e,
                    Some(B::BeatsUp),
                    "+",
                    false,
                    self.cfg.beats < MAX_BEATS,
                );
                add(&mut e, None, "BPM", false, true);
                add(&mut e, Some(B::BpmDown), "-", false, self.cfg.bpm > MIN_BPM);
                add(&mut e, None, &self.cfg.bpm.to_string(), false, true);
                add(&mut e, Some(B::BpmUp), "+", false, self.cfg.bpm < MAX_BPM);
            }
            if self.listens() {
                add(&mut e, None, "Input", false, true);
                match &self.input {
                    Some(l) => chip(&mut e, B::Input, l, false),
                    None => add(&mut e, Some(B::Input), "none", false, true),
                }
            }
            let go = if running { "Stop" } else { "Start" };
            add(&mut e, Some(B::Start), go, running, self.set.len() > 1);
            let skip_on = running && self.cfg.mode == DrillMode::Listen;
            add(&mut e, Some(B::Skip), "Skip", false, skip_on);
        }
        // Flow.
        let pad = 8.0;
        let gap = 4.0;
        let mut x = b.x0 + pad;
        let mut row = 0usize;
        let mut items = Vec::new();
        for (btn, text, lit, on, verbatim) in e {
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
            let y0 = b.y0 + 3.0 + row as f64 * BAR_ROW;
            let rect = Rect::new(x, y0, x + w, y0 + BAR_ROW - 4.0);
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
        let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + (row + 1) as f64 * BAR_ROW + 4.0);
        let strip_h =
            crate::chord_diagram::diagram_height(STRIP_DIAGRAM_WIDTH, true) + 2.0 * STRIP_PAD;
        let strip = Rect::new(b.x0, bar.y1, b.x1, (bar.y1 + strip_h).min(b.y1.max(bar.y1)));
        let body_top = strip.y1;
        let scores_w = (b.width() * 0.34).clamp(190.0, 300.0).min(b.width() * 0.5);
        let split = b.x1 - scores_w;
        let stage = Rect::new(b.x0, body_top, split, b.y1.max(body_top));
        let scores = Rect::new(split, body_top, b.x1, b.y1.max(body_top));
        Frame {
            bar,
            items,
            strip,
            stage,
            scores,
        }
    }

    /// The rectangle of diagram `index` of the strip.
    pub(crate) fn strip_cell(&self, strip: Rect, index: usize) -> Rect {
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

    fn max_strip_scroll(&self) -> f64 {
        let n = self.set.len() as f64;
        let w = 16.0 + n * (STRIP_DIAGRAM_WIDTH + STRIP_GAP) - STRIP_GAP;
        (w - self.bounds.width()).max(0.0)
    }

    // ------------------------------------------------------------------ help

    /// The tooltip of the control under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let f = self.frame();
        if f.scores.contains(pos) && paint::reset_rect(f.scores).contains(pos) {
            return Some(
                Help::new(
                    paint::reset_rect(f.scores),
                    "Forget the best scores of every chord pair",
                )
                .with_command("editor.trainer_reset_scores"),
            );
        }
        let item = f
            .items
            .iter()
            .find(|i| i.btn.is_some() && i.rect.contains(pos))?;
        let btn = item.btn?;
        use TrainerButton as B;
        let (text, cmd, arg): (&'static str, Option<&'static str>, Option<&'static str>) = match btn
        {
            B::Instrument => (
                "Instrument of the diagrams",
                Some("editor.trainer_instrument"),
                None,
            ),
            B::Tuning => (
                "Tuning of the diagrams (click for the next)",
                Some("editor.trainer_tuning"),
                None,
            ),
            B::CapoDown => (
                "Capo one fret lower",
                Some("editor.trainer_capo"),
                Some("down"),
            ),
            B::CapoUp => (
                "Capo one fret higher",
                Some("editor.trainer_capo"),
                Some("up"),
            ),
            B::Source => (
                "Where the chords come from: a key, a typed list or the song",
                None,
                None,
            ),
            B::KeyDown => (
                "Key a semitone lower",
                Some("editor.trainer_key"),
                Some("down"),
            ),
            B::KeyUp => (
                "Key a semitone higher",
                Some("editor.trainer_key"),
                Some("up"),
            ),
            B::KeyMode => ("Major or minor key", None, None),
            B::Sevenths => (
                "Seventh chords instead of triads",
                Some("editor.trainer_sevenths"),
                None,
            ),
            B::EditList => (
                "Type the chords of the drill",
                Some("editor.trainer_edit_list"),
                None,
            ),
            B::Mode => (
                "Drill mode: timed, timed and checked by the input, or wait for each chord",
                None,
                None,
            ),
            B::Order => (
                "Order of the chords: in order, random or one pair",
                None,
                None,
            ),
            B::BeatsDown => (
                "Fewer beats on each chord",
                Some("editor.trainer_beats"),
                Some("down"),
            ),
            B::BeatsUp => (
                "More beats on each chord",
                Some("editor.trainer_beats"),
                Some("up"),
            ),
            B::BpmDown => ("Slower tempo", Some("editor.trainer_bpm"), Some("down")),
            B::BpmUp => ("Faster tempo", Some("editor.trainer_bpm"), Some("up")),
            B::PairA => ("First chord of the pair (click for the next)", None, None),
            B::PairB => ("Second chord of the pair (click for the next)", None, None),
            B::Start => (
                "Start or stop the drill",
                Some("editor.trainer_start"),
                None,
            ),
            B::Skip => (
                "Give up this chord and go on (listen drill)",
                Some("editor.trainer_skip"),
                None,
            ),
            B::Input => (
                "Audio input the drill listens to (click for the next)",
                Some("practice.listen_input"),
                None,
            ),
            B::ResetScores => ("Forget the best scores of every chord pair", None, None),
            B::VoicingPrev => ("Previous fingering", None, None),
            B::VoicingNext => ("Next fingering", None, None),
            B::ApplyList => ("Make the text the list of chords (Enter)", None, None),
            B::CancelList => ("Leave the list as it was (Esc)", None, None),
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
    pub fn handle(
        &mut self,
        event: &UiEvent,
        _doc: &RackDoc,
        _registry: &Registry,
    ) -> Vec<UiAction> {
        let mut out = Vec::new();
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return out;
        }
        if self.picker {
            self.picker_event(event);
            return out;
        }
        if self.list_edit.is_some() {
            self.text_event(event, &mut out);
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, .. } => {
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
            } => self.pointer_down(*pos, &mut out),
            UiEvent::Wheel { delta, pos, .. } => {
                let f = self.frame();
                if f.strip.contains(*pos) {
                    let d = if delta.x.abs() > 1e-9 {
                        delta.x
                    } else {
                        delta.y
                    };
                    self.strip_scroll =
                        (self.strip_scroll - d * SCROLL_STEP).clamp(0.0, self.max_strip_scroll());
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if !modifiers.command() && !modifiers.alt => match key {
                Key::Right => {
                    self.cycle_voicing(1);
                }
                Key::Left => {
                    self.cycle_voicing(-1);
                }
                Key::Escape if self.running() => {
                    self.pending_start = false;
                    self.stop_now();
                }
                _ => {}
            },
            _ => {}
        }
        out
    }

    fn pointer_down(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        self.status = None;
        let f = self.frame();
        if let Some(btn) = f
            .items
            .iter()
            .find(|i| i.btn.is_some() && i.on && i.rect.contains(pos))
            .and_then(|i| i.btn)
        {
            self.press(btn, out);
            return;
        }
        if paint::reset_rect(f.scores).contains(pos) {
            self.press(TrainerButton::ResetScores, out);
            return;
        }
        if f.strip.contains(pos) {
            for i in 0..self.set.len() {
                if self.strip_cell(f.strip, i).contains(pos) {
                    if self.selected == i {
                        self.cycle_voicing(1);
                    } else {
                        self.selected = i;
                    }
                    self.repaint = true;
                    return;
                }
            }
        }
        if let Some((prev, next)) = paint::voicing_buttons(&f.stage) {
            if prev.contains(pos) {
                self.cycle_voicing(-1);
            } else if next.contains(pos) {
                self.cycle_voicing(1);
            }
        }
    }

    // ------------------------------------------------------------------ instrument list

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
                let (panel, rows) = crate::chord_sheet::paint::picker_layout(self.bounds);
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

    // ------------------------------------------------------------------ list text events

    fn text_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match event {
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Ime(ime) => self.field_ime(ime),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => match key {
                Key::Escape => self.list_edit = None,
                Key::Enter => self.apply_list(),
                _ => {
                    if let Some(e) = &mut self.list_edit {
                        e.field.handle_key(key, *modifiers);
                    }
                }
            },
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let f = self.frame();
                if let Some(btn) = f
                    .items
                    .iter()
                    .find(|i| i.btn.is_some() && i.rect.contains(*pos))
                    .and_then(|i| i.btn)
                {
                    self.press(btn, out);
                    return;
                }
                self.field_pointer(event);
            }
            _ => self.field_pointer(event),
        }
    }

    fn field_ime(&mut self, ime: &ImeEvent) {
        if let Some(e) = &mut self.list_edit {
            e.field.handle_ime(ime);
        }
    }

    fn field_pointer(&mut self, event: &UiEvent) {
        if let Some(e) = &mut self.list_edit {
            e.field.pointer_event(&mut e.dragging, event);
        }
    }

    // ------------------------------------------------------------------ paint

    /// Paints the pane.
    pub fn paint(
        &mut self,
        canvas: &mut dyn crate::canvas::Canvas,
        _doc: &RackDoc,
        _registry: &Registry,
    ) {
        self.paint_all(canvas);
        self.repaint = false;
    }
}
