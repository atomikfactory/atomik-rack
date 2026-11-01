//! Importing audio holds about its decoded size, not several copies of it, and stores the same
//! asset as before.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{AudioData, Bundle, content_hash, write_wav_f32};
use atomik_testkit::no_alloc::peak_heap_of as peak_of;

atomik_testkit::install_alloc_guard!();

fn tone(rate: u32, frames: usize) -> AudioData {
    let ch = |k: f32| -> Vec<f32> {
        (0..frames)
            .map(|i| ((i as f32) * 0.01 * k).sin() * 0.5)
            .collect()
    };
    AudioData::new(rate, vec![ch(1.0), ch(1.7)]).unwrap()
}

/// Peak heap of an import at the file's own rate, in decoded sizes. Prints the figure.
fn import_peak(rate: u32, session: u32) -> f64 {
    let dir = common::tmp_dir(&format!("peak-{rate}-{session}"));
    let src = dir.join("tone.wav");
    let data = tone(rate, 20 * rate as usize);
    write_wav_f32(&src, &data).unwrap();
    let decoded_bytes = data.frames() * 2 * 4;
    drop(data);
    let mut bundle = Bundle::create(&dir.join("P")).unwrap();
    let (info, peak) = peak_of(|| bundle.import(&src, session).unwrap());
    assert_eq!(info.frames, 20 * u64::from(session));
    let ratio = peak as f64 / decoded_bytes as f64;
    println!("import {rate} -> {session}: peak {ratio:.2}x the decoded size");
    ratio
}

#[test]
fn an_import_at_the_session_rate_peaks_below_two_and_a_fifth_decoded_sizes() {
    for rate in [48_000, 44_100] {
        let ratio = import_peak(rate, rate);
        assert!(ratio <= 2.2, "{rate} Hz: {ratio:.2}x");
    }
}

#[test]
fn the_asset_id_of_an_import_is_the_content_hash_of_the_decoded_audio() {
    let dir = common::tmp_dir("asset-id");
    let src = dir.join("tone.wav");
    let data = tone(44_100, 30_000);
    write_wav_f32(&src, &data).unwrap();
    let mut bundle = Bundle::create(&dir.join("P")).unwrap();
    let info = bundle.import(&src, 44_100).unwrap();
    assert_eq!(info.hash, content_hash(&data));
    // The stored file reads back sample for sample.
    assert_eq!(bundle.load(&info.hash).unwrap(), data);
}
