//! A MusicXML file (`.musicxml`, `.xml` or a compressed `.mxl`): any bytes must read or be refused.
//! What reads is checked the way the app checks an import, and written and read once more.
#![no_main]

use atomik_tab::musicxml::{read_musicxml, write_musicxml};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((mut score, _warnings)) = read_musicxml(data) {
        let _ = score.problems();
        let _ = score.bar_grid();
        let _ = score.sanitize();
        if let Ok((xml, _)) = write_musicxml(&score) {
            let _ = read_musicxml(xml.as_bytes());
        }
    }
});
