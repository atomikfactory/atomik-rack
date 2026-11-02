//! The song information panel: title, author, e-mail, web address and notes.
//!
//! ```text
//! SONG INFORMATION                                                       [ CLOSE ]
//! Title    [ Example Song                                    ]
//! Author   [                                                 ]
//! E-mail   [                                                 ]
//! Web      [                                                 ]
//! Notes    [ several lines                                   ]
//!          [                                                 ]
//! [ APPLY ]    Tab moves between the texts; Enter applies (a new line in the notes)
//! ```
//!
//! A modal view the app places over the window like the silence panel. The app opens it with
//! the song's information ([`SongInfoView::new`]) and reads the texts back with
//! [`SongInfoView::info`] when [`UiAction::SongInfo`] arrives:
//!
//! * `APPLY` (and Enter in a one-line text) emits [`SongInfoAction::Apply`].
//! * `CLOSE` emits [`SongInfoAction::Close`]; the app also closes the panel on Esc.
//!
//! One text has the keys at a time (click it, or Tab and Shift+Tab). Each text is a
//! [`TextField`]: a caret and a selection (the pointer, Shift+arrows, Ctrl+A), the clipboard,
//! and input-method composition ([`UiEvent::Ime`]). A text stops growing at its limit
//! ([`atomik_model::MAX_INFO_TITLE_CHARS`] and the like) and a control character never gets in
//! (the notes take Enter as a new line).

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_model::{
    MAX_INFO_AUTHOR_CHARS, MAX_INFO_EMAIL_CHARS, MAX_INFO_NOTES_CHARS, MAX_INFO_TITLE_CHARS,
    MAX_INFO_URL_CHARS, SongInfo,
};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 28.0;
const NOTES_ROWS: usize = 6;
const LINE_H: f64 = 14.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 560.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 70.0;
const FONT: f32 = 10.5;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SongInfoAction {
    /// Make the panel's texts the song information.
    Apply,
    /// Close the panel.
    Close,
}

/// One text of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoField {
    /// The title.
    Title,
    /// The author.
    Author,
    /// The e-mail address.
    Email,
    /// The web address.
    Url,
    /// The notes (several lines).
    Notes,
}

impl InfoField {
    /// Every text, top down (the Tab order).
    pub const ALL: [InfoField; 5] = [
        InfoField::Title,
        InfoField::Author,
        InfoField::Email,
        InfoField::Url,
        InfoField::Notes,
    ];

    fn title(self) -> &'static str {
        match self {
            InfoField::Title => "Title",
            InfoField::Author => "Author",
            InfoField::Email => "E-mail",
            InfoField::Url => "Web",
            InfoField::Notes => "Notes",
        }
    }

    /// The most characters the text takes.
    pub fn limit(self) -> usize {
        match self {
            InfoField::Title => MAX_INFO_TITLE_CHARS,
            InfoField::Author => MAX_INFO_AUTHOR_CHARS,
            InfoField::Email => MAX_INFO_EMAIL_CHARS,
            InfoField::Url => MAX_INFO_URL_CHARS,
            InfoField::Notes => MAX_INFO_NOTES_CHARS,
        }
    }
}

/// The song information panel. See the module documentation.
#[derive(Debug)]
pub struct SongInfoView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    info: SongInfo,
    /// The editors of the texts, in [`InfoField::ALL`] order; `info` follows them.
    fields: [TextField; 5],
    focus: InfoField,
    /// The text a pointer drag is selecting in.
    dragging: Option<InfoField>,
    hover: Option<Point>,
    repaint: bool,
}

impl SongInfoView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel editing `info`, with the keys on the title; call [`Self::set_bounds`].
    pub fn new(theme: Theme, info: SongInfo) -> Self {
        let fields = InfoField::ALL.map(|f| {
            let text = match f {
                InfoField::Title => &info.title,
                InfoField::Author => &info.author,
                InfoField::Email => &info.email,
                InfoField::Url => &info.url,
                InfoField::Notes => &info.notes,
            };
            TextField::new(text, f.limit(), f == InfoField::Notes)
        });
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            info,
            fields,
            focus: InfoField::Title,
            dragging: None,
            hover: None,
            repaint: true,
        }
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

    /// The texts as they are now.
    pub fn info(&self) -> &SongInfo {
        &self.info
    }

    /// The text that has the keys.
    pub fn focus(&self) -> InfoField {
        self.focus
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal and every key goes to a text).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    fn index(field: InfoField) -> usize {
        InfoField::ALL.iter().position(|f| *f == field).unwrap_or(0)
    }

    /// The editor of the focused text.
    fn editor(&mut self) -> &mut TextField {
        &mut self.fields[Self::index(self.focus)]
    }

    /// Copies the editors' texts into [`Self::info`].
    fn sync_info(&mut self) {
        let [t, a, e, u, n] = &self.fields;
        self.info.title = t.text().to_owned();
        self.info.author = a.text().to_owned();
        self.info.email = e.text().to_owned();
        self.info.url = u.text().to_owned();
        self.info.notes = n.text().to_owned();
        self.repaint = true;
    }

    fn move_focus(&mut self, back: bool) {
        let at = InfoField::ALL
            .iter()
            .position(|f| *f == self.focus)
            .unwrap_or(0);
        let n = InfoField::ALL.len();
        self.focus = InfoField::ALL[if back { (at + n - 1) % n } else { (at + 1) % n }];
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        let rows: f64 = InfoField::ALL
            .iter()
            .map(|f| Self::field_height(*f))
            .sum::<f64>();
        HEADER_H + rows + FOOT_H + PAD
    }

    fn field_height(field: InfoField) -> f64 {
        if field == InfoField::Notes {
            NOTES_ROWS as f64 * LINE_H + 12.0
        } else {
            ROW_H
        }
    }

    /// The box of a text (the area that takes a click).
    pub fn field_rect(&self, field: InfoField) -> Rect {
        let p = self.panel();
        let mut y = p.y0 + HEADER_H + 4.0;
        for f in InfoField::ALL {
            if f == field {
                break;
            }
            y += Self::field_height(f);
        }
        let h = Self::field_height(field) - 6.0;
        Rect::new(p.x0 + PAD + LABEL_W, y, p.x1 - PAD, y + h)
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

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let notes = self.field_rect(InfoField::Notes);
        let y = notes.y1 + (FOOT_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the panel without changing anything",
            );
        }
        if self.apply_rect().contains(pos) {
            return help(
                self.apply_rect(),
                "Make these texts the song information (one undo step)",
            );
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
                if let Some(f) = self.dragging {
                    self.fields[Self::index(f)].drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::SongInfo(SongInfoAction::Close));
                } else if self.apply_rect().contains(*pos) {
                    out.push(UiAction::SongInfo(SongInfoAction::Apply));
                } else if let Some(f) = InfoField::ALL
                    .into_iter()
                    .find(|f| self.field_rect(*f).contains(*pos))
                {
                    self.focus = f;
                    self.dragging = Some(f);
                    self.fields[Self::index(f)].press(*pos, *clicks, modifiers.shift);
                }
            }
            UiEvent::Ime(ime) => {
                if self.editor().handle_ime(ime) {
                    self.sync_info();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let used = self.editor().handle_key(key, *modifiers);
                if used.handled() {
                    self.sync_info();
                } else {
                    match key {
                        Key::Tab => self.move_focus(modifiers.shift),
                        Key::Enter => out.push(UiAction::SongInfo(SongInfoAction::Apply)),
                        _ => {}
                    }
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
            "SONG INFORMATION",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        for field in InfoField::ALL {
            let r = self.field_rect(field);
            sp::label(
                c,
                field.title(),
                p.x0 + PAD,
                r.y0 + 14.0,
                FONT,
                t.text,
                Align::Start,
            );
            let focused = field == self.focus;
            sp::lcd(c, st, r, focused.then_some(t.focus));
            let colors = FieldColors {
                text: t.text,
                caret: t.text,
                selection: mix(t.lcd, t.focus, 0.5),
            };
            self.fields[Self::index(field)].paint(c, r, FONT, LINE_H, colors, focused);
        }
        let apply = self.apply_rect();
        sp::button(c, st, apply, "APPLY", Some(st.loop_on), hovered(apply));
        sp::label(
            c,
            &fit_label(
                "Tab moves between the texts; Enter applies (a new line in the notes)",
                p.x1 - PAD - apply.x1 - 12.0,
                9.5,
            ),
            apply.x1 + 12.0,
            apply.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}
