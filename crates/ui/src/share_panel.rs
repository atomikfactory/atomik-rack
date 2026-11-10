//! The share panel: send a song to someone as one file.
//!
//! ```text
//! SHARE PROJECT                                                          [ CLOSE ]
//! Title    [ Example Song                                    ]
//! Author   [                                                 ]
//! Note     [ several lines                                   ]
//!          [                                                 ]
//!          [x] Include a preview     MP3 of the whole song
//!          [x] Include stems         one 24-bit WAV file per mixer strip
//!          [x] Include plugin list   in credits.txt
//!          This share will be about 12.3 MB
//! [ SHARE ]    Tab moves between the texts
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app opens
//! it with the song's title, author and note ([`ShareView::new`]), tells it what the share
//! would weigh ([`ShareView::set_size`]) and what the preview would be
//! ([`ShareView::set_preview_hint`]), and reads the choices back with [`ShareView::values`]
//! when [`UiAction::Share`] arrives:
//!
//! * `SHARE` (and Enter in a one-line text) emits [`ShareAction::Share`].
//! * A check box emits [`ShareAction::Changed`], so the app can work out the size again.
//! * `CLOSE` emits [`ShareAction::Close`]; the app also closes the panel on Esc.
//!
//! The texts are [`TextField`]s with the limits of the song information.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_model::{MAX_INFO_AUTHOR_CHARS, MAX_INFO_NOTES_CHARS, MAX_INFO_TITLE_CHARS};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const ROW_H: f64 = 28.0;
const NOTE_ROWS: usize = 4;
const LINE_H: f64 = 14.0;
const CHECK_H: f64 = 24.0;
const SIZE_H: f64 = 26.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 560.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 70.0;
const BOX: f64 = 12.0;
const FONT: f32 = 10.5;

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareAction {
    /// Ask for the file and make the share.
    Share,
    /// A check box changed: the size is to be worked out again.
    Changed,
    /// Close the panel.
    Close,
}

/// What the panel holds: the three texts and the three choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareValues {
    /// The title (song information).
    pub title: String,
    /// The author (song information).
    pub author: String,
    /// The note (song information, the notes).
    pub note: String,
    /// Include a listening copy of the whole song.
    pub preview: bool,
    /// Include one file per mixer strip.
    pub stems: bool,
    /// List the plugins the song uses in the credits.
    pub plugins: bool,
}

impl Default for ShareValues {
    fn default() -> Self {
        Self {
            title: String::new(),
            author: String::new(),
            note: String::new(),
            preview: true,
            stems: false,
            plugins: true,
        }
    }
}

/// One text of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareField {
    /// The title.
    Title,
    /// The author.
    Author,
    /// The note (several lines).
    Note,
}

impl ShareField {
    /// Every text, top down (the Tab order).
    pub const ALL: [ShareField; 3] = [ShareField::Title, ShareField::Author, ShareField::Note];

    fn title(self) -> &'static str {
        match self {
            ShareField::Title => "Title",
            ShareField::Author => "Author",
            ShareField::Note => "Note",
        }
    }

    /// The most characters the text takes.
    pub fn limit(self) -> usize {
        match self {
            ShareField::Title => MAX_INFO_TITLE_CHARS,
            ShareField::Author => MAX_INFO_AUTHOR_CHARS,
            ShareField::Note => MAX_INFO_NOTES_CHARS,
        }
    }
}

/// One check box of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareBox {
    /// Include a preview.
    Preview,
    /// Include the stems.
    Stems,
    /// Include the plugin list.
    Plugins,
}

impl ShareBox {
    /// Every box, top down.
    pub const ALL: [ShareBox; 3] = [ShareBox::Preview, ShareBox::Stems, ShareBox::Plugins];

    fn label(self) -> &'static str {
        match self {
            ShareBox::Preview => "Include a preview",
            ShareBox::Stems => "Include stems",
            ShareBox::Plugins => "Include plugin list",
        }
    }

    fn note(self) -> &'static str {
        match self {
            ShareBox::Stems => "one 24-bit WAV file per mixer strip",
            ShareBox::Plugins => "in credits.txt, so the receiver knows what is missing",
            ShareBox::Preview => "",
        }
    }

    fn help(self) -> &'static str {
        match self {
            ShareBox::Preview => "A listening copy of the whole song at -14 LUFS (MP3 with FFmpeg)",
            ShareBox::Stems => "Add one WAV file for each mixer strip (it makes the share bigger)",
            ShareBox::Plugins => {
                "Write the plugins the song uses, with vendor and format, in the credits"
            }
        }
    }
}

/// The share panel. See the module documentation.
#[derive(Debug)]
pub struct ShareView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: ShareValues,
    /// The editors of the texts, in [`ShareField::ALL`] order; `values` follows them.
    fields: [TextField; 3],
    focus: ShareField,
    dragging: Option<ShareField>,
    size_text: String,
    size_warn: bool,
    preview_hint: String,
    hover: Option<Point>,
    repaint: bool,
}

impl ShareView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel holding `values`, with the keys on the title; call [`Self::set_bounds`].
    pub fn new(theme: Theme, values: ShareValues) -> Self {
        let fields = ShareField::ALL.map(|f| {
            let text = match f {
                ShareField::Title => &values.title,
                ShareField::Author => &values.author,
                ShareField::Note => &values.note,
            };
            TextField::new(text, f.limit(), f == ShareField::Note)
        });
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            values,
            fields,
            focus: ShareField::Title,
            dragging: None,
            size_text: String::new(),
            size_warn: false,
            preview_hint: "MP3 of the whole song at -14 LUFS".to_owned(),
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

    /// Sets the line that says what the share would weigh; `warn` draws it as a warning.
    pub fn set_size(&mut self, text: String, warn: bool) {
        if self.size_text != text || self.size_warn != warn {
            self.size_text = text;
            self.size_warn = warn;
            self.repaint = true;
        }
    }

    /// Sets the words after "Include a preview": what the preview will be.
    pub fn set_preview_hint(&mut self, text: String) {
        if self.preview_hint != text {
            self.preview_hint = text;
            self.repaint = true;
        }
    }

    /// The size line.
    pub fn size_text(&self) -> &str {
        &self.size_text
    }

    /// The texts and choices as they are now.
    pub fn values(&self) -> &ShareValues {
        &self.values
    }

    /// The text that has the keys.
    pub fn focus(&self) -> ShareField {
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

    fn index(field: ShareField) -> usize {
        ShareField::ALL
            .iter()
            .position(|f| *f == field)
            .unwrap_or(0)
    }

    fn editor(&mut self) -> &mut TextField {
        &mut self.fields[Self::index(self.focus)]
    }

    fn sync_values(&mut self) {
        let [t, a, n] = &self.fields;
        self.values.title = t.text().to_owned();
        self.values.author = a.text().to_owned();
        self.values.note = n.text().to_owned();
        self.repaint = true;
    }

    fn move_focus(&mut self, back: bool) {
        let at = Self::index(self.focus);
        let n = ShareField::ALL.len();
        self.focus = ShareField::ALL[if back { (at + n - 1) % n } else { (at + 1) % n }];
        self.repaint = true;
    }

    fn toggle(&mut self, which: ShareBox) {
        let flag = match which {
            ShareBox::Preview => &mut self.values.preview,
            ShareBox::Stems => &mut self.values.stems,
            ShareBox::Plugins => &mut self.values.plugins,
        };
        *flag = !*flag;
        self.repaint = true;
    }

    fn checked(&self, which: ShareBox) -> bool {
        match which {
            ShareBox::Preview => self.values.preview,
            ShareBox::Stems => self.values.stems,
            ShareBox::Plugins => self.values.plugins,
        }
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
        let fields: f64 = ShareField::ALL.iter().map(|f| Self::field_height(*f)).sum();
        HEADER_H + fields + CHECK_H * ShareBox::ALL.len() as f64 + SIZE_H + FOOT_H + PAD
    }

    fn field_height(field: ShareField) -> f64 {
        if field == ShareField::Note {
            NOTE_ROWS as f64 * LINE_H + 12.0
        } else {
            ROW_H
        }
    }

    /// The box of a text (the area that takes a click).
    pub fn field_rect(&self, field: ShareField) -> Rect {
        let p = self.panel();
        let mut y = p.y0 + HEADER_H + 4.0;
        for f in ShareField::ALL {
            if f == field {
                break;
            }
            y += Self::field_height(f);
        }
        let h = Self::field_height(field) - 6.0;
        Rect::new(p.x0 + PAD + LABEL_W, y, p.x1 - PAD, y + h)
    }

    fn checks_top(&self) -> f64 {
        self.field_rect(ShareField::Note).y1 + 8.0
    }

    /// The row of a check box (the box and its words take a click).
    pub fn box_row(&self, which: ShareBox) -> Rect {
        let p = self.panel();
        let at = ShareBox::ALL.iter().position(|b| *b == which).unwrap_or(0);
        let y = self.checks_top() + CHECK_H * at as f64;
        Rect::new(p.x0 + PAD + LABEL_W, y, p.x1 - PAD, y + CHECK_H)
    }

    fn box_rect(&self, which: ShareBox) -> Rect {
        let row = self.box_row(which);
        let y = row.y0 + (CHECK_H - BOX) / 2.0;
        Rect::new(row.x0, y, row.x0 + BOX, y + BOX)
    }

    fn size_row(&self) -> Rect {
        let p = self.panel();
        let y = self.checks_top() + CHECK_H * ShareBox::ALL.len() as f64;
        Rect::new(p.x0 + PAD + LABEL_W, y, p.x1 - PAD, y + SIZE_H)
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

    /// The SHARE button.
    pub fn share_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.size_row().y1 + (FOOT_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel without making a share");
        }
        if self.share_rect().contains(pos) {
            return help(
                self.share_rect(),
                "Choose the file and write the project, its audio and the credits into it",
            );
        }
        ShareBox::ALL
            .into_iter()
            .find(|b| self.box_row(*b).contains(pos))
            .and_then(|b| help(self.box_row(b), b.help()))
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
                    out.push(UiAction::Share(ShareAction::Close));
                } else if self.share_rect().contains(*pos) {
                    out.push(UiAction::Share(ShareAction::Share));
                } else if let Some(b) = ShareBox::ALL
                    .into_iter()
                    .find(|b| self.box_row(*b).contains(*pos))
                {
                    self.toggle(b);
                    out.push(UiAction::Share(ShareAction::Changed));
                } else if let Some(f) = ShareField::ALL
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
                    self.sync_values();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let used = self.editor().handle_key(key, *modifiers);
                if used.handled() {
                    self.sync_values();
                } else {
                    match key {
                        Key::Tab => self.move_focus(modifiers.shift),
                        Key::Enter => out.push(UiAction::Share(ShareAction::Share)),
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
            "SHARE PROJECT",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        for field in ShareField::ALL {
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
        for which in ShareBox::ALL {
            let row = self.box_row(which);
            let b = self.box_rect(which);
            sp::rect(c, b, st.separator);
            sp::rect(c, b.inflate(-1.0, -1.0), t.lcd);
            if self.checked(which) {
                sp::rect(c, b.inflate(-3.0, -3.0), t.focus);
            }
            let color = if hovered(row) { t.highlight } else { t.text };
            let label_x = b.x1 + 8.0;
            sp::label(
                c,
                which.label(),
                label_x,
                row.y0 + 16.0,
                FONT,
                color,
                Align::Start,
            );
            let note = if which == ShareBox::Preview {
                self.preview_hint.as_str()
            } else {
                which.note()
            };
            sp::label(
                c,
                &fit_label(note, row.x1 - label_x - 130.0 - 8.0, 9.5),
                label_x + 130.0,
                row.y0 + 16.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
        }
        let size = self.size_row();
        sp::label(
            c,
            &fit_label(&self.size_text, size.width(), FONT),
            size.x0,
            size.y0 + 17.0,
            FONT,
            if self.size_warn { t.warning } else { t.text },
            Align::Start,
        );
        let share = self.share_rect();
        sp::button(c, st, share, "SHARE", Some(st.loop_on), hovered(share));
        sp::label(
            c,
            &fit_label(
                "Tab moves between the texts; Enter shares (a new line in the note)",
                p.x1 - PAD - share.x1 - 12.0,
                9.5,
            ),
            share.x1 + 12.0,
            share.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}
