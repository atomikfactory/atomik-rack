//! Painting the tab editor: ruler, chord row, staff with the notes and their technique symbols,
//! the rhythm under the staff, lyrics, cursor and playhead.

use super::entry::NoteValue;
use super::{DIAGRAM_WIDTH, Placed, RULER_HEIGHT, TOP_BAR_HEIGHT, TabEditorView, TopButton};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::chord_diagram::{self, Diagram};
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::{GRID_MIN_PX, GridLevel, grid_lines};
use atomik_tab::score::{BendKind, HarmonicKind, SlideKind, TechFlags, VibratoKind};
use vello::kurbo::{BezPath, Circle, Line, Rect, RoundedRect};

impl TabEditorView {
    pub(super) fn paint_all(&self, canvas: &mut dyn Canvas) {
        let c = canvas;
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        c.push_clip(&self.bounds.into());
        sp::rect(c, self.bounds, self.st.background);
        if self.work.is_some() {
            self.paint_staff_area(c);
        } else {
            sp::label(
                c,
                "No clip open",
                self.bounds.center().x,
                self.bounds.center().y,
                11.0,
                self.st.base.text_muted,
                Align::Center,
            );
        }
        self.paint_top_bar(c);
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

    fn paint_top_bar(&self, c: &mut dyn Canvas) {
        let b = self.bounds;
        let t = &self.st.base;
        let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        sp::rect(c, bar, self.st.header);
        sp::line(c, b.x0, bar.y1 - 0.5, b.x1, bar.y1 - 0.5, self.st.separator);
        let Some(w) = &self.work else {
            return;
        };
        let tab = w.tab();
        let name = atomik_tab::instrument::find_instrument(&tab.instrument_id)
            .map_or("Custom", |i| i.name);
        c.text(
            &TextRun::verbatim(
                format!("{}  -  {}", self.title, name),
                pt(b.x0 + 10.0, b.y0 + 18.0),
                11.0,
                t.text,
            )
            .weight(600),
        );
        let mut x =
            b.x0 + 10.0 + (self.title.chars().count() + name.chars().count()) as f64 * 6.6 + 40.0;
        if tab.capo.fret > 0 {
            c.text(&TextRun::new(
                format!("Capo {}", tab.capo.fret),
                pt(x, b.y0 + 18.0),
                10.0,
                t.text_muted,
            ));
            x += 60.0;
        }
        if w.automatic {
            c.text(&TextRun::new(
                "Automatic fingering",
                pt(x, b.y0 + 18.0),
                10.0,
                t.text_muted,
            ));
        }
        if let Some(s) = &self.status {
            c.text(&TextRun::new(
                s.clone(),
                pt(x + 130.0, b.y0 + 18.0),
                10.0,
                t.warning,
            ));
        }
        for (r, button) in self.top_buttons() {
            let (text, lit) = match button {
                TopButton::Value(d) => (NoteValue::plain(d).label(), d == self.value.division),
                TopButton::Dotted => (".".to_string(), self.value.dotted),
                TopButton::Triplet => ("3".to_string(), self.value.triplet),
            };
            let hover = self.hover.is_some_and(|p| r.contains(p));
            sp::button(c, &self.st, r, &text, lit.then_some(t.highlight), hover);
        }
    }

    fn paint_staff_area(&self, c: &mut dyn Canvas) {
        let Some(w) = &self.work else { return };
        let b = self.bounds;
        let t = &self.st.base;
        let x0 = self.axis.x0;
        let n = self.strings();
        let lh = self.line_h();
        let top = self.staff_top();
        let bottom = self.staff_bottom();
        let end_x = self.axis.tick_to_x(self.end_tick());
        let right = end_x.min(b.x1).max(x0);
        let area = Rect::new(b.x0, b.y0 + TOP_BAR_HEIGHT, b.x1, b.y1);
        c.push_clip(&area.into());

        // Ruler: bar numbers, tempo, section markers, time signature.
        let ruler = self.ruler_rect();
        sp::rect(c, Rect::new(b.x0, ruler.y0, b.x1, ruler.y1), self.st.ruler);
        sp::ruler_labels(c, &self.st, &self.axis, ruler, &self.bars);
        self.paint_marks(c, ruler);

        // Staff lines and bar lines.
        for i in 0..n {
            let y = (top + i as f64 * lh).round() + 0.5;
            sp::line(c, x0, y, right, y, mix(t.text_muted, self.st.lane, 0.35));
        }
        let t0 = self.axis.x_to_tick(x0) - 1;
        let t1 = self.axis.x_to_tick(b.x1) + 1;
        for g in grid_lines(t0, t1, self.axis.px_per_tick, &self.bars, GRID_MIN_PX) {
            if g.level != GridLevel::Bar || g.tick > self.end_tick() {
                continue;
            }
            let x = self.axis.tick_to_x(g.tick).round() + 0.5;
            sp::line(c, x, top - 0.5, x, bottom + 0.5, t.text_muted);
        }
        let end = end_x.round() + 0.5;
        if end <= b.x1 {
            sp::line(c, end, top - 0.5, end, bottom + 0.5, t.text);
            sp::line(c, end + 3.0, top - 0.5, end + 3.0, bottom + 0.5, t.text);
        }

        self.paint_gutter(c, top, lh);
        self.paint_chords(c);
        self.paint_symbols_and_notes(c, w);
        self.paint_rhythm(c, w, bottom);
        self.paint_lyrics(c, w, bottom);
        self.paint_cursor(c, lh);

        if let Some(p) = self.playhead_content() {
            let x = self.axis.tick_to_x(p);
            if x >= x0 && x <= b.x1 {
                sp::playhead(c, x, ruler.y0, bottom + lh, self.st.playhead);
            }
        }
        c.pop_clip();
    }

    fn paint_marks(&self, c: &mut dyn Canvas, ruler: Rect) {
        let t = &self.st.base;
        for (tick, bpm) in &self.tempo {
            let x = self.axis.tick_to_x(*tick);
            if x < ruler.x0 - 60.0 || x > ruler.x1 {
                continue;
            }
            // A drawn quarter note, then "= bpm".
            let (hx, hy) = (x + 6.0, ruler.y1 - 12.0);
            c.fill_with(Circle::new(pt(hx, hy), 2.4), t.text);
            c.stroke_with(
                Line::new(pt(hx + 2.2, hy), pt(hx + 2.2, hy - 8.0)),
                1.0,
                t.text,
            );
            c.text(&TextRun::verbatim(
                format!("= {}", fmt_bpm(*bpm)),
                pt(hx + 6.0, ruler.y1 - 8.0),
                9.0,
                t.text,
            ));
        }
        for (tick, name) in &self.markers {
            let x = self.axis.tick_to_x(*tick);
            if x < ruler.x0 || x > ruler.x1 {
                continue;
            }
            let label = crate::paint::fit_label_raw(name, 90.0, 9.0);
            let w = label.chars().count() as f64 * 5.2 + 8.0;
            let r = Rect::new(x, ruler.y0 + 1.0, x + w, ruler.y0 + 12.0);
            c.fill_with(
                RoundedRect::from_rect(r, 2.0),
                mix(t.highlight, self.st.ruler, 0.45),
            );
            c.text(
                &TextRun::verbatim(label, pt(x + 4.0, ruler.y0 + 10.0), 9.0, t.text).weight(600),
            );
        }
    }

    fn paint_gutter(&self, c: &mut dyn Canvas, top: f64, lh: f64) {
        let Some(w) = &self.work else { return };
        let t = &self.st.base;
        let b = self.bounds;
        sp::rect(
            c,
            Rect::new(
                b.x0,
                top - lh * 0.6,
                self.axis.x0 - 2.0,
                self.staff_bottom() + lh * 0.6,
            ),
            self.st.background,
        );
        let tab = w.tab();
        for (i, s) in tab.tuning.strings.iter().enumerate() {
            let y = top + i as f64 * lh;
            c.text(
                &TextRun::verbatim(
                    s.label.clone(),
                    pt(self.axis.x0 - 6.0, y + 3.5),
                    9.0,
                    t.text_muted,
                )
                .align(Align::End),
            );
        }
        // The clef: TAB spelled down the staff, and the time signature before it.
        let mid = (top + self.staff_bottom()) / 2.0;
        let size = (lh * 0.9).clamp(8.0, 13.0) as f32;
        for (k, ch) in ["T", "A", "B"].iter().enumerate() {
            c.text(
                &TextRun::verbatim(
                    *ch,
                    pt(b.x0 + 10.0, mid + (k as f64 - 1.0) * f64::from(size) + 3.0),
                    size,
                    t.text,
                )
                .align(Align::Center)
                .weight(700),
            );
        }
        let m = self.meter;
        c.text(
            &TextRun::verbatim(
                m.beats_per_bar.to_string(),
                pt(b.x0 + 30.0, mid - 2.0),
                13.0,
                t.text,
            )
            .align(Align::Center)
            .weight(700),
        );
        c.text(
            &TextRun::verbatim(
                m.beat_unit.to_string(),
                pt(b.x0 + 30.0, mid + 12.0),
                13.0,
                t.text,
            )
            .align(Align::Center)
            .weight(700),
        );
    }

    fn paint_chords(&self, c: &mut dyn Canvas) {
        let Some(w) = &self.work else { return };
        let tab = w.tab();
        if tab.diagrams.is_empty() {
            return;
        }
        let starts = Diagram::start_frets_of(&tab.tuning, tab.capo);
        let y = self.bounds.y0 + TOP_BAR_HEIGHT + RULER_HEIGHT + 3.0;
        let h = chord_diagram::diagram_height(DIAGRAM_WIDTH, true);
        let xs: Vec<f64> = tab
            .diagrams
            .iter()
            .map(|d| self.axis.tick_to_x(d.tick))
            .collect();
        for (i, d) in tab.diagrams.iter().enumerate() {
            let x = xs[i] - 4.0;
            let next = xs.get(i + 1).copied().unwrap_or(f64::MAX);
            if x + DIAGRAM_WIDTH < self.axis.x0 || x > self.bounds.x1 {
                continue;
            }
            if next - xs[i] >= DIAGRAM_WIDTH {
                let dia = Diagram {
                    name: Some(&d.name),
                    voicing: &d.voicing,
                    start_frets: &starts,
                };
                chord_diagram::paint_diagram(
                    c,
                    &self.st.base,
                    Rect::new(x, y, x + DIAGRAM_WIDTH, y + h),
                    &dia,
                );
            } else {
                // Too close for a diagram: the name alone.
                c.text(
                    &TextRun::verbatim(
                        d.name.clone(),
                        pt(xs[i], y + 14.0),
                        11.0,
                        self.st.base.text,
                    )
                    .weight(700),
                );
            }
        }
    }

    fn paint_symbols_and_notes(&self, c: &mut dyn Canvas, w: &super::Work) {
        let t = &self.st.base;
        let lh = self.line_h();
        let top = self.staff_top();
        let (xa, xz) = (self.axis.x0 - 20.0, self.bounds.x1 + 20.0);
        let n = self.strings();
        let mut by_string: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, p) in w.placed.iter().enumerate() {
            if let Some(v) = by_string.get_mut(usize::from(p.string)) {
                v.push(i);
            }
        }
        self.paint_spans(c, w, top);
        let size = (lh * 0.78).clamp(8.0, 12.0) as f32;
        for list in &by_string {
            for (k, &i) in list.iter().enumerate() {
                let p = &w.placed[i];
                let x = self.axis.tick_to_x(p.tick);
                if x < xa || x > xz {
                    continue;
                }
                let y = top + f64::from(p.string) * lh;
                let text = fret_text(p);
                let tw = text.chars().count() as f64 * f64::from(size) * 0.6 + 3.0;
                let r = Rect::new(x - tw / 2.0, y - lh * 0.42, x + tw / 2.0, y + lh * 0.42);
                c.fill_with(r, self.st.background);
                c.text(
                    &TextRun::verbatim(text, pt(x, y + f64::from(size) * 0.36), size, t.text)
                        .align(Align::Center)
                        .weight(700),
                );
                let prev = k.checked_sub(1).map(|j| &w.placed[list[j]]);
                let next = list.get(k + 1).map(|&j| &w.placed[j]);
                self.paint_techniques(c, p, x, y, prev, next, lh);
            }
        }
        // Notes no string can play: their pitch names under the staff.
        for (i, (tick, key)) in w.unplaced.iter().enumerate() {
            let x = self.axis.tick_to_x(*tick);
            c.text(&TextRun::verbatim(
                format!("?{}", atomik_model::theory::pitch_name(*key)),
                pt(x, self.staff_bottom() + lh * 0.9 + (i % 2) as f64 * 9.0),
                8.0,
                t.warning,
            ));
        }
    }

    /// Palm mute and let ring: a label and a dashed line over the run of columns that carry it.
    fn paint_spans(&self, c: &mut dyn Canvas, w: &super::Work, top: f64) {
        let t = &self.st.base;
        for (flag, label, y) in [
            (TechFlags::PALM_MUTE, "P.M.", top - 14.0),
            (TechFlags::LET_RING, "let ring", top - 22.0),
        ] {
            let mut ticks: Vec<i64> = w.placed.iter().map(|p| p.tick).collect();
            ticks.dedup();
            let on = |tick: i64| w.placed.iter().any(|p| p.tick == tick && p.tech.has(flag));
            let mut i = 0;
            while i < ticks.len() {
                if !on(ticks[i]) {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j + 1 < ticks.len() && on(ticks[j + 1]) {
                    j += 1;
                }
                let (xa, xz) = (
                    self.axis.tick_to_x(ticks[i]) - 5.0,
                    self.axis.tick_to_x(ticks[j]) + 9.0,
                );
                if xz >= self.axis.x0 && xa <= self.bounds.x1 {
                    let label = if flag == TechFlags::LET_RING {
                        crate::i18n::tr(label).into_owned()
                    } else {
                        label.to_string()
                    };
                    c.text(&TextRun::verbatim(label, pt(xa, y), 9.0, t.text).weight(600));
                    let mut x = xa + 26.0;
                    while x < xz {
                        sp::line(c, x, y - 3.0, (x + 3.0).min(xz), y - 3.0, t.text_muted);
                        x += 6.0;
                    }
                    sp::line(c, xz, y - 3.0, xz, y + 2.0, t.text_muted);
                }
                i = j + 1;
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one note's drawing context")]
    fn paint_techniques(
        &self,
        c: &mut dyn Canvas,
        p: &Placed,
        x: f64,
        y: f64,
        prev: Option<&Placed>,
        next: Option<&Placed>,
        lh: f64,
    ) {
        let t = &self.st.base;
        let ink = t.text;
        let small = |c: &mut dyn Canvas, s: &str, tx: f64, ty: f64| {
            c.text(
                &TextRun::verbatim(s, pt(tx, ty), 8.5, ink)
                    .align(Align::Center)
                    .weight(600),
            );
        };
        let up = y - lh * 0.55;
        let tech = &p.tech;
        // Hammer-on and pull-off mark the destination: an arc back to the previous note.
        for (flag, letter) in [(TechFlags::HAMMER_ON, "H"), (TechFlags::PULL_OFF, "P")] {
            if tech.has(flag)
                && let Some(from) = prev
            {
                let xf = self.axis.tick_to_x(from.tick);
                let (a, z) = (xf + 5.0, x - 5.0);
                if z > a {
                    let mut path = BezPath::new();
                    path.move_to(pt(a, up));
                    path.quad_to(pt((a + z) / 2.0, up - 7.0), pt(z, up));
                    c.stroke_with(path, 1.0, ink);
                    small(c, letter, (a + z) / 2.0, up - 8.0);
                }
            }
        }
        // Slides leave from the origin note (a line to the next fret) or enter or leave the note.
        if let Some(s) = tech.slide {
            match s {
                SlideKind::Legato | SlideKind::Shift => {
                    if let Some(nx) = next {
                        let xn = self.axis.tick_to_x(nx.tick);
                        let rising = nx.fret >= p.fret;
                        let (ya, yz) = if rising {
                            (y + 3.0, y - 3.0)
                        } else {
                            (y - 3.0, y + 3.0)
                        };
                        if xn - x > 18.0 {
                            sp::line(c, x + 8.0, ya, xn - 8.0, yz, ink);
                        }
                    }
                }
                SlideKind::InFromBelow => sp::line(c, x - 15.0, y + 5.0, x - 6.0, y - 3.0, ink),
                SlideKind::InFromAbove => sp::line(c, x - 15.0, y - 5.0, x - 6.0, y + 3.0, ink),
                SlideKind::OutDown => sp::line(c, x + 6.0, y - 3.0, x + 15.0, y + 5.0, ink),
                SlideKind::OutUp => sp::line(c, x + 6.0, y + 3.0, x + 15.0, y - 5.0, ink),
            }
        }
        if let Some(b) = &tech.bend {
            self.paint_bend(c, b, x, y, up);
        }
        if let Some(h) = tech.harmonic {
            let label = match h.kind {
                HarmonicKind::Natural => None,
                HarmonicKind::Artificial => Some("A.H."),
                HarmonicKind::Pinch => Some("P.H."),
                HarmonicKind::Tapped => Some("T.H."),
                HarmonicKind::Semi => Some("S.H."),
            };
            if let Some(l) = label {
                small(c, l, x, up - 2.0);
            }
        }
        if let Some(v) = tech.vibrato {
            let amp = if v == VibratoKind::Wide { 3.2 } else { 1.8 };
            let mut path = BezPath::new();
            let (x0, y0) = (x + 6.0, up - 1.0);
            path.move_to(pt(x0, y0));
            for k in 0..6 {
                let dx = f64::from(k + 1) * 3.0;
                let dy = if k % 2 == 0 { -amp } else { amp };
                path.line_to(pt(x0 + dx, y0 + dy));
            }
            c.stroke_with(path, 1.0, ink);
        }
        let marks: [(TechFlags, &str); 7] = [
            (TechFlags::TAP, "T"),
            (TechFlags::SLAP, "S"),
            (TechFlags::POP, "P"),
            (TechFlags::THUMB, "Th"),
            (TechFlags::ACCENT, ">"),
            (TechFlags::HEAVY_ACCENT, "^"),
            (TechFlags::STACCATO, "."),
        ];
        let mut off = 0.0;
        for (flag, s) in marks {
            if tech.has(flag) {
                small(c, s, x + off, up - 11.0);
                off += 8.0;
            }
        }
        if tech.tremolo_division > 0 {
            for k in 0..3 {
                let dy = f64::from(k) * 2.5;
                sp::line(
                    c,
                    x - 3.0,
                    up - 4.0 - dy + 2.0,
                    x + 3.0,
                    up - 4.0 - dy - 2.0,
                    ink,
                );
            }
        }
    }

    fn paint_bend(&self, c: &mut dyn Canvas, b: &atomik_tab::score::Bend, x: f64, y: f64, up: f64) {
        let ink = self.st.base.text;
        let kind = b.kind();
        let peak = b.peak();
        let label = bend_label(peak);
        let head = |c: &mut dyn Canvas, hx: f64, hy: f64, dir: f64| {
            let mut path = BezPath::new();
            path.move_to(pt(hx, hy));
            path.line_to(pt(hx - 2.5, hy + 4.0 * dir));
            path.line_to(pt(hx + 2.5, hy + 4.0 * dir));
            path.close_path();
            c.fill_with(path, ink);
        };
        let tx = x + 9.0;
        let ty = up - 10.0;
        match kind {
            BendKind::PreBend | BendKind::PreBendRelease => {
                c.stroke_with(Line::new(pt(x - 1.0, y - 5.0), pt(x - 1.0, ty)), 1.0, ink);
                head(c, x - 1.0, ty, 1.0);
            }
            _ => {
                let mut path = BezPath::new();
                path.move_to(pt(x + 5.0, y - 2.0));
                path.quad_to(pt(tx, y - 2.0), pt(tx, ty));
                c.stroke_with(path, 1.0, ink);
                head(c, tx, ty, 1.0);
            }
        }
        if matches!(kind, BendKind::BendRelease | BendKind::PreBendRelease) {
            let rx = tx + 9.0;
            let mut path = BezPath::new();
            path.move_to(pt(tx, ty));
            path.quad_to(pt(rx, ty), pt(rx, y - 3.0));
            c.stroke_with(path, 1.0, mix(ink, self.st.background, 0.2));
            head(c, rx, y - 3.0, -1.0);
        }
        c.text(
            &TextRun::new(
                label,
                pt(
                    if matches!(kind, BendKind::PreBend | BendKind::PreBendRelease) {
                        x - 1.0
                    } else {
                        tx
                    },
                    ty - 3.0,
                ),
                8.5,
                ink,
            )
            .align(Align::Center)
            .weight(600),
        );
    }

    /// Stems, beams and flags under the staff, from the note lengths: a column is the notes that
    /// start together; its value is the longest of them cut at the next column.
    fn paint_rhythm(&self, c: &mut dyn Canvas, w: &super::Work, bottom: f64) {
        let t = &self.st.base;
        let lh = self.line_h();
        let stem_top = bottom + lh * 0.8 + 4.0;
        let stem_bottom = stem_top + 18.0;
        let mut cols: Vec<(i64, i64)> = Vec::new();
        for p in &w.placed {
            match cols.last_mut() {
                Some((tick, len)) if *tick == p.tick => *len = (*len).max(p.length),
                _ => cols.push((p.tick, p.length)),
            }
        }
        struct Col {
            x: f64,
            value: NoteValue,
            beat: i64,
        }
        let mut list: Vec<Col> = Vec::with_capacity(cols.len());
        for (i, &(tick, len)) in cols.iter().enumerate() {
            let bar_end = self.bars.next_bar_line(tick);
            let gap = cols.get(i + 1).map_or(bar_end - tick, |n| n.0 - tick);
            let value = NoteValue::classify(len.min(gap).max(1));
            let beat = tick.div_euclid(self.bars.beat_ticks_at(tick).max(1));
            list.push(Col {
                x: self.axis.tick_to_x(tick),
                value,
                beat,
            });
        }
        let visible = |x: f64| x >= self.axis.x0 - 30.0 && x <= self.bounds.x1 + 30.0;
        for col in list.iter().filter(|c| visible(c.x)) {
            let x = col.x.round() + 0.5;
            if col.value.division == 1 {
                sp::line(c, x - 3.0, stem_top + 4.0, x + 3.0, stem_top + 4.0, t.text);
            } else {
                sp::line(c, x, stem_top, x, stem_bottom, t.text);
            }
            if col.value.dotted {
                c.fill_with(Circle::new(pt(x + 4.0, stem_bottom - 2.0), 1.1), t.text);
            }
            if col.value.triplet {
                c.text(
                    &TextRun::verbatim("3", pt(x, stem_bottom + 10.0), 8.0, t.text_muted)
                        .align(Align::Center),
                );
            }
        }
        // Beams: runs of columns in one beat that carry a flag; level L joins neighbours that
        // both have at least L flags; a column with no such neighbour gets a stub.
        let mut i = 0;
        while i < list.len() {
            if list[i].value.flags() == 0 {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < list.len()
                && list[j + 1].value.flags() > 0
                && list[j + 1].beat == list[i].beat
            {
                j += 1;
            }
            for level in 1..=4u8 {
                let y = stem_bottom - f64::from(level - 1) * 3.5;
                for k in i..=j {
                    if list[k].value.flags() < level || !visible(list[k].x) {
                        continue;
                    }
                    let x = list[k].x.round() + 0.5;
                    let joined_right = k < j && list[k + 1].value.flags() >= level;
                    let joined_left = k > i && list[k - 1].value.flags() >= level;
                    if joined_right {
                        let xn = list[k + 1].x.round() + 0.5;
                        c.stroke_with(Line::new(pt(x, y), pt(xn, y)), 2.0, t.text);
                    } else if !joined_left {
                        let dir = if j > i && k == j { -1.0 } else { 1.0 };
                        c.stroke_with(Line::new(pt(x, y), pt(x + 6.0 * dir, y - 3.0)), 1.6, t.text);
                    } else {
                        c.stroke_with(Line::new(pt(x, y), pt(x - 6.0, y)), 2.0, t.text);
                    }
                }
            }
            i = j + 1;
        }
    }

    fn paint_lyrics(&self, c: &mut dyn Canvas, w: &super::Work, bottom: f64) {
        let tab = w.tab();
        if tab.lyrics.is_empty() {
            return;
        }
        let lh = self.line_h();
        let y = bottom + lh * 0.8 + 4.0 + 18.0 + 16.0;
        let xs: Vec<f64> = tab
            .lyrics
            .iter()
            .map(|l| self.axis.tick_to_x(l.tick))
            .collect();
        for (i, l) in tab.lyrics.iter().enumerate() {
            let x = xs[i];
            if x < self.axis.x0 - 40.0 || x > self.bounds.x1 {
                continue;
            }
            let room = xs.get(i + 1).map_or(120.0, |n| n - x - 2.0);
            let text = crate::paint::fit_label_raw(&l.text, room.max(8.0), 10.0);
            c.text(&TextRun::verbatim(
                text,
                pt(x - 3.0, y),
                10.0,
                self.st.base.text,
            ));
        }
    }

    fn paint_cursor(&self, c: &mut dyn Canvas, lh: f64) {
        let (tick, string) = self.cursor;
        let x = self.axis.tick_to_x(tick);
        if x < self.axis.x0 - 10.0 || x > self.bounds.x1 + 10.0 {
            return;
        }
        let y = self.string_y(string);
        let col = if self.focused {
            self.st.base.highlight
        } else {
            self.st.base.text_muted
        };
        let r = Rect::new(x - 9.0, y - lh * 0.5, x + 9.0, y + lh * 0.5);
        c.fill_with(r, col.with_alpha(0.22));
        c.stroke_with(RoundedRect::from_rect(r, 2.0), 1.2, col);
    }
}

fn fmt_bpm(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.005 {
        format!("{}", bpm.round() as i64)
    } else {
        format!("{bpm:.1}")
    }
}

/// The fret as shown on the line: dead `x`, ghost `(5)`, harmonic `<5>`.
fn fret_text(p: &Placed) -> String {
    let n = p.fret.to_string();
    if p.tech.has(TechFlags::DEAD) {
        "x".to_string()
    } else if p.tech.harmonic.is_some() {
        format!("<{n}>")
    } else if p.tech.has(TechFlags::GHOST) {
        format!("({n})")
    } else {
        n
    }
}

/// The amount written over a bend arrow.
fn bend_label(cents: i32) -> String {
    match cents {
        c if c <= 0 => String::new(),
        50 => "1/4".into(),
        100 => "1/2".into(),
        150 => "3/4".into(),
        200 => "full".into(),
        250 => "1 1/4".into(),
        300 => "1 1/2".into(),
        400 => "2".into(),
        c => format!("{:.1}", f64::from(c) / 200.0),
    }
}
