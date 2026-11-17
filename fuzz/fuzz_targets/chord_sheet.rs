//! A chord sheet (chords over lyrics, or the bracket form): any text must read or be refused. What
//! reads is transposed and written in both forms, which must not panic either.
#![no_main]

use atomik_tab::sheet::parse_sheet;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(parsed) = parse_sheet(&text) {
        let mut sheet = parsed.sheet;
        let _ = sheet.to_over_lyrics();
        let _ = sheet.to_inline();
        sheet.transpose(3, None);
        let _ = sheet.chords_used();
        let _ = sheet.to_over_lyrics();
    }
});
