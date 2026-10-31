//! The rack navigator: a fixed-width overview strip at the right edge of the rack area.
//!
//! It draws every column as a scaled column of device blocks (device colour, folded heights
//! respected, the Audio I/O strip on top of column 0) and the part of the rack the rack area
//! shows now. A press centres the viewport on the point (a press inside the viewport keeps its
//! grab point instead, so it does not jump), a drag follows the pointer, the wheel scrolls the
//! rack as it does over the rack. The navigator never starts a device drag.
//!
//! **Scale.** One uniform scale fits the whole content in the strip: the narrowest of "all
//! columns fit the width" and "the whole height fits". A very tall rack therefore draws small
//! blocks (never thinner than [`MIN_BLOCK_H`] pixels) rather than scrolling the overview, so the
//! strip always shows the entire rack and the viewport rectangle is the only thing that moves.
//!
//! **Cost.** The blocks are simple rectangles recorded once into a cached display list, keyed
//! by the layout revision (bumped by every rebuild of the device views) and the strip geometry;
//! scrolling, selecting and hovering redraw only the viewport rectangle and the selection
//! outlines. No device panel is painted.

use super::{Drag, RackView};
use crate::canvas::{Canvas, CanvasExt};
use crate::display_list::DisplayList;
use crate::event::{Modifiers, PointerButton};
use crate::paint;
use atomik_device_sdk::RACK_WIDTH;
use vello::kurbo::{Point, Rect, RoundedRect, Size, Vec2};

/// Width of the strip, logical pixels (the UI scale factor multiplies it on screen).
pub const NAVIGATOR_WIDTH: f64 = 136.0;
/// Space between the strip's edge and the overview.
const PAD: f64 = 8.0;
/// The rack area never gets narrower than this because of the navigator: a window too narrow
/// for both hides the strip.
const MIN_AREA_WIDTH: f64 = 240.0;
/// The thinnest block drawn (pixels), so a tall rack keeps every device visible.
const MIN_BLOCK_H: f64 = 1.5;
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

/// What the cached block list was recorded for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NavKey {
    layout: u64,
    origin: Point,
    scale: f64,
    columns: u16,
}

/// The cached list of blocks.
#[derive(Debug)]
pub(super) struct NavCache {
    key: NavKey,
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
        let key = NavKey {
            layout: self.nav_layout,
            origin: g.origin,
            scale: g.scale,
            columns: self.columns,
        };
        if self.nav_cache.as_ref().is_none_or(|n| n.key != key) {
            self.nav_revision += 1;
            let mut list = DisplayList::cached(CACHE_ID, self.nav_revision);
            self.record_blocks(&mut list, &g);
            self.nav_cache = Some(NavCache { key, list });
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

    /// Records the column backgrounds and the device blocks.
    fn record_blocks(&self, list: &mut DisplayList, g: &NavGeo) {
        let theme = &self.theme;
        for column in 0..self.column_count() {
            let x = self.column_x(column);
            let bg = Rect::new(x, 0.0, x + f64::from(RACK_WIDTH), g.size.height);
            paint::fill_rect(list, g.rect_to_view(bg), theme.lcd);
            for (_, r, color) in self.column_device_rects(column) {
                let mut v = g.rect_to_view(r);
                // A hair between blocks, except where they would vanish.
                if v.height() > 3.0 {
                    v.y1 -= 0.75;
                    v.y0 += 0.75;
                }
                if v.height() < MIN_BLOCK_H {
                    v.y1 = v.y0 + MIN_BLOCK_H;
                }
                paint::fill_rect(list, v, color.with_alpha(0.9));
            }
        }
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
}
