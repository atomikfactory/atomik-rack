//! The built-in export presets and the user's presets file.

use super::*;
use atomik_ui::ExportDepth;
use std::sync::atomic::{AtomicU64, Ordering};

/// A temporary folder that goes away with the test.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-export-presets-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        presets_path(&self.0.join("settings.json"))
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn named(name: &str) -> PresetChoice {
    builtin().into_iter().find(|p| p.name == name).unwrap()
}

#[test]
fn there_are_five_built_in_presets_with_generic_names() {
    let names: Vec<String> = builtin().into_iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        ["Streaming", "Podcast", "Video", "Stems for mixing", "Club"]
    );
    assert!(builtin().iter().all(|p| !p.user));
    assert!(is_builtin("streaming") && is_builtin("CLUB") && !is_builtin("My master"));
}

#[test]
fn streaming_is_a_24_bit_wav_and_an_mp3_at_minus_14() {
    let p = named(STREAMING);
    assert!(!p.stems);
    assert_eq!(
        p.values,
        ExportValues {
            file: ExportFile::Wav,
            depth: ExportDepth::Pcm24,
            rate: None,
            loudness: ExportLoudness::Target(-14),
            also_mp3: true,
            mono: false,
            ..ExportValues::default()
        }
    );
    assert_eq!(atomik_ui::ALSO_MP3_KBPS, 320);
    assert_eq!(ExportLoudness::CEILING_DBTP, -1.0);
    assert!(p.needs_ffmpeg(), "its MP3 is encoded by FFmpeg");
}

#[test]
fn podcast_is_a_mono_mp3_at_128_and_minus_16() {
    let p = named(PODCAST);
    assert!(!p.stems && p.needs_ffmpeg());
    assert_eq!(p.values.file, ExportFile::Mp3);
    assert_eq!(p.values.mp3, ExportQuality::Cbr(128));
    assert!(p.values.mono);
    assert_eq!(p.values.loudness, ExportLoudness::Target(-16));
    assert!(!p.values.also_mp3);
}

#[test]
fn video_is_aac_256_at_48_khz_and_minus_14() {
    let p = named(VIDEO);
    assert!(!p.stems && p.needs_ffmpeg());
    assert_eq!(p.values.file, ExportFile::Aac);
    assert_eq!(p.values.aac, 256);
    assert_eq!(p.values.rate, Some(48_000));
    assert_eq!(p.values.loudness, ExportLoudness::Target(-14));
}

#[test]
fn stems_for_mixing_are_24_bit_wav_with_no_loudness_change_and_no_dither() {
    let p = named(STEMS);
    assert!(p.stems && !p.needs_ffmpeg());
    assert_eq!(p.values.file, ExportFile::Wav);
    assert_eq!(p.values.depth, ExportDepth::Pcm24);
    assert_eq!(p.values.dither, ExportDither::Off);
    assert_eq!(p.values.loudness, ExportLoudness::Off);
    // From the start to the end of the song: not the locators, not the selection.
    assert!(!p.values.locators && !p.values.selection && !p.values.dry);
}

#[test]
fn club_is_a_24_bit_wav_with_minus_9_as_a_guide_only() {
    let p = named(CLUB);
    assert!(!p.stems && !p.needs_ffmpeg());
    assert_eq!(p.values.file, ExportFile::Wav);
    assert_eq!(p.values.depth, ExportDepth::Pcm24);
    assert_eq!(p.values.loudness, ExportLoudness::Guide(-9));
    assert!(!p.values.mono && !p.values.also_mp3);
}

fn mine() -> ExportValues {
    ExportValues {
        file: ExportFile::Flac,
        depth: ExportDepth::Pcm16,
        dither: ExportDither::Shaped,
        rate: Some(44_100),
        tail: false,
        loudness: ExportLoudness::Guide(-9),
        mono: true,
        ..ExportValues::default()
    }
}

#[test]
fn a_user_preset_is_saved_loaded_and_deleted() {
    let t = Tmp::new();
    let mut store = PresetStore::default();
    assert_eq!(
        store.add("  My master ", false, mine()).unwrap(),
        "My master"
    );
    store
        .add("Stem set", true, ExportValues::default())
        .unwrap();
    store.save(&t.file()).unwrap();
    assert!(t.file().ends_with("export_presets.json"));

    let loaded = PresetStore::load(&t.file());
    assert_eq!(loaded, store);
    let all = loaded.all();
    assert_eq!(all.len(), 7);
    assert_eq!(all[5].name, "My master");
    assert!(all[5].user && !all[5].stems);
    assert_eq!(all[5].values, mine());
    assert!(all[6].stems);
    assert_eq!(loaded.get(5).unwrap().name, "My master");
    assert!(loaded.get(7).is_none());

    let mut loaded = loaded;
    assert!(loaded.delete("My master"));
    assert!(!loaded.delete("My master"));
    assert!(!loaded.delete("Streaming"), "a built-in preset stays");
    loaded.save(&t.file()).unwrap();
    assert_eq!(PresetStore::load(&t.file()).user().len(), 1);
}

#[test]
fn the_file_has_a_version_and_is_canonical() {
    let mut store = PresetStore::default();
    store.add("My master", false, mine()).unwrap();
    let json = store.to_json();
    assert!(json.contains("\"version\": 1"), "{json}");
    assert_eq!(PresetStore::from_json(&json).to_json(), json);
    assert!(json.ends_with('\n'));
}

#[test]
fn saving_over_a_name_replaces_it_where_it_stands() {
    let mut store = PresetStore::default();
    store.add("One", false, ExportValues::default()).unwrap();
    store.add("Two", false, ExportValues::default()).unwrap();
    store.add("two", false, mine()).unwrap();
    let names: Vec<_> = store.user().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["One", "two"]);
    assert_eq!(store.user()[1].values, mine());
}

#[test]
fn names_are_checked() {
    let mut store = PresetStore::default();
    let v = ExportValues::default();
    assert!(store.add("   ", false, v).unwrap_err().contains("name"));
    assert!(
        store
            .add("Streaming", false, v)
            .unwrap_err()
            .contains("built-in")
    );
    assert!(
        store
            .add("club", false, v)
            .unwrap_err()
            .contains("built-in")
    );
    assert!(
        store
            .add(&"x".repeat(41), false, v)
            .unwrap_err()
            .contains("too long")
    );
    assert!(
        store
            .add("a\u{7}b", false, v)
            .unwrap_err()
            .contains("control")
    );
    assert!(store.add(&"x".repeat(40), false, v).is_ok());
}

#[test]
fn there_is_a_most_to_the_presets_of_the_user() {
    let mut store = PresetStore::default();
    for i in 0..MAX_USER_PRESETS {
        store
            .add(&format!("Preset {i}"), false, ExportValues::default())
            .unwrap();
    }
    let err = store
        .add("One more", false, ExportValues::default())
        .unwrap_err();
    assert!(err.contains("delete one"), "{err}");
    // A name that is there is still replaced.
    assert!(store.add("preset 3", false, mine()).is_ok());
    // The menu has room for them all: the built-in presets and 16 more.
    assert!(builtin().len() + MAX_USER_PRESETS <= atomik_ui::keymap::EXPORT_FOR_ARGS.len());
}

#[test]
fn a_stems_preset_keeps_no_mix_down_loudness_or_extra_file() {
    let mut store = PresetStore::default();
    let v = ExportValues {
        mono: true,
        loudness: ExportLoudness::Target(-14),
        also_mp3: true,
        selection: true,
        ..ExportValues::default()
    };
    store.add("Stem set", true, v).unwrap();
    let got = store.user()[0].values;
    assert!(!got.mono && !got.also_mp3 && !got.selection);
    assert_eq!(got.loudness, ExportLoudness::Off);
    // An extra MP3 next to a compressed file means nothing either.
    store
        .add(
            "Small",
            false,
            ExportValues {
                file: ExportFile::Aac,
                also_mp3: true,
                ..ExportValues::default()
            },
        )
        .unwrap();
    assert!(!store.user()[1].values.also_mp3);
}

#[test]
fn a_missing_or_corrupt_file_is_no_presets() {
    let t = Tmp::new();
    assert_eq!(PresetStore::load(&t.file()), PresetStore::default());
    for bad in [
        "",
        "not json",
        "[1, 2]",
        "{\"version\": 1, \"presets\": 7}",
        "{\"presets\": []}",
        "{\"version\": 2, \"presets\": [{\"name\": \"Future\"}]}",
        "\u{0}\u{1}",
    ] {
        std::fs::write(t.file(), bad).unwrap();
        assert!(PresetStore::load(&t.file()).user().is_empty(), "{bad:?}");
    }
    assert_eq!(
        PresetStore::load(&t.0).user().len(),
        0,
        "a folder is no file"
    );
}

#[test]
fn entries_that_cannot_be_used_are_skipped_and_unknown_words_are_defaults() {
    let text = r#"{ "version": 1, "presets": [
        { "name": "Good", "values": { "file": "opus", "bit_depth": "banana", "loudness": "target-16" } },
        { "name": "good", "values": {} },
        { "name": "" },
        { "name": "Streaming" },
        { "name": "Odd", "stems": true, "values": { "file": "xyz", "loudness": "loud", "aac_kbps": 7 } }
    ] }"#;
    let store = PresetStore::from_json(text);
    let names: Vec<_> = store.user().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Good", "Odd"]);
    let good = store.user()[0].values;
    assert_eq!(good.file, ExportFile::Opus);
    assert_eq!(good.depth, ExportValues::default().depth, "an unknown word");
    assert_eq!(good.loudness, ExportLoudness::Target(-16));
    let odd = store.user()[1].values;
    assert!(store.user()[1].stems);
    assert_eq!(odd.file, ExportFile::Wav);
    assert_eq!(odd.loudness, ExportLoudness::Off);
    assert_eq!(odd.aac, ExportValues::default().aac);
}

#[test]
fn the_values_of_a_preset_survive_the_words_of_the_file() {
    for p in builtin() {
        let mut store = PresetStore::default();
        store
            .add(&format!("My {}", p.name), p.stems, p.values)
            .unwrap();
        let back = PresetStore::from_json(&store.to_json());
        let got = &back.user()[0];
        assert_eq!(got.values, store.user()[0].values, "{}", p.name);
        assert_eq!(got.stems, p.stems);
    }
}
