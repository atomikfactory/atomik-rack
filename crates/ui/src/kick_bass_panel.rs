//! The kick and bass check panel: pick the two tracks, run the check, read the strip, try a fix.
//!
//! ```text
//! KICK AND BASS CHECK                                                    [ CLOSE ]
//! Kick [<] Kick drum [>]     Bass [<] Sub bass [>]                       [ CHECK ]
//! 7 hits: 5 cancel, 0 build up. Overall -3.1 dB against the power sum
//! |   ||  |||  ||   |   ||  |||  ||    (one bar per kick hit; down: cancels, up: builds up)
//! Kick 51.2 Hz, bass 52.0 Hz: 0.27 semitones apart, they beat at 0.8 Hz
//! FIXES (each one was rendered again)
//! Start the bass oscillator at 90 degrees ...   |  ||  | |   +4.2 dB  [ APPLY ]
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app owns
//! the check; it hands the panel a [`KickBassInfo`] (what to draw) with
//! [`KickBassView::set_info`] and gets [`UiAction::KickBass`] back:
//!
//! * `<` and `>` beside a track step through the tracks ([`KickBassAction::Kick`],
//!   [`KickBassAction::Bass`]).
//! * `CHECK` (and Enter) emits [`KickBassAction::Run`]; while it runs the same button reads
//!   `CANCEL` and emits [`KickBassAction::Cancel`].
//! * `APPLY` on a fix emits [`KickBassAction::Apply`] with the row's number.
//! * `CLOSE` emits [`KickBassAction::Close`]; the app also closes the panel on Esc.
//!
//! A strip draws one bar per kick hit at its place in the checked stretch: a bar going down is a
//! hit whose sub range is quieter than the power sum of the two tracks (they cancel), a bar going
//! up one that is louder (they build up); the height is the size of the difference up to
//! [`FULL_DB`], and a hit within the noticing limit is a thin tick on the line.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const PICK_H: f64 = 34.0;
const STATUS_H: f64 = 24.0;
const STRIP_H: f64 = 44.0;
const FIX_H: f64 = 46.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 720.0;
const BTN_H: f64 = 22.0;
const FONT: f32 = 10.5;
/// The difference in dB at which a bar of a strip is full height.
pub const FULL_DB: f32 = 6.0;
/// The most fix rows the panel shows.
pub const MAX_FIXES: usize = 5;
/// A hit this close to the power sum (dB) is drawn as a tick.
const TICK_DB: f32 = 1.5;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KickBassAction {
    /// Close the panel.
    Close,
    /// Step the kick track by this many tracks (-1 or 1).
    Kick(i32),
    /// Step the bass track by this many tracks (-1 or 1).
    Bass(i32),
    /// Run the check.
    Run,
    /// Stop the check that is running.
    Cancel,
    /// Apply fix number `n` (an undo step).
    Apply(usize),
}

/// One hit of a strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripHit {
    /// Where the hit starts in the checked stretch, 0 to 1.
    pub start: f32,
    /// How long it is, as a fraction of the stretch.
    pub len: f32,
    /// The sum against the power sum of the two tracks, dB (negative: cancelling).
    pub delta_db: f32,
}

/// A strip to draw: the hits of one check.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StripView {
    /// The hits, in time order.
    pub hits: Vec<StripHit>,
}

/// One fix to offer.
#[derive(Debug, Clone, PartialEq)]
pub struct FixView {
    /// What it does.
    pub label: String,
    /// The strip the song gives with it (a re-render).
    pub strip: StripView,
    /// What it gains over the strip before, as text ("+4.2 dB").
    pub gain: String,
}

/// What the check is doing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RunView {
    /// Nothing has run for these two tracks.
    #[default]
    Idle,
    /// A check is running, `0..=1` done.
    Running(f32),
    /// The check ended without a result: why.
    Failed(String),
    /// A result is shown.
    Done,
}

/// Everything the panel draws.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KickBassInfo {
    /// The names of the tracks that can be picked.
    pub tracks: Vec<String>,
    /// The picked kick track (an index of `tracks`).
    pub kick: Option<usize>,
    /// The picked bass track.
    pub bass: Option<usize>,
    /// What the check is doing.
    pub run: RunView,
    /// The line under the pickers: the result in a sentence.
    pub summary: String,
    /// The strip of the song as it is.
    pub strip: Option<StripView>,
    /// The line about the two fundamentals.
    pub pitches: String,
    /// The fundamentals beat: the line is a warning.
    pub beating: bool,
    /// What was said about fixes that could not be tried.
    pub notes: Vec<String>,
    /// The fixes found, the best first.
    pub fixes: Vec<FixView>,
}

/// The kick and bass check panel. See the module documentation.
#[derive(Debug)]
pub struct KickBassView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    info: KickBassInfo,
    hover: Option<Point>,
    repaint: bool,
}

impl KickBassView {
    /// A panel with nothing to show yet; call [`Self::set_info`] and [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            info: KickBassInfo::default(),
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Shows `info`; the panel repaints when it differs from what it showed.
    pub fn set_info(&mut self, info: KickBassInfo) {
        if info != self.info {
            self.info = info;
            self.repaint = true;
        }
    }

    /// What the panel shows.
    pub fn info(&self) -> &KickBassInfo {
        &self.info
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (it does not: it has no text field).
    pub fn is_text_owner(&self) -> bool {
        false
    }

    // ------------------------------------------------------------------ geometry

    fn rows_height(&self) -> f64 {
        let fixes = self.info.fixes.len().min(MAX_FIXES) as f64;
        let notes = self.info.notes.len().min(3) as f64;
        let fix_block = if fixes > 0.0 || notes > 0.0 {
            20.0 + fixes * FIX_H + notes * 14.0
        } else {
            0.0
        };
        HEADER_H + PICK_H + STATUS_H + STRIP_H + 40.0 + fix_block + PAD
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + self.rows_height().min(room))
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    fn pick_y(&self) -> f64 {
        self.panel().y0 + HEADER_H + (PICK_H - BTN_H) / 2.0
    }

    fn picker_width(&self) -> f64 {
        ((self.panel().width() - 2.0 * PAD - 110.0) / 2.0 - 70.0).max(60.0)
    }

    /// The `<` button of the kick (`false`) or bass (`true`) picker.
    pub fn prev_rect(&self, bass: bool) -> Rect {
        let p = self.panel();
        let half = (p.width() - 2.0 * PAD - 110.0) / 2.0;
        let x = p.x0 + PAD + 38.0 + if bass { half } else { 0.0 };
        Rect::new(x, self.pick_y(), x + 20.0, self.pick_y() + BTN_H)
    }

    /// The name window of the kick (`false`) or bass (`true`) picker.
    pub fn name_rect(&self, bass: bool) -> Rect {
        let prev = self.prev_rect(bass);
        Rect::new(
            prev.x1 + 2.0,
            prev.y0,
            prev.x1 + 2.0 + self.picker_width(),
            prev.y1,
        )
    }

    /// The `>` button of the kick (`false`) or bass (`true`) picker.
    pub fn next_rect(&self, bass: bool) -> Rect {
        let name = self.name_rect(bass);
        Rect::new(name.x1 + 2.0, name.y0, name.x1 + 22.0, name.y1)
    }

    /// The CHECK (or CANCEL) button.
    pub fn run_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 90.0,
            self.pick_y(),
            p.x1 - PAD,
            self.pick_y() + BTN_H,
        )
    }

    fn summary_y(&self) -> f64 {
        self.panel().y0 + HEADER_H + PICK_H
    }

    /// The strip of the song as it is.
    pub fn strip_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.summary_y() + STATUS_H;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + STRIP_H)
    }

    fn fixes_y(&self) -> f64 {
        self.strip_rect().y1 + 40.0
    }

    /// The strip of fix `n`.
    pub fn fix_strip_rect(&self, n: usize) -> Rect {
        let p = self.panel();
        let y = self.fixes_y() + 20.0 + n as f64 * FIX_H;
        Rect::new(p.x0 + PAD, y + 16.0, p.x1 - PAD - 190.0, y + FIX_H - 4.0)
    }

    /// The APPLY button of fix `n`.
    pub fn apply_rect(&self, n: usize) -> Rect {
        let p = self.panel();
        let y = self.fixes_y() + 20.0 + n as f64 * FIX_H;
        Rect::new(
            p.x1 - PAD - 80.0,
            y + 16.0 + (FIX_H - 20.0 - BTN_H) / 2.0,
            p.x1 - PAD,
            y + 16.0 + (FIX_H - 20.0 - BTN_H) / 2.0 + BTN_H,
        )
    }

    // ------------------------------------------------------------------ events

    fn running(&self) -> bool {
        matches!(self.info.run, RunView::Running(_))
    }

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the panel; the song is not changed",
            );
        }
        for bass in [false, true] {
            if self.prev_rect(bass).contains(pos) || self.next_rect(bass).contains(pos) {
                return help(
                    if self.prev_rect(bass).contains(pos) {
                        self.prev_rect(bass)
                    } else {
                        self.next_rect(bass)
                    },
                    if bass {
                        "Pick the bass track: the track that plays the sub bass"
                    } else {
                        "Pick the kick track: the track that plays the kick drum"
                    },
                );
            }
        }
        if self.run_rect().contains(pos) {
            return help(
                self.run_rect(),
                if self.running() {
                    "Stop the check"
                } else {
                    "Render the two tracks and check how their low ends add up, kick hit by hit"
                },
            );
        }
        for n in 0..self.info.fixes.len().min(MAX_FIXES) {
            if self.apply_rect(n).contains(pos) {
                return help(
                    self.apply_rect(n),
                    "Make this change to the song (one undo step)",
                );
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = |a: KickBassAction| UiAction::KickBass(a);
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
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
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(act(KickBassAction::Close));
                } else if self.run_rect().contains(*pos) {
                    out.push(act(if self.running() {
                        KickBassAction::Cancel
                    } else {
                        KickBassAction::Run
                    }));
                } else {
                    for bass in [false, true] {
                        let step = |d: i32| {
                            if bass {
                                KickBassAction::Bass(d)
                            } else {
                                KickBassAction::Kick(d)
                            }
                        };
                        if self.prev_rect(bass).contains(*pos) {
                            out.push(act(step(-1)));
                        } else if self.next_rect(bass).contains(*pos)
                            || self.name_rect(bass).contains(*pos)
                        {
                            out.push(act(step(1)));
                        }
                    }
                    for n in 0..self.info.fixes.len().min(MAX_FIXES) {
                        if self.apply_rect(n).contains(*pos) {
                            out.push(act(KickBassAction::Apply(n)));
                        }
                    }
                }
            }
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } if !self.running() => out.push(act(KickBassAction::Run)),
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    fn paint_strip(&self, c: &mut dyn Canvas, r: Rect, strip: &StripView) {
        let st = &self.st;
        let t = &st.base;
        sp::lcd(c, st, r, None);
        let mid = r.center().y;
        sp::line(c, r.x0 + 2.0, mid, r.x1 - 2.0, mid, t.section_line);
        let inner = Rect::new(r.x0 + 3.0, r.y0 + 2.0, r.x1 - 3.0, r.y1 - 2.0);
        let half = inner.height() / 2.0;
        for hit in &strip.hits {
            let x0 = inner.x0 + f64::from(hit.start.clamp(0.0, 1.0)) * inner.width();
            let w = (f64::from(hit.len) * inner.width() - 1.0).max(2.0);
            let (y0, y1, color) = if hit.delta_db.abs() < TICK_DB {
                (mid - 1.0, mid + 1.0, t.text_muted)
            } else {
                let h = (f64::from(hit.delta_db.abs() / FULL_DB).min(1.0) * half).max(2.0);
                if hit.delta_db < 0.0 {
                    (mid, mid + h, st.record)
                } else {
                    (mid - h, mid, t.cable_notes)
                }
            };
            sp::rect(
                c,
                Rect::new(x0, y0, (x0 + w).min(inner.x1), y1),
                mix(color, t.lcd, 0.15),
            );
        }
    }

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "KICK AND BASS CHECK",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        // The two pickers.
        let name_of = |i: Option<usize>| {
            i.and_then(|i| self.info.tracks.get(i))
                .map_or("(none)", String::as_str)
        };
        for bass in [false, true] {
            let prev = self.prev_rect(bass);
            let name = self.name_rect(bass);
            let next = self.next_rect(bass);
            sp::label(
                c,
                if bass { "Bass" } else { "Kick" },
                prev.x0 - 38.0,
                prev.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::button(c, st, prev, "<", None, hovered(prev));
            sp::lcd(c, st, name, None);
            let picked = if bass { self.info.bass } else { self.info.kick };
            sp::label(
                c,
                &fit_label(name_of(picked), name.width() - 8.0, FONT),
                name.x0 + 5.0,
                name.y0 + 15.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::button(c, st, next, ">", None, hovered(next));
        }
        let run = self.run_rect();
        let running = self.running();
        sp::button(
            c,
            st,
            run,
            if running { "CANCEL" } else { "CHECK" },
            (!running).then_some(st.loop_on),
            hovered(run),
        );
        // The summary line.
        let sy = self.summary_y();
        let line = match &self.info.run {
            RunView::Running(f) => format!("Checking... {:.0} %", f * 100.0),
            RunView::Failed(why) => why.clone(),
            _ if self.info.summary.is_empty() => {
                "Pick the kick and the bass track, then CHECK".to_owned()
            }
            _ => self.info.summary.clone(),
        };
        let failed = matches!(self.info.run, RunView::Failed(_));
        sp::label(
            c,
            &fit_label(&line, p.width() - 2.0 * PAD, FONT),
            p.x0 + PAD,
            sy + 16.0,
            FONT,
            if failed { t.warning } else { t.text },
            Align::Start,
        );
        // The strip, and the line about the pitches under it.
        let strip = self.strip_rect();
        match &self.info.strip {
            Some(s) => self.paint_strip(c, strip, s),
            None => sp::lcd(c, st, strip, None),
        }
        if !self.info.pitches.is_empty() {
            sp::label(
                c,
                &fit_label(&self.info.pitches, p.width() - 2.0 * PAD, FONT),
                p.x0 + PAD,
                strip.y1 + 16.0,
                FONT,
                if self.info.beating { t.warning } else { t.text },
                Align::Start,
            );
        }
        sp::label(
            c,
            "One bar per kick hit: down, the two cancel; up, they build up",
            p.x0 + PAD,
            strip.y1 + 32.0,
            9.0,
            t.text_muted,
            Align::Start,
        );
        // The fixes.
        let shown = self.info.fixes.len().min(MAX_FIXES);
        if shown > 0 {
            let fy = self.fixes_y();
            sp::label(
                c,
                "FIXES (each one was rendered again; the strip is what it gives)",
                p.x0 + PAD,
                fy + 12.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
            for (n, fix) in self.info.fixes.iter().take(shown).enumerate() {
                let y = fy + 20.0 + n as f64 * FIX_H;
                sp::label(
                    c,
                    &fit_label(&fix.label, p.width() - 2.0 * PAD, FONT),
                    p.x0 + PAD,
                    y + 12.0,
                    FONT,
                    t.text,
                    Align::Start,
                );
                self.paint_strip(c, self.fix_strip_rect(n), &fix.strip);
                let apply = self.apply_rect(n);
                sp::label(
                    c,
                    &fix.gain,
                    apply.x0 - 10.0,
                    apply.y0 + 15.0,
                    FONT,
                    t.text,
                    Align::End,
                );
                sp::button(c, st, apply, "APPLY", Some(st.loop_on), hovered(apply));
            }
            for (n, note) in self.info.notes.iter().take(3).enumerate() {
                sp::label(
                    c,
                    &fit_label(note, p.width() - 2.0 * PAD, 9.5),
                    p.x0 + PAD,
                    fy + 20.0 + shown as f64 * FIX_H + 10.0 + n as f64 * 14.0,
                    9.5,
                    t.text_muted,
                    Align::Start,
                );
            }
        } else if !self.info.notes.is_empty() {
            let fy = self.fixes_y();
            for (n, note) in self.info.notes.iter().take(3).enumerate() {
                sp::label(
                    c,
                    &fit_label(note, p.width() - 2.0 * PAD, 9.5),
                    p.x0 + PAD,
                    fy + 12.0 + n as f64 * 14.0,
                    9.5,
                    t.text_muted,
                    Align::Start,
                );
            }
        }
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> KickBassView {
        let mut v = KickBassView::new(Theme::default());
        v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 800.0));
        v.set_info(KickBassInfo {
            tracks: vec!["Kick".into(), "Bass".into(), "Lead".into()],
            kick: Some(0),
            bass: Some(1),
            run: RunView::Done,
            summary: "7 hits".into(),
            strip: Some(StripView {
                hits: vec![StripHit {
                    start: 0.1,
                    len: 0.05,
                    delta_db: -4.0,
                }],
            }),
            pitches: "Kick 50 Hz".into(),
            beating: false,
            notes: Vec::new(),
            fixes: vec![
                FixView {
                    label: "One".into(),
                    strip: StripView::default(),
                    gain: "+3.0 dB".into(),
                },
                FixView {
                    label: "Two".into(),
                    strip: StripView::default(),
                    gain: "+1.0 dB".into(),
                },
            ],
        });
        v
    }

    fn click(v: &mut KickBassView, at: Point) -> Vec<UiAction> {
        v.handle(&UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Default::default(),
            clicks: 1,
        })
    }

    #[test]
    fn the_buttons_emit_their_actions() {
        let mut v = view();
        let expect = |a: Vec<UiAction>, want: KickBassAction| {
            assert_eq!(a, vec![UiAction::KickBass(want)]);
        };
        let c = v.close_rect().center();
        expect(click(&mut v, c), KickBassAction::Close);
        let c = v.run_rect().center();
        expect(click(&mut v, c), KickBassAction::Run);
        let c = v.prev_rect(false).center();
        expect(click(&mut v, c), KickBassAction::Kick(-1));
        let c = v.next_rect(false).center();
        expect(click(&mut v, c), KickBassAction::Kick(1));
        let c = v.prev_rect(true).center();
        expect(click(&mut v, c), KickBassAction::Bass(-1));
        let c = v.next_rect(true).center();
        expect(click(&mut v, c), KickBassAction::Bass(1));
        let c = v.apply_rect(1).center();
        expect(click(&mut v, c), KickBassAction::Apply(1));
    }

    #[test]
    fn the_run_button_cancels_while_a_check_runs() {
        let mut v = view();
        let mut info = v.info().clone();
        info.run = RunView::Running(0.4);
        v.set_info(info);
        let c = v.run_rect().center();
        assert_eq!(
            click(&mut v, c),
            vec![UiAction::KickBass(KickBassAction::Cancel)]
        );
        // Enter does not start a second check.
        let enter = UiEvent::Key {
            key: Key::Enter,
            modifiers: Default::default(),
            pressed: true,
        };
        assert!(v.handle(&enter).is_empty());
    }

    #[test]
    fn enter_runs_the_check_and_a_click_elsewhere_does_nothing() {
        let mut v = view();
        let enter = UiEvent::Key {
            key: Key::Enter,
            modifiers: Default::default(),
            pressed: true,
        };
        assert_eq!(
            v.handle(&enter),
            vec![UiAction::KickBass(KickBassAction::Run)]
        );
        let p = v.panel();
        assert!(click(&mut v, Point::new(p.x0 + 3.0, p.y1 - 3.0)).is_empty());
    }

    #[test]
    fn the_panel_grows_with_its_fixes_and_stays_inside_its_bounds() {
        let mut v = view();
        let two = v.panel().height();
        let mut info = v.info().clone();
        info.fixes.clear();
        v.set_info(info);
        assert!(v.panel().height() < two);
        v.set_bounds(Rect::new(0.0, 0.0, 300.0, 200.0));
        let p = v.panel();
        assert!(p.x0 >= 0.0 && p.x1 <= 300.0 && p.y1 <= 200.0, "{p:?}");
    }

    #[test]
    fn set_info_repaints_only_on_a_change() {
        let mut v = view();
        v.repaint = false;
        let same = v.info().clone();
        v.set_info(same);
        assert!(!v.needs_repaint());
        let mut other = v.info().clone();
        other.summary = "changed".into();
        v.set_info(other);
        assert!(v.needs_repaint());
    }

    #[test]
    fn help_is_given_for_the_buttons() {
        let v = view();
        for r in [
            v.close_rect(),
            v.run_rect(),
            v.prev_rect(false),
            v.next_rect(true),
            v.apply_rect(0),
        ] {
            assert!(v.help_at(r.center()).is_some(), "{r:?}");
        }
        assert!(v.help_at(Point::new(-5.0, -5.0)).is_none());
    }
}
