//! A Standard MIDI File: any bytes must read or be refused. What reads is written back, which
//! must not panic either.
#![no_main]

use atomik_midi_io::smf;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = smf::read(data) {
        let _ = file.to_bytes();
    }
});
