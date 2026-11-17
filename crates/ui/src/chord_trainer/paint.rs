//! Painting the chord trainer: toolbar, chord strip, the stage with the current and next diagram, the
//! scores table and the panels over them.

use super::{ChordTrainerView, Frame};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::chord_diagram::{self, Diagram};
use crate::seq_paint as sp;
use crate::text_field::FieldColors;
use crate::theme::mix;
use atomik_tab::trainer::{DrillMode, pair_label};
use atomik_tab::voicing::Voicing;
use vello::kurbo::{Affine, Circle, Rect, RoundedRect};

const PANEL_PAD: f64 = 10.0;
const SCORE_ROW: f64 = 16.0;

/// The Reset button of the scores panel.
pub(super) fn reset_rect(scores: Rect) -> Rect {
    Rect::new(
        scores.x1 - 62.0,
        scores.y0 + 5.0,
        scores.x1 - 8.0,
        scores.y0 + 23.0,
    )
}

/// Where the stage puts its parts.
struct Geom {
    stats: Rect,
    now_label: Rect,
    now: Rect,
    next_label: Rect,
    next: Rect,
    beats: Rect,
    hint: Rect,
}

fn geom(stage: Rect) -> Geom {
    let stats = Rect::new(
        stage.x0 + 8.0,
        stage.y0 + 4.0,
        stage.x1 - 8.0,
        stage.y0 + 22.0,
    );
    let label_y = stage.y0 + 26.0;
    let top = label_y + 20.0;
    let room_h = (stage.y1 - top - 44.0).max(40.0);
    let room_w = (stage.width() / 2.0 - 36.0).max(40.0);
    let dw = (room_h / 1.37).min(room_w).clamp(46.0, 150.0);
    let dh = chord_diagram::diagram_height(dw, true);
    let gap = 40.0;
    let total = 2.0 * dw + gap;
    let x0 = stage.center().x - total / 2.0;
    let now = Rect::new(x0, top, x0 + dw, top + dh);
    let next = Rect::new(x0 + dw + gap, top, x0 + 2.0 * dw + gap, top + dh);
    Geom {
        stats,
        now_label: Rect::new(now.x0, label_y, now.x1, label_y + 18.0),
        now,
        next_label: Rect::new(next.x0, label_y, next.x1, label_y + 18.0),
        next,
        beats: Rect::new(stage.x0 + 8.0, now.y1 + 8.0, stage.x1 - 8.0, now.y1 + 24.0),
        hint: Rect::new(stage.x0 + 8.0, now.y1 + 26.0, stage.x1 - 8.0, now.y1 + 42.0),
    }
}

/// The previous and next fingering buttons, beside the "Now" label.
pub(super) fn voicing_buttons(stage: &Rect) -> Option<(Rect, Rect)> {
    let g = geom(*stage);
    let y = g.now_label.y0;
    Some((
        Rect::new(g.now_label.x1 - 44.0, y, g.now_label.x1 - 22.0, y + 18.0),
        Rect::new(g.now_label.x1 - 20.0, y, g.now_label.x1 + 2.0, y + 18.0),
    ))
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl ChordTrainerView {
    pub(super) fn paint_all(&mut self, canvas: &mut dyn Canvas) {
        let c = canvas;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&self.bounds.into());
        sp::rect(c, self.bounds, self.st.background);
        let f = self.frame();
        self.paint_strip(c, &f);
        self.paint_stage(c, &f);
        self.paint_scores(c, &f);
        self.paint_toolbar(c, &f);
        if let Some(msg) = self.status.clone() {
            sp::label(
                c,
                &msg,
                f.stage.x0 + 10.0,
                f.stage.y1 - 8.0,
                10.0,
                self.st.base.text_muted,
                Align::Start,
            );
        }
        if self.picker {
            self.paint_picker(c);
        }
        if self.list_edit.is_some() {
            self.paint_list_editor(c);
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
                        sp::button(c, &self.st, item.rect, "", lit, hover);
                        let col = if lit.is_some() { t.on_accent } else { t.text };
                        c.text(
                            &TextRun::verbatim(
                                &item.text,
                                pt(item.rect.center().x, item.rect.center().y + 3.5),
                                9.5,
                                col,
                            )
                            .align(Align::Center)
                            .weight(700),
                        );
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
                            if item.lit { t.highlight } else { t.text },
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

    /// The fingering of chord `index` of the set, with the start frets the diagram needs.
    fn diagram_of(&mut self, index: usize) -> Option<(String, Voicing)> {
        let chord = self.set.get(index)?.clone();
        let (v, ..) = self.voicing_of(&chord)?;
        Some((chord.name(), v))
    }

    fn paint_strip(&mut self, c: &mut dyn Canvas, f: &Frame) {
        let t = self.st.base.clone();
        sp::rect(c, f.strip, mix(self.st.background, t.text, 0.03));
        sp::line(
            c,
            f.strip.x0,
            f.strip.y1 - 0.5,
            f.strip.x1,
            f.strip.y1 - 0.5,
            self.st.separator,
        );
        c.push_clip(&f.strip.into());
        if self.set.is_empty() {
            let msg = match self.source {
                super::Source::Key => "",
                super::Source::List => "Type some chords with Edit List",
                super::Source::Song => "Add chords to the chord lane or import a chord sheet",
            };
            sp::label(
                c,
                msg,
                f.strip.x0 + 10.0,
                f.strip.center().y + 3.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        let starts = Diagram::start_frets_of(self.tuning(), self.capo_of());
        let current = self
            .drill
            .as_ref()
            .filter(|d| d.running())
            .map(|d| d.current());
        for i in 0..self.set.len() {
            let cell = self.strip_cell(f.strip, i);
            if cell.x1 < f.strip.x0 || cell.x0 > f.strip.x1 {
                continue;
            }
            let back = cell.inflate(3.0, 3.0);
            if current == Some(i) {
                c.fill_with(
                    RoundedRect::from_rect(back, 4.0),
                    mix(t.highlight, self.st.background, 0.7),
                );
            }
            if self.selected == i {
                c.stroke_with(RoundedRect::from_rect(back, 4.0), 1.0, t.highlight);
            }
            match self.diagram_of(i) {
                Some((name, v)) => chord_diagram::paint_diagram(
                    c,
                    &t,
                    cell,
                    &Diagram {
                        name: Some(&name),
                        voicing: &v,
                        start_frets: &starts,
                    },
                ),
                None => {
                    let name = self.set[i].name();
                    c.text(
                        &TextRun::verbatim(
                            &name,
                            pt(cell.center().x, cell.y0 + 14.0),
                            11.0,
                            t.text_muted,
                        )
                        .align(Align::Center)
                        .weight(700),
                    );
                }
            }
        }
        c.pop_clip();
    }

    fn paint_stage(&mut self, c: &mut dyn Canvas, f: &Frame) {
        let t = self.st.base.clone();
        let g = geom(f.stage);
        c.push_clip(&f.stage.into());
        let n = self.set.len();
        if n < 2 {
            sp::label(
                c,
                "Pick at least two chords for the drill",
                f.stage.center().x,
                f.stage.center().y,
                11.0,
                t.text_muted,
                Align::Center,
            );
            c.pop_clip();
            return;
        }
        let running = self.running();
        let (cur, nxt) = match &self.drill {
            Some(d) if d.running() => (d.current(), d.next()),
            _ => (self.selected.min(n - 1), (self.selected + 1) % n),
        };
        self.paint_stats(c, &g);
        // The "Now" and "Next" diagrams.
        sp::label(
            c,
            "Now",
            g.now_label.x0,
            g.now_label.y0 + 13.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        sp::label(
            c,
            "Next",
            g.next_label.x0,
            g.next_label.y0 + 13.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        if let Some((prev, next)) = voicing_buttons(&f.stage) {
            let hover = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
            sp::button(c, &self.st, prev, "<", None, hover(prev));
            sp::button(c, &self.st, next, ">", None, hover(next));
        }
        let starts = Diagram::start_frets_of(self.tuning(), self.capo_of());
        for (index, area, dim) in [(cur, g.now, false), (nxt, g.next, true)] {
            if dim {
                c.push_clip(&area.inflate(0.0, 2.0).into());
            }
            match self.diagram_of(index) {
                Some((name, v)) => chord_diagram::paint_diagram(
                    c,
                    &t,
                    area,
                    &Diagram {
                        name: Some(&name),
                        voicing: &v,
                        start_frets: &starts,
                    },
                ),
                None => {
                    let name = self.set[index].name();
                    c.text(
                        &TextRun::verbatim(
                            &name,
                            pt(area.center().x, area.y0 + 20.0),
                            16.0,
                            t.text,
                        )
                        .align(Align::Center)
                        .weight(700),
                    );
                    sp::label(
                        c,
                        "No fingering found",
                        area.center().x,
                        area.center().y,
                        10.0,
                        t.text_muted,
                        Align::Center,
                    );
                }
            }
            if dim {
                sp::rect(
                    c,
                    area.inflate(0.0, 2.0),
                    self.st.background.with_alpha(0.35),
                );
                c.pop_clip();
            }
        }
        // The arrow between them.
        sp::label(
            c,
            ">",
            (g.now.x1 + g.next.x0) / 2.0,
            g.now.center().y,
            18.0,
            t.text_muted,
            Align::Center,
        );
        self.paint_beats(c, &g, running);
        let hint = match self.cfg.mode {
            DrillMode::Timed => "Change to the next chord on the accented click",
            DrillMode::Verified => "Change on the accented click: the input checks each chord",
            DrillMode::Listen => "Play the chord shown: the drill moves on when it hears it",
        };
        if !running {
            sp::label(
                c,
                hint,
                g.hint.x0,
                g.hint.y0 + 11.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        c.pop_clip();
    }

    fn paint_stats(&self, c: &mut dyn Canvas, g: &Geom) {
        let t = &self.st.base;
        let Some(d) = &self.drill else {
            return;
        };
        let now = self.now;
        let mut x = g.stats.x0;
        let mut put = |c: &mut dyn Canvas, text: String| {
            let w = text.chars().count() as f64 * 6.0 + 18.0;
            sp::label(c, &text, x, g.stats.y0 + 12.0, 10.5, t.text, Align::Start);
            x += w;
        };
        put(c, format!("Changes: {}", d.changes()));
        put(c, format!("Per minute: {:.1}", d.changes_per_minute(now)));
        put(c, format!("Time: {}", clock(d.elapsed(now))));
        if self.cfg.mode != DrillMode::Timed {
            let (hits, judged) = d.hits();
            put(c, format!("Hits: {hits} of {judged}"));
            if d.running() {
                let col = if d.recognised() {
                    t.meter_low
                } else {
                    t.led_off
                };
                c.fill_with(Circle::new(pt(x + 5.0, g.stats.y0 + 8.0), 5.0), col);
                let word = if d.recognised() { "Heard" } else { "Not heard" };
                sp::label(
                    c,
                    word,
                    x + 14.0,
                    g.stats.y0 + 12.0,
                    10.5,
                    t.text,
                    Align::Start,
                );
            }
        }
    }

    fn paint_beats(&self, c: &mut dyn Canvas, g: &Geom, running: bool) {
        let t = &self.st.base;
        if !running || self.cfg.mode == DrillMode::Listen {
            return;
        }
        let Some(d) = &self.drill else { return };
        let beats = u32::from(self.cfg.beats);
        let lit = u32::from(d.beat_in_chord());
        let step = 20.0_f64.min(g.beats.width() / f64::from(beats.max(1)));
        let x0 = g.beats.center().x - step * f64::from(beats) / 2.0 + step / 2.0;
        for i in 0..beats {
            let on = i <= lit;
            let col = if on {
                if i == 0 { t.highlight } else { t.text }
            } else {
                t.led_off
            };
            let r = if i == 0 { 5.5 } else { 4.0 };
            c.fill_with(
                Circle::new(pt(x0 + f64::from(i) * step, g.beats.center().y), r),
                col,
            );
        }
    }

    fn paint_scores(&self, c: &mut dyn Canvas, f: &Frame) {
        let t = &self.st.base;
        let r = f.scores;
        sp::rect(c, r, mix(self.st.background, t.text, 0.03));
        sp::line(c, r.x0 + 0.5, r.y0, r.x0 + 0.5, r.y1, self.st.separator);
        c.push_clip(&r.into());
        c.text(
            &TextRun::new("Best scores", pt(r.x0 + 10.0, r.y0 + 17.0), 11.0, t.text).weight(700),
        );
        let reset = reset_rect(r);
        let hover = self.hover.is_some_and(|h| reset.contains(h));
        sp::button(c, &self.st, reset, "Reset", None, hover);
        let head_y = r.y0 + 40.0;
        let cols = [
            r.x0 + 10.0,
            r.x0 + r.width() * 0.46,
            r.x0 + r.width() * 0.68,
            r.x0 + r.width() * 0.88,
        ];
        for (x, text) in cols.iter().zip(["Pair", "Per min", "Hit rate", "Runs"]) {
            sp::label(c, text, *x, head_y, 9.5, t.text_muted, Align::Start);
        }
        sp::line(
            c,
            r.x0 + 8.0,
            head_y + 4.0,
            r.x1 - 8.0,
            head_y + 4.0,
            self.st.separator,
        );
        if self.scores.is_empty() {
            sp::label(
                c,
                "No scores yet: run a drill with listening on",
                r.x0 + 10.0,
                head_y + 24.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
            c.pop_clip();
            return;
        }
        let mut rows: Vec<_> = self.scores.rows().collect();
        rows.sort_by(|a, b| b.1.cpm.total_cmp(&a.1.cpm).then(a.0.cmp(b.0)));
        let fit = (((r.y1 - head_y - 14.0) / SCORE_ROW).floor() as usize).max(1);
        let current = self.drill.as_ref().filter(|d| d.running()).and_then(|d| {
            let a = d.names().get(d.current())?;
            let b = d.names().get(d.previous()?)?;
            Some(atomik_tab::trainer::pair_key(a, b))
        });
        for (i, (key, best)) in rows.iter().take(fit).enumerate() {
            let y = head_y + 12.0 + (i as f64 + 1.0) * SCORE_ROW - 4.0;
            if current.as_deref() == Some(*key) {
                sp::rect(
                    c,
                    Rect::new(r.x0 + 4.0, y - 12.0, r.x1 - 4.0, y + 3.0),
                    mix(t.highlight, self.st.background, 0.7),
                );
            }
            c.text(&TextRun::verbatim(
                pair_label(key),
                pt(cols[0], y),
                10.0,
                t.text,
            ));
            let cpm = if best.cpm > 0.0 {
                format!("{:.0}", best.cpm)
            } else {
                "-".to_owned()
            };
            c.text(&TextRun::verbatim(cpm, pt(cols[1], y), 10.0, t.text));
            let hit = format!("{:.0}%", best.hit_rate * 100.0);
            c.text(&TextRun::verbatim(hit, pt(cols[2], y), 10.0, t.text));
            c.text(&TextRun::verbatim(
                best.sessions.to_string(),
                pt(cols[3], y),
                10.0,
                t.text_muted,
            ));
        }
        if rows.len() > fit {
            let more = format!("{} more", rows.len() - fit);
            sp::label(
                c,
                &more,
                r.x0 + 10.0,
                r.y1 - 6.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
        }
        c.pop_clip();
    }

    fn paint_picker(&self, c: &mut dyn Canvas) {
        let (panel, rows) = crate::chord_sheet::paint::picker_layout(self.bounds);
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

    fn paint_list_editor(&self, c: &mut dyn Canvas) {
        let Some(e) = &self.list_edit else { return };
        let t = &self.st.base;
        sp::rect(c, self.bounds, self.st.background.with_alpha(0.72));
        let w = 480.0_f64.min(self.bounds.width() - 16.0);
        let m = self.bounds.center();
        let panel = Rect::new(m.x - w / 2.0, m.y - 52.0, m.x + w / 2.0, m.y + 52.0);
        let rr = RoundedRect::from_rect(panel, 5.0);
        c.fill_with(rr, mix(self.st.header, t.text, 0.06));
        c.stroke_with(rr, 1.0, t.bevel_dark);
        c.text(
            &TextRun::new(
                "Chords for the drill",
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
        e.field
            .paint(c, field, 12.0, 16.0, FieldColors::of(t), true);
        let bad = self.typed_invalid();
        if bad.is_empty() {
            sp::label(
                c,
                "Chord names separated by spaces or commas. Enter applies, Esc cancels",
                panel.x0 + PANEL_PAD,
                panel.y1 - 10.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
        } else {
            let msg = crate::i18n::tf("Not chords: {words}", &[("words", &bad.join(" "))]);
            c.text(&TextRun::verbatim(
                msg,
                pt(panel.x0 + PANEL_PAD, panel.y1 - 10.0),
                9.5,
                t.clip,
            ));
        }
    }
}
