use super::*;
use crate::song::{AudioClipContent, Clip, empty_song};
use crate::song_cmd::{new_audio_track, new_track};
use atomik_core::PPQ;

const BAR: i64 = 4 * PPQ;

fn song_of(bars: i64) -> Song {
    let mut s = empty_song(120.0);
    s.end = bars * BAR;
    s
}

fn clip(id: u64, start: i64, length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(id),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content,
    }
}

/// The notes of a one-bar pattern: a hit on each beat, with `velocity` on the last.
fn pattern(velocity: f32) -> Vec<Note> {
    vec![
        Note::new(0, PPQ / 2, 36, 0.9),
        Note::new(PPQ, PPQ / 2, 38, 0.8),
        Note::new(2 * PPQ, PPQ / 2, 36, 0.9),
        Note::new(3 * PPQ, PPQ / 2, 38, velocity),
    ]
}

/// A track `key` with one note clip per bar: bar `i` holds `bars[i]` (nothing for `None`).
fn one_clip_per_bar(song: &mut Song, key: &str, bars: &[Option<Vec<Note>>]) {
    let mut t = new_track(DeviceKey(key.to_owned()));
    for (i, notes) in bars.iter().enumerate() {
        if let Some(notes) = notes {
            let id = song.next_clip_id;
            song.next_clip_id += 1;
            t.lanes[0].clips.push(clip(
                id,
                i as i64 * BAR,
                BAR,
                ClipContent::Notes(notes.clone().into()),
            ));
        }
    }
    song.tracks.push(t);
}

fn same(n: usize) -> Vec<Option<Vec<Note>>> {
    vec![Some(pattern(0.7)); n]
}

#[test]
fn a_pattern_copied_four_times_is_one_idea() {
    let mut s = song_of(16);
    one_clip_per_bar(&mut s, "drums", &same(4));
    let ideas = find(&s);
    assert_eq!(ideas.len(), 1, "{ideas:?}");
    let i = &ideas[0];
    assert_eq!(i.kind, IdeaKind::NoteBars { bars: 1 });
    assert_eq!((i.repeats, i.start, i.end), (4, 0, 4 * BAR));
    assert_eq!(i.bars(&s), (1, 4));
    assert_eq!(i.last_bar(&s), (3 * BAR, 4 * BAR));
    assert_eq!(i.what(), "a 1-bar pattern");
}

#[test]
fn two_repeats_are_not_an_idea() {
    let mut s = song_of(16);
    one_clip_per_bar(&mut s, "drums", &same(2));
    assert!(find(&s).is_empty());
}

#[test]
fn a_change_breaks_the_run() {
    let mut a = song_of(16);
    let mut bars = same(2);
    bars.push(Some(pattern(0.4)));
    bars.extend(same(2));
    one_clip_per_bar(&mut a, "drums", &bars);
    assert!(find(&a).is_empty(), "A A B A A repeats nothing three times");
    // Three of a kind then a change: the three count, the change does not.
    let mut b = song_of(16);
    let mut bars = same(3);
    bars.push(Some(pattern(0.4)));
    one_clip_per_bar(&mut b, "drums", &bars);
    let ideas = find(&b);
    assert_eq!(ideas.len(), 1);
    assert_eq!((ideas[0].repeats, ideas[0].end), (3, 3 * BAR));
}

#[test]
fn the_notes_must_match_exactly() {
    // A hit a tick late, a different length or a different key are changes.
    for tweak in 0..3 {
        let mut s = song_of(16);
        let mut third = pattern(0.7);
        match tweak {
            0 => third[1].tick += 1,
            1 => third[1].length += 1,
            _ => third[1].key += 1,
        }
        let bars = vec![
            Some(pattern(0.7)),
            Some(pattern(0.7)),
            Some(third),
            Some(pattern(0.7)),
        ];
        one_clip_per_bar(&mut s, "drums", &bars);
        assert!(find(&s).is_empty(), "tweak {tweak}");
    }
}

#[test]
fn one_clip_drawn_three_times_and_a_looped_clip_are_found() {
    let mut s = song_of(16);
    let mut notes = Vec::new();
    for bar in 0..3 {
        notes.extend(
            pattern(0.7)
                .into_iter()
                .map(|n| Note::new(n.tick + bar * BAR, n.length, n.key, n.velocity)),
        );
    }
    let mut t = new_track(DeviceKey("a".to_owned()));
    t.lanes[0]
        .clips
        .push(clip(1, 0, 3 * BAR, ClipContent::Notes(notes.into())));
    s.tracks.push(t);
    let mut looped = clip(2, 0, 4 * BAR, ClipContent::Notes(pattern(0.7).into()));
    looped.loop_length = Some(BAR);
    let mut t = new_track(DeviceKey("b".to_owned()));
    t.lanes[0].clips.push(looped);
    s.tracks.push(t);
    let ideas = find(&s);
    let by = |k: &str| ideas.iter().find(|i| i.track.0 == k).unwrap();
    assert_eq!(by("a").repeats, 3);
    assert_eq!(by("b").repeats, 4);
}

#[test]
fn a_two_bar_pattern_is_found_by_its_own_period() {
    let mut s = song_of(16);
    let mut bars = Vec::new();
    for _ in 0..3 {
        bars.push(Some(pattern(0.7)));
        bars.push(Some(pattern(0.5)));
    }
    one_clip_per_bar(&mut s, "drums", &bars);
    let ideas = find(&s);
    assert_eq!(ideas.len(), 1, "{ideas:?}");
    assert_eq!(ideas[0].kind, IdeaKind::NoteBars { bars: 2 });
    assert_eq!((ideas[0].repeats, ideas[0].end), (3, 6 * BAR));
    assert_eq!(ideas[0].what(), "a 2-bar pattern");
}

#[test]
fn eight_identical_bars_are_one_short_idea_not_several() {
    let mut s = song_of(16);
    one_clip_per_bar(&mut s, "drums", &same(8));
    let ideas = find(&s);
    assert_eq!(ideas.len(), 1);
    assert_eq!(ideas[0].kind, IdeaKind::NoteBars { bars: 1 });
    assert_eq!(ideas[0].repeats, 8);
}

#[test]
fn empty_bars_are_not_an_idea_and_gaps_split_a_run() {
    let mut s = song_of(32);
    let mut bars = vec![Some(pattern(0.7))];
    bars.extend(vec![None; 7]);
    bars.push(Some(pattern(0.7)));
    one_clip_per_bar(&mut s, "drums", &bars);
    assert!(find(&s).is_empty());
    // Three, a gap, three: two ideas of 3.
    let mut s = song_of(32);
    let mut bars = same(3);
    bars.push(None);
    bars.extend(same(3));
    one_clip_per_bar(&mut s, "drums", &bars);
    let ideas = find(&s);
    assert_eq!(ideas.len(), 2, "{ideas:?}");
    assert_eq!((ideas[0].start, ideas[1].start), (0, 4 * BAR));
}

#[test]
fn a_held_note_across_bars_is_not_a_pattern() {
    let mut s = song_of(16);
    let mut t = new_track(DeviceKey("pad".to_owned()));
    t.lanes[0].clips.push(clip(
        1,
        0,
        8 * BAR,
        ClipContent::Notes(vec![Note::new(0, 8 * BAR, 48, 0.7)].into()),
    ));
    s.tracks.push(t);
    assert!(find(&s).is_empty());
}

fn audio(id: u64, start: i64, length: i64, offset: u64) -> Clip {
    clip(
        id,
        start,
        length,
        ClipContent::Audio(AudioClipContent::new("a".repeat(64), offset)),
    )
}

#[test]
fn identical_touching_audio_clips_are_an_idea() {
    let mut s = song_of(16);
    let mut t = new_audio_track(DeviceKey("loop".to_owned()));
    for i in 0..4 {
        t.lanes[0].clips.push(audio(i + 1, i as i64 * BAR, BAR, 0));
    }
    s.tracks.push(t);
    let ideas = find(&s);
    assert_eq!(ideas.len(), 1);
    assert_eq!(
        (ideas[0].repeats, ideas[0].start, ideas[0].end),
        (4, 0, 4 * BAR)
    );
    let IdeaKind::AudioClips { lane, clips } = &ideas[0].kind else {
        panic!("{:?}", ideas[0].kind);
    };
    assert_eq!((*lane, clips.len()), (0, 4));
    assert_eq!(ideas[0].what(), "an audio clip");
}

#[test]
fn audio_that_differs_or_does_not_touch_is_not_an_idea() {
    // A different part of the file in the third clip.
    let mut s = song_of(16);
    let mut t = new_audio_track(DeviceKey("loop".to_owned()));
    for (i, offset) in [0u64, 0, 9, 0].into_iter().enumerate() {
        t.lanes[0]
            .clips
            .push(audio(i as u64 + 1, i as i64 * BAR, BAR, offset));
    }
    s.tracks.push(t);
    assert!(find(&s).is_empty());
    // A gap between clips.
    let mut s = song_of(16);
    let mut t = new_audio_track(DeviceKey("loop".to_owned()));
    for i in 0..3 {
        t.lanes[0]
            .clips
            .push(audio(i + 1, i as i64 * 2 * BAR, BAR, 0));
    }
    s.tracks.push(t);
    assert!(find(&s).is_empty());
}

#[test]
fn a_looping_audio_clip_counts_its_repeats() {
    let mut s = song_of(16);
    let mut looped = audio(1, 0, 5 * BAR + BAR / 2, 0);
    looped.loop_length = Some(BAR);
    let mut t = new_audio_track(DeviceKey("loop".to_owned()));
    t.lanes[0].clips.push(looped);
    s.tracks.push(t);
    let ideas = find(&s);
    assert_eq!(ideas.len(), 1);
    assert_eq!((ideas[0].repeats, ideas[0].end), (5, 5 * BAR));
}

#[test]
fn ideas_come_in_time_order() {
    let mut s = song_of(32);
    let mut bars = vec![None; 8];
    bars.extend(same(3));
    one_clip_per_bar(&mut s, "late", &bars);
    one_clip_per_bar(&mut s, "early", &same(3));
    let ideas = find(&s);
    assert_eq!(
        ideas.iter().map(|i| i.track.0.as_str()).collect::<Vec<_>>(),
        ["early", "late"]
    );
}

#[test]
fn a_change_of_meter_is_a_different_bar() {
    let mut s = song_of(16);
    one_clip_per_bar(&mut s, "drums", &same(5));
    // Bar 4 is 3/4: it is shorter, so it cannot be the same bar as the 4/4 ones.
    s.meter = vec![
        crate::MeterEvent {
            bar: 1,
            num: 4,
            den: 4,
        },
        crate::MeterEvent {
            bar: 4,
            num: 3,
            den: 4,
        },
    ];
    let ideas = find(&s);
    assert!(ideas.iter().all(|i| i.repeats <= 3), "{ideas:?}");
}
