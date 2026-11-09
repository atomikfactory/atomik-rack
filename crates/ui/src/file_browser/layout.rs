//! Pure geometry and text formatting of the file browser panel (logical pixels).

use super::{PREVIEW_DB_MAX, PREVIEW_DB_MIN};
use atomik_library::{AudioFormat, FileKind, FileRow};
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
/// Height of the info pane under the list (the selected file's facts and picture).
pub(super) const PANE_H: f64 = 74.0;
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
/// Width of the back and forward buttons at the left of the path bar.
pub(super) const NAV_W: f64 = 18.0;
/// Width of the preview level control in the header.
pub(super) const LEVEL_W: f64 = 54.0;
/// The narrowest the search field may get before the level control makes room for it.
pub(super) const SEARCH_MIN_W: f64 = 96.0;

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
    /// The preview level control (empty when the panel is too narrow for it).
    pub level: Rect,
    /// Tree region.
    pub tree: Rect,
    /// Splitter between tree and list.
    pub splitter: Rect,
    /// Path bar.
    pub path: Rect,
    /// Back button of the path bar.
    pub back_btn: Rect,
    /// Forward button of the path bar.
    pub fwd_btn: Rect,
    /// The part of the path bar the folder segments use (right of the buttons).
    pub path_text: Rect,
    /// Column header.
    pub colhdr: Rect,
    /// Row area of the list.
    pub list: Rect,
    /// The info pane under the list (empty when it is not shown).
    pub info: Rect,
    /// Resize handle on the right edge.
    pub handle: Rect,
}

/// Splits `bounds` into the regions of the panel.
/// `banner` adds the band that names the device the panel applies patches to, above the header.
pub(super) fn layout(bounds: Rect, tree_fraction: f64, banner: bool, info: bool) -> Layout {
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
    // The level control sits between the search field and the toggles, when the field keeps a
    // usable width.
    let level_x1 = recursive_btn.x0 - 4.0;
    let level_x0 = level_x1 - LEVEL_W;
    let level = if level_x0 - 6.0 - (bounds.x0 + PAD) >= SEARCH_MIN_W {
        Rect::new(level_x0, top, level_x1, top + FIELD_H)
    } else {
        Rect::new(recursive_btn.x0, top, recursive_btn.x0, top + FIELD_H)
    };
    let search = Rect::new(
        bounds.x0 + PAD,
        top,
        (level.x0 - 6.0).max(bounds.x0 + PAD),
        top + FIELD_H,
    );
    let avail = (bounds.y1 - header.y1).max(0.0);
    let min_list = SPLIT_H + PATH_H + COLHDR_H + ROW_H;
    let tree_h = (avail * tree_fraction.clamp(0.1, 0.9)).min((avail - min_list).max(0.0));
    let tree = Rect::new(bounds.x0, header.y1, right, header.y1 + tree_h);
    let splitter = Rect::new(bounds.x0, tree.y1, right, bottom(tree.y1 + SPLIT_H));
    let path = Rect::new(bounds.x0, splitter.y1, right, bottom(splitter.y1 + PATH_H));
    let back_btn = Rect::new(
        path.x0 + 3.0,
        path.y0 + 2.0,
        (path.x0 + 3.0 + NAV_W).min(path.x1),
        (path.y1 - 2.0).max(path.y0),
    );
    let fwd_btn = Rect::new(
        (back_btn.x1 + 1.0).min(path.x1),
        back_btn.y0,
        (back_btn.x1 + 1.0 + NAV_W).min(path.x1),
        back_btn.y1,
    );
    let path_text = Rect::new(fwd_btn.x1, path.y0, path.x1.max(fwd_btn.x1), path.y1);
    let colhdr = Rect::new(bounds.x0, path.y1, right, bottom(path.y1 + COLHDR_H));
    // The info pane takes the foot of the list when there is room for it and a few rows.
    let room = bounds.y1 - colhdr.y1;
    let pane = if info && room >= PANE_H + 3.0 * ROW_H {
        PANE_H
    } else {
        0.0
    };
    let list_end = (bounds.y1 - pane).max(colhdr.y1);
    let list = Rect::new(bounds.x0, colhdr.y1, right, list_end);
    let info = Rect::new(bounds.x0, list_end, right, bounds.y1.max(list_end));
    let handle = Rect::new(bounds.x1 - HANDLE_W, bounds.y0, bounds.x1, bounds.y1);
    Layout {
        banner: banner_rect,
        header,
        search,
        recursive_btn,
        preview_btn,
        level,
        tree,
        splitter,
        path,
        back_btn,
        fwd_btn,
        path_text,
        colhdr,
        list,
        info,
        handle,
    }
}

/// The level a pointer at `x` asks for on the control `rect`, in whole dB.
pub(super) fn preview_level_at(rect: Rect, x: f64) -> f32 {
    let span = f64::from(PREVIEW_DB_MAX - PREVIEW_DB_MIN);
    let f = ((x - rect.x0) / rect.width().max(1.0)).clamp(0.0, 1.0);
    ((f64::from(PREVIEW_DB_MIN) + f * span).round() + 0.0) as f32
}

/// Where `db` sits along the level control, as a fraction of its width.
pub(super) fn level_fraction(db: f32) -> f64 {
    let span = f64::from(PREVIEW_DB_MAX - PREVIEW_DB_MIN);
    ((f64::from(db) - f64::from(PREVIEW_DB_MIN)) / span).clamp(0.0, 1.0)
}

/// The text the control shows for `db`.
pub(super) fn level_text(db: f32) -> String {
    if db > 0.0 {
        format!("+{db:.0} dB")
    } else {
        format!("{db:.0} dB")
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

/// A tempo as `120 BPM` or `93.5 BPM`.
pub(super) fn format_tempo(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.05 {
        format!("{:.0} BPM", bpm)
    } else {
        format!("{bpm:.1} BPM")
    }
}

/// The facts line of the info pane: format, channels, rate, length, size and tempo for an audio
/// file (what is not known is left out), the kind and detail for a library item.
pub(super) fn pane_facts(row: &FileRow, tempo: Option<f64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    match row.kind {
        FileKind::Audio(format) => {
            parts.push(
                match format {
                    AudioFormat::Wav => "WAV",
                    AudioFormat::Aiff => "AIFF",
                    AudioFormat::Flac => "FLAC",
                    AudioFormat::Mp3 => "MP3",
                    AudioFormat::Ogg => "OGG",
                }
                .to_string(),
            );
            if let Some(info) = row.info {
                parts.push(match info.channels {
                    1 => "mono".to_string(),
                    2 => "stereo".to_string(),
                    n => format!("{n} ch"),
                });
                parts.push(format_rate(info.sample_rate));
                if info.duration_secs().is_some() {
                    parts.push(format_duration(info.duration_secs()));
                }
            }
            if row.size > 0 {
                parts.push(format_size(row.size));
            }
            if let Some(bpm) = tempo {
                parts.push(format_tempo(bpm));
            }
        }
        FileKind::Patch => parts.push("PATCH".to_string()),
        FileKind::Container => parts.push("CONTAINER".to_string()),
        FileKind::Clip => parts.push("CLIP".to_string()),
        FileKind::Project => parts.push("PROJECT".to_string()),
        FileKind::Midi => {
            parts.push("MIDI".to_string());
            if row.size > 0 {
                parts.push(format_size(row.size));
            }
        }
        FileKind::Folder | FileKind::Device | FileKind::Preset => {}
    }
    if let Some(item) = &row.item
        && !item.detail.is_empty()
    {
        parts.push(item.detail.clone());
    }
    parts.join(" \u{b7} ")
}

/// The line of tags, author and categories under the facts (empty when the row has none).
pub(super) fn pane_marks(row: &FileRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !row.tags.is_empty() {
        parts.push(
            row.tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if !row.author.is_empty() {
        parts.push(format!("by {}", row.author));
    }
    if !row.categories.is_empty() {
        parts.push(row.categories.join(", "));
    }
    parts.join(" \u{b7} ")
}

/// The level of picture column `x` of `columns` in `levels` (the loudest level the column
/// covers; 0 for an empty picture).
pub(super) fn level_at(levels: &[f32], x: usize, columns: usize) -> f32 {
    if levels.is_empty() || columns == 0 {
        return 0.0;
    }
    let from = x * levels.len() / columns;
    let to = ((x + 1) * levels.len() / columns).clamp(from + 1, levels.len());
    levels[from.min(levels.len() - 1)..to]
        .iter()
        .copied()
        .fold(0.0, f32::max)
}

/// The lowest and highest sample value of picture column `x` of `columns` in `peaks` (the
/// range every part the column covers spans; flat for an empty picture).
pub(super) fn peak_at(peaks: &[[f32; 2]], x: usize, columns: usize) -> [f32; 2] {
    if peaks.is_empty() || columns == 0 {
        return [0.0, 0.0];
    }
    let from = x * peaks.len() / columns;
    let to = ((x + 1) * peaks.len() / columns).clamp(from + 1, peaks.len());
    peaks[from.min(peaks.len() - 1)..to]
        .iter()
        .fold([0.0f32, 0.0f32], |[lo, hi], p| [lo.min(p[0]), hi.max(p[1])])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(w: f64, h: f64) -> Rect {
        Rect::new(10.0, 20.0, 10.0 + w, 20.0 + h)
    }

    #[test]
    fn peak_columns_span_the_parts_they_cover_and_stretch_when_there_are_few() {
        let peaks = [[-0.1, 0.2], [-0.6, 0.1], [0.0, 0.9], [-0.2, 0.0]];
        assert_eq!(peak_at(&peaks, 0, 2), [-0.6, 0.2]);
        assert_eq!(peak_at(&peaks, 1, 2), [-0.2, 0.9]);
        assert_eq!(peak_at(&peaks, 5, 8), [0.0, 0.9]);
        assert_eq!(peak_at(&peaks, 0, 0), [0.0, 0.0]);
        assert_eq!(peak_at(&[], 3, 8), [0.0, 0.0]);
    }

    #[test]
    fn regions_stack_without_gaps_or_overlap() {
        let b = bounds(300.0, 600.0);
        let l = layout(b, 0.4, false, false);
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
    fn the_info_pane_takes_the_foot_of_the_list_only_when_there_is_room() {
        let b = bounds(300.0, 600.0);
        let with = layout(b, 0.4, false, true);
        assert_eq!(with.info.height(), PANE_H);
        assert_eq!(with.info.y1, b.y1);
        assert_eq!(with.list.y1, with.info.y0);
        assert!(layout(b, 0.4, false, false).info.height() == 0.0);
        let tiny = layout(bounds(300.0, 240.0), 0.4, false, true);
        assert!(tiny.info.height() == 0.0, "no room: no pane");
        assert!(tiny.list.height() >= ROW_H - 1e-9);
    }

    fn audio_row(format: AudioFormat, channels: u16, secs: u64, size: u64) -> FileRow {
        FileRow {
            path: PathBuf::from("/lib/x"),
            name: "x".to_string(),
            kind: FileKind::Audio(format),
            size,
            info: Some(atomik_library::AudioInfo {
                sample_rate: 44_100,
                channels,
                frames: Some(44_100 * secs),
            }),
            item: None,
            favorite: false,
            tags: Vec::new(),
            author: String::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn the_facts_line_names_what_is_known_and_leaves_out_what_is_not() {
        let r = audio_row(AudioFormat::Flac, 2, 75, 3 * 1024 * 1024);
        assert_eq!(
            pane_facts(&r, Some(120.0)),
            "FLAC \u{b7} stereo \u{b7} 44.1k \u{b7} 1:15 \u{b7} 3.0 MB \u{b7} 120 BPM"
        );
        let mut bare = audio_row(AudioFormat::Wav, 1, 0, 0);
        bare.info = None;
        assert_eq!(pane_facts(&bare, None), "WAV");
        let mut six = audio_row(AudioFormat::Ogg, 6, 3, 100);
        six.info = six
            .info
            .map(|i| atomik_library::AudioInfo { frames: None, ..i });
        assert_eq!(
            pane_facts(&six, Some(93.5)),
            "OGG \u{b7} 6 ch \u{b7} 44.1k \u{b7} 100 B \u{b7} 93.5 BPM"
        );
        let mut item = audio_row(AudioFormat::Wav, 1, 1, 1);
        item.kind = FileKind::Patch;
        item.info = None;
        item.size = 0;
        item.item = Some(atomik_library::ItemInfo {
            detail: "Poly Synth".to_string(),
            ..Default::default()
        });
        assert_eq!(pane_facts(&item, None), "PATCH \u{b7} Poly Synth");
    }

    #[test]
    fn the_marks_line_joins_tags_author_and_categories() {
        let mut r = audio_row(AudioFormat::Wav, 1, 1, 1);
        assert_eq!(pane_marks(&r), "");
        r.tags = vec!["808".into(), "punchy".into()];
        r.author = "Ada".into();
        r.categories = vec!["Drums".into(), "Kicks".into()];
        assert_eq!(
            pane_marks(&r),
            "#808 #punchy \u{b7} by Ada \u{b7} Drums, Kicks"
        );
    }

    #[test]
    fn a_picture_column_shows_the_loudest_level_it_covers() {
        let levels = [0.1, 0.9, 0.2, 0.3];
        assert_eq!(level_at(&levels, 0, 2), 0.9);
        assert_eq!(level_at(&levels, 1, 2), 0.3);
        // More columns than levels: each level spreads over several columns, none is empty.
        assert_eq!(
            (0..8).map(|x| level_at(&levels, x, 8)).collect::<Vec<_>>(),
            vec![0.1, 0.1, 0.9, 0.9, 0.2, 0.2, 0.3, 0.3]
        );
        assert_eq!(level_at(&[], 0, 4), 0.0);
        assert_eq!(level_at(&levels, 0, 0), 0.0);
    }

    #[test]
    fn a_tempo_shows_a_decimal_only_when_it_has_one() {
        assert_eq!(format_tempo(120.0), "120 BPM");
        assert_eq!(format_tempo(119.97), "120 BPM");
        assert_eq!(format_tempo(93.5), "93.5 BPM");
    }

    #[test]
    fn tree_fraction_is_clamped_and_leaves_room_for_the_list() {
        let b = bounds(300.0, 600.0);
        let lo = layout(b, 0.0, false, false);
        assert!((lo.tree.height() - (600.0 - HEADER_H) * 0.1).abs() < 1e-9);
        let hi = layout(bounds(300.0, 1000.0), 5.0, false, false);
        assert!((hi.tree.height() - (1000.0 - HEADER_H) * 0.9).abs() < 1e-9);
        let tiny = layout(bounds(300.0, 120.0), 0.9, false, false);
        assert!(tiny.list.height() >= ROW_H - 1e-9);
        assert!(tiny.list.y1 <= 140.0 + 1e-9);
    }

    #[test]
    fn header_controls_fit_at_min_width() {
        let l = layout(bounds(200.0, 400.0), 0.4, false, false);
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
