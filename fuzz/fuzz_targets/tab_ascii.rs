//! Plain-text tab: any text must read or be refused. What reads is written back, which must not
//! panic either.
#![no_main]

use atomik_tab::ascii::{AsciiOptions, AsciiWriteOptions, parse_ascii, write_ascii};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(parsed) = parse_ascii(&text, &AsciiOptions::default()) {
        let _ = write_ascii(&parsed.score, &AsciiWriteOptions::default());
    }
});
