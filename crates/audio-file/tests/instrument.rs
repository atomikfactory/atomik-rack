//! Instrument files: the readers never panic on hostile input, every region names a sample that
//! exists, and the bundle stores instrument samples at the session rate and says how long they
//! were before.
#![allow(missing_docs)]

use atomik_audio_file::{
    AudioData, Bundle, InstrumentLimits, SampleSource, parse_sf2, parse_sfz, read_instrument,
    write_wav_f32,
};
use proptest::prelude::*;
use std::path::{Path, PathBuf};

const SMALL_SF2: &[u8] = include_bytes!("fixtures/small.sf2");

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-instrument-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_sfz_text_never_panics_and_regions_name_real_samples(text in "\\PC{0,400}") {
        let inst = parse_sfz(&text, Path::new("lib"), "t", InstrumentLimits::default());
        prop_assert!(inst.regions.iter().all(|r| r.sample < inst.samples.len()));
        prop_assert!(inst.regions.iter().all(|r| r.key_lo <= r.key_hi && r.vel_lo <= r.vel_hi));
    }

    #[test]
    fn sfz_built_from_opcode_pieces_stays_in_range(
        pieces in proptest::collection::vec(
            prop_oneof![
                Just("<region>".to_owned()),
                Just("<group>".to_owned()),
                Just("<global>".to_owned()),
                Just("sample=a.wav".to_owned()),
                "[a-z_]{1,12}=[-0-9a-z#.]{0,6}",
            ],
            0..40,
        )
    ) {
        let text = pieces.join(" ");
        let inst = parse_sfz(&text, Path::new(""), "t", InstrumentLimits::default());
        for r in &inst.regions {
            prop_assert!(r.sample < inst.samples.len());
            prop_assert!(r.key_hi <= 127 && r.vel_lo >= 1 && r.vel_hi <= 127);
            prop_assert!(r.root_key <= 127);
            prop_assert!((-1.0..=1.0).contains(&r.pan));
            prop_assert!(r.xfade <= r.vel_hi - r.vel_lo);
        }
    }

    #[test]
    fn a_damaged_soundfont_never_panics(at in 0..SMALL_SF2.len(), byte in any::<u8>(), cut in 0..SMALL_SF2.len()) {
        let mut bad = SMALL_SF2.to_vec();
        bad[at] = byte;
        bad.truncate(bad.len() - cut / 4);
        if let Ok(inst) = parse_sf2(&bad, 0, InstrumentLimits::default()) {
            prop_assert!(inst.regions.iter().all(|r| r.sample < inst.samples.len()));
        }
    }
}

#[test]
fn the_committed_soundfont_reads_through_the_path_reader() {
    let dir = temp("sf2");
    let path = dir.join("small.sf2");
    std::fs::write(&path, SMALL_SF2).unwrap();
    let inst = read_instrument(&path, InstrumentLimits::default()).unwrap();
    assert_eq!((inst.regions.len(), inst.samples.len()), (2, 2));
    // A size over the limit is refused, not read.
    let tiny = InstrumentLimits {
        max_file_bytes: 100,
        ..InstrumentLimits::default()
    };
    assert!(read_instrument(&path, tiny).is_err());
    assert!(read_instrument(&dir.join("x.txt"), InstrumentLimits::default()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_big_sfz_file_is_refused_and_a_missing_one_is_an_error() {
    let dir = temp("sfz-big");
    let path = dir.join("big.sfz");
    std::fs::write(
        &path,
        vec![b' '; (atomik_audio_file::MAX_SFZ_BYTES + 1) as usize],
    )
    .unwrap();
    assert!(read_instrument(&path, InstrumentLimits::default()).is_err());
    assert!(read_instrument(&dir.join("none.sfz"), InstrumentLimits::default()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_bundle_stores_instrument_samples_at_the_session_rate_and_reports_the_source_length() {
    let dir = temp("bundle");
    let mut bundle = Bundle::create(&dir.join("b")).unwrap();
    let ramp: Vec<f32> = (0..2205).map(|i| i as f32 / 2205.0).collect();
    let file = dir.join("s.wav");
    write_wav_f32(&file, &AudioData::new(22_050, vec![ramp.clone()]).unwrap()).unwrap();
    let (info, original) = bundle.import_counted(&file, 44_100).unwrap();
    assert_eq!(
        (original, info.frames, info.sample_rate),
        (2205, 4410, 44_100)
    );
    // The same audio from inside another file lands on the same asset.
    let (again, original) = bundle
        .add_sample(AudioData::new(22_050, vec![ramp]).unwrap(), 44_100, "s.wav")
        .unwrap();
    assert_eq!((original, again.hash), (2205, info.hash));
    // An SFZ naming that file reads it as a path source.
    let sfz = dir.join("i.sfz");
    std::fs::write(&sfz, "<region> sample=s.wav key=60\n").unwrap();
    let inst = read_instrument(&sfz, InstrumentLimits::default()).unwrap();
    assert!(matches!(&inst.samples[0].source, SampleSource::File(p) if p.ends_with("s.wav")));
    let _ = std::fs::remove_dir_all(&dir);
}
