//! The fretboard: a horizontal neck for the tuning of the clip the Tab Editor shows, with the
//! notes that sound at the playhead (or sit under the tab cursor while stopped) drawn on their
//! string and fret, and the next notes ghosted.
//!
//! The neck is drawn for any string count (1 to 12) with the high string on top, as in tab, and
//! the nut on the left (mirrored for a left-handed player). Frets are spaced logarithmically
//! (each is the same ratio narrower than the last) and compressed so the whole neck, up to the
//! instrument's fret count, fits the width. It shows inlays (a pair at the octave), the capo as a
//! bar over the strings it covers, and a short string, such as the banjo's drone, starting at its
//! own fret with a peg there. Fret numbers of the tab are relative to the capo; the dots sit at
//! the capo's fret plus the tab fret.
//!
//! # Data and hosts
//!
//! [`FretboardData`] is the tab of one clip as the view needs it (tuning, capo, notes with their
//! strings and frets, chord voicings). The Tab Editor builds it and hands the view a
//! [`FretboardFeed`] (the data and the tick to show); the same view paints as a strip under the
//! staff and, in its own window, as the pane the app detaches. The view holds no options of its
//! own: the host pushes [`FretboardOptions`] and the chips of the header answer with
//! `view.fretboard_*` commands, so each is also a menu entry and a palette command.
//!
//! # What is shown
//!
//! - The notes that sound at the tick ([`live_notes`]) as filled dots, the first group of notes
//!   that starts after it as outlined ghosts.
//! - With [`FretboardOptions::chord_voicing`] on, the voicing of the chord at the tick (the last
//!   diagram of the clip at or before it) as rings, muted strings as crosses.
//! - A note carries a finger number when the chord at the tick fingers that string and fret.
//! - Each dot is labelled with the note name, the fret number or the interval to the chord's
//!   root ([`FretLabels`]); without a chord the interval is to the lowest sounding note.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{PointerButton, UiEvent};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_tab::chord::ChordSymbol;
use atomik_tab::instrument::pitch_class_name;
use atomik_tab::voicing::Voicing;
use atomik_tab::{Capo, Tuning};
use std::sync::Arc;
use vello::kurbo::{Affine, Circle, Line, Point, Rect, RoundedRect};

/// The height of the strip under the tab staff, and the height a detached window gives the neck.
pub const FRETBOARD_HEIGHT: f64 = 136.0;
/// The size the detached window opens with.
pub const DEFAULT_SIZE: (f64, f64) = (1000.0, FRETBOARD_HEIGHT);
/// The height of the row of chips above the neck.
const HEADER: f64 = 20.0;
/// The width left of the nut (string names and the open-string markers).
const GUTTER: f64 = 46.0;
/// The space right of the last fret.
const MARGIN: f64 = 12.0;
/// The height of the row of fret numbers under the neck.
const NUMBERS: f64 = 14.0;
/// The frets that carry an inlay.
const INLAYS: [u8; 15] = [3, 5, 7, 9, 12, 15, 17, 19, 21, 24, 27, 29, 31, 33, 36];

/// What the dots say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FretLabels {
    /// The note name (`A`, `C#`).
    #[default]
    Notes,
    /// The fret number as the tab writes it (relative to the capo).
    Frets,
    /// The interval to the root of the chord (`1`, `b3`, `5`).
    Intervals,
}

impl FretLabels {
    /// Every kind, in the order the chip cycles through them.
    pub const ALL: [FretLabels; 3] = [FretLabels::Notes, FretLabels::Frets, FretLabels::Intervals];

    /// The argument of `view.fretboard_labels`.
    pub fn arg(self) -> &'static str {
        match self {
            FretLabels::Notes => "notes",
            FretLabels::Frets => "frets",
            FretLabels::Intervals => "intervals",
        }
    }

    /// The kind an argument names.
    pub fn from_arg(arg: &str) -> Option<FretLabels> {
        Self::ALL.into_iter().find(|l| l.arg() == arg)
    }

    /// The kind after this one.
    pub fn next(self) -> FretLabels {
        match self {
            FretLabels::Notes => FretLabels::Frets,
            FretLabels::Frets => FretLabels::Intervals,
            FretLabels::Intervals => FretLabels::Notes,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FretLabels::Notes => "Notes",
            FretLabels::Frets => "Frets",
            FretLabels::Intervals => "Intervals",
        }
    }
}

/// How the fretboard is drawn. The host owns it and pushes it to every fretboard it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FretboardOptions {
    /// Mirror the neck: the nut on the right.
    pub left_handed: bool,
    /// What the dots say.
    pub labels: FretLabels,
    /// Show the voicing of the chord at the tick under the notes.
    pub chord_voicing: bool,
}

/// A note of the clip with the place it is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FretNote {
    /// Start, in content ticks of the clip.
    pub tick: i64,
    /// Length in ticks.
    pub length: i64,
    /// The string, in tab order (0 = top).
    pub string: u8,
    /// The fret, relative to the capo (0 = open).
    pub fret: u8,
}

/// A chord of the clip's chord lane.
#[derive(Debug, Clone, PartialEq)]
pub struct FretChord {
    /// Content tick the chord applies from.
    pub tick: i64,
    /// The chord's name.
    pub name: String,
    /// Its fingering, frets relative to the capo.
    pub voicing: Voicing,
}

/// The tab of one clip as the fretboard needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct FretboardData {
    /// The strings.
    pub tuning: Tuning,
    /// The capo.
    pub capo: Capo,
    /// The notes, sorted by start tick.
    pub notes: Vec<FretNote>,
    /// The chord lane, sorted by tick.
    pub chords: Vec<FretChord>,
}

impl FretboardData {
    /// The chord in force at `tick`: the last one that starts at or before it.
    pub fn chord_at(&self, tick: i64) -> Option<&FretChord> {
        self.chords.iter().rev().find(|c| c.tick <= tick)
    }
}

/// What a fretboard shows: the clip's data and where in it.
#[derive(Debug, Clone, Default)]
pub struct FretboardFeed {
    /// The open clip's tab (none: no tab clip is open).
    pub data: Option<Arc<FretboardData>>,
    /// The content tick to show: the playhead while playing, else the tab cursor. `None` when
    /// the playhead is outside the clip.
    pub tick: Option<i64>,
    /// The transport is running.
    pub playing: bool,
    /// The verdicts of the last graded pass by place (listen and grade): a sounding note that has
    /// one is drawn in its colour.
    pub grades: Option<Arc<Vec<crate::grading::FretGrade>>>,
}

/// The notes of a tick: what sounds, and what comes next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    /// The notes sounding at the tick.
    pub sounding: Vec<FretNote>,
    /// The first group of notes that starts after the tick (all that start together).
    pub next: Vec<FretNote>,
}

/// The notes sounding at `tick` (started at or before it and not yet over) and the next onset.
pub fn live_notes(data: &FretboardData, tick: i64) -> Live {
    let sounding: Vec<FretNote> = data
        .notes
        .iter()
        .filter(|n| n.tick <= tick && tick < n.tick + n.length.max(1))
        .copied()
        .collect();
    let upcoming = data
        .notes
        .iter()
        .filter(|n| n.tick > tick)
        .map(|n| n.tick)
        .min();
    let next = match upcoming {
        Some(t) => data.notes.iter().filter(|n| n.tick == t).copied().collect(),
        None => Vec::new(),
    };
    Live { sounding, next }
}

/// The name of the interval of `pitch` above `root` (pitch classes).
pub fn interval_name(pitch: u8, root: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "1", "b2", "2", "b3", "3", "4", "b5", "5", "b6", "6", "b7", "7",
    ];
    NAMES[usize::from((pitch % 12 + 12 - root % 12) % 12)]
}

/// A control of the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FretboardItem {
    /// Cycles what the dots say.
    Labels,
    /// Mirrors the neck.
    LeftHanded,
    /// Shows the chord voicing.
    Voicing,
}

/// The fretboard view.
pub struct FretboardView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    opts: FretboardOptions,
    feed: FretboardFeed,
    live: Live,
    hover: Option<Point>,
    repaint: bool,
}

impl FretboardView {
    /// A fretboard with nothing to show.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            scale: 1.0,
            opts: FretboardOptions::default(),
            feed: FretboardFeed::default(),
            live: Live::default(),
            hover: None,
            repaint: true,
        }
    }

    /// Switches the colours.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the view in its window (logical pixels), for a view that is a pane of its own.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The options in force.
    pub fn options(&self) -> FretboardOptions {
        self.opts
    }

    /// Sets the options; true when they changed.
    pub fn set_options(&mut self, opts: FretboardOptions) -> bool {
        let changed = self.opts != opts;
        self.opts = opts;
        self.repaint |= changed;
        changed
    }

    /// Shows `feed`; true when what is drawn changed (a repaint is due). A tick that sounds the
    /// same notes as the last does not repaint.
    pub fn set_feed(&mut self, feed: &FretboardFeed) -> bool {
        let live = match (&feed.data, feed.tick) {
            (Some(d), Some(t)) => live_notes(d, t),
            _ => Live::default(),
        };
        let same_data = match (&self.feed.data, &feed.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let chord_of = |f: &FretboardFeed| match (&f.data, f.tick) {
            (Some(d), Some(t)) => d.chords.iter().rposition(|c| c.tick <= t),
            _ => None,
        };
        let same_grades = match (&self.feed.grades, &feed.grades) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
            (None, None) => true,
            _ => false,
        };
        let changed = !same_data
            || !same_grades
            || live != self.live
            || chord_of(feed) != chord_of(&self.feed)
            || feed.playing != self.feed.playing
            || feed.tick.is_some() != self.feed.tick.is_some();
        self.feed = feed.clone();
        self.live = live;
        self.repaint |= changed;
        changed
    }

    /// What the view shows now: the notes sounding and the next ones.
    pub fn live(&self) -> &Live {
        &self.live
    }

    /// True when something changed since the last paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Marks the view painted (for a host that paints it with [`Self::paint_into`]).
    pub(crate) fn clear_repaint(&mut self) {
        self.repaint = false;
    }

    /// The strip's controls placed in `area`, right-aligned in the header.
    pub fn items(&self, area: Rect) -> Vec<(Rect, FretboardItem)> {
        let y0 = area.y0 + 2.0;
        let h = HEADER - 4.0;
        let mut x = area.x1 - 6.0;
        let mut out = Vec::new();
        for (w, item) in [
            (60.0, FretboardItem::Voicing),
            (58.0, FretboardItem::LeftHanded),
            (68.0, FretboardItem::Labels),
        ] {
            if x - w < area.x0 + 90.0 {
                break;
            }
            out.push((Rect::new(x - w, y0, x, y0 + h), item));
            x -= w + 4.0;
        }
        out
    }

    fn run(id: &str, arg: Option<&str>) -> UiAction {
        UiAction::RunCommand {
            id: id.to_owned(),
            arg: arg.map(str::to_owned),
        }
    }

    /// The tooltip of the control at `pos` in the strip drawn in `area`.
    pub fn help_in(&self, area: Rect, pos: Point) -> Option<Help> {
        let (rect, item) = self
            .items(area)
            .into_iter()
            .find(|(r, _)| r.contains(pos))?;
        Some(match item {
            FretboardItem::Labels => Help::new(
                rect,
                "What the dots say: note names, fret numbers or intervals (click to change)",
            )
            .with_command("view.fretboard_labels")
            .id("editor.fretboard.labels"),
            FretboardItem::LeftHanded => {
                Help::new(rect, "Mirror the neck for a left-handed player")
                    .with_command("view.fretboard_left_handed")
                    .id("editor.fretboard.left_handed")
            }
            FretboardItem::Voicing => {
                Help::new(rect, "Show the fingering of the chord at the playhead")
                    .with_command("view.fretboard_voicing")
                    .id("editor.fretboard.voicing")
            }
        })
    }

    /// The tooltip of the control at `pos` of a view that is a pane of its own.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        self.help_in(self.bounds, pos)
    }

    /// Handles an event of a view that is a pane of its own.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return Vec::new();
        }
        self.handle_in(event, self.bounds)
    }

    /// Handles an event of the strip drawn in `area` (the pointer positions are in the same
    /// units as `area`).
    pub fn handle_in(&mut self, event: &UiEvent, area: Rect) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let over = |p: Option<Point>| {
                    p.and_then(|p| self.items(area).into_iter().find(|(r, _)| r.contains(p)))
                        .map(|(_, i)| i)
                };
                if over(self.hover) != over(Some(*pos)) {
                    self.repaint = true;
                }
                self.hover = Some(*pos);
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint = true;
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if let Some((_, item)) =
                    self.items(area).into_iter().find(|(r, _)| r.contains(*pos))
                {
                    match item {
                        FretboardItem::Labels => out.push(Self::run(
                            "view.fretboard_labels",
                            Some(self.opts.labels.next().arg()),
                        )),
                        FretboardItem::LeftHanded => {
                            out.push(Self::run("view.fretboard_left_handed", None));
                        }
                        FretboardItem::Voicing => {
                            out.push(Self::run("view.fretboard_voicing", None));
                        }
                    }
                    self.repaint = true;
                }
            }
            _ => {}
        }
        out
    }

    /// Paints a view that is a pane of its own, in physical pixels.
    pub fn paint(&mut self, canvas: &mut dyn Canvas) {
        canvas.push_transform(Affine::scale(self.scale));
        canvas.push_clip(&self.bounds.into());
        self.paint_into(canvas, self.bounds);
        canvas.pop_clip();
        canvas.pop_transform();
        self.repaint = false;
    }

    /// Paints the strip into `area` of the canvas's current (logical) units.
    pub fn paint_into(&self, c: &mut dyn Canvas, area: Rect) {
        let t = &self.st.base;
        sp::rect(c, area, self.st.background);
        sp::line(
            c,
            area.x0,
            area.y0 + 0.5,
            area.x1,
            area.y0 + 0.5,
            self.st.separator,
        );
        let Some(data) = &self.feed.data else {
            sp::label(
                c,
                "Open a note clip in the Tab Editor",
                area.center().x,
                area.center().y + 4.0,
                11.0,
                t.text_muted,
                Align::Center,
            );
            return;
        };
        self.paint_header(c, area, data);
        let geo = Geo::new(area, &data.tuning, self.opts.left_handed);
        self.paint_neck(c, &geo, data);
        let tick = self.feed.tick;
        let chord = tick.and_then(|t| data.chord_at(t));
        if self.opts.chord_voicing
            && let Some(ch) = chord
        {
            self.paint_voicing(c, &geo, data, &ch.voicing);
        }
        let root = self.root_pc(chord);
        for n in &self.live.next {
            self.paint_note(c, &geo, data, *n, chord, root, true);
        }
        for n in &self.live.sounding {
            self.paint_note(c, &geo, data, *n, chord, root, false);
        }
    }

    /// The pitch class intervals are counted from: the chord's root, else the lowest note.
    fn root_pc(&self, chord: Option<&FretChord>) -> Option<u8> {
        let data = self.feed.data.as_ref()?;
        if let Some(c) = chord
            && let Ok(sym) = ChordSymbol::parse(&c.name)
        {
            return Some(sym.root_pc());
        }
        self.live
            .sounding
            .iter()
            .chain(self.live.next.iter())
            .filter_map(|n| data.tuning.pitch(usize::from(n.string), n.fret, data.capo))
            .min()
            .map(|p| p % 12)
    }

    fn paint_header(&self, c: &mut dyn Canvas, area: Rect, data: &FretboardData) {
        let t = &self.st.base;
        let bar = Rect::new(area.x0, area.y0, area.x1, area.y0 + HEADER);
        sp::rect(c, bar, self.st.header);
        sp::line(
            c,
            bar.x0,
            bar.y1 - 0.5,
            bar.x1,
            bar.y1 - 0.5,
            self.st.separator,
        );
        let chord = self
            .feed
            .tick
            .and_then(|tick| data.chord_at(tick))
            .map(|ch| ch.name.as_str());
        if let Some(name) = chord {
            c.text(
                &TextRun::verbatim(name, pt(area.x0 + 10.0, area.y0 + 14.0), 11.0, t.text)
                    .weight(700),
            );
        }
        if data.capo.fret > 0 {
            let x = area.x0 + 10.0 + chord.map_or(0.0, |n| n.chars().count() as f64 * 7.5 + 10.0);
            let text = crate::i18n::tf("Capo {fret}", &[("fret", &data.capo.fret.to_string())]);
            c.text(&TextRun::verbatim(
                text,
                pt(x, area.y0 + 14.0),
                10.0,
                t.text_muted,
            ));
        }
        for (r, item) in self.items(area) {
            let hot = self.hover.is_some_and(|p| r.contains(p));
            match item {
                FretboardItem::Labels => {
                    sp::button(c, &self.st, r, self.opts.labels.label(), None, hot);
                }
                FretboardItem::LeftHanded => sp::button(
                    c,
                    &self.st,
                    r,
                    "Left-handed",
                    self.opts.left_handed.then_some(t.highlight),
                    hot,
                ),
                FretboardItem::Voicing => sp::button(
                    c,
                    &self.st,
                    r,
                    "Voicing",
                    self.opts.chord_voicing.then_some(t.highlight),
                    hot,
                ),
            }
        }
    }

    fn paint_neck(&self, c: &mut dyn Canvas, g: &Geo, data: &FretboardData) {
        let t = &self.st.base;
        let tuning = &data.tuning;
        let ink = t.text;
        let faint = mix(t.text_muted, self.st.background, 0.4);
        let wood = mix(self.st.lane_alt, self.st.background, 0.5);
        let neck = g.neck_rect();
        c.fill_with(neck, wood);
        // Inlays.
        let mid = (g.top + g.bottom) / 2.0;
        let spread = (g.bottom - g.top) * 0.28;
        for f in INLAYS.into_iter().filter(|f| *f <= tuning.frets) {
            let x = g.cell_x(f);
            let r = (g.cell_w(f) * 0.22).clamp(1.5, 4.0);
            let dot = mix(t.text_muted, wood, 0.55);
            if f % 12 == 0 {
                c.fill_with(Circle::new(pt(x, mid - spread), r), dot);
                c.fill_with(Circle::new(pt(x, mid + spread), r), dot);
            } else {
                c.fill_with(Circle::new(pt(x, mid), r), dot);
            }
        }
        // Frets: the nut is a thick line.
        for f in 1..=tuning.frets {
            let x = g.fx(g.ux(f64::from(f)));
            c.stroke_with(Line::new(pt(x, neck.y0), pt(x, neck.y1)), 1.0, faint);
        }
        let nut = g.fx(g.ux(0.0));
        c.stroke_with(Line::new(pt(nut, neck.y0), pt(nut, neck.y1)), 3.0, ink);
        // Strings: a short string starts at its own fret, with a peg there.
        let n = tuning.strings.len();
        let end = g.fx(g.ux(f64::from(tuning.frets)));
        for (i, s) in tuning.strings.iter().enumerate() {
            let y = g.y_of(i);
            let start = if s.start_fret > 0 {
                g.fx(g.ux(f64::from(s.start_fret) - 1.0))
            } else {
                nut
            };
            let thick = if n > 1 {
                0.8 + 1.6 * i as f64 / (n - 1) as f64
            } else {
                1.4
            };
            c.stroke_with(
                Line::new(pt(start, y), pt(end, y)),
                thick,
                mix(ink, faint, 0.35),
            );
            if s.start_fret > 0 {
                c.fill_with(Circle::new(pt(start, y), 3.0), ink);
            }
        }
        // The capo, with the covered side of the neck dimmed.
        if data.capo.fret > 0 && data.capo.fret <= tuning.frets {
            let rows: Vec<usize> = (0..n).filter(|i| data.capo.raise(*i) > 0).collect();
            if let (Some(&a), Some(&z)) = (rows.first(), rows.last()) {
                let f = data.capo.fret;
                let (y0, y1) = (g.y_of(a) - g.gap() * 0.5, g.y_of(z) + g.gap() * 0.5);
                let bar_w = (g.cell_w(f) * 0.4).clamp(3.0, 7.0);
                let x = g.fx(g.ux(f64::from(f)) - bar_w);
                let (xa, xz) = (nut.min(x), nut.max(x));
                c.fill_with(
                    Rect::new(xa, y0, xz, y1),
                    self.st.background.with_alpha(0.4),
                );
                let cx = g.fx(g.ux(f64::from(f)) - bar_w * 0.5);
                c.fill_with(
                    RoundedRect::new(cx - bar_w * 0.5, y0, cx + bar_w * 0.5, y1, 2.0),
                    mix(ink, t.warning, 0.5),
                );
            }
        }
        // String names, at the nut's side.
        for (i, s) in tuning.strings.iter().enumerate() {
            let label = TextRun::verbatim(
                s.label.clone(),
                pt(g.fx(g.nut - 24.0), g.y_of(i) + 3.5),
                9.5,
                t.text_muted,
            )
            .align(if g.mirror { Align::Start } else { Align::End });
            c.text(&label);
        }
        // Fret numbers under the inlays.
        let y = g.bottom + g.gap() * 0.5 + NUMBERS - 3.0;
        for f in INLAYS.into_iter().filter(|f| *f <= tuning.frets) {
            c.text(
                &TextRun::verbatim(f.to_string(), pt(g.cell_x(f), y), 9.0, t.text_muted)
                    .align(Align::Center),
            );
        }
    }

    /// The label of a note.
    fn label_of(&self, data: &FretboardData, n: FretNote, root: Option<u8>) -> String {
        let pitch = data.tuning.pitch(usize::from(n.string), n.fret, data.capo);
        match self.opts.labels {
            FretLabels::Frets => n.fret.to_string(),
            FretLabels::Notes => pitch.map_or_else(String::new, |p| pitch_class_name(p).to_owned()),
            FretLabels::Intervals => match (pitch, root) {
                (Some(p), Some(r)) => interval_name(p, r).to_owned(),
                _ => String::new(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_note(
        &self,
        c: &mut dyn Canvas,
        g: &Geo,
        data: &FretboardData,
        n: FretNote,
        chord: Option<&FretChord>,
        root: Option<u8>,
        ghost: bool,
    ) {
        let t = &self.st.base;
        let Some((x, y, r)) = g.dot(&data.tuning, data.capo, n.string, n.fret) else {
            return;
        };
        let label = self.label_of(data, n, root);
        if ghost {
            c.fill_with(Circle::new(pt(x, y), r), t.highlight.with_alpha(0.12));
            c.stroke_with(Circle::new(pt(x, y), r), 1.2, t.highlight.with_alpha(0.55));
            if r >= 4.5 {
                c.text(
                    &TextRun::verbatim(
                        label,
                        pt(x, y + r * 0.38),
                        (r * 1.1) as f32,
                        t.highlight.with_alpha(0.7),
                    )
                    .align(Align::Center),
                );
            }
            return;
        }
        let verdict = self.feed.grades.as_ref().and_then(|g| {
            g.iter()
                .find(|x| x.tick == n.tick && x.string == n.string && x.fret == n.fret)
                .map(|x| x.verdict)
        });
        let fill = verdict.map_or(t.highlight, |v| crate::grading::colour(&self.st.base, v));
        c.fill_with(Circle::new(pt(x, y), r), fill);
        if let Some(badge) = verdict.and_then(crate::grading::badge)
            && r >= 4.5
        {
            c.text(
                &TextRun::verbatim(badge, pt(x - r * 0.8, y - r * 0.5), (r * 1.1) as f32, fill)
                    .align(Align::Center)
                    .weight(700),
            );
        }
        if r >= 4.5 {
            c.text(
                &TextRun::verbatim(label, pt(x, y + r * 0.38), (r * 1.1) as f32, t.on_accent)
                    .align(Align::Center)
                    .weight(700),
            );
        }
        if let Some(finger) = chord.and_then(|ch| finger_of(&ch.voicing, n))
            && r >= 4.5
        {
            let (bx, by, br) = (x + r * 0.8, y + r * 0.8, (r * 0.55).max(3.0));
            c.fill_with(Circle::new(pt(bx, by), br), t.on_accent);
            c.text(
                &TextRun::verbatim(
                    finger.to_string(),
                    pt(bx, by + br * 0.38),
                    (br * 1.5) as f32,
                    t.highlight,
                )
                .align(Align::Center)
                .weight(700),
            );
        }
    }

    fn paint_voicing(&self, c: &mut dyn Canvas, g: &Geo, data: &FretboardData, v: &Voicing) {
        let t = &self.st.base;
        let ring = t.cable_notes;
        for (i, f) in v.frets.iter().enumerate() {
            match f {
                None => {
                    let (x, y) = (g.open_x(&data.tuning, i), g.y_of(i));
                    let k = (g.gap() * 0.22).clamp(2.0, 4.0);
                    c.stroke_with(Line::new(pt(x - k, y - k), pt(x + k, y + k)), 1.4, ring);
                    c.stroke_with(Line::new(pt(x - k, y + k), pt(x + k, y - k)), 1.4, ring);
                }
                Some(fret) => {
                    let Some((x, y, r)) = g.dot(
                        &data.tuning,
                        data.capo,
                        u8::try_from(i).unwrap_or(u8::MAX),
                        *fret,
                    ) else {
                        continue;
                    };
                    c.stroke_with(Circle::new(pt(x, y), r + 1.5), 1.6, ring);
                }
            }
        }
    }
}

/// The finger a voicing puts on a note's string, when it fingers that fret.
fn finger_of(v: &Voicing, n: FretNote) -> Option<u8> {
    let i = usize::from(n.string);
    (v.frets.get(i).copied().flatten() == Some(n.fret))
        .then(|| v.fingers.get(i).copied().flatten())
        .flatten()
}

/// The geometry of the neck in an area.
struct Geo {
    x0: f64,
    x1: f64,
    top: f64,
    bottom: f64,
    /// The unmirrored x of the nut and the length of the neck.
    nut: f64,
    len: f64,
    frets: f64,
    strings: usize,
    mirror: bool,
}

impl Geo {
    fn new(area: Rect, tuning: &Tuning, mirror: bool) -> Self {
        let n = tuning.strings.len().max(1);
        let top = area.y0 + HEADER + 12.0;
        let bottom = (area.y1 - NUMBERS - 8.0).max(top + 1.0);
        Self {
            x0: area.x0,
            x1: area.x1,
            top,
            bottom,
            nut: area.x0 + GUTTER,
            len: (area.width() - GUTTER - MARGIN).max(40.0),
            frets: f64::from(tuning.frets.max(1)),
            strings: n,
            mirror,
        }
    }

    /// The unmirrored x of fret `f` (fret 0 is the nut), logarithmic and fitted to the length.
    fn ux(&self, f: f64) -> f64 {
        let share = (1.0 - 2f64.powf(-f / 12.0)) / (1.0 - 2f64.powf(-self.frets / 12.0));
        self.nut + self.len * share
    }

    /// Mirrors an unmirrored x for a left-handed neck.
    fn fx(&self, x: f64) -> f64 {
        if self.mirror {
            self.x0 + self.x1 - x
        } else {
            x
        }
    }

    /// The distance of two strings.
    fn gap(&self) -> f64 {
        if self.strings > 1 {
            (self.bottom - self.top) / (self.strings - 1) as f64
        } else {
            (self.bottom - self.top).min(14.0)
        }
    }

    fn y_of(&self, string: usize) -> f64 {
        if self.strings > 1 {
            self.top + self.gap() * string as f64
        } else {
            (self.top + self.bottom) / 2.0
        }
    }

    fn neck_rect(&self) -> Rect {
        let a = self.fx(self.nut);
        let b = self.fx(self.ux(self.frets));
        Rect::new(
            a.min(b),
            self.top - self.gap() * 0.5,
            a.max(b),
            self.bottom + self.gap() * 0.5,
        )
    }

    /// The width of the space of fret `f` (between its wire and the one before).
    fn cell_w(&self, f: u8) -> f64 {
        (self.ux(f64::from(f)) - self.ux(f64::from(f) - 1.0)).abs()
    }

    /// The x of the middle of the space of fret `f`.
    fn cell_x(&self, f: u8) -> f64 {
        self.fx((self.ux(f64::from(f)) + self.ux(f64::from(f) - 1.0)) / 2.0)
    }

    /// Where an open string's marker sits: left of the nut, or at the peg of a short string.
    fn open_x(&self, tuning: &Tuning, string: usize) -> f64 {
        let start = tuning.strings.get(string).map_or(0, |s| s.start_fret);
        if start > 0 {
            self.fx(self.ux(f64::from(start) - 1.0) - 10.0)
        } else {
            self.fx(self.nut - 10.0)
        }
    }

    /// The centre and radius of the dot of a note: the string's own fret above the capo. An open
    /// string sits left of the nut (at the capo when one covers it).
    fn dot(&self, tuning: &Tuning, capo: Capo, string: u8, fret: u8) -> Option<(f64, f64, f64)> {
        let i = usize::from(string);
        let s = tuning.strings.get(i)?;
        let y = self.y_of(i);
        let raise = capo.raise(i);
        let (x, cell) = if fret == 0 && (s.start_fret > 0 || raise == 0) {
            (self.open_x(tuning, i), self.cell_w(1))
        } else {
            let abs = raise.saturating_add(fret).min(tuning.frets.max(1));
            (self.cell_x(abs), self.cell_w(abs))
        };
        let r = (self.gap() * 0.44).min(cell * 0.46).clamp(2.5, 9.0);
        Some((x, y, r))
    }
}
