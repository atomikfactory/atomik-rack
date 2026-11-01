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
//! a natural edge keeps the take's own fade. A boundary between two takes is then a fade out
//! followed by a fade in, a dip of a few milliseconds.
//!
//! A cut can instead carry a crossfade ([`CompSegment::xfade`], in ticks): the compiled clip
//! before the cut then runs on past it by that length and the clip after the cut crossfades
//! over it with an equal-power curve, the overlap rule of `flatten_audio` (the engine mixes the
//! two regions). The length is clamped to the later segment and to what is left of the earlier
//! take; a cut without it compiles exactly as before.

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
    /// Crossfade in ticks at this segment's start, with the segment that ends there (another
    /// take, audio only): 0 is the short dip of [`COMP_FADE_FRAMES`]. Ignored when no segment
    /// ends at `start`.
    pub xfade: u64,
}

impl CompSegment {
    /// A segment with no crossfade.
    pub fn new(take: usize, start: i64, end: i64) -> Self {
        Self {
            take,
            start,
            end,
            xfade: 0,
        }
    }
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
/// segments of one take that touch are joined. `start >= end` changes nothing. The new segment
/// has no crossfade, and neither has the piece of a segment that is left after it (its start is
/// a new cut).
pub fn set_segment(comp: &mut Vec<CompSegment>, take: usize, start: i64, end: i64) {
    if start >= end {
        return;
    }
    let mut out = cut_out(comp, start, end);
    out.push(CompSegment::new(take, start, end));
    out.sort_by_key(|s| s.start);
    *comp = join_touching(out);
}

/// Makes `[start, end)` silent: segments under the range are trimmed or dropped. `start >= end`
/// changes nothing.
pub fn clear_segment(comp: &mut Vec<CompSegment>, start: i64, end: i64) {
    if start >= end {
        return;
    }
    *comp = cut_out(comp, start, end);
}

/// The segments of `comp` with `[start, end)` cut out of them.
fn cut_out(comp: &[CompSegment], start: i64, end: i64) -> Vec<CompSegment> {
    let mut out: Vec<CompSegment> = Vec::with_capacity(comp.len() + 2);
    for s in comp {
        if s.end <= start || s.start >= end {
            out.push(*s);
            continue;
        }
        if s.start < start {
            out.push(CompSegment { end: start, ..*s });
        }
        if s.end > end {
            out.push(CompSegment {
                start: end,
                xfade: 0,
                ..*s
            });
        }
    }
    out
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

/// The comp after a take is inserted at index `at` (the takes from `at` up move up by one).
pub fn comp_with_take_inserted(comp: &[CompSegment], at: usize) -> Vec<CompSegment> {
    comp.iter()
        .map(|s| CompSegment {
            take: if s.take >= at { s.take + 1 } else { s.take },
            ..*s
        })
        .collect()
}

/// The comp after take `from` moves to index `to` of the reordered list: the takes between the
/// two shift by one, so every segment keeps playing the same take.
pub fn comp_with_take_moved(comp: &[CompSegment], from: usize, to: usize) -> Vec<CompSegment> {
    let renumber = |t: usize| {
        if t == from {
            to
        } else if from < to && t > from && t <= to {
            t - 1
        } else if from > to && t >= to && t < from {
            t + 1
        } else {
            t
        }
    };
    comp.iter()
        .map(|s| CompSegment {
            take: renumber(s.take),
            ..*s
        })
        .collect()
}

/// The comp with every segment cut back to `range` (segments left empty go).
pub fn comp_within(comp: &[CompSegment], range: (i64, i64)) -> Vec<CompSegment> {
    comp.iter()
        .filter_map(|s| {
            let (a, b) = (s.start.max(range.0), s.end.min(range.1));
            (a < b).then_some(CompSegment {
                start: a,
                end: b,
                xfade: if a > s.start { 0 } else { s.xfade },
                ..*s
            })
        })
        .collect()
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

/// The part of segment `seg` its take plays: the segment cut to the take's own extent, `None`
/// when nothing is left.
fn played(group: &TakeGroup, seg: &CompSegment) -> Option<(i64, i64)> {
    let take = group.takes.get(seg.take)?;
    let (a, b) = (seg.start.max(take.start), seg.end.min(take.end()));
    (a < b).then_some((a, b))
}

/// The crossfade length in ticks that segment `i` really gets at its start (0: none, the cut is
/// the short dip). It needs a segment of another audio take that plays up to this segment's
/// start, and this segment's take to play from there; it is clamped to this segment's length and
/// to what is left of the earlier take after the cut.
fn crossfade_in(group: &TakeGroup, i: usize) -> i64 {
    let seg = &group.comp[i];
    let Some(prev) = i.checked_sub(1).map(|p| &group.comp[p]) else {
        return 0;
    };
    if seg.xfade == 0 || prev.end != seg.start || prev.take == seg.take {
        return 0;
    }
    let (Some(a), Some(prev_take), Some(take)) = (
        played(group, seg),
        group.takes.get(prev.take),
        group.takes.get(seg.take),
    ) else {
        return 0;
    };
    let Some((_, prev_b)) = played(group, prev) else {
        return 0;
    };
    // Both clips must meet exactly at the cut, and both be audio.
    if a.0 != seg.start || prev_b != seg.start || !take.content.is_audio() {
        return 0;
    }
    if !prev_take.content.is_audio() {
        return 0;
    }
    let left = prev_take.end() - prev_b;
    i64::try_from(seg.xfade)
        .unwrap_or(i64::MAX)
        .min(a.1 - a.0)
        .min(left)
        .max(0)
}

/// The plain clips a comp plays as: for each segment, the take's clip cut to the segment (and to
/// the take's own extent), ids 0 (the command allocates them). Notes and automation keep their
/// content alignment (the content offset moves by the cut, as `SplitClip` does); an audio clip
/// starts the frames between the two ticks later in its asset. See the module docs for the
/// fades and the crossfades (a segment with `xfade` makes the clip before it run on and
/// overlap this one).
pub fn comp_clips(doc: &RackDoc, group: &TakeGroup) -> Vec<Clip> {
    let song = &doc.song;
    let rate = doc.sample_rate;
    let n = group.comp.len();
    let xin: Vec<i64> = (0..n).map(|i| crossfade_in(group, i)).collect();
    // Compiled on the first audio segment, once for the whole group.
    let clock = std::cell::OnceCell::new();
    let mut out = Vec::new();
    for (i, seg) in group.comp.iter().enumerate() {
        let Some(take) = group.takes.get(seg.take) else {
            continue;
        };
        let Some((a, cut_b)) = played(group, seg) else {
            continue;
        };
        // The clip runs on past its cut while the next segment crossfades in over it.
        let ahead = xin.get(i + 1).copied().unwrap_or(0);
        let b = cut_b + ahead;
        let shift = take.offset + (a - take.start);
        let mut clip = Clip {
            id: ClipId(0),
            start: a,
            length: b - a,
            offset: take.loop_length.map_or(shift, |n| shift.rem_euclid(n)),
            ..take.clone()
        };
        if let ClipContent::Audio(src) = &take.content {
            let clock = clock.get_or_init(|| song.frame_clock(rate));
            let delta = clock.at(a) - clock.at(take.start);
            let asset_frames = doc.assets.get(&src.asset).map_or(u64::MAX, |m| m.frames);
            let frames = clock.at(b) - clock.at(a);
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
            // The crossfade is the fade at a crossfaded edge: no dip on top of it.
            if xin[i] > 0 {
                audio.fade_in = 0;
                audio.xfade = Some(FadeCurve::EqualPower);
            }
            if ahead > 0 && cut_out {
                audio.fade_out = 0;
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
