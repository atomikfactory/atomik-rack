//! Moving a zone onto an edited copy of its sample.
//!
//! A sample edit makes a new asset (see `atomik_audio_file::edit`) and the zones that played the
//! old one are pointed at it. [`remap_zone`] keeps a zone on the same *audio*: a crop shifts every
//! position by the frames cut from the start (a zone cut off by the new ends shrinks, and one with
//! nothing left is dropped); a reverse mirrors the positions of the reversed range, so the zone
//! still covers the same material, now in the opposite order. An edit that keeps the length and
//! the positions (a fade, a normalise) passes the zone through.

use crate::ZoneDoc;
use atomik_device_sdk::LoopMode;

/// How an edit moved the positions of a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleEdit {
    /// The frames `[from, to)` became the whole new sample.
    Crop {
        /// First frame kept.
        from: u64,
        /// End of the kept frames (exclusive).
        to: u64,
    },
    /// The frames `[from, to)` were mirrored in place; the sample keeps its length.
    Reverse {
        /// First frame of the range.
        from: u64,
        /// End of the range (exclusive).
        to: u64,
    },
    /// The positions did not move (a fade, a normalise).
    Same,
}

/// `zone` as it plays on `asset`, the edited copy of its sample (`frames` long), or `None` when
/// the edit left nothing of the zone to play. Everything but the positions (keys, gain, envelope)
/// is kept. A zone that a reversed range only partly covers keeps its numbers: its ends cannot
/// follow a mirror that cuts through them.
pub fn remap_zone(zone: &ZoneDoc, asset: &str, frames: u64, edit: SampleEdit) -> Option<ZoneDoc> {
    let mut z = zone.clone();
    z.asset = asset.to_owned();
    match edit {
        SampleEdit::Same => {}
        SampleEdit::Crop { from, to } => {
            let start = zone.start.max(from).min(to);
            let end = zone.end.min(to).max(from);
            if start >= end {
                return None;
            }
            z.start = start - from;
            z.end = end - from;
            // A loop moves with the audio and is cut by the new ends; one with nothing left
            // (or a crossfade that no longer fits) is switched off with its fields spanning
            // the zone, which is valid.
            let ls = zone.loop_start.saturating_sub(from).clamp(z.start, z.end);
            let le = zone.loop_end.saturating_sub(from).clamp(z.start, z.end);
            if ls < le {
                z.loop_start = ls;
                z.loop_end = le;
                z.crossfade = zone.crossfade.min(le - ls).min(ls - z.start);
            } else {
                z.loop_start = z.start;
                z.loop_end = z.end;
                z.crossfade = 0;
                if z.loop_mode != LoopMode::Off {
                    z.loop_mode = LoopMode::Off;
                }
            }
        }
        SampleEdit::Reverse { from, to } => {
            let inside = |p: u64| p >= from && p <= to;
            if inside(zone.start) && inside(zone.end) {
                let mirror = |p: u64| from + to - p;
                z.start = mirror(zone.end);
                z.end = mirror(zone.start);
                if inside(zone.loop_start) && inside(zone.loop_end) {
                    z.loop_start = mirror(zone.loop_end);
                    z.loop_end = mirror(zone.loop_start);
                }
                // The crossfade reads the audio before the loop start: what it can use changed.
                z.crossfade = zone
                    .crossfade
                    .min(z.loop_end.saturating_sub(z.loop_start))
                    .min(z.loop_start.saturating_sub(z.start));
            }
        }
    }
    (z.end <= frames).then_some(z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetMeta, ZoneDoc};
    use std::collections::BTreeMap;

    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn looped() -> ZoneDoc {
        let mut z = ZoneDoc::new(OLD, 1000);
        z.start = 100;
        z.end = 900;
        z.loop_mode = LoopMode::Forward;
        z.loop_start = 300;
        z.loop_end = 700;
        z.crossfade = 100;
        z
    }

    fn valid(z: &ZoneDoc, frames: u64) {
        let mut assets = BTreeMap::new();
        assets.insert(
            NEW.to_owned(),
            AssetMeta {
                channels: 1,
                sample_rate: 48_000,
                frames,
                original_name: "x".into(),
            },
        );
        crate::zones::check_zones(&assets, std::slice::from_ref(z)).unwrap();
    }

    #[test]
    fn a_crop_shifts_every_position_by_the_cut() {
        let z = remap_zone(&looped(), NEW, 600, SampleEdit::Crop { from: 200, to: 800 }).unwrap();
        assert_eq!((z.start, z.end), (0, 600), "cut off by the new ends");
        assert_eq!((z.loop_start, z.loop_end), (100, 500));
        assert_eq!(
            z.crossfade, 100,
            "the audio before the loop start is still there"
        );
        assert_eq!(z.asset, NEW);
        valid(&z, 600);
        let z = remap_zone(&looped(), NEW, 800, SampleEdit::Crop { from: 0, to: 800 }).unwrap();
        assert_eq!(
            (z.start, z.end, z.loop_start, z.loop_end),
            (100, 800, 300, 700)
        );
        assert_eq!(z.crossfade, 100);
        valid(&z, 800);
    }

    #[test]
    fn a_crop_drops_a_zone_with_nothing_left_and_unloops_a_cut_loop() {
        let outside = remap_zone(&looped(), NEW, 50, SampleEdit::Crop { from: 900, to: 950 });
        assert_eq!(outside, None);
        let z = remap_zone(&looped(), NEW, 100, SampleEdit::Crop { from: 100, to: 200 }).unwrap();
        assert_eq!((z.start, z.end), (0, 100));
        assert_eq!(z.loop_mode, LoopMode::Off, "the loop lay beyond the cut");
        valid(&z, 100);
    }

    #[test]
    fn a_reverse_mirrors_the_positions_and_twice_restores_them() {
        let old = looped();
        let z = remap_zone(&old, NEW, 1000, SampleEdit::Reverse { from: 0, to: 1000 }).unwrap();
        assert_eq!((z.start, z.end), (100, 900));
        assert_eq!((z.loop_start, z.loop_end), (300, 700));
        let mut one = old.clone();
        one.start = 0;
        one.end = 400;
        one.loop_mode = LoopMode::Off;
        one.loop_start = 0;
        one.loop_end = 400;
        one.crossfade = 0;
        let r = remap_zone(&one, NEW, 1000, SampleEdit::Reverse { from: 0, to: 1000 }).unwrap();
        assert_eq!((r.start, r.end), (600, 1000));
        let back = remap_zone(&r, OLD, 1000, SampleEdit::Reverse { from: 0, to: 1000 }).unwrap();
        assert_eq!((back.start, back.end), (one.start, one.end));
        valid(&r, 1000);
    }

    #[test]
    fn a_reverse_leaves_a_zone_the_range_cuts_through_and_one_outside() {
        let straddle = remap_zone(
            &looped(),
            NEW,
            1000,
            SampleEdit::Reverse {
                from: 500,
                to: 1000,
            },
        )
        .unwrap();
        assert_eq!((straddle.start, straddle.end), (100, 900));
        let inside = remap_zone(
            &looped(),
            NEW,
            1000,
            SampleEdit::Reverse { from: 0, to: 950 },
        )
        .unwrap();
        assert_eq!((inside.start, inside.end), (50, 850));
        let same = remap_zone(&looped(), NEW, 1000, SampleEdit::Same).unwrap();
        assert_eq!((same.start, same.end, same.asset.as_str()), (100, 900, NEW));
    }
}
