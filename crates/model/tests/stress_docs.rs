//! The generated documents of `atomik_model::stress`: exact counts, validity and the console
//! switches of the console document.

mod common;

use atomik_model::stress::{
    STRESS_CABLES, STRESS_DEVICES, console_doc, large_song_doc, stress_doc,
};
use atomik_model::{ClipContent, load_project, save_project};
use common::registry;

#[test]
fn a_large_song_has_the_requested_tracks_clips_and_notes() {
    let reg = registry();
    let doc = large_song_doc(&reg, 10, 25, 103);
    assert_eq!(doc.song.tracks.len(), 10);
    let mut clips = 0;
    let mut notes = 0;
    for t in &doc.song.tracks {
        for lane in &t.lanes {
            for c in &lane.clips {
                clips += 1;
                if let ClipContent::Notes(n) = &c.content {
                    notes += n.len();
                }
            }
        }
    }
    assert_eq!((clips, notes), (25, 103));
    doc.validate().expect("generated song is valid");
}

#[test]
fn the_clips_of_a_track_never_overlap() {
    let reg = registry();
    let doc = large_song_doc(&reg, 3, 30, 600);
    for t in &doc.song.tracks {
        let mut end = 0;
        for c in &t.lanes[0].clips {
            assert!(c.start >= end, "clip starts at {} before {end}", c.start);
            end = c.start + c.length;
        }
    }
}

#[test]
fn an_empty_large_song_is_the_bare_document() {
    let reg = registry();
    assert!(large_song_doc(&reg, 0, 5, 5).song.tracks.is_empty());
    let doc = large_song_doc(&reg, 2, 0, 0);
    assert_eq!(doc.song.tracks.len(), 2);
}

#[test]
fn the_console_document_has_gate_and_compressor_on_for_every_strip() {
    let mut reg = atomik_device_sdk::Registry::new();
    atomik_devices::register_all(&mut reg).unwrap();
    let doc = console_doc(&reg, 64);
    assert_eq!(doc.console.channels.len(), 64);
    for ch in &doc.console.channels {
        let dev = doc
            .devices
            .iter()
            .find(|d| d.key == ch.channel)
            .expect("channel device");
        for p in ["gate.on", "comp.on"] {
            assert!(
                dev.params.iter().any(|(k, v)| k == p && *v > 0.5),
                "{p} is on for {}",
                ch.channel.0
            );
        }
    }
    let text = save_project(&doc);
    assert!(load_project(&text, &reg).is_ok());
}

#[test]
fn the_stress_rack_keeps_its_counts() {
    let doc = stress_doc(&registry(), STRESS_DEVICES, STRESS_CABLES);
    assert_eq!(
        (doc.devices.len(), doc.cables.len()),
        (STRESS_DEVICES, STRESS_CABLES)
    );
}
