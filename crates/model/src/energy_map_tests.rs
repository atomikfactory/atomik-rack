use super::*;
use crate::DeviceKey;
use crate::song::{Clip, ClipId, Marker, Note, RangeMarker, empty_song};
use crate::song_cmd::new_track;
use atomik_core::PPQ;

const BAR: i64 = 4 * PPQ;

fn song_of(bars: i64) -> Song {
    let mut s = empty_song(120.0);
    s.end = bars * BAR;
    s
}

/// A track `key` with one note clip holding one note at each of `bars` (0-based bar numbers).
fn play(song: &mut Song, key: &str, bars: &[i64]) {
    let mut t = new_track(DeviceKey(key.to_owned()));
    let notes: Vec<Note> = bars
        .iter()
        .map(|b| Note::new(b * BAR, PPQ, 60, 0.8))
        .collect();
    let last = bars.iter().max().copied().unwrap_or(0);
    t.lanes[0].clips.push(Clip {
        id: ClipId(song.next_clip_id),
        start: 0,
        length: (last + 1) * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Notes(notes.into()),
    });
    song.next_clip_id += 1;
    song.tracks.push(t);
}

fn marker(id: u32, bar: i64, name: &str) -> Marker {
    Marker {
        id,
        tick: bar * BAR,
        name: name.to_owned(),
        color: None,
    }
}

fn measure(loudness_db: f32, tracks: usize) -> Measure {
    Measure {
        loudness_db,
        brightness_hz: 1000.0,
        width: 0.4,
        tracks,
    }
}

#[test]
fn the_weights_add_up_to_one() {
    let sum = WEIGHTS.loudness + WEIGHTS.tracks + WEIGHTS.brightness + WEIGHTS.width;
    assert!((sum - 1.0).abs() < 1e-6);
}

#[test]
fn roles_come_from_the_words_of_the_name() {
    let cases = [
        ("Intro", Role::Intro),
        ("intro 2", Role::Intro),
        ("Verse 1", Role::Verse),
        ("Build-up", Role::Build),
        ("BUILD", Role::Build),
        ("Pre-Chorus", Role::Build),
        ("pre chorus", Role::Build),
        ("Riser", Role::Build),
        ("Drop", Role::Drop),
        ("Drop 2", Role::Drop),
        ("Chorus", Role::Drop),
        ("Break", Role::Break),
        ("Breakdown", Role::Break),
        ("Bridge", Role::Break),
        ("Outro", Role::Outro),
        ("End", Role::Outro),
        ("Bars 1-8", Role::Other),
        ("A", Role::Other),
        ("Premium", Role::Other),
        ("", Role::Other),
    ];
    for (name, role) in cases {
        assert_eq!(Role::guess(name), role, "{name:?}");
    }
    assert_eq!(Role::Drop.label(), "drop");
}

#[test]
fn a_song_without_markers_has_sections_of_eight_bars() {
    let s = song_of(20);
    let sec = sections(&s);
    let names: Vec<_> = sec.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Bars 1-8", "Bars 9-16", "Bars 17-20"]);
    assert_eq!((sec[0].start, sec[0].end), (0, 8 * BAR));
    assert_eq!((sec[2].start, sec[2].end), (16 * BAR, 20 * BAR));
    assert!(sec.iter().all(|x| x.role == Role::Other));
    assert!(sections(&song_of(0)).is_empty());
}

#[test]
fn markers_start_sections_that_run_to_the_next_one() {
    let mut s = song_of(32);
    s.markers = vec![
        marker(1, 4, "Build"),
        marker(2, 12, "Drop"),
        marker(3, 28, "Outro"),
    ];
    let sec = sections(&s);
    let got: Vec<_> = sec
        .iter()
        .map(|x| (x.name.as_str(), x.role, x.start / BAR, x.end / BAR))
        .collect();
    assert_eq!(
        got,
        [
            ("Start", Role::Other, 0, 4),
            ("Build", Role::Build, 4, 12),
            ("Drop", Role::Drop, 12, 28),
            ("Outro", Role::Outro, 28, 32),
        ]
    );
    // A marker at tick 0 needs no leading section; one past the End is ignored.
    s.markers = vec![marker(1, 0, "Intro"), marker(2, 99, "Late")];
    let sec = sections(&s);
    assert_eq!(sec.len(), 1);
    assert_eq!((sec[0].name.as_str(), sec[0].end), ("Intro", 32 * BAR));
}

#[test]
fn range_markers_are_the_sections_when_there_are_any() {
    let mut s = song_of(32);
    s.markers = vec![marker(1, 8, "Break")];
    s.ranges = vec![
        RangeMarker {
            id: 1,
            start: 0,
            end: 16 * BAR,
            name: "Verse".to_owned(),
            color: None,
        },
        RangeMarker {
            id: 2,
            start: 16 * BAR,
            end: 99 * BAR,
            name: "Chorus".to_owned(),
            color: None,
        },
    ];
    let sec = sections(&s);
    assert_eq!(sec.len(), 2);
    assert_eq!((sec[0].role, sec[1].role), (Role::Verse, Role::Drop));
    // A range running past the End is cut at it.
    assert_eq!(sec[1].end, 32 * BAR);
}

#[test]
fn tracks_count_in_the_sections_they_sound_in() {
    let mut s = song_of(16);
    play(&mut s, "a", &[0, 1, 2, 3, 8, 9]);
    play(&mut s, "b", &[8, 9, 10]);
    play(&mut s, "c", &[]);
    let sec = sections(&s);
    let (counts, total) = active_counts(&s, &sec);
    assert_eq!(counts, [1, 2]);
    assert_eq!(total, 2, "an empty track is not used");
    // A muted track plays nowhere.
    s.tracks[1].muted = true;
    let (counts, total) = active_counts(&s, &sec);
    assert_eq!((counts, total), (vec![1, 1], 1));
}

#[test]
fn energy_grows_with_every_measure_and_stays_in_range() {
    let base = measure(-25.0, 3);
    let e = |m: &Measure| energy(m, 6);
    let low = e(&base);
    assert!(low > 0.0 && low < 1.0);
    assert!(
        e(&Measure {
            loudness_db: -15.0,
            ..base
        }) > low
    );
    assert!(e(&Measure { tracks: 5, ..base }) > low);
    assert!(
        e(&Measure {
            brightness_hz: 3000.0,
            ..base
        }) > low
    );
    assert!(e(&Measure { width: 0.7, ..base }) > low);
    // The ends of the scales.
    let silent = Measure {
        loudness_db: -120.0,
        brightness_hz: 0.0,
        width: 0.0,
        tracks: 0,
    };
    assert_eq!(energy(&silent, 6), 0.0);
    let full = Measure {
        loudness_db: 0.0,
        brightness_hz: 20_000.0,
        width: 1.0,
        tracks: 9,
    };
    assert!((energy(&full, 6) - 1.0).abs() < 1e-6);
    // No tracks in the song, and a value that is not a number, read as 0 for that measure.
    assert_eq!(
        energy(
            &Measure {
                tracks: 3,
                ..silent
            },
            0
        ),
        0.0
    );
    let nan = Measure {
        loudness_db: f32::NAN,
        ..base
    };
    assert!(energy(&nan, 6).is_finite());
}

/// A map of sections named `names`, each `bars` long, with the given loudness and track share.
fn map_of(names: &[&str], bars: i64, levels: &[f32]) -> EnergyMap {
    let mut s = song_of(names.len() as i64 * bars);
    s.markers = names
        .iter()
        .enumerate()
        .map(|(i, n)| marker(i as u32 + 1, i as i64 * bars, n))
        .collect();
    let sec = sections(&s);
    let measures: Vec<Measure> = levels.iter().map(|l| measure(*l, 3)).collect();
    EnergyMap::new(&s, sec, &measures, 6)
}

#[test]
fn a_build_stronger_than_its_drop_is_flagged() {
    let m = map_of(&["Intro", "Build", "Drop"], 8, &[-35.0, -12.0, -22.0]);
    assert!(
        m.flags
            .contains(&Flag::BuildLouderThanDrop { build: 1, drop: 2 })
    );
    let text = m.flags[0].message(&m);
    assert!(text.contains("Build") && text.contains("Drop"), "{text}");
    let ok = map_of(&["Intro", "Build", "Drop"], 8, &[-35.0, -22.0, -12.0]);
    assert!(ok.flags.is_empty(), "{:?}", ok.flags);
}

#[test]
fn a_drop_that_is_not_the_top_is_flagged() {
    let m = map_of(&["Drop", "Verse"], 8, &[-24.0, -12.0]);
    assert_eq!(m.flags, [Flag::DropNotTop { drop: 0, top: 1 }]);
    // Only the strongest drop is named, and a drop at the top is fine.
    let two = map_of(&["Drop", "Break", "Drop"], 8, &[-14.0, -30.0, -12.0]);
    assert!(two.flags.is_empty(), "{:?}", two.flags);
    // No drop: nothing to flag.
    let none = map_of(&["Verse", "Break"], 8, &[-20.0, -30.0]);
    assert!(none.flags.is_empty());
}

#[test]
fn near_equal_neighbours_over_sixteen_bars_are_flagged() {
    let m = map_of(&["Verse", "Break"], 8, &[-20.0, -19.8]);
    assert!(m.flags.contains(&Flag::FlatRun {
        first: 0,
        second: 1
    }));
    // Shorter than sixteen bars together: no flag.
    let short = map_of(&["Verse", "Break"], 4, &[-20.0, -19.8]);
    assert!(short.flags.is_empty());
    // Far apart: no flag.
    let apart = map_of(&["Verse", "Break"], 8, &[-20.0, -35.0]);
    assert!(apart.flags.is_empty());
    // Silent neighbours are not a flat run.
    let mut song = song_of(16);
    song.markers = vec![marker(1, 0, "Verse"), marker(2, 8, "Break")];
    let nothing = Measure {
        loudness_db: -120.0,
        brightness_hz: 0.0,
        width: 0.0,
        tracks: 0,
    };
    let silent = EnergyMap::new(&song, sections(&song), &[nothing, nothing], 6);
    assert!(silent.flags.is_empty());
}

#[test]
fn a_map_finds_its_sections_and_flags() {
    let m = map_of(&["Verse", "Break", "Drop"], 8, &[-20.0, -19.8, -10.0]);
    assert_eq!(m.section_at(0), Some(0));
    assert_eq!(m.section_at(8 * BAR), Some(1));
    assert_eq!(m.section_at(100 * BAR), None);
    assert_eq!(m.flags_of(1).count(), 1);
    assert_eq!(m.flags_of(2).count(), 0);
    // Extra measures or sections are ignored.
    let s = song_of(16);
    let sec = sections(&s);
    let one = EnergyMap::new(&s, sec, &[measure(-20.0, 1)], 1);
    assert_eq!(one.sections.len(), 1);
}
