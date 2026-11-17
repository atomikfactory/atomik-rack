//! The two small modal panels of the tab editor, drawn over the pane and owned by the view: the
//! **Instrument, Tuning and Capo** dialog and the **Re-fret at Position** chooser.
//!
//! Both are plain state plus pure geometry: `layout` turns the pane's rectangle into the
//! rectangles of the controls, `hit` finds the control under a point, `paint` draws them. The
//! view feeds them the events and turns an applied dialog into an undoable edit.
//!
//! Tuning dialog: the instrument families, the instruments of the family, the tuning presets of
//! the instrument, a row of strings whose pitches step by a semitone (Shift: an octave), the
//! capo (0 to 12) and the choice between *keep pitches* (the notes are fretted again on the new
//! neck) and *keep frets* (the frets stay, the pitches change).

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::mix;
use atomik_model::TabData;
use atomik_tab::instrument::{Family, Instrument, catalogue, find_instrument, pitch_class_name};
use atomik_tab::{Capo, Tuning};
use vello::kurbo::{Point, Rect, RoundedRect};

/// The instrument families in list order.
pub const FAMILIES: [Family; 8] = [
    Family::Guitar,
    Family::Bass,
    Family::Ukulele,
    Family::Banjo,
    Family::Mandolin,
    Family::Slide,
    Family::Folk,
    Family::Lute,
];

/// The highest capo the dialog offers.
pub const DIALOG_MAX_CAPO: u8 = 12;

const ROW_H: f64 = 18.0;
const HEAD_H: f64 = 28.0;
const STRINGS_H: f64 = 64.0;
const OPTIONS_H: f64 = 28.0;
const PAD: f64 = 10.0;

/// The modal panel the view shows, if any.
#[derive(Debug, Clone)]
pub(super) enum Modal {
    /// Instrument, tuning and capo.
    Tuning(Box<TuningDialog>),
    /// Re-fret at position.
    Position(PositionPick),
}

/// What a click on a modal asks the view to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    /// Close it.
    Cancel,
    /// Apply the tuning dialog.
    Apply,
    /// Re-fret the selection.
    Refret(atomik_model::tab_edit::RefretTarget),
}

/// The panel's rectangle inside the pane.
pub(super) fn panel_rect(bounds: Rect, w: f64, h: f64) -> Rect {
    let w = w.min(bounds.width() - 16.0).max(120.0);
    let h = h.min(bounds.height() - 16.0).max(80.0);
    let c = bounds.center();
    Rect::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0)
}

fn dim_and_frame(c: &mut dyn Canvas, st: &SeqTheme, bounds: Rect, panel: Rect, title: &str) {
    sp::rect(c, bounds, st.background.with_alpha(0.72));
    let t = &st.base;
    let rr = RoundedRect::from_rect(panel, 5.0);
    c.fill_with(rr, mix(st.header, t.text, 0.06));
    c.stroke_with(rr, 1.0, t.bevel_dark);
    c.text(&TextRun::new(title, pt(panel.x0 + PAD, panel.y0 + 18.0), 11.0, t.text).weight(700));
}

fn row_text(c: &mut dyn Canvas, st: &SeqTheme, r: Rect, text: &str, lit: bool, hover: bool) {
    let t = &st.base;
    if lit {
        c.fill_with(
            RoundedRect::from_rect(r, 2.0),
            mix(t.highlight, st.background, 0.55),
        );
    } else if hover {
        c.fill_with(
            RoundedRect::from_rect(r, 2.0),
            mix(t.text, st.background, 0.9),
        );
    }
    c.text(&TextRun::verbatim(
        text,
        pt(r.x0 + 6.0, r.y0 + 13.0),
        10.0,
        t.text,
    ));
}

// ------------------------------------------------------------------ re-fret chooser

/// The list of positions the selection can be re-fretted at.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PositionPick {
    /// The last position offered.
    pub max_position: u8,
}

/// A control of the position chooser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum PickHit {
    Automatic,
    Lowest,
    Position(u8),
}

impl PositionPick {
    const COLS: u8 = 6;
    const CELL: f64 = 40.0;

    fn rect(&self, bounds: Rect) -> Rect {
        let rows = f64::from(self.max_position.div_ceil(Self::COLS));
        panel_rect(
            bounds,
            2.0 * PAD + f64::from(Self::COLS) * (Self::CELL + 4.0) - 4.0,
            HEAD_H + 40.0 + rows * (ROW_H + 6.0) + 28.0,
        )
    }

    fn cells(&self, bounds: Rect) -> Vec<(Rect, PickHit)> {
        let p = self.rect(bounds);
        let mut out = Vec::new();
        let y = p.y0 + HEAD_H + 6.0;
        let wide = (p.width() - 2.0 * PAD - 8.0) / 2.0;
        out.push((
            Rect::new(p.x0 + PAD, y, p.x0 + PAD + wide, y + 22.0),
            PickHit::Automatic,
        ));
        out.push((
            Rect::new(p.x0 + PAD + wide + 8.0, y, p.x1 - PAD, y + 22.0),
            PickHit::Lowest,
        ));
        for n in 1..=self.max_position {
            let i = f64::from(n - 1);
            let (col, row) = (
                i % f64::from(Self::COLS),
                (i / f64::from(Self::COLS)).floor(),
            );
            let x = p.x0 + PAD + col * (Self::CELL + 4.0);
            let yy = y + 32.0 + row * (ROW_H + 6.0);
            out.push((
                Rect::new(x, yy, x + Self::CELL, yy + ROW_H + 2.0),
                PickHit::Position(n),
            ));
        }
        out
    }

    pub(super) fn hit(&self, bounds: Rect, pos: Point) -> Option<PickHit> {
        self.cells(bounds)
            .into_iter()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, h)| h)
    }

    pub(super) fn contains(&self, bounds: Rect, pos: Point) -> bool {
        self.rect(bounds).contains(pos)
    }

    pub(super) fn paint(
        &self,
        c: &mut dyn Canvas,
        st: &SeqTheme,
        bounds: Rect,
        hover: Option<Point>,
    ) {
        let panel = self.rect(bounds);
        dim_and_frame(c, st, bounds, panel, "Re-fret at Position");
        for (r, h) in self.cells(bounds) {
            let text = match h {
                PickHit::Automatic => crate::i18n::tr("Automatic").into_owned(),
                PickHit::Lowest => crate::i18n::tr("Lowest frets").into_owned(),
                PickHit::Position(n) => n.to_string(),
            };
            sp::button(c, st, r, &text, None, hover.is_some_and(|p| r.contains(p)));
        }
        c.text(&TextRun::new(
            "Position n: frets n to n+3, open strings allowed",
            pt(panel.x0 + PAD, panel.y1 - 9.0),
            9.0,
            st.base.text_muted,
        ));
    }
}

// ------------------------------------------------------------------ tuning dialog

/// The state of the Instrument, Tuning and Capo dialog.
#[derive(Debug, Clone)]
pub(super) struct TuningDialog {
    /// The family shown (index in [`FAMILIES`]).
    pub family: usize,
    /// The chosen catalogue instrument; `None` keeps the clip's own (custom) id.
    pub instrument: Option<&'static str>,
    /// The id the clip had when the dialog opened.
    original_id: String,
    /// The tuning being edited.
    pub tuning: Tuning,
    /// The preset the tuning is, when it still is one.
    pub preset: Option<&'static str>,
    /// The capo fret.
    pub capo: u8,
    original_capo: Capo,
    /// Re-fret the notes to keep their pitches (else the frets stay and the pitches change).
    pub keep_pitches: bool,
    scroll_instruments: usize,
    scroll_presets: usize,
}

/// A control of the tuning dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum DialogHit {
    Cancel,
    Apply,
    Family(usize),
    Instrument(&'static str),
    Preset(&'static str),
    /// String `i` stepped by a semitone (`up`) or down; Shift makes it an octave.
    StringStep(usize, bool),
    CapoStep(i32),
    Keep(bool),
}

struct Layout {
    panel: Rect,
    cancel: Rect,
    apply: Rect,
    families: Rect,
    instruments: Rect,
    presets: Rect,
    strings: Rect,
    options: Rect,
}

impl TuningDialog {
    /// The dialog for a clip's tab data.
    pub(super) fn new(tab: &TabData) -> Self {
        let inst = find_instrument(&tab.instrument_id);
        let family = inst
            .and_then(|i| FAMILIES.iter().position(|f| *f == i.family))
            .unwrap_or(0);
        let preset = inst.and_then(|i| {
            i.tunings
                .iter()
                .find(|t| t.tuning == tab.tuning)
                .map(|t| t.id)
        });
        Self {
            family,
            instrument: inst.map(|i| i.id),
            original_id: tab.instrument_id.clone(),
            tuning: tab.tuning.clone(),
            preset,
            capo: tab.capo.fret.min(DIALOG_MAX_CAPO),
            original_capo: tab.capo,
            keep_pitches: true,
            scroll_instruments: 0,
            scroll_presets: 0,
        }
    }

    /// What Apply sends to the model: the instrument id, the tuning, the capo and whether the
    /// pitches are kept.
    pub(super) fn result(&self) -> (String, Tuning, Capo, bool) {
        let capo = if self.capo == self.original_capo.fret.min(DIALOG_MAX_CAPO)
            && self.original_capo.fret <= DIALOG_MAX_CAPO
        {
            self.original_capo
        } else {
            Capo::at(self.capo)
        };
        (
            self.instrument
                .map_or_else(|| self.original_id.clone(), str::to_owned),
            self.tuning.clone(),
            capo,
            self.keep_pitches,
        )
    }

    fn instruments(&self) -> Vec<&'static Instrument> {
        catalogue()
            .iter()
            .filter(|i| i.family == FAMILIES[self.family.min(FAMILIES.len() - 1)])
            .collect()
    }

    fn presets(&self) -> Vec<(&'static str, &'static str)> {
        self.instrument
            .and_then(find_instrument)
            .map(|i| i.tunings.iter().map(|t| (t.id, t.name)).collect())
            .unwrap_or_default()
    }

    fn layout(&self, bounds: Rect) -> Layout {
        let panel = panel_rect(bounds, 640.0, 400.0);
        let head = Rect::new(panel.x0, panel.y0, panel.x1, panel.y0 + HEAD_H);
        let body_y0 = head.y1 + 4.0;
        let strings_y0 = panel.y1 - OPTIONS_H - STRINGS_H - 2.0 * PAD;
        let lists_h = (strings_y0 - body_y0 - 4.0).max(40.0);
        let fam_w = 104.0;
        let inst_w = 186.0;
        let x0 = panel.x0 + PAD;
        let families = Rect::new(x0, body_y0, x0 + fam_w, body_y0 + lists_h);
        let instruments = Rect::new(
            families.x1 + 6.0,
            body_y0,
            families.x1 + 6.0 + inst_w,
            body_y0 + lists_h,
        );
        let presets = Rect::new(
            instruments.x1 + 6.0,
            body_y0,
            (panel.x1 - PAD).max(instruments.x1 + 80.0),
            body_y0 + lists_h,
        );
        let strings = Rect::new(x0, strings_y0, panel.x1 - PAD, strings_y0 + STRINGS_H);
        let options = Rect::new(
            x0,
            strings.y1 + 4.0,
            panel.x1 - PAD,
            strings.y1 + 4.0 + OPTIONS_H,
        );
        Layout {
            panel,
            cancel: Rect::new(
                panel.x1 - 150.0,
                panel.y0 + 4.0,
                panel.x1 - 80.0,
                panel.y0 + 24.0,
            ),
            apply: Rect::new(
                panel.x1 - 74.0,
                panel.y0 + 4.0,
                panel.x1 - 8.0,
                panel.y0 + 24.0,
            ),
            families,
            instruments,
            presets,
            strings,
            options,
        }
    }

    fn visible(list: Rect) -> usize {
        ((list.height() / ROW_H).floor() as usize).max(1)
    }

    fn string_cells(&self, strings: Rect) -> Vec<(usize, f64, f64)> {
        let n = self.tuning.len().max(1);
        let w = ((strings.width() - 4.0) / n as f64).clamp(28.0, 52.0);
        (0..self.tuning.len())
            .map(|i| (i, strings.x0 + 2.0 + i as f64 * w, w))
            .collect()
    }

    fn option_buttons(&self, o: Rect) -> [(Rect, DialogHit); 4] {
        let y = o.y0 + 4.0;
        let capo_minus = Rect::new(o.x0 + 44.0, y, o.x0 + 64.0, y + 20.0);
        let capo_plus = Rect::new(o.x0 + 96.0, y, o.x0 + 116.0, y + 20.0);
        let keep_x = o.x0 + 136.0;
        let half = ((o.x1 - keep_x - 8.0) / 2.0).max(60.0);
        [
            (capo_minus, DialogHit::CapoStep(-1)),
            (capo_plus, DialogHit::CapoStep(1)),
            (
                Rect::new(keep_x, y, keep_x + half, y + 20.0),
                DialogHit::Keep(true),
            ),
            (
                Rect::new(keep_x + half + 8.0, y, keep_x + 2.0 * half + 8.0, y + 20.0),
                DialogHit::Keep(false),
            ),
        ]
    }

    /// Whether `pos` is inside the panel.
    pub(super) fn contains(&self, bounds: Rect, pos: Point) -> bool {
        self.layout(bounds).panel.contains(pos)
    }

    /// The control under `pos`.
    pub(super) fn hit(&self, bounds: Rect, pos: Point) -> Option<DialogHit> {
        let l = self.layout(bounds);
        if l.cancel.contains(pos) {
            return Some(DialogHit::Cancel);
        }
        if l.apply.contains(pos) {
            return Some(DialogHit::Apply);
        }
        let row = |list: Rect| -> Option<usize> {
            list.contains(pos)
                .then(|| ((pos.y - list.y0) / ROW_H).floor() as usize)
        };
        if let Some(r) = row(l.families)
            && r < FAMILIES.len()
        {
            return Some(DialogHit::Family(r));
        }
        if let Some(r) = row(l.instruments) {
            let list = self.instruments();
            if let Some(i) = list.get(r + self.scroll_instruments) {
                return Some(DialogHit::Instrument(i.id));
            }
        }
        if let Some(r) = row(l.presets)
            && let Some((id, _)) = self.presets().get(r + self.scroll_presets)
        {
            return Some(DialogHit::Preset(id));
        }
        for (i, x, w) in self.string_cells(l.strings) {
            let (up, down) = (
                Rect::new(
                    x + 2.0,
                    l.strings.y0 + 14.0,
                    x + w - 2.0,
                    l.strings.y0 + 28.0,
                ),
                Rect::new(
                    x + 2.0,
                    l.strings.y0 + 46.0,
                    x + w - 2.0,
                    l.strings.y0 + 60.0,
                ),
            );
            if up.contains(pos) {
                return Some(DialogHit::StringStep(i, true));
            }
            if down.contains(pos) {
                return Some(DialogHit::StringStep(i, false));
            }
        }
        self.option_buttons(l.options)
            .into_iter()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, h)| h)
    }

    /// Carries out a click. Returns what the view must do (close or apply), if anything.
    pub(super) fn click(&mut self, hit: DialogHit, octave: bool) -> Option<Outcome> {
        match hit {
            DialogHit::Cancel => return Some(Outcome::Cancel),
            DialogHit::Apply => return Some(Outcome::Apply),
            DialogHit::Family(f) => {
                self.family = f;
                self.scroll_instruments = 0;
            }
            DialogHit::Instrument(id) => {
                if let Some(i) = find_instrument(id) {
                    self.instrument = Some(i.id);
                    self.tuning = i.default_tuning().tuning.clone();
                    self.preset = Some(i.default_tuning().id);
                    self.scroll_presets = 0;
                }
            }
            DialogHit::Preset(id) => {
                if let Some(t) = self.instrument.and_then(|i| find_instrument(i)?.tuning(id)) {
                    self.tuning = t.tuning.clone();
                    self.preset = Some(t.id);
                }
            }
            DialogHit::StringStep(i, up) => {
                let step: i32 = if octave { 12 } else { 1 };
                if let Some(s) = self.tuning.strings.get_mut(i) {
                    let open = i32::from(s.open) + if up { step } else { -step };
                    if (0..=127).contains(&open) {
                        s.open = open as u8;
                        s.label = pitch_class_name(s.open).to_owned();
                        self.preset = None;
                    }
                }
            }
            DialogHit::CapoStep(d) => {
                self.capo = (i32::from(self.capo) + d).clamp(0, i32::from(DIALOG_MAX_CAPO)) as u8;
            }
            DialogHit::Keep(k) => self.keep_pitches = k,
        }
        None
    }

    /// Scrolls the list under `pos` by `rows`.
    pub(super) fn wheel(&mut self, bounds: Rect, pos: Point, rows: i32) {
        let l = self.layout(bounds);
        let bump = |scroll: &mut usize, len: usize, list: Rect| {
            let max = len.saturating_sub(Self::visible(list));
            *scroll = (*scroll as i64 + i64::from(rows)).clamp(0, max as i64) as usize;
        };
        if l.instruments.contains(pos) {
            let n = self.instruments().len();
            bump(&mut self.scroll_instruments, n, l.instruments);
        } else if l.presets.contains(pos) {
            let n = self.presets().len();
            bump(&mut self.scroll_presets, n, l.presets);
        }
    }

    pub(super) fn paint(
        &self,
        c: &mut dyn Canvas,
        st: &SeqTheme,
        bounds: Rect,
        hover: Option<Point>,
    ) {
        let l = self.layout(bounds);
        let t = &st.base;
        let over = |r: Rect| hover.is_some_and(|p| r.contains(p));
        dim_and_frame(c, st, bounds, l.panel, "Instrument, Tuning and Capo");
        sp::button(
            c,
            st,
            l.cancel,
            &crate::i18n::tr("Cancel"),
            None,
            over(l.cancel),
        );
        sp::button(
            c,
            st,
            l.apply,
            &crate::i18n::tr("Apply"),
            Some(t.highlight),
            over(l.apply),
        );
        // Families, instruments, presets.
        for (i, f) in FAMILIES.iter().enumerate() {
            let r = Rect::new(
                l.families.x0,
                l.families.y0 + i as f64 * ROW_H,
                l.families.x1,
                l.families.y0 + (i + 1) as f64 * ROW_H,
            );
            if r.y1 > l.families.y1 + 0.5 {
                break;
            }
            let text = crate::i18n::tr(f.label()).into_owned();
            row_text(c, st, r, &text, i == self.family, over(r));
        }
        for (list, rows, scroll) in [
            (
                l.instruments,
                self.instruments()
                    .into_iter()
                    .map(|i| (i.name.to_owned(), self.instrument == Some(i.id)))
                    .collect::<Vec<_>>(),
                self.scroll_instruments,
            ),
            (
                l.presets,
                self.presets()
                    .into_iter()
                    .map(|(id, name)| (name.to_owned(), self.preset == Some(id)))
                    .collect(),
                self.scroll_presets,
            ),
        ] {
            c.stroke_with(RoundedRect::from_rect(list, 2.0), 1.0, t.bevel_dark);
            c.push_clip(&list.into());
            for (k, (name, lit)) in rows.iter().skip(scroll).enumerate() {
                let r = Rect::new(
                    list.x0,
                    list.y0 + k as f64 * ROW_H,
                    list.x1,
                    list.y0 + (k + 1) as f64 * ROW_H,
                );
                if r.y0 > list.y1 {
                    break;
                }
                row_text(c, st, r, name, *lit, over(r));
            }
            c.pop_clip();
        }
        if self.presets().is_empty() {
            c.text(&TextRun::new(
                "Custom tuning",
                pt(l.presets.x0 + 6.0, l.presets.y0 + 13.0),
                10.0,
                t.text_muted,
            ));
        } else if self.preset.is_none() {
            c.text(
                &TextRun::new(
                    "Custom tuning",
                    pt(l.presets.x1 - 6.0, l.presets.y1 - 4.0),
                    9.0,
                    t.text_muted,
                )
                .align(Align::End),
            );
        }
        // The strings: a pitch per string with step buttons above and below.
        c.text(&TextRun::new(
            "Strings: click to step a semitone, Shift+click an octave",
            pt(l.strings.x0, l.strings.y0 + 9.0),
            9.0,
            t.text_muted,
        ));
        for (i, x, w) in self.string_cells(l.strings) {
            let Some(s) = self.tuning.strings.get(i) else {
                continue;
            };
            let (up, down) = (
                Rect::new(
                    x + 2.0,
                    l.strings.y0 + 14.0,
                    x + w - 2.0,
                    l.strings.y0 + 28.0,
                ),
                Rect::new(
                    x + 2.0,
                    l.strings.y0 + 46.0,
                    x + w - 2.0,
                    l.strings.y0 + 60.0,
                ),
            );
            sp::button(c, st, up, "+", None, over(up));
            sp::button(c, st, down, "-", None, over(down));
            c.text(
                &TextRun::verbatim(
                    atomik_model::theory::pitch_name(s.open),
                    pt(x + w / 2.0, l.strings.y0 + 41.0),
                    10.0,
                    t.text,
                )
                .align(Align::Center)
                .weight(700),
            );
        }
        // Capo and what the notes do.
        let o = l.options;
        c.text(&TextRun::new("Capo", pt(o.x0, o.y0 + 18.0), 10.0, t.text));
        for (r, h) in self.option_buttons(o) {
            let (text, lit) = match h {
                DialogHit::CapoStep(d) => (if d < 0 { "-" } else { "+" }.to_owned(), false),
                DialogHit::Keep(true) => (
                    crate::i18n::tr("Keep pitches (re-fret)").into_owned(),
                    self.keep_pitches,
                ),
                _ => (
                    crate::i18n::tr("Keep frets (pitches change)").into_owned(),
                    !self.keep_pitches,
                ),
            };
            sp::button(c, st, r, &text, lit.then_some(t.highlight), over(r));
        }
        let capo_box = Rect::new(o.x0 + 66.0, o.y0 + 4.0, o.x0 + 94.0, o.y0 + 24.0);
        sp::lcd(c, st, capo_box, None);
        c.text(
            &TextRun::verbatim(
                self.capo.to_string(),
                pt(capo_box.center().x, o.y0 + 18.0),
                10.5,
                t.text,
            )
            .align(Align::Center)
            .weight(700),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 420.0)
    }

    fn guitar() -> TabData {
        TabData::for_instrument("guitar6").expect("catalogue")
    }

    #[test]
    fn it_opens_on_the_clips_instrument_and_preset() {
        let d = TuningDialog::new(&guitar());
        assert_eq!(d.instrument, Some("guitar6"));
        assert_eq!(d.preset, Some("standard"));
        assert_eq!(FAMILIES[d.family], Family::Guitar);
        assert!(d.keep_pitches);
    }

    #[test]
    fn every_catalogue_instrument_is_in_a_listed_family() {
        for i in catalogue() {
            assert!(FAMILIES.contains(&i.family), "{}", i.id);
        }
    }

    #[test]
    fn picking_a_preset_a_string_step_and_the_capo_change_the_result() {
        let mut d = TuningDialog::new(&guitar());
        d.click(DialogHit::Preset("drop_d"), false);
        assert_eq!(d.preset, Some("drop_d"));
        assert_eq!(d.tuning.strings[5].open, 38);
        d.click(DialogHit::StringStep(5, false), false);
        assert_eq!(d.tuning.strings[5].open, 37);
        assert_eq!(d.preset, None, "no longer a preset");
        d.click(DialogHit::StringStep(0, true), true);
        assert_eq!(d.tuning.strings[0].open, 76, "Shift steps an octave");
        d.click(DialogHit::CapoStep(1), false);
        d.click(DialogHit::CapoStep(1), false);
        d.click(DialogHit::Keep(false), false);
        let (id, tuning, capo, keep) = d.result();
        assert_eq!(id, "guitar6");
        assert_eq!(tuning.strings[5].open, 37);
        assert_eq!(capo.fret, 2);
        assert!(!keep);
    }

    #[test]
    fn the_capo_stays_between_zero_and_twelve_and_a_partial_capo_survives_untouched() {
        let mut d = TuningDialog::new(&guitar());
        for _ in 0..20 {
            d.click(DialogHit::CapoStep(1), false);
        }
        assert_eq!(d.capo, DIALOG_MAX_CAPO);
        for _ in 0..20 {
            d.click(DialogHit::CapoStep(-1), false);
        }
        assert_eq!(d.capo, 0);
        let mut tab = guitar();
        tab.capo = Capo::partial(3, 0b11);
        let d = TuningDialog::new(&tab);
        assert_eq!(d.result().2, Capo::partial(3, 0b11));
    }

    #[test]
    fn choosing_an_instrument_takes_its_default_tuning() {
        let mut d = TuningDialog::new(&guitar());
        d.click(DialogHit::Instrument("banjo5"), false);
        assert_eq!(d.result().0, "banjo5");
        assert_eq!(d.tuning.len(), 5);
        assert!(d.tuning.strings.iter().any(|s| s.start_fret == 5));
    }

    #[test]
    fn the_controls_are_found_where_they_are_drawn() {
        let d = TuningDialog::new(&guitar());
        let l = d.layout(bounds());
        assert_eq!(d.hit(bounds(), l.apply.center()), Some(DialogHit::Apply));
        assert_eq!(d.hit(bounds(), l.cancel.center()), Some(DialogHit::Cancel));
        let fam = Point::new(l.families.x0 + 5.0, l.families.y0 + ROW_H * 3.5);
        assert_eq!(d.hit(bounds(), fam), Some(DialogHit::Family(3)));
        let inst = Point::new(l.instruments.x0 + 5.0, l.instruments.y0 + 5.0);
        assert!(matches!(
            d.hit(bounds(), inst),
            Some(DialogHit::Instrument(_))
        ));
        let preset = Point::new(l.presets.x0 + 5.0, l.presets.y0 + 5.0);
        assert!(matches!(
            d.hit(bounds(), preset),
            Some(DialogHit::Preset(_))
        ));
        assert!(d.hit(bounds(), Point::new(1.0, 1.0)).is_none());
    }

    #[test]
    fn a_small_pane_still_holds_the_panel() {
        let small = Rect::new(0.0, 0.0, 420.0, 300.0);
        let d = TuningDialog::new(&guitar());
        let l = d.layout(small);
        assert!(l.panel.x0 >= 0.0 && l.panel.x1 <= 420.0 && l.panel.y1 <= 300.0);
        assert!(l.presets.width() > 0.0 && l.families.height() >= 40.0);
    }

    #[test]
    fn the_position_chooser_lists_automatic_lowest_and_the_positions() {
        let p = PositionPick { max_position: 12 };
        let cells = p.cells(bounds());
        assert_eq!(cells.len(), 14);
        let c = cells
            .iter()
            .find(|(_, h)| *h == PickHit::Position(7))
            .unwrap()
            .0
            .center();
        assert_eq!(p.hit(bounds(), c), Some(PickHit::Position(7)));
        assert_eq!(
            p.hit(bounds(), cells[0].0.center()),
            Some(PickHit::Automatic)
        );
    }
}
