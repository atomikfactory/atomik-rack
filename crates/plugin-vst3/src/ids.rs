//! VST3 class ids as the 32 lowercase hex digits of the type id (`vst3.<hex>`).
//!
//! The text is the four 32-bit words of `uid(a, b, c, d)` (what the SDK's `FUID::toString`
//! prints), so it is the same on every operating system although the in-memory byte order of a
//! `TUID` is not (the COM layout on Windows, big-endian elsewhere).

use vst3::Steinberg::TUID;

fn bytes(tuid: &TUID) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (d, s) in b.iter_mut().zip(tuid.iter()) {
        *d = *s as u8;
    }
    b
}

/// The four words `a, b, c, d` of a `TUID` made by `uid(a, b, c, d)`.
pub(crate) fn words(tuid: &TUID) -> [u32; 4] {
    let b = bytes(tuid);
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if cfg!(windows) {
        [
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (u32::from(b[5]) << 24)
                | (u32::from(b[4]) << 16)
                | (u32::from(b[7]) << 8)
                | u32::from(b[6]),
            be(8),
            be(12),
        ]
    } else {
        [be(0), be(4), be(8), be(12)]
    }
}

/// The 32 lowercase hex digits of a class id.
pub(crate) fn class_id_hex(tuid: &TUID) -> String {
    let [a, b, c, d] = words(tuid);
    format!("{a:08x}{b:08x}{c:08x}{d:08x}")
}

/// The `TUID` of 32 hex digits (any case); `None` for anything else.
pub(crate) fn tuid_of_hex(hex: &str) -> Option<TUID> {
    if hex.len() != 32 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let word = |i: usize| u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok();
    Some(vst3::uid(word(0)?, word(1)?, word(2)?, word(3)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hex_is_the_four_words_on_every_operating_system() {
        assert_eq!(
            class_id_hex(&vst3::uid(1, 2, 3, 4)),
            "00000001000000020000000300000004"
        );
        assert_eq!(
            class_id_hex(&vst3::uid(0xA70F_1C01, 0x4A5B_4C01, 0x8D2E_1F00, 1)),
            "a70f1c014a5b4c018d2e1f0000000001"
        );
    }

    #[test]
    fn a_class_id_round_trips_through_its_hex() {
        for t in [
            vst3::uid(0, 0, 0, 0),
            vst3::uid(u32::MAX, 0x1234_5678, 0x9ABC_DEF0, 7),
            vst3::uid(0xDEAD_BEEF, 0xCAFE_F00D, 0x0102_0304, 0xFFFF_0000),
        ] {
            assert_eq!(tuid_of_hex(&class_id_hex(&t)), Some(t));
        }
    }

    #[test]
    fn bad_hex_is_refused() {
        assert_eq!(tuid_of_hex(""), None);
        assert_eq!(tuid_of_hex("abc"), None);
        assert_eq!(tuid_of_hex(&"g".repeat(32)), None);
        assert_eq!(tuid_of_hex(&"a".repeat(33)), None);
        assert!(tuid_of_hex("A70F1C014A5B4C018D2E1F0000000001").is_some());
    }
}
