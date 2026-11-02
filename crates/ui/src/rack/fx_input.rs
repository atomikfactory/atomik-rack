//! Rack view code of the six effect panels: the meter feed, the EQ handles and the
//! compressor's "SC from..." list.

use super::{Drag, EqAxis, EqDrag, RackView, SC_MAX_ROWS, SC_ROW_H, ScMenu, UiAction, param_cmd};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{self, FxKind};
use crate::fx_layout;
use crate::paint;
use crate::sidechain::{self, ScSource};
use crate::widget;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_model::{DeviceKey, RackDoc};
use vello::kurbo::{Point, Rect, RoundedRect};

impl RackView {
    // ------------------------------------------------------------------ meter feed

    /// Feeds the values an effect device publishes (`AppCore::device_meter_values`), once per
    /// frame while the rack shows: the raw values of `Device::meter_values` in the layout of
    /// `docs/DEVICES.md` (compressor `[in peak, out peak, GR dB]`, EQ `[16 band energies,
    /// out peak L, R]`, distortion `[in peak, out peak]`, reverb `[peak L, peak R, tail RMS]`,
    /// mastering `[M, S, I LUFS, TP, GR 1-3, limiter GR]`, Character `[in peak, out peak,
    /// transform_eff, activity]`; the modulation publishes none).
    ///
    /// The view turns them into display values ([`fx::update_display`]: dB, quantised, with a
    /// per-call release, since the view has no clock), re-records the panel only when a
    /// displayed value changed and requests a repaint only when that panel is on screen, so a
    /// quiet device settles and stops repainting. Values for unknown devices or devices without
    /// a visual are ignored; the per-device buffer is reused, nothing allocates after the first
    /// call.
    pub fn set_device_meter_values(&mut self, key: &DeviceKey, values: &[f32]) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        if self.devices[i].spec.seq.is_some()
            || self.devices[i].spec.arp.is_some()
            || self.devices[i].spec.drum_gen.is_some()
        {
            self.player_meter_feed(i, values);
            return;
        }
        if self.devices[i].spec.modsrc.is_some() {
            self.modsrc_meter_feed(i, values);
            return;
        }
        if self.devices[i].spec.granular.is_some() {
            self.granular_meter_feed(i, values);
            return;
        }
        if self.devices[i].spec.plugin.is_some() {
            self.plugin_meter_feed(i, values);
            return;
        }
        let Some(kind) = self.devices[i].fx else {
            return;
        };
        if !fx::update_display(kind, values, &mut self.devices[i].meters) {
            return;
        }
        self.devices[i].bump_live();
        if kind == FxKind::Character {
            // The screen's animation advances while the published values move, so an idle
            // device keeps its picture and requests no repaint.
            self.devices[i].anim = self.devices[i].anim.wrapping_add(1);
        }
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// Feeds the analysis of a spectrum or phase meter device (`AppCore::feed_device_scopes`):
    /// the spectrum trace and peaks, the loudest bin and the decimated scope frames, in
    /// [`AnalyserData`](crate::analyser::AnalyserData). The panel is re-recorded and a repaint
    /// requested only when the data changed and the panel is on screen. Devices that are not
    /// analysers are ignored; the stored buffers are reused.
    pub fn set_analyser(&mut self, key: &DeviceKey, data: &crate::analyser::AnalyserData) {
        let Some(i) = self.device_index(key) else {
            return;
        };
        if !matches!(
            self.devices[i].fx,
            Some(FxKind::Spectrum | FxKind::PhaseMeter | FxKind::AudioTrack)
        ) || self.devices[i].analyser == *data
        {
            return;
        }
        self.devices[i].analyser.clone_from(data);
        self.devices[i].bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// The analyser data of a spectrum or phase meter panel (`None` for other devices).
    pub fn analyser_data(&self, key: &DeviceKey) -> Option<&crate::analyser::AnalyserData> {
        let d = &self.devices[self.device_index(key)?];
        matches!(
            d.fx,
            Some(FxKind::Spectrum | FxKind::PhaseMeter | FxKind::AudioTrack)
        )
        .then_some(&d.analyser)
    }

    /// Folds a Mod Source meter frame into the scope ring buffer (no allocation after the
    /// first feed) and repaints the visible panel when a point or the live marker moved.
    fn modsrc_meter_feed(&mut self, i: usize, values: &[f32]) {
        if !self.devices[i].scope.feed(values) {
            return;
        }
        self.devices[i].bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// Keeps the values a granular sampler published (the source, the position and spray, the
    /// grain counts and the live grains) and repaints the visible panel when they changed. The
    /// vector is reused after the first feed.
    fn granular_meter_feed(&mut self, i: usize, values: &[f32]) {
        let n = values.len().min(crate::granular_panel::METER_COUNT);
        let shown = &mut self.devices[i].meters;
        if shown.as_slice() == &values[..n] {
            return;
        }
        shown.clear();
        shown.extend_from_slice(&values[..n]);
        self.devices[i].bump_live();
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, 0.0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let d = &self.devices[i];
        if self.flip == super::Flip::Front && !d.folded && d.y + d.height >= top && d.y <= bottom {
            self.repaint = true;
        }
    }

    /// The scope history of a Mod Source panel (`None` for other devices).
    pub fn modsrc_scope(&self, key: &DeviceKey) -> Option<&crate::modsrc_panel::Scope> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.modsrc.is_some().then_some(&d.scope)
    }

    /// The display meter values of an effect panel (dB; empty before the first feed).
    pub fn device_meters(&self, key: &DeviceKey) -> &[f32] {
        self.device_index(key)
            .map_or(&[], |i| self.devices[i].meters.as_slice())
    }

    // ------------------------------------------------------------------ geometry for hosts and tests

    /// Centre of an EQ band's handle (logical view coordinates), front side only.
    pub fn eq_handle_center(&self, key: &DeviceKey, band: usize) -> Option<Point> {
        if self.flip != super::Flip::Front || band >= 6 {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let fx = d.spec.fx.filter(|f| f.kind == FxKind::Eq)?;
        let bands = fx::eq_bands_of(&d.desc?, d.device.as_ref());
        let p = fx_layout::eq_handle_pos(fx.plot, &bands[band]);
        Some(self.rack_to_view(p + d.offset()))
    }

    /// Centre of the "SC from" button of a compressor panel (logical view coordinates).
    pub fn sc_button_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let b = d.spec.fx?.button?;
        (!d.folded).then(|| self.rack_to_view(b.center() + d.offset()))
    }

    /// Centre of the TUNER button of an audio track panel (logical view coordinates).
    pub fn tuner_button_center(&self, key: &DeviceKey) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        (d.fx == Some(FxKind::AudioTrack) && self.flip == super::Flip::Front && !d.folded)
            .then(|| self.rack_to_view(crate::paint::tuner_button_rect().center() + d.offset()))
    }

    /// A press on the TUNER button: shows or hides the tuner of the track and tells the app
    /// ([`UiAction::SetTuner`]); the lamp follows at once, the app confirms with its next feed.
    pub(super) fn tuner_toggle(&mut self, i: usize, out: &mut Vec<UiAction>) {
        let on = !self.devices[i].analyser.tuner_on;
        self.devices[i].analyser.tuner_on = on;
        if !on {
            self.devices[i].analyser.tuner = None;
        }
        self.devices[i].revision += 1;
        self.repaint = true;
        out.push(UiAction::SetTuner {
            device: self.devices[i].key.clone(),
            on,
        });
    }

    /// The labels of the open "SC from" list (empty when closed).
    pub fn sc_menu_labels(&self) -> Vec<String> {
        self.sc_menu.as_ref().map_or_else(Vec::new, |m| {
            m.entries.iter().map(|e| e.label.clone()).collect()
        })
    }

    /// Centre of row `i` of the open "SC from" list (logical view coordinates).
    pub fn sc_menu_row_center(&self, i: usize) -> Option<Point> {
        let m = self.sc_menu.as_ref()?;
        (i < m.entries.len().min(SC_MAX_ROWS)).then(|| {
            let r = row_rect(m.rect, i);
            self.rack_to_view(r.center())
        })
    }

    // ------------------------------------------------------------------ EQ handles

    fn param_of_key(&self, i: usize, key: &str) -> Option<&'static ParamDescriptor> {
        let desc = self.devices.get(i)?.desc?;
        desc.params.iter().find(|p| p.key == key)
    }

    fn eq_plot(&self, i: usize) -> Option<Rect> {
        let d = self.devices.get(i)?;
        let fx = d.spec.fx.filter(|f| f.kind == FxKind::Eq)?;
        Some(fx.plot + d.offset())
    }

    fn eq_bands(&self, i: usize) -> Option<[fx::EqBand; 6]> {
        let d = self.devices.get(i)?;
        Some(fx::eq_bands_of(&d.desc?, d.device.as_ref()))
    }

    /// A press on an EQ plot: picks a handle (double-click toggles its band) or selects the
    /// device.
    pub(super) fn eq_down(&mut self, i: usize, rp: Point, clicks: u8, out: &mut Vec<UiAction>) {
        let (Some(plot), Some(bands)) = (self.eq_plot(i), self.eq_bands(i)) else {
            return;
        };
        let Some(band) = fx_layout::eq_pick(plot, &bands, rp) else {
            self.select(i, out);
            return;
        };
        let keys = fx::eq_keys(band);
        let key = self.devices[i].key.clone();
        if clicks >= 2 {
            let on = if bands[band].on { 0.0 } else { 1.0 };
            out.push(param_cmd(&key, &keys.on, on, None));
            return;
        }
        let merge = self.next_merge;
        self.next_merge += 1;
        let b = &bands[band];
        self.drag = Drag::Eq(EqDrag {
            dev: i,
            band,
            merge,
            grab: fx_layout::eq_handle_pos(plot, b) - rp,
            origin: rp,
            axis: None,
            start_freq: b.freq as f32,
            start_gain: b.gain as f32,
            freq: b.freq as f32,
            gain: b.gain as f32,
        });
        self.repaint = true;
    }

    /// Moves an EQ handle: x sets the frequency, y the gain; the drag locks to the dominant
    /// axis after 3 px (a cut has no gain: always the frequency), because the model merges
    /// consecutive `SetParam`s of one param only. The result is clamped by the descriptor.
    pub(super) fn eq_move(&mut self, mut e: EqDrag, rp: Point, out: &mut Vec<UiAction>) {
        let Some(plot) = self.eq_plot(e.dev) else {
            return;
        };
        let cut = fx::EQ_SHAPES[e.band].is_cut();
        if e.axis.is_none() {
            let (dx, dy) = (rp.x - e.origin.x, rp.y - e.origin.y);
            if dx.hypot(dy) < 3.0 {
                self.drag = Drag::Eq(e);
                return;
            }
            e.axis = Some(if cut || dx.abs() >= dy.abs() {
                EqAxis::Freq
            } else {
                EqAxis::Gain
            });
        }
        let target = rp + e.grab;
        let keys = fx::eq_keys(e.band);
        let key = self.devices[e.dev].key.clone();
        match e.axis {
            Some(EqAxis::Freq) => {
                if let Some(p) = self.param_of_key(e.dev, &keys.freq) {
                    let v = p.clamp(fx_layout::eq_freq_of_x(plot, target.x) as f32);
                    if v.to_bits() != e.freq.to_bits() {
                        out.push(param_cmd(&key, p.key, v, Some(e.merge)));
                        e.freq = v;
                    }
                }
            }
            Some(EqAxis::Gain) => {
                if let Some(gk) = &keys.gain
                    && let Some(p) = self.param_of_key(e.dev, gk)
                {
                    let g = (fx_layout::eq_gain_of_y(plot, target.y) * 10.0).round() / 10.0;
                    let v = p.clamp(g as f32);
                    if v.to_bits() != e.gain.to_bits() {
                        out.push(param_cmd(&key, p.key, v, Some(e.merge)));
                        e.gain = v;
                    }
                }
            }
            None => {}
        }
        self.drag = Drag::Eq(e);
        self.repaint = true;
    }

    /// Esc during an EQ drag: restores the start value of the param the drag moved.
    pub(super) fn eq_cancel(&mut self, e: &EqDrag, out: &mut Vec<UiAction>) {
        let keys = fx::eq_keys(e.band);
        let key = self.devices[e.dev].key.clone();
        match e.axis {
            Some(EqAxis::Freq) if e.freq.to_bits() != e.start_freq.to_bits() => {
                out.push(param_cmd(&key, &keys.freq, e.start_freq, Some(e.merge)));
            }
            Some(EqAxis::Gain) if e.gain.to_bits() != e.start_gain.to_bits() => {
                if let Some(g) = &keys.gain {
                    out.push(param_cmd(&key, g, e.start_gain, Some(e.merge)));
                }
            }
            _ => {}
        }
        out.push(UiAction::EndMerge);
    }

    /// The wheel over an EQ handle steps its Q. Returns whether a handle took the wheel.
    pub(super) fn eq_wheel(
        &mut self,
        i: usize,
        rp: Point,
        notches: f64,
        fine: bool,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let (Some(plot), Some(bands)) = (self.eq_plot(i), self.eq_bands(i)) else {
            return false;
        };
        let Some(band) = fx_layout::eq_pick(plot, &bands, rp) else {
            return false;
        };
        let keys = fx::eq_keys(band);
        if let Some(p) = self.param_of_key(i, &keys.q) {
            let q = bands[band].q as f32;
            let new = widget::wheel_value(p, q, notches, fine);
            if new.to_bits() != q.to_bits() {
                out.push(param_cmd(&self.devices[i].key, p.key, new, None));
            }
        }
        true
    }

    // ------------------------------------------------------------------ Character matrix

    /// Centre of preset button `preset` (0..16) of a Character panel (logical view
    /// coordinates), front side only.
    pub fn preset_cell_center(&self, key: &DeviceKey, preset: usize) -> Option<Point> {
        if self.flip != super::Flip::Front || preset >= fx::CHARACTER_PRESETS {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        let g = d.spec.fx?.character?;
        let r = crate::character_panel::cell_rect(g.matrix, preset);
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// The `preset` value of device `i`, rounded into the matrix.
    fn preset_of(&self, i: usize) -> usize {
        let d = &self.devices[i];
        let value = match (d.desc, d.device.as_ref()) {
            (Some(desc), Some(dev)) => desc
                .param_by_key("preset")
                .map_or(0.0, |p| widget::current_value(dev, p)),
            _ => 0.0,
        };
        (value.round().max(0.0) as usize).min(fx::CHARACTER_PRESETS - 1)
    }

    /// `SetParam` of `preset` unless it already has that value.
    fn choose_preset(&self, i: usize, n: usize, out: &mut Vec<UiAction>) {
        if n < fx::CHARACTER_PRESETS && n != self.preset_of(i) {
            out.push(param_cmd(&self.devices[i].key, "preset", n as f32, None));
        }
    }

    /// A press on a matrix button: selects the device, focuses the matrix (so the arrow keys
    /// work) and chooses the preset; the active button emits nothing.
    pub(super) fn preset_down(&mut self, i: usize, n: u8, out: &mut Vec<UiAction>) {
        self.select(i, out);
        if let Some(node) = self.tree.find(|h| *h == super::Hit::PresetGrid(i)) {
            self.tree.set_focus(Some(node));
        }
        self.choose_preset(i, usize::from(n), out);
        self.repaint = true;
    }

    /// An arrow key, Home or End with the matrix focused: moves the selection (no wrap).
    pub(super) fn preset_key(&mut self, i: usize, key: &crate::event::Key) -> Option<UiAction> {
        let target = crate::character_panel::move_selection(self.preset_of(i), key)?;
        let mut out = Vec::new();
        self.choose_preset(i, target, &mut out);
        out.pop()
    }

    /// One wheel notch over the matrix: the next (up) or the previous preset.
    pub(super) fn preset_step(&mut self, i: usize, notches: f64) -> Option<UiAction> {
        let now = self.preset_of(i) as f64;
        let target = (now + notches.round()).clamp(0.0, (fx::CHARACTER_PRESETS - 1) as f64);
        let mut out = Vec::new();
        self.choose_preset(i, target as usize, &mut out);
        out.pop()
    }

    /// The name of preset `n` (the descriptor's choice name), for the hover tooltip.
    pub(super) fn preset_name(&self, i: usize, n: u8) -> Option<String> {
        let desc = self.devices.get(i)?.desc?;
        match desc.param_by_key("preset")?.unit {
            atomik_core::Unit::Choice(names) => names.get(usize::from(n)).map(|s| (*s).to_string()),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ SC from...

    /// Opens (or closes) the "SC from" list under the compressor's button.
    pub(super) fn sc_open(&mut self, i: usize, doc: &RackDoc, registry: &Registry) {
        self.repaint = true;
        if self.sc_menu.as_ref().is_some_and(|m| m.dev == i) {
            self.sc_menu = None;
            return;
        }
        let d = &self.devices[i];
        let Some(button) = d.spec.fx.and_then(|f| f.button) else {
            return;
        };
        let entries = sidechain::sidechain_entries(doc, registry, &d.key);
        let current = sidechain::current_source(doc, &d.key);
        let rows = entries.len().min(SC_MAX_ROWS);
        let b = button + d.offset();
        let w = b.width().max(160.0);
        let h = rows as f64 * SC_ROW_H + 4.0;
        let x0 = b.x0.min(d.x + f64::from(RACK_WIDTH) - 16.0 - w);
        let bottom = self.view_to_rack(Point::new(0.0, self.size.height)).y;
        let y0 = if b.y1 + 2.0 + h > bottom {
            b.y0 - 2.0 - h
        } else {
            b.y1 + 2.0
        };
        self.sc_menu = Some(ScMenu {
            dev: i,
            entries,
            current,
            rect: Rect::new(x0, y0, x0 + w, y0 + h),
            hover: None,
        });
    }

    /// A press while the list is open: a row applies its choice as one `Command::Batch`; any
    /// press closes the list and is consumed. Returns whether a press was consumed.
    pub(super) fn sc_press(
        &mut self,
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Some(menu) = self.sc_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        if let Some(row) = row_at(&menu, rp)
            && let Some(entry) = menu.entries.get(row)
        {
            let comp = self.devices[menu.dev].key.clone();
            if let Some(c) = sidechain::sidechain_command(doc, registry, &comp, &entry.source) {
                out.push(UiAction::Command(c));
            }
        }
        true
    }

    pub(super) fn sc_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.sc_menu {
            let hover = row_at(menu, rp);
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Paints the open "SC from" list (rack coordinates).
    pub(super) fn paint_sc_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.sc_menu else { return };
        let theme = &self.theme;
        c.fill_with(
            RoundedRect::from_rect(m.rect.inset(0.0), 4.0),
            theme.tooltip_bg,
        );
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            theme.ink.with_alpha(0.14),
        );
        for (k, e) in m.entries.iter().take(SC_MAX_ROWS).enumerate() {
            let r = row_rect(m.rect, k);
            if m.hover == Some(k) {
                c.fill_with(
                    RoundedRect::from_rect(r.inset(-1.0), 3.0),
                    theme.browser_row,
                );
            }
            let is_current = m.current.as_ref().is_some_and(|cur| *cur == e.source)
                || (m.current.is_none() && e.source == ScSource::Off);
            let tag = match &e.source {
                ScSource::Off => "",
                ScSource::Channel(_) => "CH ",
                ScSource::Device(_) => "",
            };
            let label = paint::fit_label(&format!("{tag}{}", e.label), r.width() - 24.0, 9.5);
            c.text(
                &TextRun::new(
                    if is_current { "\u{2022}" } else { "" },
                    pt(r.x0 + 6.0, r.y1 - 5.5),
                    9.5,
                    theme.highlight,
                )
                .weight(700),
            );
            c.text(
                &TextRun::new(label, pt(r.x0 + 16.0, r.y1 - 5.5), 9.5, theme.text)
                    .align(Align::Start)
                    .weight(if is_current { 700 } else { 400 }),
            );
        }
    }
}

fn row_rect(menu: Rect, i: usize) -> Rect {
    let y = menu.y0 + 2.0 + i as f64 * SC_ROW_H;
    Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + SC_ROW_H)
}

fn row_at(menu: &ScMenu, rp: Point) -> Option<usize> {
    (0..menu.entries.len().min(SC_MAX_ROWS)).find(|&i| row_rect(menu.rect, i).contains(rp))
}
