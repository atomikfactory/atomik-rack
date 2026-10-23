//! Markers and range markers of the arrangement: the pure edits and the palette. Every edit
//! takes the current list and returns the whole new one; the view sends it as `SetMarkers` /
//! `DragMarkers` (and the range twins), so one gesture is one undo step.

use atomik_model::{MAX_MARKER_NAME, MAX_TICK, Marker, RangeMarker};

/// What a marker or a range is, for selecting and hit testing: its kind and id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MarkSel {
    /// A point marker.
    Marker(u32),
    /// A range marker.
    Range(u32),
}

/// The palette of the context menu: the argument of `sequencer.marker_color` and the colour.
pub const PALETTE: [(&str, [u8; 3]); 6] = [
    ("red", [0xe0, 0x5a, 0x4f]),
    ("orange", [0xe8, 0x9a, 0x3c]),
    ("yellow", [0xe0, 0xc8, 0x4a]),
    ("green", [0x5a, 0xb8, 0x6a]),
    ("blue", [0x3f, 0x8f, 0xd2]),
    ("purple", [0x9a, 0x6a, 0xd0]),
];

/// The colour a palette name stands for (`"none"` clears it).
pub fn palette_color(name: &str) -> Option<Option<[u8; 3]>> {
    if name == "none" {
        return Some(None);
    }
    PALETTE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| Some(*c))
}

/// The next automatic name `"<prefix> N"`: one above the highest `N` among the names that
/// look like it (so deleting a marker never makes a name come back).
pub fn auto_name<'a>(prefix: &str, names: impl IntoIterator<Item = &'a str>) -> String {
    let next = names
        .into_iter()
        .filter_map(|n| {
            n.strip_prefix(prefix)
                .and_then(|r| r.strip_prefix(' '))
                .and_then(|r| r.parse::<u32>().ok())
        })
        .max()
        .map_or(1, |n| n.saturating_add(1));
    format!("{prefix} {next}")
}

/// A name as the model takes it: trimmed, cut to the length limit; `None` when it is empty.
pub fn clean_name(text: &str) -> Option<String> {
    let name: String = text.trim().chars().take(MAX_MARKER_NAME).collect();
    let name = name.trim_end().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The markers with a new one at `tick` (`None` when `tick` is outside the song limits or a
/// marker is there already) and the new id.
pub fn add_marker(markers: &[Marker], tick: i64, name: &str) -> Option<(Vec<Marker>, u32)> {
    if !(0..=MAX_TICK).contains(&tick) || markers.iter().any(|m| m.tick == tick) {
        return None;
    }
    let id = markers.iter().map(|m| m.id).max().map_or(1, |m| m + 1);
    let mut new = markers.to_vec();
    new.push(Marker {
        id,
        tick,
        name: clean_name(name)?,
        color: None,
    });
    new.sort_by_key(|m| (m.tick, m.id));
    Some((new, id))
}

/// The markers with marker `id` moved to `tick` (clamped to the song limits).
pub fn move_marker(markers: &[Marker], id: u32, tick: i64) -> Vec<Marker> {
    let mut new = markers.to_vec();
    if let Some(m) = new.iter_mut().find(|m| m.id == id) {
        m.tick = tick.clamp(0, MAX_TICK);
    }
    new.sort_by_key(|m| (m.tick, m.id));
    new
}

/// The markers without marker `id`.
pub fn delete_marker(markers: &[Marker], id: u32) -> Vec<Marker> {
    markers.iter().filter(|m| m.id != id).cloned().collect()
}

/// The markers with marker `id` renamed (`None` for an empty name).
pub fn rename_marker(markers: &[Marker], id: u32, text: &str) -> Option<Vec<Marker>> {
    let name = clean_name(text)?;
    let mut new = markers.to_vec();
    new.iter_mut().find(|m| m.id == id)?.name = name;
    Some(new)
}

/// The markers with the colour of marker `id` set.
pub fn color_marker(markers: &[Marker], id: u32, color: Option<[u8; 3]>) -> Vec<Marker> {
    let mut new = markers.to_vec();
    if let Some(m) = new.iter_mut().find(|m| m.id == id) {
        m.color = color;
    }
    new
}

/// The ranges with a new one over `[start, end)` (`None` for an empty or out-of-limits span)
/// and the new id.
pub fn add_range(
    ranges: &[RangeMarker],
    start: i64,
    end: i64,
    name: &str,
) -> Option<(Vec<RangeMarker>, u32)> {
    if start < 0 || end <= start || end > MAX_TICK {
        return None;
    }
    let id = ranges.iter().map(|r| r.id).max().map_or(1, |r| r + 1);
    let mut new = ranges.to_vec();
    new.push(RangeMarker {
        id,
        start,
        end,
        name: clean_name(name)?,
        color: None,
    });
    new.sort_by_key(|r| (r.start, r.id));
    Some((new, id))
}

/// Which part of a range a gesture grabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangePart {
    /// The body: the whole range moves.
    Body,
    /// The left edge.
    Start,
    /// The right edge.
    End,
}

/// The ranges with range `id` moved so that it starts at `start` (its length is kept; the
/// start is clamped so the range stays inside the song limits).
pub fn move_range(ranges: &[RangeMarker], id: u32, start: i64) -> Vec<RangeMarker> {
    let mut new = ranges.to_vec();
    if let Some(r) = new.iter_mut().find(|r| r.id == id) {
        let len = r.end - r.start;
        r.start = start.clamp(0, (MAX_TICK - len).max(0));
        r.end = r.start + len;
    }
    new.sort_by_key(|r| (r.start, r.id));
    new
}

/// The ranges with one edge of range `id` at `tick`: the start stays at least `min` below the
/// end and at least 0, the end at least `min` above the start and within the song limits.
pub fn resize_range(
    ranges: &[RangeMarker],
    id: u32,
    part: RangePart,
    tick: i64,
    min: i64,
) -> Vec<RangeMarker> {
    let mut new = ranges.to_vec();
    if let Some(r) = new.iter_mut().find(|r| r.id == id) {
        match part {
            RangePart::Start => r.start = tick.clamp(0, (r.end - min).max(0)),
            RangePart::End => r.end = tick.clamp(r.start + min, MAX_TICK.max(r.start + min)),
            RangePart::Body => {}
        }
    }
    new.sort_by_key(|r| (r.start, r.id));
    new
}

/// The ranges without range `id`.
pub fn delete_range(ranges: &[RangeMarker], id: u32) -> Vec<RangeMarker> {
    ranges.iter().filter(|r| r.id != id).cloned().collect()
}

/// The ranges with range `id` renamed (`None` for an empty name).
pub fn rename_range(ranges: &[RangeMarker], id: u32, text: &str) -> Option<Vec<RangeMarker>> {
    let name = clean_name(text)?;
    let mut new = ranges.to_vec();
    new.iter_mut().find(|r| r.id == id)?.name = name;
    Some(new)
}

/// The ranges with the colour of range `id` set.
pub fn color_range(ranges: &[RangeMarker], id: u32, color: Option<[u8; 3]>) -> Vec<RangeMarker> {
    let mut new = ranges.to_vec();
    if let Some(r) = new.iter_mut().find(|r| r.id == id) {
        r.color = color;
    }
    new
}
