//! Pure geometry and text formatting of the file browser panel (logical pixels).

use std::path::{Path, PathBuf};
use vello::kurbo::Rect;

/// Height of the header (search field and toggles).
pub(super) const HEADER_H: f64 = 34.0;
/// Height of the band that names the target device of patch browsing.
pub(super) const BANNER_H: f64 = 16.0;
/// Height of the search field and the toggle buttons.
pub(super) const FIELD_H: f64 = 22.0;
/// Horizontal padding.
pub(super) const PAD: f64 = 8.0;
/// Width of the resize handle on the right edge.
pub(super) const HANDLE_W: f64 = 5.0;
/// Height of the tree/list splitter.
pub(super) const SPLIT_H: f64 = 6.0;
/// Height of the path bar.
pub(super) const PATH_H: f64 = 22.0;
/// Height of the column header.
pub(super) const COLHDR_H: f64 = 20.0;
/// Height of a list row.
pub(super) const ROW_H: f64 = 22.0;
/// Height of a tree row.
pub(super) const TREE_ROW_H: f64 = 20.0;
/// Width of a scroll bar.
pub(super) const SB_W: f64 = 7.0;
/// Shortest scroll bar thumb.
pub(super) const MIN_THUMB: f64 = 20.0;
/// Panel width from which the size and rate columns appear.
pub(super) const WIDE_MIN: f64 = 420.0;
/// Indentation per tree level.
pub(super) const INDENT: f64 = 12.0;
/// Width of the disclosure arrow zone of a tree row.
pub(super) const ARROW_W: f64 = 14.0;
/// Estimated advance per character, as a fraction of the font size.
pub(super) const CHAR_EM: f64 = 0.56;
/// Font size of the path bar segments.
pub(super) const PATH_FONT: f64 = 10.0;
/// Width of the separator between path segments.
pub(super) const SEP_W: f64 = 12.0;
/// Width of the leading ellipsis of a shortened path.
pub(super) const ELLIPSIS_W: f64 = 14.0;

/// The regions of the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Layout {
    /// The band naming the patch target (empty without one).
    pub banner: Rect,
    /// Header band.
    pub header: Rect,
    /// Search field.
    pub search: Rect,
    /// "sub-folders" toggle.
    pub recursive_btn: Rect,
    /// Auto-preview toggle.
    pub preview_btn: Rect,
    /// Tree region.
    pub tree: Rect,
    /// Splitter between tree and list.
    pub splitter: Rect,
    /// Path bar.
    pub path: Rect,
    /// Column header.
    pub colhdr: Rect,
    /// Row area of the list.
    pub list: Rect,
    /// Resize handle on the right edge.
    pub handle: Rect,
}

/// Splits `bounds` into the regions of the panel.
/// `banner` adds the band that names the device the panel applies patches to, above the header.
pub(super) fn layout(bounds: Rect, tree_fraction: f64, banner: bool) -> Layout {
    let band = if banner { BANNER_H } else { 0.0 };
    let banner_rect = Rect::new(
        bounds.x0,
        bounds.y0,
        bounds.x1,
        (bounds.y0 + band).min(bounds.y1),
    );
    let right = (bounds.x1 - HANDLE_W).max(bounds.x0);
    let bottom = |y: f64| y.min(bounds.y1).max(bounds.y0);
    let header = Rect::new(
        bounds.x0,
        banner_rect.y1,
        right,
        bottom(banner_rect.y1 + HEADER_H),
    );
    let top = header.y0 + (HEADER_H - FIELD_H) / 2.0;
    let preview_btn = Rect::new(
        (right - PAD - 24.0).max(bounds.x0),
        top,
        (right - PAD).max(bounds.x0),
        top + FIELD_H,
    );
    let recursive_btn = Rect::new(
        (preview_btn.x0 - 4.0 - 34.0).max(bounds.x0),
        top,
        (preview_btn.x0 - 4.0).max(bounds.x0),
        top + FIELD_H,
    );
    let search = Rect::new(
        bounds.x0 + PAD,
        top,
        (recursive_btn.x0 - 6.0).max(bounds.x0 + PAD),
        top + FIELD_H,
    );
    let avail = (bounds.y1 - header.y1).max(0.0);
    let min_list = SPLIT_H + PATH_H + COLHDR_H + ROW_H;
    let tree_h = (avail * tree_fraction.clamp(0.1, 0.9)).min((avail - min_list).max(0.0));
    let tree = Rect::new(bounds.x0, header.y1, right, header.y1 + tree_h);
    let splitter = Rect::new(bounds.x0, tree.y1, right, bottom(tree.y1 + SPLIT_H));
    let path = Rect::new(bounds.x0, splitter.y1, right, bottom(splitter.y1 + PATH_H));
    let colhdr = Rect::new(bounds.x0, path.y1, right, bottom(path.y1 + COLHDR_H));
    let list = Rect::new(bounds.x0, colhdr.y1, right, bounds.y1.max(colhdr.y1));
    let handle = Rect::new(bounds.x1 - HANDLE_W, bounds.y0, bounds.x1, bounds.y1);
    Layout {
        banner: banner_rect,
        header,
        search,
        recursive_btn,
        preview_btn,
        tree,
        splitter,
        path,
        colhdr,
        list,
        handle,
    }
}

/// Horizontal extents `(x0, x1)` of the list columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Columns {
    /// Favourite star.
    pub star: (f64, f64),
    /// Name (takes the remaining width).
    pub name: (f64, f64),
    /// Duration.
    pub duration: (f64, f64),
    /// Sample rate (wide panels only).
    pub rate: Option<(f64, f64)>,
    /// File size (wide panels only).
    pub size: Option<(f64, f64)>,
}

/// The columns of a list spanning `x0..x1` (the scroll bar lane at the right is excluded) in a
/// panel `panel_width` wide: size and rate appear from [`WIDE_MIN`].
pub(super) fn columns(x0: f64, x1: f64, panel_width: f64) -> Columns {
    let mut edge = (x1 - SB_W - 4.0).max(x0);
    let mut take = |w: f64| {
        let r = (edge - w).max(x0);
        let out = (r, edge);
        edge = r;
        out
    };
    let wide = panel_width >= WIDE_MIN;
    let size = wide.then(|| take(62.0));
    let rate = wide.then(|| take(48.0));
    let duration = take(54.0);
    let star = (x0, (x0 + 22.0).min(edge));
    Columns {
        star,
        name: (star.1, edge),
        duration,
        rate,
        size,
    }
}

fn thumb_len(view: f64, content: f64) -> f64 {
    (view * view / content).clamp(MIN_THUMB.min(view), view)
}

/// The scroll bar thumb for `content` pixels of content shown in `track`, or `None` when
/// everything fits.
pub(super) fn scroll_thumb(track: Rect, content: f64, scroll: f64) -> Option<Rect> {
    let view = track.height();
    if content <= view || view <= 0.0 {
        return None;
    }
    let len = thumb_len(view, content);
    let max = content - view;
    let y = track.y0 + (view - len) * (scroll / max).clamp(0.0, 1.0);
    Some(Rect::new(track.x0, y, track.x1, y + len))
}

/// The scroll offset that puts the top of the thumb at `thumb_top` (inverse of
/// [`scroll_thumb`]).
pub(super) fn scroll_from_thumb(track: Rect, content: f64, thumb_top: f64) -> f64 {
    let view = track.height();
    if content <= view || view <= 0.0 {
        return 0.0;
    }
    let range = view - thumb_len(view, content);
    if range <= 0.0 {
        return 0.0;
    }
    ((thumb_top - track.y0) / range).clamp(0.0, 1.0) * (content - view)
}

/// The indices of the rows that intersect a viewport of `view_h` pixels scrolled by `scroll`.
pub(super) fn visible_range(
    scroll: f64,
    view_h: f64,
    row_h: f64,
    count: usize,
) -> std::ops::Range<usize> {
    if count == 0 || view_h <= 0.0 {
        return 0..0;
    }
    let s = scroll.max(0.0);
    let first = (s / row_h).floor() as usize;
    let last = (((s + view_h) / row_h).ceil() as usize).min(count);
    first.min(count)..last
}

/// One clickable segment of the path bar.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Segment {
    /// Shown text.
    pub label: String,
    /// The folder the segment stands for.
    pub path: PathBuf,
    /// Its hit rectangle.
    pub rect: Rect,
}

/// The path bar of a folder.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct PathBar {
    /// Leading segments were dropped for lack of room (an ellipsis is drawn first).
    pub ellipsis: bool,
    /// The visible segments, left to right.
    pub segments: Vec<Segment>,
}

fn segment_width(label: &str) -> f64 {
    label.chars().count() as f64 * PATH_FONT * CHAR_EM + 8.0
}

/// Lays out the segments of `path` in `bar`, dropping leading ones that do not fit.
pub(super) fn path_segments(path: &Path, bar: Rect) -> PathBar {
    use std::path::Component;
    let mut all: Vec<(String, PathBuf)> = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        match comp {
            Component::RootDir => match all.last_mut() {
                Some(last) => last.1 = acc.clone(),
                None => all.push(("/".to_string(), acc.clone())),
            },
            other => all.push((
                other.as_os_str().to_string_lossy().into_owned(),
                acc.clone(),
            )),
        }
    }
    let avail = (bar.width() - 2.0 * PAD).max(0.0);
    let total = |from: usize| -> f64 {
        let ell = if from > 0 { ELLIPSIS_W } else { 0.0 };
        ell + all[from..]
            .iter()
            .enumerate()
            .map(|(i, (l, _))| segment_width(l) + if i > 0 { SEP_W } else { 0.0 })
            .sum::<f64>()
    };
    let mut from = 0;
    while from + 1 < all.len() && total(from) > avail {
        from += 1;
    }
    let ellipsis = from > 0;
    let mut x = bar.x0 + PAD + if ellipsis { ELLIPSIS_W } else { 0.0 };
    let mut segments = Vec::new();
    for (i, (label, p)) in all.into_iter().enumerate().skip(from) {
        if i > from {
            x += SEP_W;
        }
        let w = segment_width(&label);
        segments.push(Segment {
            label,
            path: p,
            rect: Rect::new(x, bar.y0, (x + w).min(bar.x1), bar.y1),
        });
        x += w;
    }
    PathBar { ellipsis, segments }
}

/// Seconds with one decimal below 10 s (one-shots: `0.6s`), else `m:ss` or `h:mm:ss`; an em
/// dash when the length is unknown.
pub(super) fn format_duration(secs: Option<f64>) -> String {
    let Some(s) = secs.filter(|s| s.is_finite() && *s >= 0.0) else {
        return "\u{2014}".to_string();
    };
    if s < 9.95 {
        return format!("{s:.1}s");
    }
    let t = s.round() as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A file size in B / KB / MB / GB.
pub(super) fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// A sample rate as `44.1k` / `48k`.
pub(super) fn format_rate(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{}k", rate / 1000)
    } else {
        format!("{:.1}k", f64::from(rate) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(w: f64, h: f64) -> Rect {
        Rect::new(10.0, 20.0, 10.0 + w, 20.0 + h)
    }

    #[test]
    fn regions_stack_without_gaps_or_overlap() {
        let b = bounds(300.0, 600.0);
        let l = layout(b, 0.4, false);
        assert_eq!(l.header.y0, b.y0);
        assert_eq!(l.tree.y0, l.header.y1);
        assert_eq!(l.splitter.y0, l.tree.y1);
        assert_eq!(l.path.y0, l.splitter.y1);
        assert_eq!(l.colhdr.y0, l.path.y1);
        assert_eq!(l.list.y0, l.colhdr.y1);
        assert_eq!(l.list.y1, b.y1);
        assert_eq!(l.handle.x1, b.x1);
        assert_eq!(l.tree.x1, b.x1 - HANDLE_W);
        let avail = b.height() - HEADER_H;
        assert!((l.tree.height() - avail * 0.4).abs() < 1e-9);
    }

    #[test]
    fn tree_fraction_is_clamped_and_leaves_room_for_the_list() {
        let b = bounds(300.0, 600.0);
        let lo = layout(b, 0.0, false);
        assert!((lo.tree.height() - (600.0 - HEADER_H) * 0.1).abs() < 1e-9);
        let hi = layout(bounds(300.0, 1000.0), 5.0, false);
        assert!((hi.tree.height() - (1000.0 - HEADER_H) * 0.9).abs() < 1e-9);
        let tiny = layout(bounds(300.0, 120.0), 0.9, false);
        assert!(tiny.list.height() >= ROW_H - 1e-9);
        assert!(tiny.list.y1 <= 140.0 + 1e-9);
    }

    #[test]
    fn header_controls_fit_at_min_width() {
        let l = layout(bounds(200.0, 400.0), 0.4, false);
        assert!(l.search.width() > 80.0);
        assert!(l.search.x1 <= l.recursive_btn.x0);
        assert!(l.recursive_btn.x1 <= l.preview_btn.x0);
        assert!(l.preview_btn.x1 <= l.header.x1);
    }

    #[test]
    fn wide_columns_only_when_the_panel_is_wide() {
        let narrow = columns(0.0, 300.0, 300.0);
        assert!(narrow.rate.is_none() && narrow.size.is_none());
        let wide = columns(0.0, 500.0, 500.0);
        let (rate, size) = (wide.rate.unwrap(), wide.size.unwrap());
        assert!(wide.name.1 <= wide.duration.0);
        assert!(wide.duration.1 <= rate.0);
        assert!(rate.1 <= size.0);
        assert!(wide.name.1 > wide.name.0 + 100.0);
        assert!(columns(0.0, 420.0, 420.0).size.is_some());
        assert!(columns(0.0, 419.0, 419.0).size.is_none());
    }

    #[test]
    fn thumb_tracks_the_scroll_offset_and_inverts() {
        let track = Rect::new(0.0, 0.0, 7.0, 200.0);
        assert!(scroll_thumb(track, 150.0, 0.0).is_none());
        let t0 = scroll_thumb(track, 1000.0, 0.0).unwrap();
        assert!(t0.y0.abs() < 1e-9 && (t0.height() - 40.0).abs() < 1e-9);
        let t1 = scroll_thumb(track, 1000.0, 800.0).unwrap();
        assert!((t1.y1 - 200.0).abs() < 1e-9);
        let mid = scroll_thumb(track, 1000.0, 400.0).unwrap();
        let back = scroll_from_thumb(track, 1000.0, mid.y0);
        assert!((back - 400.0).abs() < 1e-6);
        let huge = scroll_thumb(track, 1.0e7, 0.0).unwrap();
        assert!((huge.height() - MIN_THUMB).abs() < 1e-9);
    }

    #[test]
    fn visible_range_is_bounded_by_the_viewport() {
        assert_eq!(visible_range(0.0, 100.0, 22.0, 1_000_000), 0..5);
        assert_eq!(visible_range(220.0, 100.0, 22.0, 1_000_000), 10..15);
        assert_eq!(visible_range(235.0, 100.0, 22.0, 1_000_000), 10..16);
        assert_eq!(visible_range(0.0, 100.0, 22.0, 3), 0..3);
        assert_eq!(visible_range(0.0, 100.0, 22.0, 0), 0..0);
    }

    #[test]
    fn path_segments_hold_cumulative_prefixes() {
        let bar = Rect::new(0.0, 0.0, 600.0, PATH_H);
        let pb = path_segments(Path::new("/samples/drums/kicks"), bar);
        assert!(!pb.ellipsis);
        let labels: Vec<&str> = pb.segments.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["/", "samples", "drums", "kicks"]);
        assert_eq!(pb.segments[2].path, PathBuf::from("/samples/drums"));
        for w in pb.segments.windows(2) {
            assert!(w[0].rect.x1 <= w[1].rect.x0);
        }
    }

    #[test]
    fn long_paths_drop_leading_segments() {
        let bar = Rect::new(0.0, 0.0, 150.0, PATH_H);
        let pb = path_segments(Path::new("/aaaaaaaa/bbbbbbbb/cccccccc/dddddddd"), bar);
        assert!(pb.ellipsis);
        assert_eq!(pb.segments.last().unwrap().label, "dddddddd");
        assert!(pb.segments.len() < 5);
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_segment_points_at_the_root() {
        let bar = Rect::new(0.0, 0.0, 600.0, PATH_H);
        let pb = path_segments(Path::new(r"C:\Windows\Kicks"), bar);
        assert_eq!(pb.segments[0].label, "C:");
        assert_eq!(pb.segments[0].path, PathBuf::from(r"C:\"));
        assert_eq!(pb.segments[1].path, PathBuf::from(r"C:\Windows"));
    }

    #[test]
    fn durations_sizes_and_rates_format() {
        assert_eq!(format_duration(Some(0.0)), "0.0s");
        assert_eq!(format_duration(Some(0.6)), "0.6s");
        assert_eq!(format_duration(Some(9.94)), "9.9s");
        assert_eq!(format_duration(Some(9.96)), "0:10");
        assert_eq!(format_duration(Some(75.4)), "1:15");
        assert_eq!(format_duration(Some(3661.0)), "1:01:01");
        assert_eq!(format_duration(None), "\u{2014}");
        assert_eq!(format_duration(Some(f64::NAN)), "\u{2014}");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_rate(48000), "48k");
        assert_eq!(format_rate(44100), "44.1k");
    }
}
