//! Comparing two songs: tracks, devices, clips, tempo, meter, length and mixer strips.

mod common;

use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, InsertEffect, RackDoc, SongCommand, TempoEvent, compare_docs,
};
use common::song::*;
use common::*;

const BAR: i64 = 4 * PPQ;

/// Two synths `a` and `b` with a track each.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut d = Document::new(legacy_project_doc());
    d.apply(add(0, SYNTH, Some("a")), &r).unwrap();
    d.apply(add(1, SYNTH, Some("b")), &r).unwrap();
    d.apply(add_track("a"), &r).unwrap();
    d.apply(add_track("b"), &r).unwrap();
    (d, r)
}

fn diff(before: &RackDoc, after: &RackDoc, r: &Registry) -> Vec<String> {
    compare_docs(before, after, r).lines()
}

fn has(lines: &[String], text: &str) -> bool {
    lines.iter().any(|l| l.trim() == text)
}

#[test]
fn the_same_song_has_no_differences() {
    let (d, r) = project();
    let diff = compare_docs(d.doc(), d.doc(), &r);
    assert!(diff.is_empty());
    assert_eq!(diff.lines().len(), 1);
    assert!(diff.lines()[0].starts_with("No differences"));
}

#[test]
fn a_track_that_came_and_a_track_that_went_are_listed() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(add(2, SYNTH, Some("c")), &r).unwrap();
    d.apply(add_track("c"), &r).unwrap();
    d.apply(
        add_clip("c", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        &r,
    )
    .unwrap();
    d.apply(Command::RemoveDevice { key: key("b") }, &r)
        .unwrap();
    let lines = diff(&before, d.doc(), &r);
    let c_name = d.doc().device(&key("c")).unwrap().name.clone();
    let b_name = before.device(&key("b")).unwrap().name.clone();
    assert!(
        has(&lines, &format!("+ Added {c_name} (1 clip)")),
        "{lines:?}"
    );
    assert!(
        has(&lines, &format!("- Removed {b_name} (0 clips)")),
        "{lines:?}"
    );
    assert!(!has(&lines, "~ Tracks: 2 -> 2"));
    // The strips of the new and the removed track show under the mixer, not as devices.
    assert!(lines.iter().any(|l| l.contains("Added strip")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("Removed strip")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("channel")), "{lines:?}");
}

#[test]
fn a_renamed_track_is_listed_once() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    let old = before.device(&key("a")).unwrap().name.clone();
    d.apply(
        Command::Rename {
            key: key("a"),
            name: "Lead".into(),
        },
        &r,
    )
    .unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(
        has(&lines, &format!("~ Renamed {old} -> Lead")),
        "{lines:?}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("Renamed")).count(),
        1,
        "{lines:?}"
    );
}

#[test]
fn clip_and_note_counts_per_track() {
    let (mut d, r) = project();
    d.apply(
        add_clip("a", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        &r,
    )
    .unwrap();
    let before = d.doc().clone();
    d.apply(
        add_clip(
            "a",
            0,
            notes_clip(
                BAR,
                BAR,
                vec![note(0, PPQ, 62, 0.8), note(PPQ, PPQ, 64, 0.8)],
            ),
        ),
        &r,
    )
    .unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(has(&lines, "Clips: 1 -> 2"), "{lines:?}");
    assert!(has(&lines, "Notes: 1 -> 3"), "{lines:?}");
    // Nothing about the other track.
    let name_b = d.doc().device(&key("b")).unwrap().name.clone();
    assert!(!lines.iter().any(|l| l.contains(&name_b)), "{lines:?}");
}

#[test]
fn mute_and_solo_of_a_track() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("a"),
            muted: true,
            solo: false,
            armed: false,
        }),
        &r,
    )
    .unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(has(&lines, "Mute: off -> on"), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("Solo")), "{lines:?}");
}

#[test]
fn tempo_meter_and_length_changes() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 120.0),
            TempoEvent::step(8 * BAR, 90.0),
        ])),
        &r,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetMeter {
            beats_per_bar: 3,
            beat_unit: 4,
        }),
        &r,
    )
    .unwrap();
    d.apply(sc(SongCommand::SetSongEnd(24 * BAR)), &r).unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(
        has(&lines, "~ Tempo: 120 BPM -> 120 BPM, 1 change"),
        "{lines:?}"
    );
    assert!(has(&lines, "~ Meter: 4/4 -> 3/4"), "{lines:?}");
    let length = lines.iter().find(|l| l.contains("Length:")).unwrap();
    assert!(length.contains("bars"), "{length}");
    assert!(length.contains("->"), "{length}");
}

#[test]
fn a_plain_tempo_change_reads_as_two_values() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![TempoEvent::step(0, 127.5)])),
        &r,
    )
    .unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(has(&lines, "~ Tempo: 120 BPM -> 127.5 BPM"), "{lines:?}");
}

#[test]
fn song_length_is_in_bars_and_minutes() {
    let (mut d, r) = project();
    d.apply(sc(SongCommand::SetSongEnd(32 * BAR)), &r).unwrap();
    let before = d.doc().clone();
    d.apply(sc(SongCommand::SetSongEnd(48 * BAR)), &r).unwrap();
    let lines = diff(&before, d.doc(), &r);
    // 120 BPM in 4/4: a bar is two seconds.
    assert!(
        has(&lines, "~ Length: 32 bars (1:04) -> 48 bars (1:36)"),
        "{lines:?}"
    );
}

#[test]
fn an_insert_effect_is_listed_on_its_track() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(
        Command::Console(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: DELAY.into(),
            }),
        }),
        &r,
    )
    .unwrap();
    let lines = diff(&before, d.doc(), &r);
    let track = d.doc().device(&key("a")).unwrap().name.clone();
    let effect = d
        .doc()
        .devices
        .iter()
        .find(|x| x.type_id == DELAY)
        .unwrap()
        .name
        .clone();
    assert!(
        has(&lines, &format!("+ Added {effect} on {track}")),
        "{lines:?}"
    );
    assert!(has(&lines, &format!("+ Effect {effect}")), "{lines:?}");
}

#[test]
fn parameter_changes_are_counted_per_device() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    let name = before.device(&key("a")).unwrap().name.clone();
    let param = {
        let desc = r.get(SYNTH).unwrap().descriptor;
        let p = &desc.params[1];
        (p.key.to_owned(), p.default, p.max)
    };
    d.apply(set_param("a", &param.0, param.2), &r).unwrap();
    let lines = diff(&before, d.doc(), &r);
    assert!(
        has(&lines, &format!("~ {name}: 1 parameter changed")),
        "{lines:?}"
    );
}

#[test]
fn a_stored_default_is_not_a_change() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    let desc = r.get(SYNTH).unwrap().descriptor;
    let p = &desc.params[1];
    // Move away and back: the document stores the default explicitly.
    d.apply(set_param("a", p.key, p.max), &r).unwrap();
    d.apply(set_param("a", p.key, p.default), &r).unwrap();
    assert!(diff(&before, d.doc(), &r)[0].starts_with("No differences"));
}

#[test]
fn a_moved_channel_fader_is_a_mixer_line() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(set_param("channel", "fader", 0.5), &r).unwrap();
    d.apply(set_param("channel", "pan", -0.3), &r).unwrap();
    let lines = diff(&before, d.doc(), &r);
    let strip = d.doc().device(&key("channel")).unwrap().name.clone();
    assert!(
        has(
            &lines,
            &format!("~ {strip}: fader 75% -> 50%, pan centre -> L30")
        ),
        "{lines:?}"
    );
    // The strip is not repeated as a device.
    assert!(!lines.iter().any(|l| l.contains("parameter")), "{lines:?}");
}

#[test]
fn sections_are_titled_and_only_present_when_they_differ() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(sc(SongCommand::SetSongEnd(8 * BAR)), &r).unwrap();
    let diff = compare_docs(&before, d.doc(), &r);
    let titles: Vec<_> = diff.sections.iter().map(|s| s.title).collect();
    assert_eq!(titles, ["Song"]);
    assert_eq!(diff.lines()[0], "SONG");
    assert!(diff.lines()[1].starts_with("  ~ Length"));
}

#[test]
fn comparing_is_symmetric_in_what_it_names() {
    let (mut d, r) = project();
    let before = d.doc().clone();
    d.apply(add(2, SYNTH, Some("c")), &r).unwrap();
    d.apply(add_track("c"), &r).unwrap();
    let forward = diff(&before, d.doc(), &r);
    let backward = diff(d.doc(), &before, &r);
    assert!(
        forward
            .iter()
            .any(|l| l.trim_start().starts_with("+ Added"))
    );
    assert!(
        backward
            .iter()
            .any(|l| l.trim_start().starts_with("- Removed"))
    );
}
