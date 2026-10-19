//! Text helpers: VST3 strings are UTF-16 (`String128`) or fixed C arrays.

use std::ffi::c_char;

/// The text of a UTF-16 buffer up to its first NUL (lossy).
pub(crate) fn utf16_text(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// The text of a fixed C char array up to its first NUL (lossy).
pub(crate) fn c_array_text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Copies `text` into a UTF-16 buffer, always NUL-terminated.
pub(crate) fn write_utf16(text: &str, out: &mut [u16]) {
    let room = out.len().saturating_sub(1);
    let mut n = 0;
    for (d, s) in out.iter_mut().take(room).zip(text.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if let Some(end) = out.get_mut(n) {
        *end = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_text_stops_at_the_first_nul_and_survives_bad_units() {
        let mut buf = [0u16; 8];
        write_utf16("Gain", &mut buf);
        assert_eq!(utf16_text(&buf), "Gain");
        assert_eq!(utf16_text(&[0x48, 0xD800, 0x49, 0]), "H\u{FFFD}I");
        assert_eq!(utf16_text(&[]), "");
        assert_eq!(utf16_text(&[0x61, 0x62]), "ab", "no terminator");
    }

    #[test]
    fn write_utf16_truncates_and_always_terminates() {
        let mut buf = [0xFFFFu16; 4];
        write_utf16("abcdef", &mut buf);
        assert_eq!(buf, [0x61, 0x62, 0x63, 0]);
        let mut empty: [u16; 0] = [];
        write_utf16("x", &mut empty);
    }

    #[test]
    fn c_array_text_is_lossy_and_stops_at_nul() {
        let a: [c_char; 5] = [b'F' as c_char, b'x' as c_char, 0, b'z' as c_char, 0];
        assert_eq!(c_array_text(&a), "Fx");
        assert_eq!(c_array_text(&[]), "");
    }
}
