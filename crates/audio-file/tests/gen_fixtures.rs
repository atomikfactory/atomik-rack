//! Writes the 16-bit source WAVs that every other fixture is encoded from with ffmpeg.
//! See `tests/fixtures/README.md`. Run with
//! `cargo test -p atomik-audio-file --test gen_fixtures -- --ignored`.
#![allow(missing_docs)]

mod common;

#[test]
#[ignore = "regenerates the committed source fixtures"]
fn write_source_wavs() {
    let spec = |channels| hound::WavSpec {
        channels,
        sample_rate: common::RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let q = |x: f32| (f64::from(x) * 32767.0).round() as i16;
    let left = common::sine(common::RATE, 440.0, 0.5, common::FRAMES);
    let right = common::sine(common::RATE, 660.0, 0.25, common::FRAMES);

    let mut w = hound::WavWriter::create(common::fixture("stereo_wav16.wav"), spec(2)).unwrap();
    for i in 0..common::FRAMES {
        w.write_sample(q(left[i])).unwrap();
        w.write_sample(q(right[i])).unwrap();
    }
    w.finalize().unwrap();

    let mut w = hound::WavWriter::create(common::fixture("mono_wav16.wav"), spec(1)).unwrap();
    for s in &left {
        w.write_sample(q(*s)).unwrap();
    }
    w.finalize().unwrap();
}
