//! Pitch edits of an audio clip: the saved part of the audio clip editor's Vocal mode.
//!
//! A monophonic clip (a voice, a bass, a lead) is cut into notes by an analysis the app runs when
//! the editor opens (it is not saved). What the user changes is saved per note as a
//! [`PitchEdit`]: the span of the asset the note covers, a shift in cents, how much of the note's
//! slow drift is flattened and whether its vibrato is kept. The span is in **asset frames**, so an
//! edit is pinned to the audio: it survives warp marker and tempo changes, and a split or a join
//! of the clip keeps it by its source span ([`split_pitch_edits`], [`join_pitch_edits`]).
//!
//! The edits are part of the clip's warp job ([`crate::WarpJob::pitch`]): a clip with edits is
//! always rendered, even with warp off, and the render applies them to the asset before the time
//! map (see `atomik_dsp::stretch::psola`).

/// The largest shift of a note, in cents (two octaves either way).
pub const PITCH_EDIT_MAX_CENTS: i32 = 2400;
/// The largest drift amount (a straight line at the note's centre).
pub const PITCH_EDIT_MAX_DRIFT: u8 = 100;
/// The shortest span an edit may cover, in milliseconds of its asset.
pub const PITCH_EDIT_MIN_MS: u64 = 10;
/// Most pitch edits on one clip.
pub const MAX_PITCH_EDITS: usize = 4096;

/// The edit of one note of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PitchEdit {
    /// First asset frame of the note.
    pub src_start: u64,
    /// The asset frame after the note's last.
    pub src_end: u64,
    /// The shift of the whole note, in cents (`-2400..=2400`).
    pub shift_cents: i32,
    /// How much of the pitch curve's slow deviation from the note's centre is flattened, in
    /// percent (`0..=100`; 100 makes the note a straight line).
    pub drift: u8,
    /// While flattening, keep the deviation faster than about 4 Hz (the vibrato).
    pub vibrato_keep: bool,
}

impl PitchEdit {
    /// An edit of the span `[src_start, src_end)` that changes nothing yet (the vibrato is kept).
    pub fn new(src_start: u64, src_end: u64) -> Self {
        Self {
            src_start,
            src_end,
            shift_cents: 0,
            drift: 0,
            vibrato_keep: true,
        }
    }

    /// True when the edit changes nothing (no shift, no flattening).
    pub fn is_neutral(&self) -> bool {
        self.shift_cents == 0 && self.drift == 0
    }

    /// Asset frames the edit covers.
    pub fn frames(&self) -> u64 {
        self.src_end.saturating_sub(self.src_start)
    }
}

/// The shortest span of an edit at `sample_rate`, in frames ([`PITCH_EDIT_MIN_MS`]).
pub fn min_edit_frames(sample_rate: u32) -> u64 {
    (u64::from(sample_rate) * PITCH_EDIT_MIN_MS / 1000).max(1)
}

/// Checks a clip's edits without its asset: the ranges, the order (sorted by start, disjoint)
/// and the count. `Err` says what is wrong.
pub fn check_pitch_edit_list(edits: &[PitchEdit]) -> Result<(), String> {
    if edits.len() > MAX_PITCH_EDITS {
        return Err(format!(
            "{} pitch edits is above the {MAX_PITCH_EDITS} allowed",
            edits.len()
        ));
    }
    for (i, e) in edits.iter().enumerate() {
        if e.src_end <= e.src_start {
            return Err(format!("pitch edit {i} has an empty span"));
        }
        if e.src_end > crate::song::MAX_AUDIO_FRAMES {
            return Err(format!("pitch edit {i} is out of range"));
        }
        if e.shift_cents.abs() > PITCH_EDIT_MAX_CENTS {
            return Err(format!(
                "pitch edit {i}: a shift of {} cents is outside ±{PITCH_EDIT_MAX_CENTS}",
                e.shift_cents
            ));
        }
        if e.drift > PITCH_EDIT_MAX_DRIFT {
            return Err(format!(
                "pitch edit {i}: a drift of {} is above {PITCH_EDIT_MAX_DRIFT}",
                e.drift
            ));
        }
        if i > 0 && edits[i - 1].src_end > e.src_start {
            return Err(format!(
                "pitch edits must be sorted and must not overlap (edit {i})"
            ));
        }
    }
    Ok(())
}

/// Checks a clip's edits against its asset (`asset_frames` frames at `sample_rate`): what
/// [`check_pitch_edit_list`] checks, every span inside the asset and at least
/// [`PITCH_EDIT_MIN_MS`] long.
pub fn validate_pitch_edits(
    edits: &[PitchEdit],
    asset_frames: u64,
    sample_rate: u32,
) -> Result<(), String> {
    check_pitch_edit_list(edits)?;
    let min = min_edit_frames(sample_rate);
    for (i, e) in edits.iter().enumerate() {
        if e.src_end > asset_frames {
            return Err(format!(
                "pitch edit {i} ends past the asset's {asset_frames} frames"
            ));
        }
        if e.frames() < min {
            return Err(format!(
                "pitch edit {i} is shorter than {PITCH_EDIT_MIN_MS} ms"
            ));
        }
    }
    Ok(())
}

/// The edits of the two pieces a cut at asset frame `cut` makes: the piece that plays the frames
/// below the cut keeps every edit that starts there, trimmed to the cut (an edit trimmed below
/// `min_frames` is dropped), and the piece that plays from the cut on keeps the edits that start
/// at or after it. Returns `(below, from)`.
pub fn split_pitch_edits(
    edits: &[PitchEdit],
    cut: u64,
    min_frames: u64,
) -> (Vec<PitchEdit>, Vec<PitchEdit>) {
    let mut below = Vec::new();
    let mut from = Vec::new();
    for e in edits {
        if e.src_start < cut {
            let trimmed = PitchEdit {
                src_end: e.src_end.min(cut),
                ..*e
            };
            if trimmed.frames() >= min_frames.max(1) {
                below.push(trimmed);
            }
        } else {
            from.push(*e);
        }
    }
    (below, from)
}

/// The edits of a clip joined from two pieces of one asset: both lists, in order; an edit of `b`
/// that overlaps one already kept is dropped.
pub fn join_pitch_edits(a: &[PitchEdit], b: &[PitchEdit]) -> Vec<PitchEdit> {
    let mut all: Vec<PitchEdit> = a.iter().chain(b).copied().collect();
    all.sort_by_key(|e| (e.src_start, e.src_end));
    let mut out: Vec<PitchEdit> = Vec::with_capacity(all.len());
    for e in all {
        if out.last().is_none_or(|p| p.src_end <= e.src_start) {
            out.push(e);
        }
    }
    out.truncate(MAX_PITCH_EDITS);
    out
}

/// `edits` with `edit` put in place of any edit that overlaps its span, kept sorted.
pub fn with_pitch_edit(edits: &[PitchEdit], edit: PitchEdit) -> Vec<PitchEdit> {
    let mut out: Vec<PitchEdit> = edits
        .iter()
        .filter(|e| e.src_end <= edit.src_start || e.src_start >= edit.src_end)
        .copied()
        .collect();
    let at = out.partition_point(|e| e.src_start < edit.src_start);
    out.insert(at, edit);
    out
}

/// `edits` without the edits that overlap the span `[src_start, src_end)`.
pub fn without_pitch_edits(edits: &[PitchEdit], src_start: u64, src_end: u64) -> Vec<PitchEdit> {
    edits
        .iter()
        .filter(|e| e.src_end <= src_start || e.src_start >= src_end)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: u64, b: u64, c: i32) -> PitchEdit {
        PitchEdit {
            shift_cents: c,
            ..PitchEdit::new(a, b)
        }
    }

    #[test]
    fn validation_wants_sorted_disjoint_spans_inside_the_asset_and_long_enough() {
        let ok = [e(0, 480, 100), e(480, 1000, -50)];
        assert!(validate_pitch_edits(&ok, 1000, 48_000).is_ok());
        // Overlap, order, empty, too short, past the end, shift and drift ranges.
        assert!(validate_pitch_edits(&[e(0, 500, 0), e(400, 1000, 0)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(500, 1000, 0), e(0, 480, 0)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(10, 10, 0)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(0, 479, 0)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(0, 1001, 0)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(0, 480, 2401)], 1000, 48_000).is_err());
        assert!(validate_pitch_edits(&[e(0, 480, -2400)], 1000, 48_000).is_ok());
        let drift = PitchEdit {
            drift: 101,
            ..e(0, 480, 0)
        };
        assert!(validate_pitch_edits(&[drift], 1000, 48_000).is_err());
    }

    #[test]
    fn a_split_keeps_each_edit_where_it_starts_and_trims_it() {
        let edits = [e(0, 1000, 100), e(1500, 3000, 200), e(3000, 4000, 300)];
        let (below, from) = split_pitch_edits(&edits, 2000, 480);
        assert_eq!(below, vec![e(0, 1000, 100), e(1500, 2000, 200)]);
        assert_eq!(from, vec![e(3000, 4000, 300)]);
        // A trimmed edit too short to keep goes.
        let (below, _) = split_pitch_edits(&edits, 1600, 480);
        assert_eq!(below, vec![e(0, 1000, 100)]);
    }

    #[test]
    fn a_join_keeps_both_lists() {
        let a = [e(0, 1000, 100)];
        let b = [e(2000, 3000, 200), e(500, 1500, 9)];
        assert_eq!(
            join_pitch_edits(&a, &b),
            vec![e(0, 1000, 100), e(2000, 3000, 200)]
        );
    }

    #[test]
    fn replacing_and_removing_edits_keeps_the_list_sorted() {
        let list = [e(0, 1000, 1), e(2000, 3000, 2)];
        let put = with_pitch_edit(&list, e(900, 1500, 7));
        assert_eq!(put, vec![e(900, 1500, 7), e(2000, 3000, 2)]);
        assert_eq!(without_pitch_edits(&list, 2500, 2600), vec![e(0, 1000, 1)]);
    }
}
