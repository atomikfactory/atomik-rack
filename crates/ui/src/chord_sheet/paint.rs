//! Painting the chord sheet pane: toolbar, diagram strip, text, popup and the panels over it.

use super::content::{self, Content, DrawRow, Kind};
use super::layout;
use super::{ChordSheetView, Frame, Mode, POPUP_DIAGRAM_WIDTH, Popup};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::chord_diagram::{self, Diagram};
use crate::seq_paint as sp;
use crate::text_field::FieldColors;
use crate::theme::mix;
use atomik_tab::Capo;
use atomik_tab::instrument::{Instrument, catalogue};
use vello::kurbo::{Affine, Rect, RoundedRect};

const PANEL_PAD: f64 = 10.0;
const PICK_COLUMNS: usize = 3;
const PICK_COLUMN_W: f64 = 168.0;
const PICK_ROW_H: f64 = 18.0;
const PICK_HEAD_H: f64 = 30.0;

/// The popup's size.
pub(super) fn popup_size() -> (f64, f64) {
    let w = POPUP_DIAGRAM_WIDTH + 2.0 * PANEL_PAD + 8.0;
    let h = PANEL_PAD
        + chord_diagram::diagram_height(POPUP_DIAGRAM_WIDTH, true)
        + 8.0
        + 22.0
        + PANEL_PAD;
    (w, h)
}

/// The popup's buttons: previous voicing, next voicing, close.
pub(super) fn popup_buttons(r: Rect) -> (Rect, Rect, Rect) {
    let y = r.y1 - PANEL_PAD - 22.0;
    (
        Rect::new(r.x0 + PANEL_PAD, y, r.x0 + PANEL_PAD + 28.0, y + 22.0),
        Rect::new(r.x1 - PANEL_PAD - 28.0, y, r.x1 - PANEL_PAD, y + 22.0),
        Rect::new(r.x1 - 22.0, r.y0 + 4.0, r.x1 - 4.0, r.y0 + 22.0),
    )
}

/// The instrument list: the panel and one rectangle per instrument.
pub(super) fn picker_layout(bounds: Rect) -> (Rect, Vec<(Rect, &'static Instrument)>) {
    let all = catalogue();
    let per = all.len().div_ceil(PICK_COLUMNS);
    let cols = all.len().div_ceil(per.max(1)).max(1);
    let w = (cols as f64 * PICK_COLUMN_W + 2.0 * PANEL_PAD).min(bounds.width() - 16.0);
    let h = PICK_HEAD_H + per as f64 * PICK_ROW_H + PANEL_PAD;
    let c = bounds.center();
    let panel = Rect::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0);
    let col_w = (w - 2.0 * PANEL_PAD) / cols as f64;
    let rows = all
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let (col, row) = (i / per.max(1), i % per.max(1));
            let x = panel.x0 + PANEL_PAD + col as f64 * col_w;
            let y = panel.y0 + PICK_HEAD_H + row as f64 * PICK_ROW_H;
            (Rect::new(x, y, x + col_w - 4.0, y + PICK_ROW_H), inst)
        })
        .collect();
    (panel, rows)
}

impl ChordSheetView {
    pub(super) fn paint_all(&mut self, canvas: &mut dyn Canvas) {
        let c = canvas;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&self.bounds.into());
        sp::rect(c, self.bounds, self.st.background);
        let f = self.frame();
        match &self.mode {
            Mode::Edit(_) => self.paint_edit(c, &f),
            _ if !self.has_sheet() => self.paint_empty(c, &f),
            _ => {
                self.paint_body(c, &f);
                if let Some(strip) = f.strip {
                    self.paint_strip(c, strip);
                }
            }
        }
        self.paint_toolbar(c, &f);
        self.paint_status(c, &f);
        if let Some(p) = self.popup.clone()
            && matches!(self.mode, Mode::View)
        {
            self.paint_popup(c, &p);
        }
        if self.picker {
            self.paint_picker(c);
        }
        if let Mode::Rename(_) = &self.mode {
            self.paint_rename(c);
        }
        if self.focused {
            sp::line(
                c,
                self.bounds.x0,
                self.bounds.y0 + 0.5,
                self.bounds.x1,
                self.bounds.y0 + 0.5,
                self.st.base.focus,
            );
        }
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_toolbar(&self, c: &mut dyn Canvas, f: &Frame) {
        let t = &self.st.base;
        sp::rect(c, f.bar, self.st.header);
        sp::line(
            c,
            f.bar.x0,
            f.bar.y1 - 0.5,
            f.bar.x1,
            f.bar.y1 - 0.5,
            self.st.separator,
        );
        for item in &f.items {
            match item.btn {
                Some(_) => {
                    let hover = item.on && self.hover.is_some_and(|p| item.rect.contains(p));
                    let lit = item.lit.then_some(t.highlight);
                    if item.verbatim {
                        self.paint_chip(c, item.rect, &item.text, lit, hover);
                    } else {
                        sp::button(c, &self.st, item.rect, &item.text, lit, hover);
                    }
                    if !item.on {
                        sp::rect(c, item.rect, self.st.header.with_alpha(0.55));
                    }
                }
                None => {
                    let color = if item.lit { t.highlight } else { t.text_muted };
                    let run = if item.verbatim {
                        TextRun::verbatim(
                            &item.text,
                            pt(item.rect.x0, item.rect.y0 + 14.0),
                            10.0,
                            t.text,
                        )
                    } else {
                        TextRun::new(
                            &item.text,
                            pt(item.rect.x0, item.rect.y0 + 14.0),
                            10.0,
                            color,
                        )
                    };
                    c.text(&run);
                }
            }
        }
    }

    /// A button whose text is the user's or the catalogue's own (never translated).
    fn paint_chip(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        text: &str,
        lit: Option<vello::peniko::Color>,
        hover: bool,
    ) {
        sp::button(c, &self.st, r, "", lit, hover);
        let t = &self.st.base;
        let col = if lit.is_some() { t.on_accent } else { t.text };
        c.text(
            &TextRun::verbatim(text, pt(r.center().x, r.center().y + 3.5), 9.5, col)
                .align(Align::Center)
                .weight(700),
        );
    }

    fn paint_status(&self, c: &mut dyn Canvas, f: &Frame) {
        if let Some(msg) = &self.status {
            sp::label(
                c,
                msg,
                f.body.x0 + 10.0,
                f.body.y1 - 8.0,
                10.0,
                self.st.base.text_muted,
                Align::Start,
            );
        }
    }

    fn paint_empty(&self, c: &mut dyn Canvas, f: &Frame) {
        let t = &self.st.base;
        let m = f.body.center();
        sp::label(
            c,
            "No chord sheet yet",
            m.x,
            m.y - 8.0,
            12.0,
            t.text,
            Align::Center,
        );
        sp::label(
            c,
            "Create one with New Sheet, or import a chords-over-lyrics or ChordPro file",
            m.x,
            m.y + 12.0,
            10.0,
            t.text_muted,
            Align::Center,
        );
    }

    // ------------------------------------------------------------------ strip

    fn paint_strip(&mut self, c: &mut dyn Canvas, strip: Rect) {
        let content = self.content();
        sp::rect(c, strip, self.st.header);
        sp::line(
            c,
            strip.x0,
            strip.y1 - 0.5,
            strip.x1,
            strip.y1 - 0.5,
            self.st.separator,
        );
        c.push_clip(&strip.into());
        let starts = Diagram::start_frets_of(self.tuning(), Capo::NONE);
        let theme = self.st.base.clone();
        for (i, chord) in content.chords.iter().enumerate() {
            let cell = self.strip_cell(strip, i);
            if cell.x1 < strip.x0 || cell.x0 > strip.x1 {
                continue;
            }
            let name = chord.name();
            match self.voicing_of(chord) {
                Some((v, _, _)) => chord_diagram::paint_diagram(
                    c,
                    &theme,
                    cell,
                    &Diagram {
                        name: Some(&name),
                        voicing: &v,
                        start_frets: &starts,
                    },
                ),
                None => {
                    c.text(
                        &TextRun::verbatim(
                            &name,
                            pt(cell.center().x, cell.y0 + 14.0),
                            11.0,
                            theme.text,
                        )
                        .align(Align::Center)
                        .weight(700),
                    );
                    sp::label(
                        c,
                        "?",
                        cell.center().x,
                        cell.y0 + 40.0,
                        11.0,
                        theme.text_muted,
                        Align::Center,
                    );
                }
            }
            if self.popup.as_ref().is_some_and(|p| p.chord == *chord) {
                c.stroke_with(
                    RoundedRect::from_rect(cell.inset(2.0), 3.0),
                    1.0,
                    theme.highlight,
                );
            }
        }
        c.pop_clip();
    }

    // ------------------------------------------------------------------ text

    fn paint_body(&mut self, c: &mut dyn Canvas, f: &Frame) {
        let content = self.content();
        let active = if self.playing {
            self.active_anchor()
        } else {
            None
        };
        c.push_clip(&f.body.into());
        c.push_transform(Affine::translate((
            f.body.x0 + content::MARGIN,
            f.body.y0 - self.scroll,
        )));
        let top = self.scroll - 40.0;
        let bottom = self.scroll + f.body.height() + 40.0;
        let mut anchor_i = 0usize;
        for e in &content.entries {
            let anchored = matches!(&e.kind, Kind::Lines(_))
                && content.anchors.iter().any(|(y, _)| (*y - e.y).abs() < 1e-9);
            let mine = anchored.then(|| {
                let i = anchor_i;
                anchor_i += 1;
                i
            });
            if e.y + e.h < top || e.y > bottom {
                continue;
            }
            if let Some(i) = mine {
                self.paint_anchor_mark(c, e.y, e.h, active == Some(i));
            }
            self.paint_entry(c, e, f);
        }
        c.pop_transform();
        c.pop_clip();
        self.paint_scrollbar(c, f, &content);
    }

    fn paint_anchor_mark(&self, c: &mut dyn Canvas, y: f64, h: f64, active: bool) {
        let t = &self.st.base;
        let col = if active {
            t.highlight
        } else {
            mix(t.highlight, self.st.background, 0.7)
        };
        sp::rect(c, Rect::new(-12.0, y, -9.0, y + h), col);
    }

    fn paint_scrollbar(&self, c: &mut dyn Canvas, f: &Frame, content: &Content) {
        let view = f.body.height();
        if content.height <= view + 1.0 {
            return;
        }
        let ratio = view / content.height;
        let h = (view * ratio).max(18.0);
        let y = f.body.y0 + (view - h) * (self.scroll / (content.height - view));
        sp::rect(
            c,
            Rect::new(f.body.x1 - 5.0, y, f.body.x1 - 2.0, y + h),
            mix(self.st.base.text_muted, self.st.background, 0.55),
        );
    }

    fn paint_entry(&self, c: &mut dyn Canvas, e: &content::Entry, f: &Frame) {
        let t = &self.st.base;
        let s = f64::from(self.size);
        let size = self.size;
        match &e.kind {
            Kind::Title {
                title,
                artist,
                info,
            } => {
                let mut y = e.y + s * 1.5;
                c.text(&TextRun::verbatim(title, pt(0.0, y), size * 1.5, t.text).weight(700));
                if !artist.is_empty() {
                    y += s * 1.3;
                    c.text(&TextRun::verbatim(artist, pt(0.0, y), size, t.text_muted));
                }
                if !info.is_empty() {
                    y += s * 1.3;
                    let line: Vec<String> = info.iter().map(content::Fact::text).collect();
                    c.text(&TextRun::verbatim(
                        line.join("   "),
                        pt(0.0, y),
                        size * 0.85,
                        t.text_muted,
                    ));
                }
            }
            Kind::Section(name) => {
                let w = layout::text_width(name, size) + 14.0;
                let r = Rect::new(0.0, e.y, w, e.y + e.h);
                c.fill_with(
                    RoundedRect::from_rect(r, 3.0),
                    mix(t.highlight, self.st.background, 0.78),
                );
                c.text(
                    &TextRun::verbatim(name, pt(7.0, e.y + e.h - s * 0.4), size * 0.95, t.text)
                        .weight(700),
                );
            }
            Kind::Lines(rows) => self.paint_rows(c, e, rows, f, false),
            Kind::Comment(rows) => self.paint_rows(c, e, rows, f, true),
            Kind::Tab(lines) => self.paint_tab(c, e, lines),
            Kind::Blank => {}
        }
    }

    fn paint_rows(
        &self,
        c: &mut dyn Canvas,
        e: &content::Entry,
        rows: &[DrawRow],
        f: &Frame,
        comment: bool,
    ) {
        let t = &self.st.base;
        let size = self.size;
        let s = f64::from(size);
        let ox = f.body.x0 + content::MARGIN;
        let oy = f.body.y0 - self.scroll;
        for r in rows {
            let y = e.y + r.y;
            for (x, k) in &r.row.chords {
                let name = &r.names[*k];
                let w = layout::chord_width(name, size);
                let rect = Rect::new(*x, y, x + w, y + r.chord_h);
                let open = self.popup.as_ref().is_some_and(|p| p.chord.name() == *name);
                let hover = self
                    .hover
                    .is_some_and(|p| rect.contains(pt(p.x - ox, p.y - oy)));
                if open || hover {
                    c.fill_with(
                        RoundedRect::from_rect(rect.inflate(2.0, 0.0), 3.0),
                        mix(
                            t.highlight,
                            self.st.background,
                            if open { 0.7 } else { 0.85 },
                        ),
                    );
                }
                c.text(
                    &TextRun::verbatim(
                        name,
                        pt(*x, y + r.chord_h - s * 0.28),
                        size * 0.92,
                        t.highlight,
                    )
                    .weight(700),
                );
            }
            let base = y + r.chord_h + r.lyric_h - s * 0.3;
            let color = if comment { t.text_muted } else { t.text };
            for (x, text) in &r.row.segs {
                c.text(&TextRun::verbatim(
                    text,
                    pt(*x, base),
                    if comment { size * 0.92 } else { size },
                    color,
                ));
            }
        }
    }

    /// Tab text on a fixed grid: digits and letters at their column, dashes as a line.
    fn paint_tab(&self, c: &mut dyn Canvas, e: &content::Entry, lines: &[String]) {
        let t = &self.st.base;
        let s = f64::from(self.size);
        let adv = s * layout::LYRIC_ADVANCE;
        for (row, line) in lines.iter().enumerate() {
            let y = e.y + s * 1.15 * (row as f64 + 1.0);
            let mut dash_from: Option<usize> = None;
            let chars: Vec<char> = line.chars().collect();
            let flush = |c: &mut dyn Canvas, from: Option<usize>, to: usize| {
                if let Some(a) = from {
                    sp::line(
                        c,
                        a as f64 * adv,
                        y - s * 0.3,
                        to as f64 * adv,
                        y - s * 0.3,
                        mix(t.text_muted, self.st.background, 0.5),
                    );
                }
            };
            for (i, ch) in chars.iter().enumerate() {
                if *ch == '-' {
                    dash_from.get_or_insert(i);
                    continue;
                }
                flush(c, dash_from.take(), i);
                if !ch.is_whitespace() {
                    c.text(&TextRun::verbatim(
                        ch.to_string(),
                        pt(i as f64 * adv, y),
                        self.size * 0.95,
                        t.text,
                    ));
                }
            }
            flush(c, dash_from.take(), chars.len());
        }
    }

    // ------------------------------------------------------------------ edit mode

    fn paint_edit(&self, c: &mut dyn Canvas, f: &Frame) {
        let Mode::Edit(e) = &self.mode else { return };
        let area = Rect::new(
            f.body.x0 + 8.0,
            f.body.y0 + 6.0,
            f.body.x1 - 8.0,
            f.body.y1 - 22.0,
        );
        sp::lcd(c, &self.st, area, None);
        e.field
            .paint(c, area, 12.0, 16.0, FieldColors::of(&self.st.base), true);
        let msg = e.error.as_deref().unwrap_or(
            "Chord names go on their own line over the lyric; [Section] lines start a part. Ctrl+Enter applies, Esc cancels",
        );
        let color = if e.error.is_some() {
            self.st.base.warning
        } else {
            self.st.base.text_muted
        };
        sp::label(
            c,
            msg,
            f.body.x0 + 10.0,
            f.body.y1 - 7.0,
            10.0,
            color,
            Align::Start,
        );
    }

    // ------------------------------------------------------------------ panels

    fn paint_popup(&mut self, c: &mut dyn Canvas, p: &Popup) {
        let r = self.popup_rect(p);
        let t = self.st.base.clone();
        let rr = RoundedRect::from_rect(r, 5.0);
        c.fill_with(rr, mix(self.st.header, t.text, 0.08));
        c.stroke_with(rr, 1.0, t.bevel_dark);
        let (prev, next, close) = popup_buttons(r);
        let name = p.chord.name();
        let starts = Diagram::start_frets_of(self.tuning(), Capo::NONE);
        let area = Rect::new(
            r.x0 + PANEL_PAD + 4.0,
            r.y0 + PANEL_PAD,
            r.x1 - PANEL_PAD - 4.0,
            r.y0 + PANEL_PAD + chord_diagram::diagram_height(POPUP_DIAGRAM_WIDTH, true),
        );
        let hover_at = self.hover;
        let hover = |b: Rect| hover_at.is_some_and(|h| b.contains(h));
        sp::button(c, &self.st, close, "x", None, hover(close));
        match self.voicing_of(&p.chord) {
            Some((v, i, n)) => {
                chord_diagram::paint_diagram(
                    c,
                    &t,
                    area,
                    &Diagram {
                        name: Some(&name),
                        voicing: &v,
                        start_frets: &starts,
                    },
                );
                sp::button(c, &self.st, prev, "<", None, hover(prev));
                sp::button(c, &self.st, next, ">", None, hover(next));
                sp::label(
                    c,
                    &format!("{}/{}", i + 1, n),
                    r.center().x,
                    prev.y0 + 15.0,
                    10.0,
                    t.text_muted,
                    Align::Center,
                );
            }
            None => {
                c.text(
                    &TextRun::verbatim(&name, pt(r.center().x, r.y0 + 24.0), 14.0, t.text)
                        .align(Align::Center)
                        .weight(700),
                );
                sp::label(
                    c,
                    "No fingering found",
                    r.center().x,
                    r.center().y,
                    10.0,
                    t.text_muted,
                    Align::Center,
                );
            }
        }
    }

    fn paint_picker(&self, c: &mut dyn Canvas) {
        let (panel, rows) = self.picker_layout();
        let t = &self.st.base;
        sp::rect(c, self.bounds, self.st.background.with_alpha(0.72));
        let rr = RoundedRect::from_rect(panel, 5.0);
        c.fill_with(rr, mix(self.st.header, t.text, 0.06));
        c.stroke_with(rr, 1.0, t.bevel_dark);
        c.text(
            &TextRun::new(
                "Instrument of the diagrams",
                pt(panel.x0 + PANEL_PAD, panel.y0 + 18.0),
                11.0,
                t.text,
            )
            .weight(700),
        );
        for (r, inst) in rows {
            let lit = inst.id == self.instrument;
            let hover = self.hover.is_some_and(|p| r.contains(p));
            if lit {
                c.fill_with(
                    RoundedRect::from_rect(r, 2.0),
                    mix(t.highlight, self.st.background, 0.55),
                );
            } else if hover {
                c.fill_with(
                    RoundedRect::from_rect(r, 2.0),
                    mix(t.text, self.st.background, 0.9),
                );
            }
            c.text(&TextRun::verbatim(
                inst.name,
                pt(r.x0 + 6.0, r.y0 + 13.0),
                10.0,
                t.text,
            ));
        }
    }

    fn paint_rename(&self, c: &mut dyn Canvas) {
        let Mode::Rename(r) = &self.mode else { return };
        let t = &self.st.base;
        sp::rect(c, self.bounds, self.st.background.with_alpha(0.72));
        let w = 320.0_f64.min(self.bounds.width() - 16.0);
        let m = self.bounds.center();
        let panel = Rect::new(m.x - w / 2.0, m.y - 40.0, m.x + w / 2.0, m.y + 40.0);
        let rr = RoundedRect::from_rect(panel, 5.0);
        c.fill_with(rr, mix(self.st.header, t.text, 0.06));
        c.stroke_with(rr, 1.0, t.bevel_dark);
        c.text(
            &TextRun::new(
                "Rename Sheet",
                pt(panel.x0 + PANEL_PAD, panel.y0 + 18.0),
                11.0,
                t.text,
            )
            .weight(700),
        );
        let field = Rect::new(
            panel.x0 + PANEL_PAD,
            panel.y0 + 26.0,
            panel.x1 - PANEL_PAD,
            panel.y0 + 50.0,
        );
        sp::lcd(c, &self.st, field, None);
        r.field
            .paint(c, field, 12.0, 16.0, FieldColors::of(t), true);
        sp::label(
            c,
            "Enter renames, Esc cancels",
            panel.x0 + PANEL_PAD,
            panel.y1 - 8.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }
}
