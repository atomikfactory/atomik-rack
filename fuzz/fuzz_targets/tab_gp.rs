//! A Guitar Pro file (`.gp3`, `.gp4`, `.gp5`, `.gpx`, `.gp` or a bare GPIF score): any bytes must
//! read or be refused. What reads is checked the way the app checks an import.
#![no_main]

use atomik_tab::gp::read_gp;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((mut score, _warnings)) = read_gp(data) {
        let _ = score.problems();
        let _ = score.bar_grid();
        let _ = score.sanitize();
    }
});
