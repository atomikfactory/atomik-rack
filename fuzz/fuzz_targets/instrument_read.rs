//! Instrument files: the SFZ text reader and the SoundFont 2 reader. The first byte picks the
//! reader (and which SoundFont preset), the rest is the file.
#![no_main]

use std::path::Path;

use atomik_audio_file::{InstrumentLimits, parse_sf2, parse_sfz, sf2_presets};
use libfuzzer_sys::fuzz_target;

/// Small caps keep memory flat however the input lies.
const LIMITS: InstrumentLimits = InstrumentLimits {
    max_file_bytes: 1 << 24,
    max_regions: 256,
    max_samples: 64,
};

fuzz_target!(|data: &[u8]| {
    let Some((&which, bytes)) = data.split_first() else {
        return;
    };
    if which & 1 == 0 {
        let text = String::from_utf8_lossy(bytes);
        let inst = parse_sfz(&text, Path::new("lib"), "fuzz", LIMITS);
        assert!(inst.regions.iter().all(|r| r.sample < inst.samples.len()));
    } else {
        let _ = sf2_presets(bytes);
        if let Ok(inst) = parse_sf2(bytes, usize::from(which >> 1), LIMITS) {
            assert!(inst.regions.iter().all(|r| r.sample < inst.samples.len()));
        }
    }
});
