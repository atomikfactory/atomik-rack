//! The theme editor: every colour token of a user theme in a grouped list.
//!
//! ```text
//! THEME EDITOR  Night drive                                   [UNDO] [SAVE] [CLOSE]
//! Click a swatch to pick a colour, or type a hex colour. Changes show at once.
//! BACKGROUNDS
//! [#] Window background                                       [RESET]  #140c28
//! [#] Rack rail                                                        #2a1a4d
//! ...
//! ```
//!
//! A modal view the app places over the window. It edits a [`ThemeDoc`]: a swatch asks the app to
//! open the colour picker for its token ([`ThemeEditorAction::PickColor`]; the app gives the
//! colour back with [`ThemeEditorView::set_color`]), a click on the hex text selects its row and
//! the one [`TextField`] of the view edits it (a colour is taken as soon as the text names one),
//! and RESET sets a token back to what the theme it is based on says. Every change raises
//! [`ThemeEditorAction::Changed`]; the app paints the whole interface with
//! [`ThemeEditorView::preview`] meanwhile. The history of changes (UNDO, Ctrl+Z) lasts as long as
//! the editor is open; SAVE and CLOSE write the theme file (the app does it), so changes reach the
//! file when the editor closes.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::{fit_label, fit_label_raw};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::theme_doc::{
    Group, TOKENS, ThemeDoc, builtin, colour_of, hex_text, parse_colour, rgba_of,
};
use vello::kurbo::{Affine, Point, Rect, RoundedRect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const NOTE_H: f64 = 24.0;
const ROW_H: f64 = 24.0;
const HEAD_ROW_H: f64 = 26.0;
const MARGIN: f64 = 16.0;
const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 640.0;
const BTN_H: f64 = 22.0;
const BTN_W: f64 = 78.0;
const FONT: f32 = 10.5;
const HEX_W: f64 = 92.0;
const RESET_W: f64 = 50.0;
const SWATCH_W: f64 = 26.0;
/// The most steps of history kept.
const HISTORY: usize = 200;

/// What the editor asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeEditorAction {
    /// A colour changed: paint with [`ThemeEditorView::preview`].
    Changed,
    /// Write the theme file.
    Save,
    /// Write the theme file if it changed, and close the editor.
    Close,
    /// Open the colour picker for the token at this place of [`crate::theme_doc::TOKENS`].
    PickColor(usize),
}

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Heading(Group),
    Token(usize),
}

fn lines() -> Vec<Line> {
    let mut out = Vec::new();
    for g in Group::ALL {
        out.push(Line::Heading(g));
        for (i, t) in TOKENS.iter().enumerate() {
            if t.group == g {
                out.push(Line::Token(i));
            }
        }
    }
    out
}

/// The theme editor panel. See the module documentation.
#[derive(Debug)]
pub struct ThemeEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    id: String,
    doc: ThemeDoc,
    lines: Vec<Line>,
    scroll: f64,
    /// The token whose hex text is being edited.
    selected: Option<usize>,
    hex: TextField,
    /// The text names a colour (false while it is half typed).
    valid: bool,
    /// The edit of the selected row already has its step of history.
    stepped: bool,
    /// What each step of history set the token to before (`None`: not set by the file).
    history: Vec<(usize, Option<[u8; 4]>)>,
    saved: ThemeDoc,
    changed: bool,
    hover: Option<Point>,
    repaint: bool,
}

impl ThemeEditorView {
    /// An editor of `doc`, the user theme `id`; `theme` is what the panel itself is drawn with.
    pub fn new(theme: Theme, id: String, doc: ThemeDoc) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 700.0),
            scale: 1.0,
            id,
            saved: doc.clone(),
            doc,
            lines: lines(),
            scroll: 0.0,
            selected: None,
            hex: TextField::new("", 9, false).with_filter(|c| c.is_ascii_hexdigit() || c == '#'),
            valid: true,
            stepped: false,
            history: Vec::new(),
            changed: false,
            hover: None,
            repaint: true,
        }
    }

    /// Switches the colours the panel is drawn with.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The id of the theme being edited.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The theme as edited.
    pub fn doc(&self) -> &ThemeDoc {
        &self.doc
    }

    /// The colours the interface paints with while the editor is open.
    pub fn preview(&self) -> Theme {
        self.doc.resolve()
    }

    /// There are changes the file does not have yet.
    pub fn dirty(&self) -> bool {
        self.doc != self.saved
    }

    /// The file has what the editor has now.
    pub fn mark_saved(&mut self) {
        self.saved = self.doc.clone();
        self.repaint = true;
    }

    /// True once after a colour changed (the app then repaints with the new preview).
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// The steps of history that can be undone.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// The colour of the token at `index`, as the theme has it now.
    pub fn color(&self, index: usize) -> [u8; 4] {
        TOKENS
            .get(index)
            .and_then(|t| self.doc.colour(t.key))
            .unwrap_or([0, 0, 0, 255])
    }

    /// Whether the token at `index` is not what the base theme says.
    pub fn differs_from_base(&self, index: usize) -> bool {
        TOKENS
            .get(index)
            .is_some_and(|t| self.color(index) != rgba_of((t.get)(builtin(self.doc.based_on))))
    }

    /// The token whose hex text is being edited.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The hex text as it is now.
    pub fn hex_text(&self) -> &str {
        self.hex.text()
    }

    fn push_step(&mut self, index: usize) {
        let key = TOKENS[index].key;
        let before = self.doc.colors.get(key).copied();
        self.history.push((index, before));
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
    }

    fn put(&mut self, index: usize, rgba: [u8; 4]) {
        if self.color(index) == rgba {
            return;
        }
        self.push_step(index);
        self.doc.set_colour(TOKENS[index].key, rgba);
        self.changed = true;
        self.repaint = true;
    }

    /// Gives the token at `index` the colour `rgb` (its transparency stays).
    pub fn set_color(&mut self, index: usize, rgb: [u8; 3]) {
        if index >= TOKENS.len() {
            return;
        }
        let a = self.color(index)[3];
        self.put(index, [rgb[0], rgb[1], rgb[2], a]);
        if self.selected == Some(index) {
            self.show_hex(index);
        }
    }

    /// Sets the token at `index` back to what the base theme says.
    pub fn reset(&mut self, index: usize) {
        let Some(t) = TOKENS.get(index) else { return };
        if !self.differs_from_base(index) && !self.doc.sets(t.key) {
            return;
        }
        self.push_step(index);
        self.doc.reset_colour(t.key);
        self.changed = true;
        self.repaint = true;
        if self.selected == Some(index) {
            self.show_hex(index);
        }
    }

    /// Takes back the last change; false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some((index, before)) = self.history.pop() else {
            return false;
        };
        match before {
            Some(c) => self.doc.set_colour(TOKENS[index].key, c),
            None => self.doc.reset_colour(TOKENS[index].key),
        }
        self.stepped = false;
        self.changed = true;
        self.repaint = true;
        if let Some(sel) = self.selected {
            self.show_hex(sel);
        }
        true
    }

    fn show_hex(&mut self, index: usize) {
        self.hex.set_text(&hex_text(self.color(index)));
        self.valid = true;
        self.repaint = true;
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        self.stepped = false;
        if let Some(i) = index {
            self.show_hex(i);
            self.hex.select_all();
            self.scroll_to(i);
        }
        self.repaint = true;
    }

    /// The hex text was edited: the colour follows when it names one.
    fn read_hex(&mut self) {
        let Some(index) = self.selected else { return };
        match parse_colour(self.hex.text()) {
            Some(rgba) => {
                self.valid = true;
                if rgba != self.color(index) {
                    // A run of typing is one step.
                    if self.stepped {
                        self.doc.set_colour(TOKENS[index].key, rgba);
                        self.changed = true;
                    } else {
                        self.put(index, rgba);
                        self.stepped = true;
                    }
                }
            }
            None => self.valid = false,
        }
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let w = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, WIDTH);
        let h = (self.bounds.height() - 2.0 * MARGIN).clamp(1.0, HEIGHT);
        let c = self.bounds.center();
        Rect::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0)
    }

    fn button_rect(&self, from_right: usize) -> Rect {
        let p = self.panel();
        let x1 = p.x1 - PAD - from_right as f64 * (BTN_W + 8.0);
        Rect::new(x1 - BTN_W, p.y0 + 6.0, x1, p.y0 + 6.0 + BTN_H)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        self.button_rect(0)
    }

    /// The SAVE button.
    pub fn save_rect(&self) -> Rect {
        self.button_rect(1)
    }

    /// The UNDO button.
    pub fn undo_rect(&self) -> Rect {
        self.button_rect(2)
    }

    /// The area of the list.
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + HEADER_H + NOTE_H, p.x1, p.y1 - 6.0)
    }

    fn line_height(line: Line) -> f64 {
        match line {
            Line::Heading(_) => HEAD_ROW_H,
            Line::Token(_) => ROW_H,
        }
    }

    fn content_height(&self) -> f64 {
        self.lines.iter().map(|l| Self::line_height(*l)).sum()
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_height() - self.list_rect().height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// The top of a line, relative to the top of the list before scrolling.
    fn line_top(&self, at: usize) -> f64 {
        self.lines[..at].iter().map(|l| Self::line_height(*l)).sum()
    }

    fn scroll_to(&mut self, index: usize) {
        let Some(at) = self.lines.iter().position(|l| *l == Line::Token(index)) else {
            return;
        };
        let top = self.line_top(at);
        let view = self.list_rect().height();
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + view {
            self.scroll = top + ROW_H - view;
        }
        self.clamp_scroll();
    }

    /// The row of the token at `index` on screen (the whole line), or `None` while it is
    /// scrolled out of the list.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        let at = self.lines.iter().position(|l| *l == Line::Token(index))?;
        let list = self.list_rect();
        let y = list.y0 + self.line_top(at) - self.scroll;
        let r = Rect::new(list.x0, y, list.x1, y + ROW_H);
        (r.y1 > list.y0 && r.y0 < list.y1).then_some(r)
    }

    /// The swatch of a row.
    pub fn swatch_rect(&self, index: usize) -> Option<Rect> {
        let r = self.row_rect(index)?;
        Some(Rect::new(
            r.x0 + PAD,
            r.y0 + 4.0,
            r.x0 + PAD + SWATCH_W,
            r.y1 - 4.0,
        ))
    }

    /// The hex text of a row.
    pub fn hex_rect(&self, index: usize) -> Option<Rect> {
        let r = self.row_rect(index)?;
        Some(Rect::new(
            r.x1 - PAD - HEX_W,
            r.y0 + 2.0,
            r.x1 - PAD,
            r.y1 - 2.0,
        ))
    }

    /// The RESET button of a row.
    pub fn reset_rect(&self, index: usize) -> Option<Rect> {
        let hex = self.hex_rect(index)?;
        Some(Rect::new(
            hex.x0 - 8.0 - RESET_W,
            hex.y0 + 1.0,
            hex.x0 - 8.0,
            hex.y1 - 1.0,
        ))
    }

    fn token_at(&self, pos: Point) -> Option<usize> {
        if !self.list_rect().contains(pos) {
            return None;
        }
        (0..TOKENS.len()).find(|i| self.row_rect(*i).is_some_and(|r| r.contains(pos)))
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        let act = |a: ThemeEditorAction| UiAction::ThemeEditor(a);
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
            UiEvent::Wheel { delta, .. } => {
                self.scroll -= delta.y * 40.0;
                self.clamp_scroll();
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(act(ThemeEditorAction::Close));
                } else if self.save_rect().contains(*pos) {
                    out.push(act(ThemeEditorAction::Save));
                } else if self.undo_rect().contains(*pos) {
                    self.undo();
                    if self.take_changed() {
                        out.push(act(ThemeEditorAction::Changed));
                    }
                } else if let Some(i) = self.token_at(*pos) {
                    if self.swatch_rect(i).is_some_and(|r| r.contains(*pos)) {
                        self.select(Some(i));
                        out.push(act(ThemeEditorAction::PickColor(i)));
                    } else if self.reset_rect(i).is_some_and(|r| r.contains(*pos))
                        && self.differs_from_base(i)
                    {
                        self.reset(i);
                    } else if self.hex_rect(i).is_some_and(|r| r.contains(*pos)) {
                        if self.selected != Some(i) {
                            self.select(Some(i));
                        }
                        self.hex.press(*pos, *clicks, modifiers.shift);
                    } else {
                        self.select(Some(i));
                    }
                    if self.take_changed() {
                        out.push(act(ThemeEditorAction::Changed));
                    }
                }
            }
            UiEvent::PointerUp { .. } => {}
            UiEvent::Ime(ime) => {
                if self.selected.is_some() && self.hex.handle_ime(ime) {
                    self.read_hex();
                    if self.take_changed() {
                        out.push(act(ThemeEditorAction::Changed));
                    }
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let command = modifiers.command();
                match key {
                    Key::Escape => out.push(act(ThemeEditorAction::Close)),
                    Key::Char(c) if command && c.eq_ignore_ascii_case(&'z') && !modifiers.shift => {
                        self.undo();
                    }
                    Key::Char(c) if command && c.eq_ignore_ascii_case(&'s') => {
                        out.push(act(ThemeEditorAction::Save));
                    }
                    Key::Up | Key::Down if self.selected.is_none() || !modifiers.shift => {
                        let at = self.selected.map_or(0, |i| {
                            if *key == Key::Up {
                                i.saturating_sub(1)
                            } else {
                                (i + 1).min(TOKENS.len() - 1)
                            }
                        });
                        self.select(Some(at));
                    }
                    Key::Enter if self.selected.is_some() => {
                        if self.valid {
                            let i = self.selected.unwrap_or(0);
                            self.select(Some(i));
                        }
                    }
                    _ if self.selected.is_some()
                        && self.hex.handle_key(key, *modifiers).handled() =>
                    {
                        self.read_hex();
                    }
                    _ => {}
                }
                self.repaint = true;
                if self.take_changed() {
                    out.push(act(ThemeEditorAction::Changed));
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

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
            "THEME EDITOR",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let name_x = p.x0 + PAD + 100.0;
        c.text(&TextRun::verbatim(
            fit_label_raw(&self.doc.name, self.undo_rect().x0 - name_x - 10.0, FONT),
            Point::new(name_x, p.y0 + 22.0),
            FONT,
            t.text_muted,
        ));
        sp::button(
            c,
            st,
            self.undo_rect(),
            "UNDO",
            None,
            hovered(self.undo_rect()) && !self.history.is_empty(),
        );
        sp::button(
            c,
            st,
            self.save_rect(),
            "SAVE",
            self.dirty().then_some(st.loop_on),
            hovered(self.save_rect()),
        );
        sp::button(
            c,
            st,
            self.close_rect(),
            "CLOSE",
            None,
            hovered(self.close_rect()),
        );
        sp::label(
            c,
            &fit_label(
                "Click a swatch to pick a colour, or type a hex colour. Changes show at once.",
                p.width() - 2.0 * PAD,
                9.5,
            ),
            p.x0 + PAD,
            p.y0 + HEADER_H + 16.0,
            9.5,
            t.text_muted,
            Align::Start,
        );

        let list = self.list_rect();
        c.push_clip(&list.into());
        let mut y = list.y0 - self.scroll;
        for line in &self.lines {
            let h = Self::line_height(*line);
            if y + h > list.y0 && y < list.y1 {
                match line {
                    Line::Heading(g) => {
                        c.text(&TextRun::verbatim(
                            crate::i18n::tr(g.label()).to_uppercase(),
                            Point::new(p.x0 + PAD, y + 18.0),
                            9.5,
                            t.text_muted,
                        ));
                        sp::line(
                            c,
                            p.x0 + PAD,
                            y + h - 0.5,
                            p.x1 - PAD,
                            y + h - 0.5,
                            st.separator,
                        );
                    }
                    Line::Token(i) => self.paint_row(c, *i, y),
                }
            }
            y += h;
        }
        c.pop_clip();
        // A thin bar says how much of the list shows.
        let total = self.content_height();
        if total > list.height() {
            let h = (list.height() * list.height() / total).max(16.0);
            let top = list.y0 + (list.height() - h) * (self.scroll / (total - list.height()));
            sp::rect(
                c,
                Rect::new(p.x1 - 4.0, top, p.x1 - 1.0, top + h),
                mix(t.text_muted, t.background, 0.5),
            );
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_row(&self, c: &mut dyn Canvas, index: usize, y: f64) {
        let st = &self.st;
        let t = &st.base;
        let list = self.list_rect();
        let row = Rect::new(list.x0, y, list.x1, y + ROW_H);
        let selected = self.selected == Some(index);
        if selected {
            sp::rect(c, row, mix(t.browser_row, t.background, 0.2));
        } else if self.hover.is_some_and(|h| row.contains(h)) {
            sp::rect(c, row, mix(st.background, t.text, 0.05));
        }
        let rgba = self.color(index);
        let sw = Rect::new(
            row.x0 + PAD,
            y + 4.0,
            row.x0 + PAD + SWATCH_W,
            y + ROW_H - 4.0,
        );
        // The plate shows through a colour that is not opaque.
        c.fill_with(RoundedRect::from_rect(sw, 3.0), t.plate);
        c.fill_with(RoundedRect::from_rect(sw, 3.0), colour_of(rgba));
        c.stroke_with(
            RoundedRect::from_rect(sw.inset(-0.5), 3.5),
            if selected { 2.0 } else { 1.0 },
            if selected { t.text } else { st.separator },
        );
        let hex = Rect::new(row.x1 - PAD - HEX_W, y + 2.0, row.x1 - PAD, y + ROW_H - 2.0);
        let reset = Rect::new(
            hex.x0 - 8.0 - RESET_W,
            hex.y0 + 1.0,
            hex.x0 - 8.0,
            hex.y1 - 1.0,
        );
        c.text(&TextRun::new(
            fit_label(TOKENS[index].label, reset.x0 - sw.x1 - 20.0, FONT),
            Point::new(sw.x1 + 10.0, y + 16.0),
            FONT,
            t.text,
        ));
        if self.differs_from_base(index) {
            sp::button(
                c,
                st,
                reset,
                "RESET",
                None,
                self.hover.is_some_and(|h| reset.contains(h)),
            );
        }
        if selected {
            sp::lcd(
                c,
                st,
                hex,
                Some(if self.valid { t.focus } else { st.record }),
            );
            let colors = FieldColors {
                text: if self.valid { t.text } else { st.record },
                caret: t.text,
                selection: mix(t.lcd, t.focus, 0.5),
            };
            self.hex.paint(c, hex, FONT, 14.0, colors, true);
        } else {
            c.text(&TextRun::verbatim(
                hex_text(rgba),
                Point::new(hex.x0 + 6.0, y + 16.0),
                FONT,
                t.text_muted,
            ));
        }
    }
}
