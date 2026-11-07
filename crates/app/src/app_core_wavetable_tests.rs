//! The table editor in the app core: an edited table is stored as the device's one sample in one
//! undo step, reads back as its key frames, survives save and reload, reaches the engine as a
//! sample map the synth turns into the drawn table, and a dropped sample is cut by its cycle.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, Bundle, write_wav_f32};
use atomik_dsp::wavetable::edit::{self, BINS, Bin, FRAME_LEN, Frame};
use atomik_dsp::wavetable::{FRAMES, read_pyramid};
use atomik_model::empty_doc;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const WAVETABLE: &str = "atomik.wavetable-synth";

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-wavetable-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: WAVETABLE.into(),
        key: Some(key("wt")),
    })
    .unwrap();
    c.sink_mut().take();
    c
}

fn bins(f: impl Fn(usize) -> f32) -> Vec<Bin> {
    (1..=BINS)
        .map(|h| Bin {
            level: f(h),
            phase: 0.0,
        })
        .collect()
}

/// Four frames that differ: the harmonics 1 to 4 each stronger in turn.
fn keys() -> Vec<Frame> {
    (1..=4)
        .map(|n| {
            edit::rebuild(&bins(|h| {
                if h <= 8 {
                    (1.0 + (h == n) as u8 as f32 * 2.0) / h as f32
                } else {
                    0.0
                }
            }))
        })
        .collect()
}

fn zones(c: &AppCore<RecordingSink>) -> Vec<atomik_model::ZoneDoc> {
    c.doc().device(&key("wt")).unwrap().zones.clone()
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn a_commit_stores_the_table_as_the_one_sample_in_one_undo_step() {
    let mut c = core();
    let depth = c.document().undo_depth();
    c.commit_wavetable(&key("wt"), &keys(), true).unwrap();
    let z = zones(&c);
    assert_eq!(z.len(), 1);
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let meta = &c.doc().assets[&z[0].asset];
    assert_eq!(meta.original_name, "Table frames 4 aligned");
    assert_eq!(
        (meta.channels, meta.frames),
        (1, (FRAMES * FRAME_LEN) as u64)
    );
    assert_eq!((z[0].start, z[0].end), (0, (FRAMES * FRAME_LEN) as u64));
    // A second edit replaces the sample (the device takes one), again one step.
    let mut more = keys();
    more.push(more[0].clone());
    c.commit_wavetable(&key("wt"), &more, false).unwrap();
    assert_eq!(zones(&c).len(), 1);
    assert_eq!(c.document().undo_depth(), depth + 2);
    assert_ne!(zones(&c)[0].asset, z[0].asset);
    // Undo and redo walk back through the tables.
    assert!(c.undo());
    assert_eq!(zones(&c)[0].asset, z[0].asset);
    assert!(c.undo());
    assert!(zones(&c).is_empty());
    assert!(c.redo() && c.redo());
    assert_ne!(zones(&c)[0].asset, z[0].asset);
}

#[test]
fn the_table_reads_back_as_its_key_frames_and_follows_undo() {
    let mut c = core();
    let keys = keys();
    c.commit_wavetable(&key("wt"), &keys, true).unwrap();
    let data = c.wavetable_data(&key("wt")).unwrap();
    assert!(data.stored && data.phase_safe && !data.imported);
    assert_eq!(data.keys.len(), 4);
    assert!(data.cycle.is_none());
    for (got, put) in data.keys.iter().zip(&keys) {
        let want = edit::align_fundamental(put);
        for n in (0..FRAME_LEN).step_by(37) {
            assert!(close(got[n], want[n], 1.0e-4), "sample {n}");
        }
    }
    // Plain frames come back as they were put.
    c.commit_wavetable(&key("wt"), &keys, false).unwrap();
    let data = c.wavetable_data(&key("wt")).unwrap();
    assert!(!data.phase_safe);
    assert_eq!(data.keys, keys);
    // Undo shows the table before.
    assert!(c.undo());
    assert!(c.wavetable_data(&key("wt")).unwrap().phase_safe);
    assert!(c.undo());
    let none = c.wavetable_data(&key("wt")).unwrap();
    assert!(!none.stored);
}

#[test]
fn a_device_without_a_sample_starts_from_the_table_of_oscillator_one() {
    let mut c = core();
    let data = c.wavetable_data(&key("wt")).unwrap();
    assert!(!data.stored && !data.imported);
    assert_eq!(data.keys.len(), 4, "four frames of the factory table");
    assert!(data.keys.iter().all(|k| k.len() == FRAME_LEN));
    assert_ne!(data.keys[0], data.keys[3]);
    assert!(!data.plays, "no oscillator plays the imported table");
    // Another table, another start.
    c.apply_command(Command::SetParam {
        key: key("wt"),
        param: "osc1.table".into(),
        value: 3.0,
        merge: None,
    })
    .unwrap();
    let other = c.wavetable_data(&key("wt")).unwrap();
    assert_ne!(other.keys, data.keys);
    // The imported table starts from one sine.
    c.apply_command(Command::SetParam {
        key: key("wt"),
        param: "osc1.table".into(),
        value: 12.0,
        merge: None,
    })
    .unwrap();
    let sine = c.wavetable_data(&key("wt")).unwrap();
    assert!(sine.plays);
    assert_eq!(sine.keys.len(), 1);
    assert!(close(sine.keys[0][FRAME_LEN / 4], 1.0, 1.0e-4));
    // Nothing was stored by looking.
    assert!(zones(&c).is_empty());
}

#[test]
fn use_on_oscillator_one_is_one_undo_step() {
    let mut c = core();
    let depth = c.document().undo_depth();
    c.wavetable_use_on_osc1(&key("wt"));
    assert!(c.wavetable_data(&key("wt")).unwrap().plays);
    assert_eq!(c.document().undo_depth(), depth + 1);
    assert!(c.undo());
    assert!(!c.wavetable_data(&key("wt")).unwrap().plays);
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let mut c = core();
    let depth = c.document().undo_depth();
    assert!(c.commit_wavetable(&key("wt"), &[], true).is_err());
    let many: Vec<Frame> = (0..17).map(|_| keys()[0].clone()).collect();
    assert!(c.commit_wavetable(&key("wt"), &many, true).is_err());
    assert!(c.commit_wavetable(&key("gone"), &keys(), true).is_err());
    assert!(c.wavetable_data(&key("gone")).is_none());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.sampler".into(),
        key: Some(key("smp")),
    })
    .unwrap();
    let depth = depth + 1;
    assert!(c.commit_wavetable(&key("smp"), &keys(), true).is_err());
    assert!(
        c.wavetable_data(&key("smp")).is_none(),
        "no wavetable synth"
    );
    assert_eq!(c.document().undo_depth(), depth);
}

#[test]
fn the_engine_gets_a_sample_map_the_synth_turns_into_the_drawn_table() {
    let mut c = core();
    let keys = keys();
    c.commit_wavetable(&key("wt"), &keys, false).unwrap();
    let map = c
        .sink_mut()
        .take()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::Samples { map: Some(m), .. } => Some(m),
            _ => None,
        })
        .expect("a sample map reached the engine");
    let attached = atomik_devices::wavetable_synth::attach_table(map);
    let table = attached
        .buffers()
        .last()
        .and_then(|b| b.data.first())
        .expect("the table buffer")
        .clone();
    // Position 0 and 1 are the first and last key frames at full level (the synth scales each
    // frame to peak at 1); the table is silent only without a sample.
    for (position, frame) in [(0.0f32, &keys[0]), (1.0, &keys[3])] {
        let peak = frame.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        for n in (0..FRAME_LEN).step_by(53) {
            let want = frame[n] / peak;
            let got = read_pyramid(&table, 0, position, n as f64 / FRAME_LEN as f64);
            assert!(
                close(got, want, 2.0e-3),
                "position {position}, sample {n}: {got} vs {want}"
            );
        }
    }
    // A position between two key frames is their blend: frame 5 is the second key.
    let second = &keys[1];
    let peak = second.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let got = read_pyramid(&table, 0, 5.0 / 15.0, 0.1);
    assert!(close(
        got,
        second[(0.1 * FRAME_LEN as f32) as usize] / peak,
        2.0e-3
    ));
}

#[test]
fn the_table_survives_save_and_reload() {
    let t = Tmp::new("roundtrip");
    let mut c = core();
    let keys = keys();
    c.commit_wavetable(&key("wt"), &keys, true).unwrap();
    let before = c.wavetable_data(&key("wt")).unwrap();
    c.save_as(&t.path("song.atomik")).unwrap();
    let file = t.path("song").join("song.atomik");
    let mut fresh = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    fresh.open(&file).unwrap();
    fresh.wait_background();
    let after = fresh.wavetable_data(&key("wt")).unwrap();
    assert_eq!(after, before, "the same key frames, flags and name");
    assert_eq!(after.keys.len(), 4);
    // The asset is in the project's audio.
    let z = fresh.doc().device(&key("wt")).unwrap().zones[0].clone();
    let bundle = Bundle::open(fresh.bundle_root().expect("a bundle")).unwrap();
    let audio = bundle.load(&z.asset).unwrap();
    assert_eq!(audio.channels.len(), 1);
    assert_eq!(audio.channels[0].len(), FRAMES * FRAME_LEN);
    // The reloaded device sends the engine its table again.
    let calls = fresh.sink_mut().take();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, SinkCall::Samples { map: Some(_), .. }))
    );
}

/// A tone of `cycles` cycles of `len` samples whose second harmonic grows from none to full.
fn evolving(len: usize, cycles: usize) -> Vec<f32> {
    (0..len * cycles)
        .map(|n| {
            let p = n as f64 / len as f64;
            let grow = p / cycles as f64;
            let tau = std::f64::consts::TAU;
            ((tau * p).sin() + grow * (tau * 2.0 * p + 0.5).sin()) as f32
        })
        .collect()
}

fn drop_wav(c: &mut AppCore<RecordingSink>, t: &Tmp, samples: Vec<f32>) {
    let path = t.path("source.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![samples]).unwrap()).unwrap();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![path],
        device: key("wt"),
        at_key: None,
    });
    c.wait_background();
}

#[test]
fn a_dropped_sample_shows_its_cycle_and_is_cut_by_it() {
    let t = Tmp::new("cut");
    let mut c = core();
    drop_wav(&mut c, &t, evolving(441, 12));
    let data = c.wavetable_data(&key("wt")).unwrap();
    assert!(data.imported && data.stored, "{data:?}");
    let (len, hz) = data.cycle.expect("a cycle is found");
    assert!(close(len, 441.0, 2.0), "{len}");
    assert!(close(hz, 48_000.0 / len, 1.0e-3));
    assert_eq!(data.keys.len(), 2, "two whole frames of 2048 samples");
    let depth = c.document().undo_depth();
    c.cut_wavetable_cycles(&key("wt"));
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let cut = c.wavetable_data(&key("wt")).unwrap();
    assert!(!cut.imported && cut.phase_safe);
    assert!(
        cut.keys.len() >= 4 && cut.keys.len() <= 12,
        "{}",
        cut.keys.len()
    );
    let note = c.last_rejection().expect("a message").to_owned();
    assert!(note.starts_with("Cut "), "{note}");
    // Each cut frame is one cycle: its fundamental carries most of the energy.
    for k in &cut.keys {
        let b = edit::analyse(k, 8);
        assert!(b[0].level > 0.8, "{:?}", b[0]);
    }
    // Undo brings the dropped sample back.
    assert!(c.undo());
    assert!(c.wavetable_data(&key("wt")).unwrap().imported);
}

#[test]
fn cutting_without_a_cycle_or_a_sample_says_so() {
    let t = Tmp::new("nocycle");
    let mut c = core();
    c.cut_wavetable_cycles(&key("wt"));
    assert_eq!(c.last_rejection(), Some("The table has no sample to cut"));
    let mut rng = atomik_dsp::Rng::new(9);
    let noise: Vec<f32> = (0..20_000).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
    drop_wav(&mut c, &t, noise);
    let data = c.wavetable_data(&key("wt")).unwrap();
    assert!(data.imported && data.cycle.is_none());
    let depth = c.document().undo_depth();
    c.cut_wavetable_cycles(&key("wt"));
    assert_eq!(c.last_rejection(), Some("No cycle was found in the sample"));
    assert_eq!(c.document().undo_depth(), depth);
    // Closing the editor lets go of what it read.
    c.release_wavetable();
    assert!(c.wavetable_data(&key("wt")).unwrap().imported);
}
