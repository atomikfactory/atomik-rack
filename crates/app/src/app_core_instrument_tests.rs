//! Instrument files (SFZ, SoundFont) dropped on a sampler: one undo step that replaces the zones,
//! scaling to the session rate, warnings, refusals and a save and reopen round trip. The
//! fixtures are written into a temporary folder (the SoundFont is a small committed file).

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_device_sdk::LoopMode;
use atomik_model::{ZoneDoc, empty_doc};
use std::sync::atomic::{AtomicU64, Ordering};

const SAMPLER: &str = "atomik.sampler";
const SMALL_SF2: &[u8] = include_bytes!("../../audio-file/tests/fixtures/small.sf2");

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-instrument-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn wav(dir: &Tmp, name: &str, rate: u32, frames: usize, seed: f32) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * seed)
        .collect();
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(rate, vec![data]).unwrap()).unwrap();
    path
}

fn sampler_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SAMPLER.into(),
        key: Some(key("smp")),
    })
    .unwrap();
    c.sink_mut().take();
    c
}

fn zones(c: &AppCore<RecordingSink>) -> Vec<ZoneDoc> {
    c.doc().device(&key("smp")).unwrap().zones.clone()
}

fn drop_files(c: &mut AppCore<RecordingSink>, paths: Vec<PathBuf>) {
    c.handle_action(UiAction::ImportSamples {
        paths,
        device: key("smp"),
        at_key: None,
    });
    c.wait_background();
}

const SFZ: &str = "<control> default_path=samples/\n\
    <global> ampeg_release=0.3\n\
    <group> lovel=1 hivel=63 seq_length=2\n\
    <region> sample=low.wav lokey=c2 hikey=b3 pitch_keycenter=c3 seq_position=1\n\
    <region> sample=low.wav lokey=c2 hikey=b3 pitch_keycenter=c3 seq_position=2 tune=10\n\
    <group> lovel=64 hivel=127 loop_mode=loop_continuous loop_start=1000 loop_end=3999\n\
    <region> sample=high.wav lokey=c4 hikey=c6 pitch_keycenter=c5 lorand=0.5\n";

fn sfz_fixture(t: &Tmp) -> PathBuf {
    std::fs::create_dir_all(t.path("samples")).unwrap();
    // A 24 kHz sample: the session is 48 kHz, so its length and positions double.
    wav(t, "samples/low.wav", 24_000, 12_000, 1.0);
    wav(t, "samples/high.wav", 48_000, 8_000, 0.5);
    t.write("piano.sfz", SFZ)
}

#[test]
fn an_sfz_drop_replaces_the_zones_in_one_undo_step() {
    let t = Tmp::new();
    let mut c = sampler_core();
    let sfz = sfz_fixture(&t);
    drop_files(&mut c, vec![sfz]);
    let z = zones(&c);
    assert_eq!(z.len(), 3, "{:?}", c.last_rejection());
    assert_eq!(c.doc().assets.len(), 2);
    let msg = c.last_rejection().unwrap_or_default();
    assert!(msg.contains("3 zones"), "{msg}");
    assert!(
        msg.contains("lorand"),
        "the unknown opcode is named once: {msg}"
    );
    // Round-robin and groups.
    assert_eq!((z[0].ext.rr_len, z[0].ext.rr_pos), (2, 0));
    assert_eq!((z[1].ext.rr_len, z[1].ext.rr_pos), (2, 1));
    assert_eq!((z[0].ext.group, z[2].ext.group), (1, 2));
    assert_eq!((z[0].key_lo, z[0].key_hi, z[0].root_key), (36, 59, 48));
    assert_eq!((z[2].key_lo, z[2].root_key), (60, 72));
    assert_eq!((z[0].vel_lo, z[0].vel_hi, z[2].vel_lo), (1, 63, 64));
    assert!((z[1].tune_cents - 10.0).abs() < 1e-3);
    let env = z[0].ext.env.expect("the global release");
    assert!((env.release_ms - 300.0).abs() < 1e-3);
    // The loop: frames of the 48 kHz sample, the inclusive end made exclusive.
    assert_eq!(z[2].loop_mode, LoopMode::Forward);
    assert_eq!((z[2].loop_start, z[2].loop_end), (1000, 4000));
    // The 24 kHz sample doubled in length; its zones span all of it.
    let low = &c.doc().assets[&z[0].asset];
    assert_eq!(low.frames, 24_000);
    assert_eq!(z[0].end, 24_000);
    // One map reached the engine, and one undo takes everything back.
    let calls = c.sink_mut().take();
    let maps: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Samples { map, .. } => map.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(maps.len(), 1, "{calls:?}");
    assert_eq!(maps[0].zones().len(), 3);
    c.undo();
    assert!(zones(&c).is_empty());
    c.redo();
    assert_eq!(zones(&c).len(), 3);
}

#[test]
fn an_sfz_drop_replaces_what_the_sampler_held() {
    let t = Tmp::new();
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "kick.wav", 48_000, 4_800, 1.0)]);
    assert_eq!(zones(&c).len(), 1);
    drop_files(&mut c, vec![sfz_fixture(&t)]);
    assert_eq!(zones(&c).len(), 3, "the old zone is replaced");
}

#[test]
fn a_soundfont_drop_stores_its_samples_and_maps_its_zones() {
    let t = Tmp::new();
    let mut c = sampler_core();
    let sf2 = t.path("small.sf2");
    std::fs::write(&sf2, SMALL_SF2).unwrap();
    drop_files(&mut c, vec![sf2]);
    let z = zones(&c);
    assert_eq!(z.len(), 2, "{:?}", c.last_rejection());
    assert_eq!((z[0].key_lo, z[0].key_hi, z[0].root_key), (0, 59, 60));
    assert_eq!((z[1].key_lo, z[1].root_key), (60, 64));
    assert_eq!(z[0].loop_mode, LoopMode::Forward);
    assert!((z[0].gain_db + 6.0).abs() < 1e-3);
    // 100 frames at 22.05 kHz become about 218 frames at 48 kHz; the loop scales with them.
    let frames = c.doc().assets[&z[0].asset].frames;
    assert!((217..=219).contains(&frames), "{frames}");
    assert!(
        (z[0].loop_start as i64 - 44).abs() <= 1,
        "{}",
        z[0].loop_start
    );
    assert!((z[0].loop_end as i64 - 174).abs() <= 1, "{}", z[0].loop_end);
    // The stored samples play: the map reaches the engine.
    let maps = c
        .sink_mut()
        .take()
        .into_iter()
        .filter(|c| matches!(c, SinkCall::Samples { map: Some(_), .. }))
        .count();
    assert_eq!(maps, 1);
}

#[test]
fn a_file_that_is_not_an_instrument_or_has_no_regions_is_reported_and_changes_nothing() {
    let t = Tmp::new();
    let mut c = sampler_core();
    drop_files(&mut c, vec![t.write("empty.sfz", "<region> key=60\n")]);
    assert!(zones(&c).is_empty());
    let msg = c.last_rejection().unwrap_or_default();
    assert!(
        msg.contains("empty.sfz") && msg.contains("no playable"),
        "{msg}"
    );
    let bad = t.path("bad.sf2");
    std::fs::write(&bad, b"not a soundfont").unwrap();
    drop_files(&mut c, vec![bad]);
    assert!(zones(&c).is_empty());
    assert!(c.last_rejection().unwrap_or_default().contains("bad.sf2"));
}

#[test]
fn missing_sample_files_leave_their_regions_out_with_a_message() {
    let t = Tmp::new();
    let mut c = sampler_core();
    wav(&t, "a.wav", 48_000, 4_800, 1.0);
    let sfz = t.write(
        "m.sfz",
        "<region> sample=a.wav key=60\n<region> sample=gone.wav key=61\n",
    );
    drop_files(&mut c, vec![sfz]);
    assert_eq!(zones(&c).len(), 1);
    let msg = c.last_rejection().unwrap_or_default();
    assert!(
        msg.contains("gone.wav") && msg.contains("could not be read"),
        "{msg}"
    );
}

#[test]
fn audio_files_dropped_with_an_instrument_are_ignored_and_other_devices_refuse() {
    let t = Tmp::new();
    let mut c = sampler_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.granular".into(),
        key: Some(key("grain")),
    })
    .unwrap();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![sfz_fixture(&t)],
        device: key("grain"),
        at_key: None,
    });
    c.wait_background();
    assert!(c.doc().device(&key("grain")).unwrap().zones.is_empty());
    assert!(
        c.last_rejection()
            .unwrap_or_default()
            .contains("load into a sampler")
    );
    let extra = wav(&t, "extra.wav", 48_000, 480, 1.0);
    drop_files(&mut c, vec![sfz_fixture(&t), extra]);
    assert_eq!(zones(&c).len(), 3);
    assert_eq!(c.doc().assets.len(), 2, "the extra file was not imported");
}

#[test]
fn a_saved_instrument_reopens_with_its_zones_and_samples() {
    let t = Tmp::new();
    let mut c = sampler_core();
    drop_files(&mut c, vec![sfz_fixture(&t)]);
    let before = zones(&c);
    let project = t.path("song.atomik");
    c.save_as(&project).unwrap();
    let mut d = AppCore::new(RecordingSink::new());
    d.open(&t.path("song").join("song.atomik")).unwrap();
    d.wait_background();
    assert_eq!(zones(&d), before);
    assert!(d.last_rejection().is_none(), "{:?}", d.last_rejection());
}
