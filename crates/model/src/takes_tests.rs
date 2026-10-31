use super::*;
use crate::song::Note;
use crate::{AssetMeta, empty_doc};

fn seg(take: usize, start: i64, end: i64) -> CompSegment {
    CompSegment { take, start, end }
}

fn notes_take(start: i64, length: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Notes(vec![Note::new(0, 10, 60, 0.8)].into()),
    }
}

fn audio_take(start: i64, length: i64, asset: &str, offset: u64) -> Clip {
    Clip {
        content: ClipContent::Audio(AudioClipContent {
            asset: asset.to_owned(),
            source_offset: offset,
            gain_db: 0.0,
            fade_in: 7,
            fade_out: 9,
            fade_curve: FadeCurve::Linear,
            reverse: false,
            envelope: Vec::new(),
            xfade: None,
            warp: Default::default(),
        }),
        ..notes_take(start, length)
    }
}

fn doc_with_asset(asset: &str, frames: u64) -> RackDoc {
    let mut doc = empty_doc();
    doc.assets.insert(
        asset.to_owned(),
        AssetMeta {
            channels: 1,
            sample_rate: doc.sample_rate,
            frames,
            original_name: "t.wav".into(),
        },
    );
    doc
}

fn audio_of(c: &Clip) -> AudioClipContent {
    match &c.content {
        ClipContent::Audio(a) => a.clone(),
        ClipContent::Notes(_) | ClipContent::Automation(_) => panic!("not an audio clip"),
    }
}

#[test]
fn set_segment_cuts_the_range_out_of_what_played_there() {
    let mut comp = vec![seg(0, 0, 100)];
    set_segment(&mut comp, 1, 30, 60);
    assert_eq!(comp, [seg(0, 0, 30), seg(1, 30, 60), seg(0, 60, 100)]);
}

#[test]
fn set_segment_trims_a_neighbour_and_drops_what_it_covers() {
    let mut comp = vec![seg(0, 0, 40), seg(1, 40, 60), seg(2, 60, 100)];
    set_segment(&mut comp, 3, 30, 70);
    assert_eq!(comp, [seg(0, 0, 30), seg(3, 30, 70), seg(2, 70, 100)]);
}

#[test]
fn set_segment_joins_touching_segments_of_the_same_take() {
    let mut comp = vec![seg(0, 0, 40), seg(1, 40, 60)];
    set_segment(&mut comp, 0, 40, 60);
    assert_eq!(comp, [seg(0, 0, 60)]);
    set_segment(&mut comp, 0, 80, 90);
    assert_eq!(comp, [seg(0, 0, 60), seg(0, 80, 90)], "a gap keeps two");
}

#[test]
fn set_segment_with_an_empty_range_changes_nothing() {
    let mut comp = vec![seg(0, 0, 40)];
    set_segment(&mut comp, 1, 20, 20);
    set_segment(&mut comp, 1, 30, 10);
    assert_eq!(comp, [seg(0, 0, 40)]);
}

#[test]
fn a_layered_comp_lets_a_later_take_hide_an_earlier_one() {
    let takes = [notes_take(0, 100), notes_take(0, 100), notes_take(0, 40)];
    assert_eq!(layered_comp(&takes), [seg(2, 0, 40), seg(1, 40, 100)]);
    let shifted = [notes_take(0, 50), notes_take(25, 50)];
    assert_eq!(layered_comp(&shifted), [seg(0, 0, 25), seg(1, 25, 75)]);
}

#[test]
fn deleting_a_take_gives_its_ranges_to_the_take_below() {
    let comp = [seg(0, 0, 10), seg(1, 10, 20), seg(2, 20, 30)];
    // Take 1 goes: its range joins take 0 and take 2 becomes take 1.
    assert_eq!(
        comp_without_take(&comp, 1, 3).unwrap(),
        [seg(0, 0, 20), seg(1, 20, 30)]
    );
    // The bottom take goes: its range goes to the one above it, now the bottom.
    assert_eq!(
        comp_without_take(&comp, 0, 3).unwrap(),
        [seg(0, 0, 20), seg(1, 20, 30)]
    );
    assert_eq!(comp_without_take(&comp, 2, 3).unwrap().len(), 2);
    assert!(comp_without_take(&[seg(0, 0, 10)], 0, 1).is_none());
}

#[test]
fn check_comp_names_what_is_wrong() {
    let ok = [seg(0, 0, 10), seg(1, 10, 20)];
    assert!(check_comp(&ok, 2, (0, 20)).is_ok());
    let e = check_comp(&ok, 1, (0, 20)).unwrap_err();
    assert!(e.contains("names take 1"), "{e}");
    let e = check_comp(&[seg(0, 5, 5)], 1, (0, 20)).unwrap_err();
    assert!(e.contains("empty"), "{e}");
    let e = check_comp(&[seg(0, 0, 30)], 1, (0, 20)).unwrap_err();
    assert!(e.contains("outside"), "{e}");
    let e = check_comp(&[seg(0, 0, 10), seg(0, 5, 15)], 1, (0, 20)).unwrap_err();
    assert!(e.contains("overlaps"), "{e}");
}

#[test]
fn the_range_is_the_union_of_the_takes_and_a_picked_take_covers_all_of_it() {
    let group = TakeGroup {
        id: 1,
        takes: vec![notes_take(10, 30), notes_take(20, 50)],
        comp: vec![seg(1, 10, 70)],
        clips: Vec::new(),
    };
    assert_eq!(group.range(), (10, 70));
    assert_eq!(group.picked(), Some(1));
    assert_eq!(group.take_at(15), Some(1));
    assert_eq!(group.take_at(70), None);
    let mixed = TakeGroup {
        comp: vec![seg(0, 10, 20), seg(1, 20, 70)],
        ..group
    };
    assert_eq!(mixed.picked(), None);
}

#[test]
fn note_comp_clips_keep_content_alignment_like_a_split() {
    let doc = empty_doc();
    let group = TakeGroup {
        id: 1,
        takes: vec![notes_take(100, 400)],
        comp: vec![seg(0, 150, 300)],
        clips: Vec::new(),
    };
    let out = comp_clips(&doc, &group);
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].start, out[0].length, out[0].offset), (150, 150, 50));
    // A segment outside the extent of the take makes no clip.
    let beyond = TakeGroup {
        comp: vec![seg(0, 0, 100), seg(0, 500, 600)],
        ..group
    };
    assert!(comp_clips(&doc, &beyond).is_empty());
}

#[test]
fn audio_cuts_start_later_in_the_asset_and_fade_where_they_cut() {
    let asset = "ab".repeat(32);
    let doc = doc_with_asset(&asset, 10_000_000);
    let ppq = atomik_core::PPQ;
    let group = TakeGroup {
        id: 1,
        takes: vec![audio_take(0, 8 * ppq, &asset, 1000)],
        comp: vec![
            seg(0, 0, 2 * ppq),
            seg(0, 3 * ppq, 5 * ppq),
            seg(0, 6 * ppq, 8 * ppq),
        ],
        clips: Vec::new(),
    };
    let out = comp_clips(&doc, &group);
    assert_eq!(out.len(), 3);
    // 120 BPM, 48 kHz: one beat is 24 000 frames.
    let (a, b, c) = (audio_of(&out[0]), audio_of(&out[1]), audio_of(&out[2]));
    assert_eq!(a.source_offset, 1000);
    assert_eq!(b.source_offset, 1000 + 3 * 24_000);
    assert_eq!(c.source_offset, 1000 + 6 * 24_000);
    // The own fade-in of the take survives on its natural start and its fade-out on its natural
    // end; every cut gets the short comp fade, equal power.
    assert_eq!((a.fade_in, a.fade_out), (7, COMP_FADE_FRAMES));
    assert_eq!(
        (b.fade_in, b.fade_out),
        (COMP_FADE_FRAMES, COMP_FADE_FRAMES)
    );
    assert_eq!((c.fade_in, c.fade_out), (COMP_FADE_FRAMES, 9));
    assert_eq!(a.fade_curve, FadeCurve::EqualPower);
    assert_eq!(c.fade_curve, FadeCurve::EqualPower);
}

#[test]
fn a_whole_take_segment_compiles_to_the_take_itself() {
    let asset = "cd".repeat(32);
    let doc = doc_with_asset(&asset, 1_000_000);
    let take = audio_take(480, 4000, &asset, 33);
    let group = TakeGroup {
        id: 1,
        comp: vec![seg(0, 480, 4480)],
        takes: vec![take.clone()],
        clips: Vec::new(),
    };
    assert_eq!(comp_clips(&doc, &group), [take]);
}

#[test]
fn a_fade_never_exceeds_half_of_a_short_piece() {
    let asset = "ef".repeat(32);
    let doc = doc_with_asset(&asset, 1_000_000);
    let group = TakeGroup {
        id: 1,
        takes: vec![audio_take(0, 100_000, &asset, 0)],
        // 4 000 ticks are 6 250 frames at 120 BPM and 48 kHz (1.5625 frames per tick).
        comp: vec![seg(0, 1000, 5000)],
        clips: Vec::new(),
    };
    let out = comp_clips(&doc, &group);
    let a = audio_of(&out[0]);
    assert_eq!(
        (a.fade_in, a.fade_out),
        (COMP_FADE_FRAMES, COMP_FADE_FRAMES)
    );
    // 200 ticks are 312 frames, so each fade is at most 156.
    let short = TakeGroup {
        comp: vec![seg(0, 1000, 1200)],
        ..group
    };
    let a = audio_of(&comp_clips(&doc, &short)[0]);
    assert_eq!((a.fade_in, a.fade_out), (156, 156));
}
