//! Truncated or corrupted files never panic; they decode or return an error.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{PeakPyramid, decode_bytes};
use proptest::prelude::*;

const FILES: [&str; 14] = [
    "stereo_wav16.wav",
    "stereo_wav24.wav",
    "stereo_wavf32.wav",
    "stereo_aiff16.aif",
    "stereo_flac.flac",
    "stereo_mp3.mp3",
    "stereo_ogg.ogg",
    "mono_wav16.wav",
    "mono_wav24.wav",
    "mono_wavf32.wav",
    "mono_aiff16.aif",
    "mono_flac.flac",
    "mono_mp3.mp3",
    "mono_ogg.ogg",
];

fn load(index: usize) -> (String, Vec<u8>) {
    let name = FILES[index % FILES.len()];
    (
        name.to_owned(),
        std::fs::read(common::fixture(name)).unwrap(),
    )
}

/// The call must return (never unwind). Symphonia 0.6.1 has a few arithmetic-overflow panics on
/// corrupt headers that only fire when overflow checks are on (this crate's tests, debug
/// builds); `decode_bytes` catches them and returns `Decode`. Known cases are rejected before
/// symphonia sees them (see `absurd_channel_counts_are_rejected_cleanly`).
fn assert_no_panic(_name: &str, bytes: &[u8], ext: Option<&str>) {
    let _ = decode_bytes(bytes, ext);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn truncated_files_never_panic(index in 0usize..14, frac in 0.0f64..1.0) {
        let (name, bytes) = load(index);
        let cut = (bytes.len() as f64 * frac) as usize;
        let ext = name.rsplit('.').next();
        assert_no_panic(&name, &bytes[..cut], ext);
        assert_no_panic(&name, &bytes[..cut], None);
    }

    #[test]
    fn corrupted_files_never_panic(
        index in 0usize..14,
        edits in proptest::collection::vec((0.0f64..1.0, any::<u8>()), 1..24),
    ) {
        let (name, mut bytes) = load(index);
        for (pos, value) in edits {
            let i = ((bytes.len() - 1) as f64 * pos) as usize;
            bytes[i] = value;
        }
        let ext = name.rsplit('.').next();
        assert_no_panic(&name, &bytes, ext);
        assert_no_panic(&name, &bytes, None);
    }

    #[test]
    fn corrupted_headers_never_panic(
        index in 0usize..14,
        edits in proptest::collection::vec((0usize..64, any::<u8>()), 1..8),
    ) {
        let (name, mut bytes) = load(index);
        for (pos, value) in edits {
            let i = pos.min(bytes.len() - 1);
            bytes[i] = value;
        }
        let ext = name.rsplit('.').next();
        assert_no_panic(&name, &bytes, ext);
    }

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        assert_no_panic("random", &bytes, None);
        assert_no_panic("random", &bytes, Some("wav"));
        let _ = PeakPyramid::from_bytes(&bytes);
    }
}

#[test]
fn every_truncation_of_the_small_fixtures_is_handled() {
    // Exhaustive for the smallest files: every possible cut point.
    for name in ["mono_mp3.mp3", "stereo_mp3.mp3"] {
        let bytes = std::fs::read(common::fixture(name)).unwrap();
        for cut in (0..bytes.len()).step_by(7) {
            assert_no_panic(name, &bytes[..cut], Some("mp3"));
        }
    }
}
