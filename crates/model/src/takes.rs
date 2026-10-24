//! Takes and comping: stacked recordings of the same time range on one lane, and the choice of
//! which parts of which take play.
//!
//! # Model
//!
//! A [`TakeGroup`] lives on a lane ([`crate::song::Lane::take_groups`]). It keeps its takes (whole
//! clips, bottom to top in recording order) **outside** the lane's clip list, so nothing that
//! reads `Lane::clips` (the engine's flatten, the editors, selection, the clipboard) ever sees a
//! hidden take. What plays is the **comp**: an ordered list of [`CompSegment`]s `{take, start,
//! end}`, disjoint and sorted, each saying "this range plays from that take".
//!
//! # The comp is compiled to plain clips at command time
//!
//! [`comp_clips`] turns the comp into ordinary clips (a take clip cut to the segment, with the
//! content offset or the source offset moved the way `SplitClip` moves them), and the take
//! commands put those clips on the lane in the same undo step that changes the comp. The group
//! remembers their ids ([`TakeGroup::clips`]) so the next change can swap them. The engine,
//! `flatten_*`, the bounce and every older reader therefore see only plain clips and need no
//! change. The alternative, a comp the engine resolves while playing, would need new engine and
//! song-input types, a second place that knows the comp rules, and a take stored as a hidden clip
//! the playback path has to skip; compiling at command time keeps one source of truth (the
//! compile function) at the price of one extra copy of the notes or clip metadata per segment.
//!
//! # Audio boundaries
//!
//! A cut that is not the natural edge of its take (a segment that starts or ends strictly inside
//! the take) gets a short fade ([`COMP_FADE_FRAMES`], equal power) so the splice does not click;
//! a natural edge keeps the take's own fade. Overlapping clips never sum in the engine (a later
//! clip hides an earlier one), so a boundary between two takes is a fade out followed by a fade
//! in, a dip of a few milliseconds, not an overlapped crossfade.

use crate::RackDoc;
use crate::song::{AudioClipContent, Clip, ClipContent, ClipId, FadeCurve};

/// Length in frames of the fade at a comp cut inside an audio take (5 ms at 48 kHz).
pub const COMP_FADE_FRAMES: u64 = 240;

/// One stretch of the comp: `[start, end)` plays from take number `take` (an index into
/// [`TakeGroup::takes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompSegment {
    /// Index of the take.
    pub take: usize,
    /// First song tick.
    pub start: i64,
    /// End tick (exclusive), greater than `start`.
    pub end: i64,
}

/// A set of stacked takes on one lane and the comp that picks what plays (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct TakeGroup {
    /// Unique in the song. Not saved: a loaded song numbers its groups from 1 in file order.
    pub id: u32,
    /// The takes, bottom to top (recording order). Each is a whole clip of the lane's kind; its
    /// own id is unique in the song but it is not in the lane's clip list.
    pub takes: Vec<Clip>,
    /// The comp: sorted, disjoint, each segment within [`TakeGroup::range`] and naming an
    /// existing take. A range with no segment is silent.
    pub comp: Vec<CompSegment>,
    /// The lane clips compiled from the comp (see the module docs).
    pub clips: Vec<ClipId>,
}

impl TakeGroup {
    /// The time range of the group: from the earliest take start to the latest take end.
    pub fn range(&self) -> (i64, i64) {
        let start = self.takes.iter().map(|c| c.start).min().unwrap_or(0);
        let end = self.takes.iter().map(Clip::end).max().unwrap_or(start);
        (start, end)
    }

    /// The take that plays at `tick`, if any.
    pub fn take_at(&self, tick: i64) -> Option<usize> {
        self.comp
            .iter()
            .find(|s| s.start <= tick && tick < s.end)
            .map(|s| s.take)
    }

    /// The take that plays the whole range of the group alone (a "picked" take), if the comp is
    /// exactly one segment over the whole range.
    pub fn picked(&self) -> Option<usize> {
        let (start, end) = self.range();
        match self.comp.as_slice() {
            [s] if s.start == start && s.end == end => Some(s.take),
            _ => None,
        }
    }
}

/// The comp a fresh group gets: every take in turn over its own extent, so a later take hides an
/// earlier one where both have content. This is what plain stacked clips played ("later clips
/// win"), so grouping a recording does not change how it sounds.
pub fn layered_comp(takes: &[Clip]) -> Vec<CompSegment> {
    let mut comp = Vec::new();
    for (i, t) in takes.iter().enumerate() {
        set_segment(&mut comp, i, t.start, t.end());
    }
    comp
}

/// Makes `[start, end)` play from `take`: segments under the range are trimmed or dropped, and
/// segments of one take that touch are joined. `start >= end` changes nothing.
pub fn set_segment(comp: &mut Vec<CompSegment>, take: usize, start: i64, end: i64) {
    if start >= end {
        return;
    }
    let mut out: Vec<CompSegment> = Vec::with_capacity(comp.len() + 2);
    for s in comp.iter() {
        if s.end <= start || s.start >= end {
            out.push(*s);
            continue;
        }
        if s.start < start {
            out.push(CompSegment { end: start, ..*s });
        }
        if s.end > end {
            out.push(CompSegment { start: end, ..*s });
        }
    }
    out.push(CompSegment { take, start, end });
    out.sort_by_key(|s| s.start);
    *comp = join_touching(out);
}

/// Joins neighbouring segments of the same take that touch.
fn join_touching(sorted: Vec<CompSegment>) -> Vec<CompSegment> {
    let mut out: Vec<CompSegment> = Vec::with_capacity(sorted.len());
    for s in sorted {
        match out.last_mut() {
            Some(last) if last.take == s.take && last.end == s.start => last.end = s.end,
            _ => out.push(s),
        }
    }
    out
}

/// The comp after take `removed` is deleted (`takes` is the count before): its segments go to the
/// take below it (the one that was above it when it was the bottom take), and later take numbers
/// move down by one. `None` when no take is left.
pub fn comp_without_take(
    comp: &[CompSegment],
    removed: usize,
    takes: usize,
) -> Option<Vec<CompSegment>> {
    if takes <= 1 {
        return None;
    }
    let heir = if removed == 0 { 0 } else { removed - 1 };
    let renumber = |t: usize| {
        if t == removed {
            heir
        } else if t > removed {
            t - 1
        } else {
            t
        }
    };
    let moved = comp
        .iter()
        .map(|s| CompSegment {
            take: renumber(s.take),
            ..*s
        })
        .collect();
    Some(join_touching(moved))
}

/// Checks a comp against `takes` takes whose range is `range`: segments sorted, disjoint,
/// non-empty, inside the range, naming existing takes.
pub fn check_comp(comp: &[CompSegment], takes: usize, range: (i64, i64)) -> Result<(), String> {
    let mut prev_end = i64::MIN;
    for (i, s) in comp.iter().enumerate() {
        if s.take >= takes {
            return Err(format!(
                "comp segment {i} names take {} but the group has {takes}",
                s.take
            ));
        }
        if s.start >= s.end {
            return Err(format!(
                "comp segment {i} [{}, {}) is empty",
                s.start, s.end
            ));
        }
        if s.start < range.0 || s.end > range.1 {
            return Err(format!(
                "comp segment {i} [{}, {}) is outside the group's range [{}, {})",
                s.start, s.end, range.0, range.1
            ));
        }
        if s.start < prev_end {
            return Err(format!(
                "comp segment {i} overlaps or precedes the one before"
            ));
        }
        prev_end = s.end;
    }
    Ok(())
}

/// The plain clips a comp plays as: for each segment, the take's clip cut to the segment (and to
/// the take's own extent), ids 0 (the command allocates them). Notes and automation keep their
/// content alignment (the content offset moves by the cut, as `SplitClip` does); an audio clip
/// starts the frames between the two ticks later in its asset. See the module docs for the
/// fades.
pub fn comp_clips(doc: &RackDoc, group: &TakeGroup) -> Vec<Clip> {
    let song = &doc.song;
    let rate = doc.sample_rate;
    let mut out = Vec::new();
    for seg in &group.comp {
        let Some(take) = group.takes.get(seg.take) else {
            continue;
        };
        let (a, b) = (seg.start.max(take.start), seg.end.min(take.end()));
        if a >= b {
            continue;
        }
        let shift = take.offset + (a - take.start);
        let mut clip = Clip {
            id: ClipId(0),
            start: a,
            length: b - a,
            offset: take.loop_length.map_or(shift, |n| shift.rem_euclid(n)),
            ..take.clone()
        };
        if let ClipContent::Audio(src) = &take.content {
            let delta = song.frame_at(a, rate) - song.frame_at(take.start, rate);
            let asset_frames = doc.assets.get(&src.asset).map_or(u64::MAX, |m| m.frames);
            let frames = song.frame_at(b, rate) - song.frame_at(a, rate);
            let (cut_in, cut_out) = (a > take.start, b < take.end());
            let cap = (frames / 2).max(1);
            let (source_offset, markers) =
                src.piece_from(song, rate, take.start, a, delta, asset_frames);
            let mut audio = AudioClipContent {
                source_offset,
                ..src.clone()
            };
            audio.warp.markers = markers;
            if cut_in {
                audio.fade_in = COMP_FADE_FRAMES.min(cap);
            }
            if cut_out {
                audio.fade_out = COMP_FADE_FRAMES.min(cap);
            }
            if cut_in || cut_out {
                audio.fade_curve = FadeCurve::EqualPower;
            }
            clip.offset = 0;
            clip.content = ClipContent::Audio(audio);
        }
        out.push(clip);
    }
    out
}

#[cfg(test)]
#[path = "takes_tests.rs"]
mod tests;
