//! Painting of the file browser: everything is O(visible rows).

use super::layout::{self as lay, Layout};
use super::menu::{self, Menu};
use super::{FileBrowserView, Gesture, Hover, Pane, tree};
use crate::action::Place;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use atomik_library::{FileKind, FileRow, LibrarySection, SortKey};
use vello::kurbo::{Affine, BezPath, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

const ROW_FONT: f32 = 11.0;
const TREE_FONT: f32 = 10.0;
const SMALL_FONT: f32 = 9.0;

/// What an empty library section says.
const EMPTY_SECTION_HINT: &str =
    "Nothing saved yet: right-click a device or clip and choose Save to Library";

fn baseline(r: Rect, size: f32) -> f64 {
    r.center().y + f64::from(size) * 0.35
}

fn est_width(text: &str, size: f32) -> f64 {
    text.chars().count() as f64 * f64::from(size) * lay::CHAR_EM
}

fn polygon(points: &[(f64, f64)]) -> BezPath {
    let mut p = BezPath::new();
    for (i, &(x, y)) in points.iter().enumerate() {
        if i == 0 {
            p.move_to(pt(x, y));
        } else {
            p.line_to(pt(x, y));
        }
    }
    p.close_path();
    p
}

/// A five-pointed star centred at `(cx, cy)` with outer radius `r`.
fn star(cx: f64, cy: f64, r: f64) -> BezPath {
    let pts: Vec<(f64, f64)> = (0..10)
        .map(|i| {
            let a = -std::f64::consts::FRAC_PI_2 + f64::from(i) * std::f64::consts::PI / 5.0;
            let rad = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rad * a.cos(), cy + rad * a.sin())
        })
        .collect();
    polygon(&pts)
}

/// A disclosure arrow: pointing right, or down when `open`.
fn arrow(cx: f64, cy: f64, open: bool) -> BezPath {
    if open {
        polygon(&[(cx - 3.5, cy - 2.0), (cx + 3.5, cy - 2.0), (cx, cy + 3.0)])
    } else {
        polygon(&[(cx - 2.0, cy - 3.5), (cx - 2.0, cy + 3.5), (cx + 3.0, cy)])
    }
}

fn folder_glyph(c: &mut dyn Canvas, cx: f64, cy: f64, color: Color) {
    c.fill_with(Rect::new(cx - 6.0, cy - 4.5, cx - 1.0, cy - 2.0), color);
    c.fill_with(
        RoundedRect::from_rect(Rect::new(cx - 6.0, cy - 3.0, cx + 6.0, cy + 4.5), 1.5),
        color,
    );
}

impl FileBrowserView {
    pub(super) fn paint_panel(&self, c: &mut dyn Canvas) {
        let lay = self.lay();
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&self.bounds.into());
        c.fill_with(self.bounds, self.theme.browser_bg);
        self.paint_banner(c, &lay);
        self.paint_header(c, &lay);
        self.paint_tree(c, &lay);
        self.paint_splitter(c, &lay);
        self.paint_path_bar(c, &lay);
        self.paint_column_header(c, &lay);
        self.paint_list(c, &lay);
        let hot = self.gesture == Gesture::Resize;
        c.fill_with(
            Rect::new(
                lay.handle.x1 - if hot { 3.0 } else { 1.0 },
                lay.handle.y0,
                lay.handle.x1,
                lay.handle.y1,
            ),
            if hot {
                self.theme.focus
            } else {
                self.theme.bevel_dark
            },
        );
        if let Some(m) = &self.menu {
            self.paint_menu(c, m);
        }
        c.pop_clip();
        c.pop_transform();
    }

    /// The band of the patch target: orange, naming the device the panel applies patches to.
    fn paint_banner(&self, c: &mut dyn Canvas, lay: &Layout) {
        let Some(target) = &self.target else {
            return;
        };
        c.fill_with(lay.banner, crate::theme::PATCH_TARGET);
        c.push_clip(&lay.banner.into());
        c.text(&TextRun::new(
            fit_label(
                &format!("PATCHES FOR {}", target.label.to_uppercase()),
                lay.banner.width() - 2.0 * lay::PAD,
                SMALL_FONT,
            ),
            pt(lay.banner.x0 + lay::PAD, baseline(lay.banner, SMALL_FONT)),
            SMALL_FONT,
            Color::from_rgba8(0x20, 0x12, 0x04, 255),
        ));
        c.pop_clip();
    }

    fn paint_field(
        &self,
        c: &mut dyn Canvas,
        rect: Rect,
        text: &str,
        placeholder: &str,
        focused: bool,
        all: bool,
    ) {
        let t = &self.theme;
        let shape = RoundedRect::from_rect(rect, 5.0);
        c.fill_with(shape, t.lcd);
        c.stroke_with(shape, 1.0, if focused { t.focus } else { t.section_line });
        let room = ((rect.width() - 16.0) / (f64::from(ROW_FONT) * lay::CHAR_EM)).floor() as usize;
        let room = room.max(1);
        let count = text.chars().count();
        let shown: String = text.chars().skip(count.saturating_sub(room)).collect();
        c.push_clip(&rect.into());
        let x = rect.x0 + 9.0;
        if text.is_empty() && !focused {
            c.text(&TextRun::new(
                placeholder,
                pt(x, baseline(rect, ROW_FONT)),
                ROW_FONT,
                t.text_muted,
            ));
        } else {
            if all && focused && !shown.is_empty() {
                let w = est_width(&shown, ROW_FONT);
                c.fill_with(
                    Rect::new(x - 1.0, rect.y0 + 4.0, x + w + 1.0, rect.y1 - 4.0),
                    t.focus.with_alpha(0.35),
                );
            }
            let caret = if focused && !all { "|" } else { "" };
            c.text(&TextRun::new(
                format!("{shown}{caret}"),
                pt(x, baseline(rect, ROW_FONT)),
                ROW_FONT,
                t.text,
            ));
        }
        c.pop_clip();
    }

    fn paint_toggle(&self, c: &mut dyn Canvas, rect: Rect, on: bool, label: Option<&str>) {
        let t = &self.theme;
        let shape = RoundedRect::from_rect(rect, 5.0);
        c.fill_with(
            shape,
            if on {
                t.focus.with_alpha(0.35)
            } else {
                t.button
            },
        );
        c.stroke_with(shape, 1.0, if on { t.focus } else { t.section_line });
        let ink = if on { t.text } else { t.text_muted };
        match label {
            Some(l) => c.text(
                &TextRun::new(
                    l,
                    pt(rect.center().x, baseline(rect, SMALL_FONT)),
                    SMALL_FONT,
                    ink,
                )
                .align(Align::Center)
                .weight(600),
            ),
            None => {
                let (cx, cy) = (rect.center().x, rect.center().y);
                c.fill_with(
                    polygon(&[(cx - 3.0, cy - 4.5), (cx - 3.0, cy + 4.5), (cx + 4.5, cy)]),
                    ink,
                );
            }
        }
    }

    fn paint_header(&self, c: &mut dyn Canvas, lay: &Layout) {
        let t = &self.theme;
        c.fill_with(
            Rect::new(
                lay.header.x0,
                lay.header.y1 - 1.0,
                lay.header.x1,
                lay.header.y1,
            ),
            t.section_line,
        );
        self.paint_field(
            c,
            lay.search,
            self.search.text(),
            "Search  (#tag, fav)",
            self.search_focused && self.focused,
            self.search.is_all_selected(),
        );
        self.paint_toggle(c, lay.recursive_btn, self.recursive, Some("Sub"));
        self.paint_toggle(c, lay.preview_btn, self.auto_preview, None);
    }

    fn paint_scroll_thumb(&self, c: &mut dyn Canvas, track: Rect, content: f64, scroll: f64) {
        if let Some(th) = lay::scroll_thumb(track, content, scroll) {
            c.fill_with(
                RoundedRect::from_rect(th, 3.0),
                self.theme.text_muted.with_alpha(0.4),
            );
        }
    }

    fn paint_tree(&self, c: &mut dyn Canvas, lay: &Layout) {
        c.push_clip(&lay.tree.into());
        let range = lay::visible_range(
            self.tree_scroll,
            lay.tree.height(),
            lay::TREE_ROW_H,
            self.tree_items.len(),
        );
        for i in range {
            let item = &self.tree_items[i];
            let y = lay.tree.y0 + i as f64 * lay::TREE_ROW_H - self.tree_scroll;
            let row = Rect::new(lay.tree.x0, y, lay.tree.x1, y + lay::TREE_ROW_H);
            self.paint_tree_row(c, row, i, item);
        }
        c.pop_clip();
        self.paint_scroll_thumb(
            c,
            Rect::new(
                lay.tree.x1 - lay::SB_W,
                lay.tree.y0,
                lay.tree.x1,
                lay.tree.y1,
            ),
            self.tree_items.len() as f64 * lay::TREE_ROW_H,
            self.tree_scroll,
        );
    }

    fn paint_tree_row(&self, c: &mut dyn Canvas, row: Rect, i: usize, item: &tree::Item) {
        let t = &self.theme;
        let cy = row.center().y;
        if let tree::Key::Heading(_) = item.key {
            c.text(
                &TextRun::new(
                    item.label.to_uppercase(),
                    pt(row.x0 + lay::PAD, baseline(row, 8.0) + 2.0),
                    8.0,
                    t.text_muted,
                )
                .weight(700)
                .spacing(1.2),
            );
            return;
        }
        let here = item.key.place();
        if here.is_some() && here.as_ref() == self.location.as_ref() {
            c.fill_with(row, t.browser_row);
        } else if self.hover == Hover::Tree(i) {
            c.fill_with(row, t.browser_row.with_alpha(0.5));
        }
        if self.pane == Pane::Tree && self.focused && self.tree_cursor.as_ref() == Some(&item.key) {
            c.stroke_with(row.inset(-0.5), 1.0, t.focus);
        }
        let ax = row.x0 + lay::PAD + f64::from(item.depth) * lay::INDENT;
        if item.expandable {
            let col = if item.loading { t.text_muted } else { t.text };
            c.fill_with(arrow(ax + lay::ARROW_W / 2.0, cy, item.expanded), col);
        }
        let ix = ax + lay::ARROW_W + 6.0;
        match &item.key {
            tree::Key::Favorites => c.fill_with(star(ix, cy, 5.0), t.warning),
            tree::Key::TagsHeader | tree::Key::Tag(_) => {
                let accent = match &item.key {
                    tree::Key::Tag(name) => t.accent_for_type(name),
                    _ => t.text_muted,
                };
                c.fill_with(
                    RoundedRect::from_rect(Rect::new(ix - 4.5, cy - 3.0, ix + 4.5, cy + 3.0), 2.0),
                    accent,
                );
            }
            tree::Key::Folder { .. } => folder_glyph(c, ix, cy, t.text_muted),
            tree::Key::Heading(_) | tree::Key::Library(_) => {}
        }
        let lx = ix + 11.0;
        let count = item.count.map(|n| n.to_string());
        let reserve = count
            .as_ref()
            .map_or(0.0, |s| est_width(s, SMALL_FONT) + 10.0);
        let mut label = fit_label(&item.label, row.x1 - lx - reserve - lay::SB_W, TREE_FONT);
        if item.loading {
            label.push_str(" ...");
        }
        c.text(&TextRun::new(
            label,
            pt(lx, baseline(row, TREE_FONT)),
            TREE_FONT,
            t.text,
        ));
        if let Some(n) = count {
            c.text(
                &TextRun::new(
                    n,
                    pt(row.x1 - lay::SB_W - 6.0, baseline(row, SMALL_FONT)),
                    SMALL_FONT,
                    t.text_muted,
                )
                .align(Align::End),
            );
        }
    }

    fn paint_splitter(&self, c: &mut dyn Canvas, lay: &Layout) {
        let t = &self.theme;
        let r = lay.splitter;
        c.fill_with(r, t.panel_bottom);
        let hot = self.gesture == Gesture::Splitter;
        let (cx, cy) = (r.center().x, r.center().y);
        c.fill_with(
            RoundedRect::from_rect(Rect::new(cx - 14.0, cy - 1.0, cx + 14.0, cy + 1.0), 1.0),
            if hot {
                t.focus
            } else {
                t.text_muted.with_alpha(0.45)
            },
        );
    }

    fn paint_path_bar(&self, c: &mut dyn Canvas, lay: &Layout) {
        let t = &self.theme;
        let bar = lay.path;
        c.fill_with(bar, t.lcd);
        c.push_clip(&bar.into());
        if let Some(e) = &self.entry {
            let label = match e.kind {
                super::EntryKind::NewTag => "New tag",
                super::EntryKind::RenameTag(_) | super::EntryKind::RenameItem => "Rename",
            };
            c.text(&TextRun::new(
                label,
                pt(bar.x0 + lay::PAD, baseline(bar, SMALL_FONT)),
                SMALL_FONT,
                t.text_muted,
            ));
            let field = Rect::new(
                bar.x0 + lay::PAD + 52.0,
                bar.y0 + 2.0,
                bar.x1 - lay::PAD,
                bar.y1 - 2.0,
            );
            self.paint_field(
                c,
                field,
                e.field.text(),
                "",
                true,
                e.field.is_all_selected(),
            );
        } else {
            match &self.location {
                Some(Place::Folder(p)) => {
                    let pb = lay::path_segments(p, bar);
                    let base = baseline(bar, lay::PATH_FONT as f32);
                    if pb.ellipsis {
                        c.text(&TextRun::new(
                            "\u{2026}",
                            pt(bar.x0 + lay::PAD, base),
                            lay::PATH_FONT as f32,
                            t.text_muted,
                        ));
                    }
                    for (k, s) in pb.segments.iter().enumerate() {
                        if k > 0 {
                            c.text(
                                &TextRun::new(
                                    ">",
                                    pt(s.rect.x0 - lay::SEP_W / 2.0, base),
                                    lay::PATH_FONT as f32,
                                    t.text_muted,
                                )
                                .align(Align::Center),
                            );
                        }
                        c.text(&TextRun::new(
                            s.label.clone(),
                            pt(s.rect.x0 + 4.0, base),
                            lay::PATH_FONT as f32,
                            t.text,
                        ));
                    }
                }
                Some(Place::Favorites) => self.paint_place_title(c, bar, "Favourites"),
                Some(Place::Tag(tag)) => self.paint_place_title(c, bar, &format!("#{tag}")),
                Some(Place::List(list)) => self.paint_place_title(c, bar, list),
                Some(Place::Library(section)) => self.paint_place_title(c, bar, section.title()),
                None => self.paint_place_title(c, bar, "No folder"),
            }
        }
        c.pop_clip();
    }

    fn paint_place_title(&self, c: &mut dyn Canvas, bar: Rect, text: &str) {
        c.text(&TextRun::new(
            text,
            pt(bar.x0 + lay::PAD, baseline(bar, lay::PATH_FONT as f32)),
            lay::PATH_FONT as f32,
            self.theme.text,
        ));
    }

    fn paint_column_header(&self, c: &mut dyn Canvas, lay: &Layout) {
        let t = &self.theme;
        let r = lay.colhdr;
        c.fill_with(r, t.browser_row.with_alpha(0.6));
        c.fill_with(Rect::new(r.x0, r.y1 - 1.0, r.x1, r.y1), t.section_line);
        let cols = lay::columns(r.x0, r.x1, self.bounds.width());
        let base = baseline(r, SMALL_FONT);
        let mut label = |text: &str, key: Option<SortKey>, x: f64, align: Align| {
            let active = key == Some(self.sort_key);
            let color = if active { t.text } else { t.text_muted };
            c.text(
                &TextRun::new(text, pt(x, base), SMALL_FONT, color)
                    .align(align)
                    .weight(600),
            );
            if active {
                let w = est_width(text, SMALL_FONT);
                let ax = match align {
                    Align::End => x - w - 10.0,
                    _ => x + w + 10.0,
                };
                let cy = r.center().y;
                let tri = if self.descending {
                    polygon(&[(ax - 3.0, cy - 2.0), (ax + 3.0, cy - 2.0), (ax, cy + 2.5)])
                } else {
                    polygon(&[(ax - 3.0, cy + 2.0), (ax + 3.0, cy + 2.0), (ax, cy - 2.5)])
                };
                c.fill_with(tri, t.text);
            }
        };
        label("Name", Some(SortKey::Name), cols.name.0 + 4.0, Align::Start);
        label(
            "Duration",
            Some(SortKey::Duration),
            cols.duration.1,
            Align::End,
        );
        if let Some(rate) = cols.rate {
            label("Rate", None, rate.1, Align::End);
        }
        if let Some(size) = cols.size {
            label("Size", Some(SortKey::Size), size.1, Align::End);
        }
    }

    fn paint_list(&self, c: &mut dyn Canvas, lay: &Layout) {
        let t = &self.theme;
        c.push_clip(&lay.list.into());
        let range = lay::visible_range(
            self.list_scroll,
            lay.list.height(),
            lay::ROW_H,
            self.view.len(),
        );
        let cols = lay::columns(lay.list.x0, lay.list.x1, self.bounds.width());
        for pos in range {
            let row_idx = self.view[pos];
            let r = &self.rows[row_idx as usize];
            let y = lay.list.y0 + pos as f64 * lay::ROW_H - self.list_scroll;
            let rect = Rect::new(lay.list.x0, y, lay.list.x1 - lay::SB_W, y + lay::ROW_H);
            let selected = self.selection.contains(&r.path);
            let cursor = self.cursor == Some(row_idx);
            self.paint_row(
                c,
                rect,
                &cols,
                r,
                selected,
                cursor,
                self.hover == Hover::Row(pos),
            );
        }
        c.pop_clip();
        self.paint_scroll_thumb(
            c,
            Rect::new(
                lay.list.x1 - lay::SB_W,
                lay.list.y0,
                lay.list.x1,
                lay.list.y1,
            ),
            self.content_h(),
            self.list_scroll,
        );
        if self.view.is_empty() {
            let msg = if self.target.is_some() && self.query_empty {
                "No patches for this device here"
            } else if !self.rows.is_empty() {
                "No match"
            } else if matches!(
                self.location,
                Some(Place::Library(s)) if s != LibrarySection::Devices
            ) {
                EMPTY_SECTION_HINT
            } else {
                "Nothing here"
            };
            c.text(
                &TextRun::new(
                    msg,
                    pt(lay.list.center().x, lay.list.y0 + 28.0),
                    ROW_FONT,
                    t.text_muted,
                )
                .align(Align::Center),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        c: &mut dyn Canvas,
        rect: Rect,
        cols: &lay::Columns,
        r: &FileRow,
        selected: bool,
        cursor: bool,
        hover: bool,
    ) {
        let t = &self.theme;
        let playing = self
            .preview
            .as_ref()
            .filter(|(p, _)| *p == r.path)
            .map(|(_, progress)| *progress);
        if selected {
            let tint = if self.focused { t.focus } else { t.text_muted };
            c.fill_with(rect, tint.with_alpha(0.25));
        } else if hover {
            c.fill_with(rect, t.browser_row.with_alpha(0.6));
        }
        if let Some(progress) = playing {
            c.fill_with(rect, t.cable_audio.with_alpha(0.12));
            c.fill_with(
                Rect::new(
                    rect.x0,
                    rect.y1 - 2.5,
                    rect.x0 + rect.width() * f64::from(progress),
                    rect.y1,
                ),
                t.cable_audio,
            );
        }
        if cursor && self.focused && self.pane == Pane::List {
            c.stroke_with(rect.inset(-0.5), 1.0, t.focus);
        }
        let cy = rect.center().y;
        let star_cx = (cols.star.0 + cols.star.1) / 2.0;
        let is_folder = r.kind == FileKind::Folder;
        if is_folder {
            folder_glyph(c, star_cx, cy, t.text_muted);
        } else if r.favorite {
            c.fill_with(star(star_cx, cy, 5.5), t.warning);
        } else {
            c.stroke(
                &star(star_cx, cy, 5.0).into(),
                &Stroke::new(1.0),
                &t.text_muted.with_alpha(0.45).into(),
            );
        }
        let name_x = cols.name.0 + 4.0;
        // An item shares the width of the name, duration, rate and size columns with its detail.
        let item = r.kind.is_item();
        let right = if item {
            cols.size.or(cols.rate).map_or(cols.duration.1, |c| c.1)
        } else {
            cols.name.1
        };
        let name_w = if item {
            (right - name_x - 8.0) * 0.5
        } else {
            cols.name.1 - name_x - 4.0
        };
        let name = fit_label(&r.name, name_w, ROW_FONT);
        let text_w = est_width(&name, ROW_FONT);
        let mut run = TextRun::new(name, pt(name_x, baseline(rect, ROW_FONT)), ROW_FONT, t.text);
        if is_folder {
            run = run.weight(600);
        }
        c.text(&run);
        let mut cx = name_x + text_w + 8.0;
        for tag in &r.tags {
            let w = est_width(tag, 8.0) + 8.0;
            if cx + w > name_x + name_w {
                break;
            }
            let accent = t.accent_for_type(tag);
            let chip = Rect::new(cx, cy - 6.5, cx + w, cy + 6.5);
            c.fill_with(RoundedRect::from_rect(chip, 6.0), accent.with_alpha(0.28));
            c.text(&TextRun::new(
                tag.clone(),
                pt(cx + 4.0, baseline(chip, 8.0)),
                8.0,
                t.text,
            ));
            cx += w + 4.0;
        }
        if is_folder {
            return;
        }
        if item {
            let detail = r.item.as_ref().map_or("", |i| i.detail.as_str());
            let w = right - name_x - name_w - 12.0;
            c.text(
                &TextRun::new(
                    fit_label(detail, w, SMALL_FONT),
                    pt(right, baseline(rect, SMALL_FONT)),
                    SMALL_FONT,
                    t.text_muted,
                )
                .align(Align::End),
            );
            return;
        }
        let secs = r.info.as_ref().and_then(|i| i.duration_secs());
        let known = secs.is_some();
        c.text(
            &TextRun::new(
                lay::format_duration(secs),
                pt(cols.duration.1, baseline(rect, SMALL_FONT)),
                SMALL_FONT,
                if known { t.text } else { t.text_muted },
            )
            .align(Align::End),
        );
        if let Some(rate) = cols.rate {
            let text = r.info.as_ref().map_or_else(
                || "\u{2014}".to_string(),
                |i| lay::format_rate(i.sample_rate),
            );
            c.text(
                &TextRun::new(
                    text,
                    pt(rate.1, baseline(rect, SMALL_FONT)),
                    SMALL_FONT,
                    t.text_muted,
                )
                .align(Align::End),
            );
        }
        if let Some(size) = cols.size {
            c.text(
                &TextRun::new(
                    lay::format_size(r.size),
                    pt(size.1, baseline(rect, SMALL_FONT)),
                    SMALL_FONT,
                    t.text_muted,
                )
                .align(Align::End),
            );
        }
    }

    fn paint_menu(&self, c: &mut dyn Canvas, m: &Menu) {
        let t = &self.theme;
        c.fill_with(
            RoundedRect::from_rect(m.rect + vello::kurbo::Vec2::new(2.0, 3.0), 6.0),
            Color::from_rgba8(0, 0, 0, 90),
        );
        c.fill_with(RoundedRect::from_rect(m.rect, 6.0), t.browser_bg);
        c.stroke_with(RoundedRect::from_rect(m.rect, 6.0), 1.0, t.section_line);
        for (i, item) in m.items.iter().enumerate() {
            let r = m.item_rect(i);
            if item.act.is_none() {
                c.fill_with(
                    Rect::new(
                        r.x0 + 6.0,
                        r.center().y - 0.5,
                        r.x1 - 6.0,
                        r.center().y + 0.5,
                    ),
                    t.section_line,
                );
                continue;
            }
            if m.hover == Some(i) && item.enabled {
                c.fill_with(RoundedRect::from_rect(r, 4.0), t.selection);
            }
            let color = if item.enabled { t.text } else { t.text_muted };
            let base = baseline(r, menu::FONT);
            if item.checked == Some(true) {
                c.text(&TextRun::new(
                    "\u{2713}",
                    pt(r.x0 + 7.0, base),
                    menu::FONT,
                    color,
                ));
            }
            c.text(&TextRun::new(
                item.label.clone(),
                pt(r.x0 + 24.0, base),
                menu::FONT,
                color,
            ));
        }
    }
}
