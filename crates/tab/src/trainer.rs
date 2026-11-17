//! The logic of the chord trainer, pure and clock-free.
//!
//! The trainer asks the player to change between chords: one chord every `beats` beats at a tempo
//! (timed), or each chord until the audio input recognises it (listen). This module holds what
//! needs no window and no audio device:
//!
//! * **Chord sets**: the diatonic triads or sevenths of a key ([`diatonic`]) and a typed list of
//!   chord names ([`parse_chord_list`], invalid names reported, never dropped silently).
//! * **The drill** ([`Drill`]): a state machine driven by a clock the caller supplies (`now` in
//!   seconds) and by what the input heard ([`Heard`]: a time and a chroma vector). It emits the
//!   clicks and the chord changes ([`DrillEvent`]), orders the chords (sequential, random, one pair
//!   back and forth) and judges every change.
//! * **Scores** ([`Scores`]): the best changes per minute and hit rate per chord pair, updated from
//!   the results of a session ([`Drill::pair_results`]).
//!
//! # Judging
//!
//! A chord is recognised when [`chord_score`] of the heard chroma against the pitch classes of the
//! chosen voicing reaches the threshold. In the verified timed mode a change is a hit when its chord
//! is recognised from just before its beat to the end of its window; the window is judged a little
//! after it ends, so a late note of the input tracker still counts. In the listen mode the drill
//! waits for the chord: it moves on at the first recognition after the chord was shown, and a
//! skipped chord is a miss. A plain timed drill verifies nothing and records no result.

use crate::chord::{ChordSymbol, Extension, Quality};
use crate::grade::{DEFAULT_CHORD_THRESHOLD, chord_score};
use crate::transpose::Key;
use std::collections::BTreeMap;

/// The slowest tempo, beats per minute.
pub const MIN_BPM: u16 = 30;
/// The fastest tempo.
pub const MAX_BPM: u16 = 240;
/// The tempo a drill starts with.
pub const DEFAULT_BPM: u16 = 60;
/// The fewest beats a chord lasts.
pub const MIN_BEATS: u8 = 1;
/// The most beats a chord lasts.
pub const MAX_BEATS: u8 = 16;
/// The beats a chord lasts by default.
pub const DEFAULT_BEATS: u8 = 4;
/// The most chords of a set.
pub const MAX_CHORDS: usize = 24;
/// The fewest judged changes of a pair that make a session count for its score.
pub const MIN_ATTEMPTS: u32 = 3;
/// The hit rate a session needs for its changes per minute to count as the pair's best speed.
pub const CLEAN_RATE: f32 = 0.75;
/// Seconds after a window's end a late note may still be heard for it (the tracker's delay).
const SETTLE: f64 = 0.5;
/// How long what was heard is kept for windows that open after it, seconds.
const RECENT: f64 = 1.0;
/// The longest a window's early tolerance is, seconds.
const MAX_TOLERANCE: f64 = 0.25;

// ----------------------------------------------------------------------------- chord sets

/// The key whose tonic has pitch class `pc`, spelled the usual way (fewest accidentals).
pub fn key_of(pc: u8, minor: bool) -> Key {
    const MAJOR: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ];
    const MINOR: [&str; 12] = [
        "Cm", "C#m", "Dm", "Ebm", "Em", "Fm", "F#m", "Gm", "G#m", "Am", "Bbm", "Bm",
    ];
    let name = if minor { MINOR } else { MAJOR }[usize::from(pc % 12)];
    Key::parse(name).unwrap_or(Key::new(crate::chord::Note::new(0, 0), minor))
}

/// The seven diatonic chords of `key`, triads or (with `sevenths`) seventh chords, from the tonic
/// up. Natural minor is used for a minor key.
pub fn diatonic(key: Key, sevenths: bool) -> Vec<ChordSymbol> {
    const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
    let steps = if key.minor { MINOR } else { MAJOR };
    let scale = key.scale();
    let tonic = i32::from(key.tonic.pitch_class());
    (0..7)
        .map(|i| {
            let above = |k: usize| (steps[(i + k) % 7] - steps[i]).rem_euclid(12);
            let (third, fifth, seventh) = (above(2), above(4), above(6));
            let quality = match (third, fifth) {
                (3, 7) => Quality::Minor,
                (3, 6) => Quality::Dim,
                (4, 8) => Quality::Aug,
                _ => Quality::Major,
            };
            let extension = match (sevenths, quality, seventh) {
                (false, ..) => Extension::None,
                (true, Quality::Major, 11) | (true, Quality::Aug, 11) => Extension::MajSeventh,
                _ => Extension::Seventh,
            };
            let quality = if quality == Quality::Dim && sevenths && seventh == 10 {
                Quality::HalfDim
            } else {
                quality
            };
            let pc = (tonic + steps[i]).rem_euclid(12) as u8;
            let root = match scale {
                Some(s) => s[i],
                None => crate::transpose::spell(pc, crate::transpose::Spelling::Key(key)),
            };
            ChordSymbol::new(root, quality, extension)
        })
        .collect()
}

/// A typed list of chords.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChordList {
    /// The chords read, in order, each once.
    pub chords: Vec<ChordSymbol>,
    /// The words that are not chords (at most a few, in order).
    pub invalid: Vec<String>,
}

/// Reads chord names separated by spaces, commas, semicolons, bars or line breaks. A repeated chord
/// counts once; more than [`MAX_CHORDS`] chords are cut off.
pub fn parse_chord_list(text: &str) -> ChordList {
    let mut out = ChordList::default();
    let words = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '|'))
        .filter(|w| !w.is_empty());
    for word in words {
        match ChordSymbol::parse(word) {
            Ok(c) => {
                if out.chords.len() < MAX_CHORDS && !out.chords.iter().any(|k| k.name() == c.name())
                {
                    out.chords.push(c);
                }
            }
            Err(_) if out.invalid.len() < 8 => {
                out.invalid.push(word.chars().take(24).collect());
            }
            Err(_) => {}
        }
    }
    out
}

// ----------------------------------------------------------------------------- the drill

/// The order the chords come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// The set from the first chord to the last, then again.
    #[default]
    Sequential,
    /// Any chord but the one just played.
    Random,
    /// One pair, back and forth.
    Pair,
}

/// How a drill moves on and what it checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrillMode {
    /// A chord every few beats at a tempo; nothing is checked.
    #[default]
    Timed,
    /// As timed, and the input must hear each chord within its window.
    Verified,
    /// Each chord stays until the input hears it.
    Listen,
}

/// The settings of a drill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrillConfig {
    /// How it moves on.
    pub mode: DrillMode,
    /// The order of the chords.
    pub order: Order,
    /// Beats per minute ([`MIN_BPM`] to [`MAX_BPM`]).
    pub bpm: u16,
    /// Beats per chord ([`MIN_BEATS`] to [`MAX_BEATS`]).
    pub beats: u8,
    /// The [`chord_score`] a chord must reach to count as heard.
    pub threshold: f32,
}

impl Default for DrillConfig {
    fn default() -> Self {
        Self {
            mode: DrillMode::Timed,
            order: Order::Sequential,
            bpm: DEFAULT_BPM,
            beats: DEFAULT_BEATS,
            threshold: DEFAULT_CHORD_THRESHOLD,
        }
    }
}

impl DrillConfig {
    /// Seconds per beat.
    pub fn beat_seconds(&self) -> f64 {
        60.0 / f64::from(self.bpm.clamp(MIN_BPM, MAX_BPM))
    }

    /// Seconds per chord window in the timed modes.
    pub fn window_seconds(&self) -> f64 {
        self.beat_seconds() * f64::from(self.beats.clamp(MIN_BEATS, MAX_BEATS))
    }
}

/// What the input heard: a moment on the drill's clock and the chroma there (largest value 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    /// Seconds, on the clock of [`Drill::start`] and [`Drill::advance`].
    pub time: f64,
    /// Energy per pitch class, C first.
    pub chroma: [f32; 12],
}

/// What happened while the clock advanced.
#[derive(Debug, Clone, PartialEq)]
pub enum DrillEvent {
    /// A beat of the click; the accent opens a chord.
    Click {
        /// The first beat of a chord's window.
        accent: bool,
    },
    /// The chord changed (indexes into the set).
    Changed {
        /// The chord that was shown.
        from: usize,
        /// The chord now shown.
        to: usize,
    },
    /// A change was judged.
    Judged {
        /// The chord changed from.
        from: usize,
        /// The chord changed to.
        to: usize,
        /// Whether it was heard.
        hit: bool,
    },
}

/// The result of one judged change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    /// The chord changed from.
    pub from: usize,
    /// The chord changed to.
    pub to: usize,
    /// Whether it was heard.
    pub hit: bool,
    /// Seconds from showing the chord to hearing it (listen mode).
    pub seconds: Option<f64>,
}

/// A timed window still open for its chord to be heard.
#[derive(Debug, Clone, Copy)]
struct Window {
    from: Option<usize>,
    chord: usize,
    start: f64,
    end: f64,
    hit: bool,
}

/// A running (or finished) chord drill.
#[derive(Debug, Clone)]
pub struct Drill {
    names: Vec<String>,
    masks: Vec<u16>,
    cfg: DrillConfig,
    pair: (usize, usize),
    rng: u64,
    running: bool,
    t0: f64,
    stopped: f64,
    current: usize,
    next: usize,
    prev: Option<usize>,
    /// Beats emitted so far (timed modes).
    beat: u64,
    /// When the current chord was shown.
    shown_at: f64,
    open: Vec<Window>,
    outcomes: Vec<Outcome>,
    changes: u32,
    recognised: bool,
    /// What was heard lately, so a window that opens just after a note still sees it.
    recent: Vec<Heard>,
}

impl Drill {
    /// A drill over `names` (the chords' names) whose pitch-class masks are `masks` (what each is
    /// heard as; see [`crate::grade::pc_mask`]). `pair` is the two chords of [`Order::Pair`]
    /// (clamped and made different when it can be); `seed` starts the random order.
    pub fn new(
        names: Vec<String>,
        masks: Vec<u16>,
        cfg: DrillConfig,
        pair: (usize, usize),
        seed: u64,
    ) -> Self {
        let n = names.len();
        let mut masks = masks;
        masks.resize(n, 0);
        let a = pair.0.min(n.saturating_sub(1));
        let mut b = pair.1.min(n.saturating_sub(1));
        if a == b && n > 1 {
            b = (a + 1) % n;
        }
        let mut d = Self {
            names,
            masks,
            cfg,
            pair: (a, b),
            rng: seed | 1,
            running: false,
            t0: 0.0,
            stopped: 0.0,
            current: 0,
            next: 0,
            prev: None,
            beat: 0,
            shown_at: 0.0,
            open: Vec::new(),
            outcomes: Vec::new(),
            changes: 0,
            recognised: false,
            recent: Vec::new(),
        };
        d.reset_order();
        d
    }

    fn reset_order(&mut self) {
        let n = self.names.len();
        self.current = match self.cfg.order {
            Order::Sequential => 0,
            Order::Random => self.random(n),
            Order::Pair => self.pair.0,
        };
        self.next = self.choose_next();
    }

    fn random(&mut self, n: usize) -> usize {
        // xorshift64*: small, deterministic for a seed.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        if n == 0 {
            return 0;
        }
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n
    }

    fn choose_next(&mut self) -> usize {
        let n = self.names.len();
        if n < 2 {
            return self.current;
        }
        match self.cfg.order {
            Order::Sequential => (self.current + 1) % n,
            Order::Random => {
                // Any chord but the current one.
                let k = self.random(n - 1);
                if k >= self.current { k + 1 } else { k }
            }
            Order::Pair => {
                if self.current == self.pair.0 {
                    self.pair.1
                } else {
                    self.pair.0
                }
            }
        }
    }

    /// The settings.
    pub fn config(&self) -> &DrillConfig {
        &self.cfg
    }

    /// The names of the chords.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether the drill runs.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The chord to play now.
    pub fn current(&self) -> usize {
        self.current
    }

    /// The chord after it.
    pub fn next(&self) -> usize {
        self.next
    }

    /// The chord before the current one, once there has been one.
    pub fn previous(&self) -> Option<usize> {
        self.prev
    }

    /// Whether the current chord is being heard (the indicator of the listening modes).
    pub fn recognised(&self) -> bool {
        self.recognised
    }

    /// The chord changes so far (in the listen mode, the chords heard).
    pub fn changes(&self) -> u32 {
        self.changes
    }

    /// The judged changes so far (verified and listen modes).
    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    /// Hits and judged changes so far.
    pub fn hits(&self) -> (u32, u32) {
        let hits = self.outcomes.iter().filter(|o| o.hit).count() as u32;
        (hits, self.outcomes.len() as u32)
    }

    /// Seconds since the start (frozen at the stop).
    pub fn elapsed(&self, now: f64) -> f64 {
        if self.running {
            (now - self.t0).max(0.0)
        } else {
            self.stopped
        }
    }

    /// Chord changes per minute over the session so far.
    pub fn changes_per_minute(&self, now: f64) -> f32 {
        let minutes = self.elapsed(now) / 60.0;
        if minutes < 1e-6 {
            return 0.0;
        }
        (f64::from(self.changes) / minutes) as f32
    }

    /// The beat of the current chord's window, `0..beats` (timed modes).
    pub fn beat_in_chord(&self) -> u8 {
        if self.beat == 0 {
            return 0;
        }
        ((self.beat - 1) % u64::from(self.cfg.beats.max(1))) as u8
    }

    /// Whether the chord sounded when `chroma` was heard.
    fn is_chord(&self, chord: usize, chroma: &[f32; 12]) -> bool {
        let mask = self.masks.get(chord).copied().unwrap_or(0);
        chord_score(chroma, mask) >= self.cfg.threshold
    }

    /// The early tolerance of a window: half a beat, at most a quarter second.
    fn tolerance(&self) -> f64 {
        (self.cfg.beat_seconds() * 0.5).min(MAX_TOLERANCE)
    }

    /// Starts the drill at `now`: the first chord shows, the first click is at `now`.
    pub fn start(&mut self, now: f64) {
        if self.names.len() < 2 {
            return;
        }
        self.running = true;
        self.t0 = now;
        self.stopped = 0.0;
        self.beat = 0;
        self.shown_at = now;
        self.prev = None;
        self.open.clear();
        self.recent.clear();
        self.outcomes.clear();
        self.changes = 0;
        self.recognised = false;
        self.reset_order();
    }

    /// Stops the drill at `now`; windows already over are judged.
    pub fn stop(&mut self, now: f64) {
        if !self.running {
            return;
        }
        self.settle(now, true);
        self.stopped = (now - self.t0).max(0.0);
        self.running = false;
        self.open.clear();
    }

    fn move_on(&mut self, shown_at: f64) -> (usize, usize) {
        let from = self.current;
        self.prev = Some(from);
        self.current = self.next;
        self.next = self.choose_next();
        self.shown_at = shown_at;
        self.recognised = false;
        (from, self.current)
    }

    /// Moves the clock to `now`: the beats and chord changes since the last call.
    pub fn advance(&mut self, now: f64) -> Vec<DrillEvent> {
        let mut events = Vec::new();
        if !self.running || self.cfg.mode == DrillMode::Listen {
            return events;
        }
        let dur = self.cfg.beat_seconds();
        let per = u64::from(self.cfg.beats.clamp(MIN_BEATS, MAX_BEATS));
        loop {
            let at = self.t0 + self.beat as f64 * dur;
            if at > now {
                break;
            }
            let accent = self.beat.is_multiple_of(per);
            events.push(DrillEvent::Click { accent });
            if accent {
                let from = (self.beat > 0).then(|| self.move_on(at));
                if let Some((f, t)) = from {
                    self.changes += 1;
                    events.push(DrillEvent::Changed { from: f, to: t });
                }
                if self.cfg.mode == DrillMode::Verified {
                    let mut w = Window {
                        from: from.map(|(f, _)| f),
                        chord: self.current,
                        start: at,
                        end: at + dur * per as f64,
                        hit: false,
                    };
                    // A note played just before the beat may have been heard already.
                    w.hit = self.recent.iter().any(|h| self.in_window(&w, h));
                    self.open.push(w);
                }
            }
            self.beat += 1;
        }
        events.extend(self.settle(now, false));
        events
    }

    /// Whether `h` is the chord of window `w` heard in time for it.
    fn in_window(&self, w: &Window, h: &Heard) -> bool {
        h.time >= w.start - self.tolerance() && h.time <= w.end && self.is_chord(w.chord, &h.chroma)
    }

    /// Judges the windows whose time (and the tracker's delay) is over.
    fn settle(&mut self, now: f64, stopping: bool) -> Vec<DrillEvent> {
        let mut events = Vec::new();
        let mut keep = Vec::new();
        for w in std::mem::take(&mut self.open) {
            let due = if stopping {
                w.end <= now
            } else {
                w.end + SETTLE <= now
            };
            if !due {
                keep.push(w);
                continue;
            }
            if let Some(from) = w.from {
                self.outcomes.push(Outcome {
                    from,
                    to: w.chord,
                    hit: w.hit,
                    seconds: None,
                });
                events.push(DrillEvent::Judged {
                    from,
                    to: w.chord,
                    hit: w.hit,
                });
            }
        }
        self.open = keep;
        events
    }

    /// Takes what the input heard. In the verified mode it marks the windows whose chord it is; in
    /// the listen mode a chord heard after it was shown moves the drill on.
    pub fn hear(&mut self, h: &Heard) -> Vec<DrillEvent> {
        let mut events = Vec::new();
        if !self.running {
            return events;
        }
        if self.is_chord(self.current, &h.chroma) && h.time >= self.shown_at {
            self.recognised = true;
        }
        match self.cfg.mode {
            DrillMode::Timed => {}
            DrillMode::Verified => {
                self.recent.retain(|r| r.time > h.time - RECENT);
                self.recent.push(h.clone());
                let wanted: Vec<usize> = self
                    .open
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| self.in_window(w, h))
                    .map(|(i, _)| i)
                    .collect();
                for i in wanted {
                    self.open[i].hit = true;
                }
            }
            DrillMode::Listen => {
                if h.time >= self.shown_at && self.is_chord(self.current, &h.chroma) {
                    let seconds = h.time - self.shown_at;
                    let (from, to) = (self.prev, self.current);
                    self.changes += 1;
                    self.record_listen(from, to, true, Some(seconds));
                    let (f, t) = self.move_on(h.time);
                    events.push(DrillEvent::Changed { from: f, to: t });
                    self.recognised = false;
                }
            }
        }
        events
    }

    fn record_listen(&mut self, from: Option<usize>, to: usize, hit: bool, seconds: Option<f64>) {
        if let Some(from) = from {
            self.outcomes.push(Outcome {
                from,
                to,
                hit,
                seconds,
            });
        }
    }

    /// Gives up the chord now shown (listen mode): a miss for the change into it, and on to the
    /// next. False in the timed modes, which keep their own time.
    pub fn skip(&mut self, now: f64) -> bool {
        if !self.running || self.cfg.mode != DrillMode::Listen {
            return false;
        }
        let (from, to) = (self.prev, self.current);
        self.record_listen(from, to, false, None);
        self.move_on(now);
        true
    }

    /// The results of the session per chord pair (the unordered pair, both directions together),
    /// for [`Scores::record_all`]. The timed mode judges nothing and gives none.
    pub fn pair_results(&self) -> Vec<PairResult> {
        let mut by: BTreeMap<String, (u32, u32, f64, u32)> = BTreeMap::new();
        for o in &self.outcomes {
            let (Some(a), Some(b)) = (self.names.get(o.from), self.names.get(o.to)) else {
                continue;
            };
            let e = by.entry(pair_key(a, b)).or_default();
            e.0 += 1;
            if o.hit {
                e.1 += 1;
                if let Some(s) = o.seconds {
                    e.2 += s;
                    e.3 += 1;
                }
            }
        }
        by.into_iter()
            .map(|(key, (attempts, hits, secs, timed))| {
                let cpm = match self.cfg.mode {
                    DrillMode::Listen if timed > 0 && secs > 1e-3 => {
                        (60.0 * f64::from(timed) / secs) as f32
                    }
                    DrillMode::Listen => 0.0,
                    _ => (60.0 / self.cfg.window_seconds()) as f32,
                };
                PairResult {
                    key,
                    attempts,
                    hits,
                    cpm,
                }
            })
            .collect()
    }
}

// ----------------------------------------------------------------------------- scores

/// The key of an unordered chord pair, `Am|C` (names sorted).
pub fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

/// The words of a pair key for display: `Am - C`.
pub fn pair_label(key: &str) -> String {
    key.replacen('|', " - ", 1)
}

/// How a session went for one chord pair.
#[derive(Debug, Clone, PartialEq)]
pub struct PairResult {
    /// The pair ([`pair_key`]).
    pub key: String,
    /// Judged changes.
    pub attempts: u32,
    /// Changes heard.
    pub hits: u32,
    /// The speed it was played at, changes per minute.
    pub cpm: f32,
}

impl PairResult {
    /// The share of changes heard, 0 to 1.
    pub fn hit_rate(&self) -> f32 {
        if self.attempts == 0 {
            0.0
        } else {
            self.hits as f32 / self.attempts as f32
        }
    }
}

/// The best a chord pair has done.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Best {
    /// The most changes per minute reached with a hit rate of at least [`CLEAN_RATE`].
    pub cpm: f32,
    /// The best hit rate, 0 to 1.
    pub hit_rate: f32,
    /// The sessions that counted.
    pub sessions: u32,
}

/// The best scores per chord pair. Kept in the user's settings, never in a project.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scores {
    map: BTreeMap<String, Best>,
}

impl Scores {
    /// No scores.
    pub fn new() -> Self {
        Self::default()
    }

    /// The scores of `key` ([`pair_key`]).
    pub fn get(&self, key: &str) -> Option<&Best> {
        self.map.get(key)
    }

    /// Every pair with its best, ordered by name.
    pub fn rows(&self) -> impl Iterator<Item = (&str, &Best)> {
        self.map.iter().map(|(k, b)| (k.as_str(), b))
    }

    /// The number of pairs.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Scores read back from storage: values out of range are brought into it, nonsense dropped.
    pub fn from_rows(rows: impl IntoIterator<Item = (String, Best)>) -> Self {
        let mut map = BTreeMap::new();
        for (key, b) in rows.into_iter().take(512) {
            if key.is_empty() || key.len() > 120 || !b.cpm.is_finite() || !b.hit_rate.is_finite() {
                continue;
            }
            map.insert(
                key,
                Best {
                    cpm: b.cpm.clamp(0.0, 1000.0),
                    hit_rate: b.hit_rate.clamp(0.0, 1.0),
                    sessions: b.sessions.min(1_000_000),
                },
            );
        }
        Self { map }
    }

    /// Counts one session of a pair; true when a best moved. Sessions of fewer than
    /// [`MIN_ATTEMPTS`] changes are ignored.
    pub fn record(&mut self, r: &PairResult) -> bool {
        if r.attempts < MIN_ATTEMPTS {
            return false;
        }
        let rate = r.hit_rate();
        let best = self.map.entry(r.key.clone()).or_default();
        let before = *best;
        best.sessions = best.sessions.saturating_add(1);
        if rate > best.hit_rate {
            best.hit_rate = rate;
        }
        if rate >= CLEAN_RATE && r.cpm > best.cpm {
            best.cpm = r.cpm;
        }
        *best != before
    }

    /// Counts the pairs of a finished session; true when anything changed.
    pub fn record_all(&mut self, results: &[PairResult]) -> bool {
        results.iter().fold(false, |any, r| self.record(r) | any)
    }
}

#[cfg(test)]
mod tests;
