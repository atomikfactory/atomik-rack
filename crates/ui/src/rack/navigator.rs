//! The rack navigator: a fixed-width overview strip at the right edge of the rack area.
//!
//! It draws every column as a miniature of the real rack: each device is its own panel picture
//! (the display list the rack records for it) replayed at the strip's scale, folded heights
//! respected, the Audio I/O strip on top of column 0; the part of the rack the rack area shows
//! now sits on top. A press centres the viewport on the point (a press inside the viewport keeps its
//! grab point instead, so it does not jump), a drag follows the pointer, the wheel scrolls the
//! rack as it does over the rack. The navigator never starts a device drag.
//!
//! **Scale.** One uniform scale fits the whole content in the strip: the narrowest of "all
//! columns fit the width" and "the whole height fits". A very tall rack therefore draws small
//! pictures rather than scrolling the overview, so the strip always shows the entire rack and
//! the viewport rectangle is the only thing that moves. A panel shorter than [`MIN_PICTURE_H`]
//! pixels is a flat block of its device colour instead (never thinner than [`MIN_BLOCK_H`]).
//!
//! **Detail.** Ops too small to see are dropped; text of a readable size is kept, text that
//! would be a smudge is drawn as a bar of its colour, text below [`MIN_TEXT_PX`] is dropped.
//!
//! **Cost.** Each panel is recorded for the strip once per picture change ([`Mini`], keyed by
//! the panel's revision without its live meter and scope feeds, so playback never re-records
//! it; a panel the rack already recorded is reused). The miniature is one cached display list,
//! assembled again only when a panel picture, the layout revision (bumped by every rebuild of
//! the device views) or the strip geometry changes; scrolling, selecting and hovering redraw
//! only the viewport rectangle and the selection outlines.

use super::{Drag, RackView};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::display_list::{DisplayList, Op};
use crate::event::{Modifiers, PointerButton};
use crate::paint;
use atomik_device_sdk::RACK_WIDTH;
use vello::kurbo::{Affine, Point, Rect, RoundedRect, Size, Vec2};

/// Width of the strip, logical pixels (the UI scale factor multiplies it on screen).
pub const NAVIGATOR_WIDTH: f64 = 136.0;
/// Space between the strip's edge and the overview.
const PAD: f64 = 8.0;
/// The rack area never gets narrower than this because of the navigator: a window too narrow
/// for both hides the strip.
const MIN_AREA_WIDTH: f64 = 240.0;
/// The thinnest block drawn (pixels), so a tall rack keeps every device visible.
const MIN_BLOCK_H: f64 = 1.5;
/// A panel drawn shorter than this (pixels) is a flat block: its controls would be dust.
const MIN_PICTURE_H: f64 = 4.0;
/// Text drawn smaller than this (pixels) is dropped; up to [`TEXT_PX`] it is a bar.
const MIN_TEXT_PX: f64 = 1.4;
/// Text drawn at least this big (pixels) is kept as text.
const TEXT_PX: f64 = 4.0;
/// A fill smaller than this in both directions (pixels) is invisible and dropped.
const MIN_FILL_PX: f64 = 0.3;
/// Display-list cache id of the blocks (device lists use small ids).
const CACHE_ID: u64 = u64::MAX - 1;

/// The mapping between rack coordinates and the strip.
#[derive(Debug, Clone, Copy)]
pub(super) struct NavGeo {
    /// Strip pixels per rack unit.
    pub scale: f64,
    /// Where rack point (0, 0) is, view coordinates.
    pub origin: Point,
    /// The content size, rack coordinates.
    pub size: Size,
}

impl NavGeo {
    /// A rack point in the strip.
    pub fn to_view(self, p: Point) -> Point {
        Point::new(
            self.origin.x + p.x * self.scale,
            self.origin.y + p.y * self.scale,
        )
    }

    /// A strip point as a rack point.
    pub fn to_rack(self, p: Point) -> Point {
        Point::new(
            (p.x - self.origin.x) / self.scale,
            (p.y - self.origin.y) / self.scale,
        )
    }

    /// A rack rectangle in the strip.
    pub fn rect_to_view(&self, r: Rect) -> Rect {
        let (a, b) = (
            self.to_view(r.origin()),
            self.to_view(Point::new(r.x1, r.y1)),
        );
        Rect::new(a.x, a.y, b.x, b.y)
    }
}

/// What the cached miniature was recorded for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NavKey {
    layout: u64,
    origin: Point,
    scale: f64,
    columns: u16,
    /// 0: the front of the panels, 1: the back.
    side: usize,
}

/// The cached miniature.
#[derive(Debug)]
pub(super) struct NavCache {
    key: NavKey,
    /// The picture revision of every device view the list was assembled from.
    stills: Vec<u64>,
    list: DisplayList,
}

/// One panel as the navigator shows it: the panel's own display list and what it was
/// recorded for.
#[derive(Debug, Clone)]
pub(super) struct Mini {
    /// `DeviceView::still_revision` when recorded.
    still: u64,
    side: usize,
    list: DisplayList,
}

impl RackView {
    // ------------------------------------------------------------------ the setting

    /// Shows or hides the rack navigator (hidden by default in the view; the app persists the
    /// choice and turns it on). The rack area gives up [`NAVIGATOR_WIDTH`] while it shows.
    pub fn set_navigator_visible(&mut self, visible: bool) {
        if visible != self.navigator {
            self.navigator = visible;
            self.nav_hover = false;
            if !visible && matches!(self.drag, Drag::Navigator { .. }) {
                self.drag = Drag::None;
            }
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Whether the navigator is switched on (it may still be hidden by a window too narrow).
    pub fn navigator_visible(&self) -> bool {
        self.navigator
    }

    // ------------------------------------------------------------------ geometry

    /// The strip, view coordinates: `None` while the navigator is off or the view too narrow.
    pub fn navigator_rect(&self) -> Option<Rect> {
        if !self.navigator {
            return None;
        }
        let x0 = self.browser_edge();
        (self.size.width - x0 >= NAVIGATOR_WIDTH + MIN_AREA_WIDTH).then(|| {
            Rect::new(
                self.size.width - NAVIGATOR_WIDTH,
                self.bar_height(),
                self.size.width,
                self.size.height,
            )
        })
    }

    /// The navigator's mapping, or `None` when it is hidden or has nothing to show.
    pub(super) fn nav_geo(&self) -> Option<NavGeo> {
        let strip = self.navigator_rect()?;
        let inner = Rect::new(
            strip.x0 + PAD,
            strip.y0 + PAD,
            strip.x1 - PAD,
            strip.y1 - PAD,
        );
        let size = self.content_size();
        if inner.width() <= 0.0 || inner.height() <= 0.0 || size.width <= 0.0 || size.height <= 0.0
        {
            return None;
        }
        let scale = (inner.width() / size.width).min(inner.height() / size.height);
        let origin = Point::new(
            inner.x0 + (inner.width() - size.width * scale) / 2.0,
            inner.y0,
        );
        Some(NavGeo {
            scale,
            origin,
            size,
        })
    }

    /// The position in the strip of rack point `p`.
    pub fn navigator_point(&self, p: Point) -> Option<Point> {
        self.nav_geo().map(|g| g.to_view(p))
    }

    /// The viewport rectangle in the strip (the part of the rack the rack area shows, cut to
    /// the rack).
    pub fn navigator_viewport(&self) -> Option<Rect> {
        let g = self.nav_geo()?;
        let content = Rect::from_origin_size(Point::ZERO, g.size);
        Some(g.rect_to_view(self.viewport_rack_rect().intersect(content)))
    }

    /// Whether a view position is over the strip.
    pub(super) fn in_navigator(&self, pos: Point) -> bool {
        self.navigator_rect().is_some_and(|r| r.contains(pos))
    }

    // ------------------------------------------------------------------ input

    /// A press over the strip: starts following the pointer. Returns whether the press was
    /// over the strip (it is consumed either way).
    pub(super) fn navigator_press(&mut self, pos: Point, button: PointerButton) -> bool {
        if !self.in_navigator(pos) {
            return false;
        }
        self.nav_hover = true;
        if button != PointerButton::Primary {
            return true;
        }
        let Some(g) = self.nav_geo() else {
            return true;
        };
        let vp = self.viewport_rack_rect();
        let rp = g.to_rack(pos);
        // Inside the viewport the grab point stays under the pointer; outside it the viewport
        // is centred on the pointer.
        let grab = if vp.contains(rp) {
            rp - vp.origin()
        } else {
            Vec2::new(vp.width() / 2.0, vp.height() / 2.0)
        };
        self.drag = Drag::Navigator { grab };
        self.navigator_follow(pos);
        true
    }

    /// Moves the viewport so the grab point sits under `pos`.
    pub(super) fn navigator_follow(&mut self, pos: Point) {
        let (Some(g), Drag::Navigator { grab }) = (self.nav_geo(), &self.drag) else {
            return;
        };
        let top_left = g.to_rack(pos) - *grab;
        self.scroll_to_rack(top_left);
    }

    /// Hover feedback of the pointer moving over (or off) the strip.
    pub(super) fn navigator_hover(&mut self, pos: Point) {
        let over = self.in_navigator(pos);
        if over != self.nav_hover {
            self.nav_hover = over;
            self.repaint = true;
        }
    }

    /// The wheel over the strip scrolls the rack as over the rack (Shift: sideways). Returns
    /// whether the pointer was over the strip.
    pub(super) fn navigator_wheel(
        &mut self,
        delta: Vec2,
        pos: Point,
        modifiers: Modifiers,
    ) -> bool {
        if !self.in_navigator(pos) {
            return false;
        }
        self.wheel_scroll(delta, modifiers);
        true
    }

    // ------------------------------------------------------------------ painting

    /// Paints the strip (view coordinates).
    pub(super) fn paint_navigator(&mut self, c: &mut dyn Canvas) {
        let Some(strip) = self.navigator_rect() else {
            return;
        };
        let theme = self.theme.clone();
        paint::fill_rect(c, strip, theme.plate);
        paint::fill_rect(
            c,
            Rect::new(strip.x0, strip.y0, strip.x0 + 1.0, strip.y1),
            theme.section_line,
        );
        let Some(g) = self.nav_geo() else { return };
        let side = self.flip as usize;
        let key = NavKey {
            layout: self.nav_layout,
            origin: g.origin,
            scale: g.scale,
            columns: self.columns,
            side,
        };
        let current = self.nav_cache.as_ref().is_some_and(|n| {
            n.key == key
                && self
                    .devices
                    .iter()
                    .map(super::DeviceView::still_revision)
                    .eq(n.stills.iter().copied())
        });
        if !current {
            self.refresh_minis(side);
            self.nav_revision += 1;
            let mut list = DisplayList::cached(CACHE_ID, self.nav_revision);
            self.record_blocks(&mut list, &g);
            let stills = self
                .devices
                .iter()
                .map(super::DeviceView::still_revision)
                .collect();
            self.nav_cache = Some(NavCache { key, stills, list });
        }
        if let Some(cache) = &self.nav_cache {
            c.draw_list(&cache.list);
        }
        // The selected devices.
        for key in &self.selection {
            if let Some(i) = self.device_index(key) {
                let r = g.rect_to_view(self.devices[i].rect());
                c.stroke(
                    &r.into(),
                    &vello::kurbo::Stroke::new(1.0),
                    &theme.selection.into(),
                );
            }
        }
        // The viewport.
        if let Some(vp) = self.navigator_viewport() {
            let hot = self.nav_hover || matches!(self.drag, Drag::Navigator { .. });
            let shape = RoundedRect::from_rect(vp, 1.5);
            c.fill_with(
                shape,
                theme.selection.with_alpha(if hot { 0.28 } else { 0.14 }),
            );
            c.stroke_with(
                shape,
                1.5,
                theme.selection.with_alpha(if hot { 1.0 } else { 0.8 }),
            );
        }
    }

    /// Records the panel of every device the miniature does not hold a current picture of
    /// (a panel the rack recorded already is reused; one recorded here is kept for the rack).
    fn refresh_minis(&mut self, side: usize) {
        for i in 0..self.devices.len() {
            let d = &self.devices[i];
            let still = d.still_revision();
            if d.mini
                .as_ref()
                .is_some_and(|m| m.still == still && m.side == side)
            {
                continue;
            }
            let list = match &d.lists[side] {
                Some(l) if l.revision() == d.revision => l.clone(),
                _ => {
                    let l = self.record_panel_list(i, side);
                    self.devices[i].lists[side] = Some(l.clone());
                    l
                }
            };
            self.devices[i].mini = Some(Mini { still, side, list });
        }
    }

    /// Records the column backgrounds and the device pictures.
    fn record_blocks(&self, list: &mut DisplayList, g: &NavGeo) {
        let theme = &self.theme;
        for column in 0..self.column_count() {
            let x = self.column_x(column);
            let bg = Rect::new(x, 0.0, x + f64::from(RACK_WIDTH), g.size.height);
            paint::fill_rect(list, g.rect_to_view(bg), theme.lcd);
            for d in self.devices.iter().filter(|d| d.column == column) {
                let mut v = g.rect_to_view(d.rect());
                match &d.mini {
                    Some(m) if v.height() >= MIN_PICTURE_H => {
                        list.push_transform(
                            Affine::translate((v.x0, v.y0)) * Affine::scale(g.scale),
                        );
                        replay_scaled(&m.list, list, g.scale);
                        list.pop_transform();
                    }
                    _ => {
                        // A hair between blocks, except where they would vanish.
                        if v.height() > 3.0 {
                            v.y1 -= 0.75;
                            v.y0 += 0.75;
                        }
                        if v.height() < MIN_BLOCK_H {
                            v.y1 = v.y0 + MIN_BLOCK_H;
                        }
                        paint::fill_rect(list, v, d.accent.with_alpha(0.9));
                    }
                }
            }
        }
    }
}

/// Replays a panel's ops into `out` under a transform that scales by `scale`, dropping what
/// the scale makes invisible: fills smaller than [`MIN_FILL_PX`] and text below
/// [`MIN_TEXT_PX`]; text that would be a smudge becomes a bar.
fn replay_scaled(panel: &DisplayList, out: &mut DisplayList, scale: f64) {
    let mut depth = 0u32;
    for op in panel.ops() {
        match op {
            Op::Fill { shape, brush } => {
                // Inside a nested transform the bounding box is not in panel units.
                if depth == 0 {
                    let b = shape.bounding_box();
                    if b.width() * scale < MIN_FILL_PX && b.height() * scale < MIN_FILL_PX {
                        continue;
                    }
                }
                out.fill(shape, brush);
            }
            Op::Stroke {
                shape,
                stroke,
                brush,
            } => out.stroke(shape, stroke, brush),
            Op::PushClip(s) => out.push_clip(s),
            Op::PopClip => out.pop_clip(),
            Op::PushTransform(t) => {
                depth += 1;
                out.push_transform(*t);
            }
            Op::PopTransform => {
                depth = depth.saturating_sub(1);
                out.pop_transform();
            }
            Op::Text(run) => text_at_scale(out, run, scale, depth == 0),
        }
    }
}

/// One text run of a miniature: kept as text when big enough, a bar of its colour when small,
/// nothing when tiny (`flat`: the run is in panel units, so a bar can be sized from it).
fn text_at_scale(out: &mut DisplayList, run: &TextRun, scale: f64, flat: bool) {
    let px = f64::from(run.size) * scale;
    if px >= TEXT_PX {
        out.text(run);
    } else if px >= MIN_TEXT_PX && flat && !run.text.trim().is_empty() {
        let size = f64::from(run.size);
        let width = run.text.chars().count() as f64 * size * 0.55;
        let x0 = match run.align {
            Align::Start => run.pos.x,
            Align::Center => run.pos.x - width / 2.0,
            Align::End => run.pos.x - width,
        };
        let height = (size * 0.5).max(1.0 / scale);
        let bar = Rect::new(x0, run.pos.y - height, x0 + width, run.pos.y);
        out.fill_with(bar, run.color.with_alpha(0.45));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Theme, UiEvent};
    use atomik_device_sdk::Registry;
    use atomik_model::{RackDevice, RackDoc};
    use std::collections::BTreeMap;

    fn setup(columns: u16, window: Size) -> (RackView, RackDoc, Registry) {
        let mut reg = Registry::new();
        atomik_devices::register_all(&mut reg).expect("register");
        let devices = (0..9)
            .map(|i| RackDevice {
                key: atomik_model::DeviceKey(format!("d{i}")),
                type_id: "atomik.stereo-delay".to_string(),
                name: format!("d{i}"),
                params: BTreeMap::new(),
                folded: false,
                color: None,
                zones: Vec::new(),
                steps: None,
                curves: None,
                drums: None,
                inner: None,
                plugin: None,
                column: i % columns,
                patch_name: String::new(),
                midi_out: None,
            })
            .collect::<Vec<_>>();
        let mut devices = devices;
        devices.sort_by_key(|d| d.column);
        let doc = RackDoc {
            sample_rate: 48_000,
            tempo_bpm: 120.0,
            seed: 0,
            output_channels: 2,
            input_channels: 0,
            assets: BTreeMap::new(),
            devices,
            columns,
            cables: Vec::new(),
            song: atomik_model::empty_song(120.0).into(),
            console: atomik_model::Console::default(),
            midi_maps: Vec::new(),
            macros: Default::default(),
            info: Default::default(),
        };
        let mut v = RackView::new(Theme::dark());
        v.set_navigator_visible(true);
        let _ = v.handle(&UiEvent::Resize(window), &doc, &reg);
        v.sync(&doc, &reg);
        (v, doc, reg)
    }

    #[test]
    fn the_scale_fits_every_column_and_the_whole_height() {
        for columns in [1, 2, 5] {
            let (v, _, _) = setup(columns, Size::new(1400.0, 500.0));
            let g = v.nav_geo().expect("shown");
            let inner_w = NAVIGATOR_WIDTH - 2.0 * PAD;
            assert!(
                g.size.width * g.scale <= inner_w + 1e-9,
                "{columns} columns"
            );
            assert!(g.size.height * g.scale <= 500.0 - 2.0 * PAD + 1e-9);
            // One of the two bounds is tight.
            let tight_w = (g.size.width * g.scale - inner_w).abs() < 1e-9;
            let tight_h = (g.size.height * g.scale - (500.0 - 2.0 * PAD)).abs() < 1e-9;
            assert!(tight_w || tight_h, "{columns} columns");
        }
    }

    #[test]
    fn rack_and_strip_coordinates_round_trip() {
        let (v, _, _) = setup(3, Size::new(1400.0, 800.0));
        let g = v.nav_geo().unwrap();
        let p = Point::new(812.5, 140.25);
        let back = g.to_rack(g.to_view(p));
        assert!((back - p).hypot() < 1e-9);
    }

    #[test]
    fn the_blocks_are_recorded_again_only_when_the_layout_changes() {
        let (mut v, mut doc, reg) = setup(2, Size::new(1400.0, 800.0));
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        assert_eq!(v.nav_revision, 1);
        // Scrolling, selecting and hovering touch the viewport and outlines only.
        v.scroll_by(Vec2::new(200.0, 100.0));
        v.set_selected(Some(atomik_model::DeviceKey("d1".to_string())));
        v.navigator_hover(v.navigator_rect().unwrap().center());
        let mut again = DisplayList::new();
        v.paint(&mut again, &doc, &reg);
        assert_eq!(v.nav_revision, 1);
        // A folded device changes a block's height: the list is recorded again.
        doc.devices[1].folded = true;
        v.sync(&doc, &reg);
        v.paint(&mut again, &doc, &reg);
        assert_eq!(v.nav_revision, 2);
        // So does a resize that moves the strip.
        let _ = v.handle(&UiEvent::Resize(Size::new(1300.0, 800.0)), &doc, &reg);
        v.paint(&mut again, &doc, &reg);
        assert_eq!(v.nav_revision, 3);
    }

    #[test]
    fn a_folded_device_is_a_shorter_block() {
        let (mut v, mut doc, reg) = setup(1, Size::new(1400.0, 2000.0));
        let tall = v.column_device_rects(0)[2].1.height();
        doc.devices[1].folded = true;
        v.sync(&doc, &reg);
        assert!(v.column_device_rects(0)[2].1.height() < tall);
    }

    #[test]
    fn a_window_too_narrow_for_both_hides_the_strip() {
        let (v, _, _) = setup(1, Size::new(NAVIGATOR_WIDTH + MIN_AREA_WIDTH - 1.0, 600.0));
        assert!(v.navigator_visible());
        assert!(v.navigator_rect().is_none());
    }

    fn nav_ops(v: &RackView) -> &[Op] {
        v.nav_cache.as_ref().expect("recorded").list.ops()
    }

    #[test]
    fn the_miniature_replays_the_device_panels() {
        let (mut v, doc, reg) = setup(1, Size::new(1400.0, 1400.0));
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        let g = v.nav_geo().unwrap();
        // A panel is dozens of ops (plate, ears, knobs, jacks), not one block.
        let fills = nav_ops(&v)
            .iter()
            .filter(|op| matches!(op, Op::Fill { .. }))
            .count();
        assert!(fills > 30 * v.devices.len(), "{fills} fills");
        // Every device's picture sits under a scale transform to its slot in the strip.
        let slot = g.rect_to_view(v.devices[3].rect());
        let want = Affine::translate((slot.x0, slot.y0)) * Affine::scale(g.scale);
        assert!(
            nav_ops(&v)
                .iter()
                .any(|op| matches!(op, Op::PushTransform(t) if *t == want))
        );
    }

    #[test]
    fn small_text_is_a_bar_and_tiny_text_is_dropped() {
        let mut panel = DisplayList::new();
        let c = vello::peniko::Color::WHITE;
        panel.text(&TextRun::new("Large", Point::new(10.0, 20.0), 20.0, c));
        panel.text(&TextRun::new("Small", Point::new(10.0, 40.0), 8.0, c));
        panel.text(&TextRun::new("Tiny", Point::new(10.0, 60.0), 4.0, c));
        let mut out = DisplayList::new();
        replay_scaled(&panel, &mut out, 0.3);
        // 6 px text stays text, 2.4 px text is one bar, 1.2 px text is gone.
        assert_eq!(out.texts(), ["Large"]);
        assert_eq!(out.len(), 2);
        assert!(matches!(out.ops()[1], Op::Fill { .. }));
    }

    #[test]
    fn invisible_fills_are_dropped() {
        let mut panel = DisplayList::new();
        let c = vello::peniko::Color::WHITE;
        paint::fill_rect(&mut panel, Rect::new(0.0, 0.0, 1.0, 1.0), c);
        paint::fill_rect(&mut panel, Rect::new(0.0, 0.0, 100.0, 1.0), c);
        let mut out = DisplayList::new();
        replay_scaled(&panel, &mut out, 0.1);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_very_tall_rack_falls_back_to_flat_blocks() {
        let (mut v, doc, reg) = setup(1, Size::new(1400.0, 30.0));
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        let g = v.nav_geo().unwrap();
        assert!(
            v.devices[3].rect().height() * g.scale < MIN_PICTURE_H,
            "{} x {}",
            v.devices[3].rect().height(),
            g.scale
        );
        // One background and one block per device, no panel ops.
        assert_eq!(nav_ops(&v).len(), 1 + v.devices.len());
    }

    #[test]
    fn live_meter_feeds_do_not_record_the_miniature_again() {
        let (mut v, mut doc, reg) = setup(1, Size::new(1400.0, 1400.0));
        doc.devices[2].type_id = "atomik.compressor".to_string();
        v.sync(&doc, &reg);
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        assert_eq!(v.nav_revision, 1);
        let key = doc.devices[2].key.clone();
        let rack_revision = v.panel_revisions()[3].1;
        for peak in [0.2_f32, 0.5, 0.9] {
            v.set_device_meter_values(&key, &[peak, peak, 3.0 * peak]);
            v.paint(&mut list, &doc, &reg);
        }
        // The panel in the rack moved on; the miniature did not follow.
        assert!(v.panel_revisions()[3].1 > rack_revision);
        assert_eq!(v.nav_revision, 1);
        // A real change of the picture does.
        v.set_selected_pad(&key, 1);
        doc.devices[2].folded = true;
        v.sync(&doc, &reg);
        v.paint(&mut list, &doc, &reg);
        assert_eq!(v.nav_revision, 2);
    }

    #[test]
    fn only_the_changed_panel_is_recorded_again() {
        let (mut v, mut doc, reg) = setup(1, Size::new(1400.0, 1400.0));
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        let before: Vec<u64> = v
            .devices
            .iter()
            .map(|d| d.mini.as_ref().unwrap().still)
            .collect();
        doc.devices[4].folded = true;
        v.sync(&doc, &reg);
        v.paint(&mut list, &doc, &reg);
        let after: Vec<u64> = v
            .devices
            .iter()
            .map(|d| d.mini.as_ref().unwrap().still)
            .collect();
        let moved: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(moved, [5]);
    }

    #[test]
    fn a_folded_device_is_drawn_folded_in_the_miniature() {
        let (mut v, mut doc, reg) = setup(1, Size::new(1400.0, 1400.0));
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        let open = v.devices[3].mini.as_ref().unwrap().list.len();
        doc.devices[2].folded = true;
        v.sync(&doc, &reg);
        v.paint(&mut list, &doc, &reg);
        let folded = v.devices[3].mini.as_ref().unwrap().list.len();
        assert!(folded < open, "{folded} vs {open}");
    }
}
