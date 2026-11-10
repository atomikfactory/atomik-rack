//! The references and decision zone panel: the song's reference tracks and, for each attribute of
//! a mix, the range the references span with the mix as a marker.
//!
//! ```text
//! REFERENCES AND DECISION ZONE                              [ REFRESH ] [ LISTEN ] [ CLOSE ]
//! References
//!   > Song A.wav          loudest -8.9 LUFS   matched +1.4 dB
//!     Song B.flac         loudest -10.2 LUFS
//! Decision zone
//!   Loudness     |    [=======|====]    |                -9.8 LUFS   inside
//!   Punch        |  [====]        |      |                11.2 dB     above by 1.4 dB
//!   ...
//! Measured against 2 references. Rows that say "any level" read the same at every level.
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app gives it
//! a [`ZoneData`] each frame ([`ZoneView::set_data`]) and handles the [`ZoneAction`]s that arrive
//! as [`UiAction::Zone`]:
//!
//! * a click on a reference (or Up and Down) emits [`ZoneAction::Select`];
//! * `REFRESH` (and Enter) emits [`ZoneAction::Refresh`]: the app renders the mix again;
//! * `LISTEN` emits [`ZoneAction::Listen`], the reference key of the Mixer menu;
//! * `CLOSE` emits [`ZoneAction::Close`]; the app also closes the panel on Esc.
//!
//! Each attribute draws the lowest-to-highest reading of the references as a band and the mix as
//! a marker; the marker is lit while the mix is inside the band, and the text says `inside`,
//! `above by ..` or `below by ..`. The panel never changes the song.

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
const SECTION_H: f64 = 22.0;
const REF_H: f64 = 20.0;
const ROW_H: f64 = 38.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 640.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 120.0;
const TEXT_W: f64 = 210.0;
/// The most references listed at once; the list follows the selection.
const REFS_SHOWN: usize = 6;
const FONT: f32 = 10.5;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneAction {
    /// Select this reference.
    Select(usize),
    /// Render the mix again and measure it.
    Refresh,
    /// Switch between the mix and the selected reference.
    Listen,
    /// Close the panel.
    Close,
}

/// Where the mix stands against the range of the references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Standing {
    /// Nothing to compare (no reading yet).
    #[default]
    Unknown,
    /// Within the range.
    Inside,
    /// Above the range.
    Above,
    /// Below the range.
    Below,
}

/// One listed reference.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ZoneReference {
    /// The file name.
    pub name: String,
    /// True for the selected one.
    pub selected: bool,
    /// What is known about it: its loudest section, the matched gain, why it cannot be used.
    pub detail: String,
}

/// One attribute of the decision zone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ZoneRowData {
    /// The name.
    pub label: String,
    /// The unit written after a value.
    pub unit: String,
    /// True when the reading does not depend on the level of the file.
    pub any_level: bool,
    /// The lowest and highest reading of the references.
    pub range: Option<(f32, f32)>,
    /// The mix's reading.
    pub mix: Option<f32>,
    /// Where the mix stands.
    pub standing: Standing,
    /// What the verdict says in words (`inside`, `above by 1.4 dB`).
    pub verdict: String,
    /// The smallest span the bar shows, in the unit.
    pub min_span: f32,
    /// Digits after the decimal point of the readings.
    pub digits: u8,
}

/// Everything the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ZoneData {
    /// The reference tracks of the song.
    pub references: Vec<ZoneReference>,
    /// One row per attribute.
    pub rows: Vec<ZoneRowData>,
    /// The line at the foot.
    pub status: String,
    /// True while an analysis runs.
    pub busy: bool,
    /// True while the reference is what is heard.
    pub listening: bool,
}

/// The references and decision zone panel. See the module documentation.
#[derive(Debug)]
pub struct ZoneView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: ZoneData,
    hover: Option<Point>,
    repaint: bool,
}

impl ZoneView {
    /// A panel showing `data`; call [`Self::set_bounds`].
    pub fn new(theme: Theme, data: ZoneData) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            data,
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

    /// Shows `data`; returns true when it differs from what was shown.
    pub fn set_data(&mut self, data: ZoneData) -> bool {
        if data == self.data {
            return false;
        }
        self.data = data;
        self.repaint = true;
        true
    }

    /// What the panel shows.
    pub fn data(&self) -> &ZoneData {
        &self.data
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ geometry

    fn shown(&self) -> usize {
        self.data.references.len().clamp(1, REFS_SHOWN)
    }

    /// The first listed reference: the list follows the selection.
    fn first(&self) -> usize {
        let at = self
            .data
            .references
            .iter()
            .position(|r| r.selected)
            .unwrap_or(0);
        (at + 1).saturating_sub(REFS_SHOWN)
    }

    fn height(&self) -> f64 {
        HEADER_H
            + SECTION_H
            + self.shown() as f64 * REF_H
            + SECTION_H
            + self.data.rows.len() as f64 * ROW_H
            + FOOT_H
            + PAD
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + self.height().min(room))
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 70.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The LISTEN button.
    pub fn listen_rect(&self) -> Rect {
        let c = self.close_rect();
        Rect::new(c.x0 - 8.0 - 74.0, c.y0, c.x0 - 8.0, c.y1)
    }

    /// The REFRESH button.
    pub fn refresh_rect(&self) -> Rect {
        let l = self.listen_rect();
        Rect::new(l.x0 - 8.0 - 84.0, l.y0, l.x0 - 8.0, l.y1)
    }

    /// The box of the listed reference `n` (counted on the screen, from the first listed).
    fn ref_rect(&self, n: usize) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + SECTION_H + n as f64 * REF_H;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + REF_H)
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + SECTION_H + self.shown() as f64 * REF_H + SECTION_H
    }

    /// The box of attribute row `i`.
    pub fn row_rect(&self, i: usize) -> Rect {
        let p = self.panel();
        let y = self.rows_top() + i as f64 * ROW_H;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + ROW_H)
    }

    /// The bar of attribute row `i`.
    pub fn bar_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(
            r.x0 + LABEL_W,
            r.y0 + 6.0,
            (r.x1 - TEXT_W - 8.0).max(r.x0 + LABEL_W + 20.0),
            r.y1 - 6.0,
        )
    }

    // ------------------------------------------------------------------ events

    /// What the button or reference under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.listen_rect().contains(pos) {
            return help(
                self.listen_rect(),
                "Switch between the mix and the selected reference",
            );
        }
        if self.refresh_rect().contains(pos) {
            return help(
                self.refresh_rect(),
                "Render the mix again and measure it against the references",
            );
        }
        let first = self.first();
        for n in 0..self.shown() {
            if first + n < self.data.references.len() && self.ref_rect(n).contains(pos) {
                return help(self.ref_rect(n), "Select this reference");
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
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
                    out.push(UiAction::Zone(ZoneAction::Close));
                } else if self.listen_rect().contains(*pos) {
                    out.push(UiAction::Zone(ZoneAction::Listen));
                } else if self.refresh_rect().contains(*pos) {
                    out.push(UiAction::Zone(ZoneAction::Refresh));
                } else {
                    let first = self.first();
                    for n in 0..self.shown() {
                        let at = first + n;
                        if at < self.data.references.len() && self.ref_rect(n).contains(*pos) {
                            out.push(UiAction::Zone(ZoneAction::Select(at)));
                        }
                    }
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => {
                let count = self.data.references.len();
                let at = self.data.references.iter().position(|r| r.selected);
                match key {
                    Key::Enter => out.push(UiAction::Zone(ZoneAction::Refresh)),
                    Key::Up if count > 0 => {
                        let to = at.map_or(0, |a| a.saturating_sub(1));
                        out.push(UiAction::Zone(ZoneAction::Select(to)));
                    }
                    Key::Down if count > 0 => {
                        let to = at.map_or(0, |a| (a + 1).min(count - 1));
                        out.push(UiAction::Zone(ZoneAction::Select(to)));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    fn number(value: f32, digits: u8, unit: &str) -> String {
        let digits = usize::from(digits);
        if unit.is_empty() {
            format!("{value:.digits$}")
        } else {
            format!("{value:.digits$} {unit}")
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
            "REFERENCES AND DECISION ZONE",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let (refresh, listen, close) = (self.refresh_rect(), self.listen_rect(), self.close_rect());
        let busy_label = if self.data.busy {
            "MEASURING"
        } else {
            "REFRESH"
        };
        sp::button(c, st, refresh, busy_label, None, hovered(refresh));
        let listening = self.data.listening.then_some(st.record);
        sp::button(c, st, listen, "LISTEN", listening, hovered(listen));
        sp::button(c, st, close, "CLOSE", None, hovered(close));

        // The references.
        let top = p.y0 + HEADER_H;
        sp::label(
            c,
            "References",
            p.x0 + PAD,
            top + 15.0,
            FONT,
            t.text_muted,
            Align::Start,
        );
        if self.data.references.is_empty() {
            sp::label(
                c,
                "None yet: Mixer > Reference > Add Reference Track",
                p.x0 + PAD + 8.0,
                top + SECTION_H + 14.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        let first = self.first();
        for n in 0..self.shown() {
            let Some(r) = self.data.references.get(first + n) else {
                break;
            };
            let rect = self.ref_rect(n);
            if r.selected {
                sp::rect(c, rect, mix(st.background, t.focus, 0.25));
            } else if hovered(rect) {
                sp::rect(c, rect, mix(st.background, t.text, 0.08));
            }
            let marker = if r.selected { ">" } else { " " };
            sp::label(
                c,
                marker,
                rect.x0 + 6.0,
                rect.y0 + 14.0,
                FONT,
                t.text,
                Align::Start,
            );
            let name_w = (rect.width() - TEXT_W - 30.0).max(40.0);
            sp::label(
                c,
                &crate::paint::fit_label_raw(&r.name, name_w, FONT),
                rect.x0 + 22.0,
                rect.y0 + 14.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::label(
                c,
                &fit_label(&r.detail, TEXT_W, 9.5),
                rect.x1 - 4.0,
                rect.y0 + 14.0,
                9.5,
                t.text_muted,
                Align::End,
            );
        }
        let more = self
            .data
            .references
            .len()
            .saturating_sub(first + REFS_SHOWN);
        if more > 0 {
            sp::label(
                c,
                &format!("+{more} more (Up and Down)"),
                p.x1 - PAD - 4.0,
                top + 15.0,
                9.5,
                t.text_muted,
                Align::End,
            );
        }

        // The attributes.
        let zone_top = self.rows_top() - SECTION_H;
        sp::label(
            c,
            "Decision zone",
            p.x0 + PAD,
            zone_top + 15.0,
            FONT,
            t.text_muted,
            Align::Start,
        );
        for (i, row) in self.data.rows.iter().enumerate() {
            self.paint_row(c, i, row);
        }

        // The foot.
        let foot = Rect::new(
            p.x0 + PAD,
            p.y1 - FOOT_H - PAD + 6.0,
            p.x1 - PAD,
            p.y1 - PAD,
        );
        sp::line(c, foot.x0, foot.y0, foot.x1, foot.y0, st.separator);
        sp::label(
            c,
            &fit_label(&self.data.status, foot.width(), FONT),
            foot.x0,
            foot.y0 + 16.0,
            FONT,
            t.text,
            Align::Start,
        );
        sp::label(
            c,
            &fit_label(
                "Rows marked any level read the same at every level; loudness does not",
                foot.width(),
                9.5,
            ),
            foot.x0,
            foot.y0 + 31.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_row(&self, c: &mut dyn Canvas, i: usize, row: &ZoneRowData) {
        let st = &self.st;
        let t = &st.base;
        let r = self.row_rect(i);
        sp::label(
            c,
            &row.label,
            r.x0 + 4.0,
            r.y0 + 17.0,
            FONT,
            t.text,
            Align::Start,
        );
        if row.any_level {
            sp::label(
                c,
                "any level",
                r.x0 + 4.0,
                r.y0 + 30.0,
                8.5,
                t.text_muted,
                Align::Start,
            );
        }
        let bar = self.bar_rect(i);
        sp::rect(c, bar, mix(st.background, t.lcd, 0.8));
        let (lit, word_color) = match row.standing {
            Standing::Inside => (st.loop_on, t.text),
            Standing::Above | Standing::Below => (st.solo, st.solo),
            Standing::Unknown => (t.text_muted, t.text_muted),
        };
        let mid_y = bar.center().y;
        sp::line(c, bar.x0, mid_y, bar.x1, mid_y, st.separator);
        // The axis covers the references and the mix with room to spare.
        let ends = [row.range.map(|r| r.0), row.range.map(|r| r.1), row.mix];
        let values: Vec<f32> = ends.into_iter().flatten().collect();
        if let (Some(&lo), Some(&hi)) = (
            values.iter().min_by(|a, b| a.total_cmp(b)),
            values.iter().max_by(|a, b| a.total_cmp(b)),
        ) {
            let span = (hi - lo).max(row.min_span);
            let centre = (hi + lo) / 2.0;
            let pad = span * 0.3;
            let (a, b) = (centre - span / 2.0 - pad, centre + span / 2.0 + pad);
            let x = |v: f32| bar.x0 + f64::from((v - a) / (b - a)) * bar.width();
            if let Some((rl, rh)) = row.range {
                let band = Rect::new(x(rl), bar.y0 + 4.0, x(rh).max(x(rl) + 3.0), bar.y1 - 4.0);
                sp::rect(c, band, mix(st.background, st.loop_on, 0.55));
            }
            if let Some(m) = row.mix {
                let mx = x(m);
                sp::rect(c, Rect::new(mx - 1.5, bar.y0, mx + 1.5, bar.y1), lit);
            }
        }
        // The readings: the mix and the verdict, then the references.
        let right = r.x1 - 4.0;
        let first = match row.mix {
            Some(m) => format!(
                "{}   {}",
                Self::number(m, row.digits, &row.unit),
                row.verdict
            ),
            None => "no reading".to_owned(),
        };
        sp::label(
            c,
            &fit_label(&first, TEXT_W, FONT),
            right,
            r.y0 + 17.0,
            FONT,
            word_color,
            Align::End,
        );
        let second = match row.range {
            Some((lo, hi)) => format!(
                "references {} to {}",
                Self::number(lo, row.digits, ""),
                Self::number(hi, row.digits, "")
            ),
            None => "no reference reading".to_owned(),
        };
        sp::label(
            c,
            &fit_label(&second, TEXT_W, 9.0),
            right,
            r.y0 + 30.0,
            9.0,
            t.text_muted,
            Align::End,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn data() -> ZoneData {
        ZoneData {
            references: vec![
                ZoneReference {
                    name: "a.wav".into(),
                    selected: true,
                    detail: "loudest -9.0 LUFS".into(),
                },
                ZoneReference {
                    name: "b.wav".into(),
                    selected: false,
                    detail: String::new(),
                },
            ],
            rows: vec![ZoneRowData {
                label: "Punch".into(),
                unit: "dB".into(),
                any_level: true,
                range: Some((8.0, 11.0)),
                mix: Some(12.0),
                standing: Standing::Above,
                verdict: "above by 1.0 dB".into(),
                min_span: 1.0,
                digits: 1,
            }],
            status: "Measured".into(),
            ..ZoneData::default()
        }
    }

    fn click(v: &mut ZoneView, at: Point) -> Vec<UiAction> {
        v.handle(&UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Default::default(),
            clicks: 1,
        })
    }

    #[test]
    fn the_buttons_and_the_reference_rows_emit_their_actions() {
        let mut v = ZoneView::new(Theme::dark(), data());
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 700.0));
        let at = v.close_rect().center();
        assert_eq!(click(&mut v, at), vec![UiAction::Zone(ZoneAction::Close)]);
        let at = v.refresh_rect().center();
        assert_eq!(click(&mut v, at), vec![UiAction::Zone(ZoneAction::Refresh)]);
        let at = v.listen_rect().center();
        assert_eq!(click(&mut v, at), vec![UiAction::Zone(ZoneAction::Listen)]);
        let second = v.ref_rect(1).center();
        assert_eq!(
            click(&mut v, second),
            vec![UiAction::Zone(ZoneAction::Select(1))]
        );
        assert!(click(&mut v, Point::new(1.0, 1.0)).is_empty());
    }

    #[test]
    fn the_keys_move_the_selection_and_refresh() {
        let mut v = ZoneView::new(Theme::dark(), data());
        let key = |v: &mut ZoneView, key: Key| {
            v.handle(&UiEvent::Key {
                key,
                modifiers: Default::default(),
                pressed: true,
            })
        };
        assert_eq!(
            key(&mut v, Key::Down),
            vec![UiAction::Zone(ZoneAction::Select(1))]
        );
        assert_eq!(
            key(&mut v, Key::Up),
            vec![UiAction::Zone(ZoneAction::Select(0))]
        );
        assert_eq!(
            key(&mut v, Key::Enter),
            vec![UiAction::Zone(ZoneAction::Refresh)]
        );
        let mut empty = ZoneView::new(Theme::dark(), ZoneData::default());
        assert!(key(&mut empty, Key::Down).is_empty());
    }

    #[test]
    fn new_data_repaints_and_the_same_data_does_not() {
        let mut v = ZoneView::new(Theme::dark(), data());
        assert!(!v.set_data(data()));
        let mut other = data();
        other.busy = true;
        assert!(v.set_data(other));
        assert!(v.needs_repaint());
    }

    #[test]
    fn a_long_list_follows_the_selection() {
        let mut d = data();
        d.references = (0..16)
            .map(|i| ZoneReference {
                name: format!("r{i}"),
                selected: i == 9,
                detail: String::new(),
            })
            .collect();
        let v = ZoneView::new(Theme::dark(), d);
        assert_eq!(v.first(), 4);
        assert_eq!(v.shown(), REFS_SHOWN);
    }

    #[test]
    fn the_tooltips_name_the_buttons() {
        let mut v = ZoneView::new(Theme::dark(), data());
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 700.0));
        for r in [
            v.close_rect(),
            v.listen_rect(),
            v.refresh_rect(),
            v.ref_rect(0),
        ] {
            let help = v.help_at(r.center()).expect("a tooltip");
            assert!(help.text.len() < 80, "{}", help.text);
        }
    }
}
