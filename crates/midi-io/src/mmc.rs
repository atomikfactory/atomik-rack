//! MIDI Machine Control (MMC): the transport commands a recorder, a controller or another
//! program sends as universal real-time SysEx to play, stop, record or locate the song.
//!
//! A command is `F0 7F <device> 06 <command> [<length> <data>...] F7`. [`parse_mmc`] is pure and
//! never panics: bytes in, a [`Mmc`] out, `None` for anything else (other SysEx, a command the
//! song has no use for, a locate with an impossible time). The device id is not checked: `7F`
//! addresses every device, and a port the user set to the mapping role is already one the user
//! picked to control the transport.

/// The universal real-time SysEx id and the MMC command sub-id.
const REAL_TIME: u8 = 0x7F;
const MMC_COMMAND: u8 = 0x06;

/// A machine control command the transport understands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mmc {
    /// Stop (01).
    Stop,
    /// Play (02) or deferred play (03): the song plays from where it is.
    Play,
    /// Fast forward (04).
    FastForward,
    /// Rewind (05).
    Rewind,
    /// Record strobe (06): punch in.
    RecordStrobe,
    /// Record exit (07): punch out.
    RecordExit,
    /// Pause (09): the song stops where it is.
    Pause,
    /// Locate (44, sub-command 01): go to this time, in seconds from the start of the song.
    Locate {
        /// The target time.
        seconds: f64,
    },
}

/// The frame rates of the SMPTE type bits of a locate time, in frames per second.
fn frame_rate(kind: u8) -> f64 {
    match kind & 0x03 {
        0 => 24.0,
        1 => 25.0,
        2 => 30_000.0 / 1_001.0,
        _ => 30.0,
    }
}

/// Reads the MMC command in `bytes`, a whole SysEx message (the closing `F7` is optional, as
/// some backends cut it).
pub fn parse_mmc(bytes: &[u8]) -> Option<Mmc> {
    let [0xF0, REAL_TIME, _device, MMC_COMMAND, command, rest @ ..] = bytes else {
        return None;
    };
    let data = match rest {
        [data @ .., 0xF7] => data,
        data => data,
    };
    if data.iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    match (*command, data) {
        (0x01, []) => Some(Mmc::Stop),
        (0x02 | 0x03, []) => Some(Mmc::Play),
        (0x04, []) => Some(Mmc::FastForward),
        (0x05, []) => Some(Mmc::Rewind),
        (0x06, []) => Some(Mmc::RecordStrobe),
        (0x07, []) => Some(Mmc::RecordExit),
        (0x09, []) => Some(Mmc::Pause),
        // Locate: length 6 (or 7 with the status bits), sub-command 01 "target", then the time.
        (0x44, [length, 0x01, hours, minutes, seconds, frames, sub, ..]) if *length >= 6 => {
            let rate = frame_rate(hours >> 5);
            let (h, m, s) = (
                f64::from(hours & 0x1F),
                f64::from(*minutes),
                f64::from(*seconds),
            );
            if h > 23.0 || m > 59.0 || s > 59.0 || f64::from(*frames) >= rate.ceil() || *sub > 99 {
                return None;
            }
            let frame = f64::from(*frames) + f64::from(*sub) / 100.0;
            Some(Mmc::Locate {
                seconds: h * 3600.0 + m * 60.0 + s + frame / rate,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple(command: u8) -> Vec<u8> {
        vec![0xF0, 0x7F, 0x7F, 0x06, command, 0xF7]
    }

    #[test]
    fn the_transport_commands_are_read() {
        for (command, expected) in [
            (0x01, Mmc::Stop),
            (0x02, Mmc::Play),
            (0x03, Mmc::Play),
            (0x04, Mmc::FastForward),
            (0x05, Mmc::Rewind),
            (0x06, Mmc::RecordStrobe),
            (0x07, Mmc::RecordExit),
            (0x09, Mmc::Pause),
        ] {
            assert_eq!(parse_mmc(&simple(command)), Some(expected), "{command:#x}");
        }
        // The closing byte is optional and the device id is free.
        assert_eq!(parse_mmc(&[0xF0, 0x7F, 0x03, 0x06, 0x02]), Some(Mmc::Play));
    }

    #[test]
    fn a_locate_is_a_time_in_seconds() {
        // 01:02:03 and 12 frames at 25 fps, plus half a frame.
        let bytes = [
            0xF0,
            0x7F,
            0x7F,
            0x06,
            0x44,
            0x06,
            0x01,
            0x20 | 1,
            2,
            3,
            12,
            50,
            0xF7,
        ];
        let Some(Mmc::Locate { seconds }) = parse_mmc(&bytes) else {
            panic!("a locate");
        };
        assert!(
            (seconds - (3600.0 + 120.0 + 3.0 + 12.5 / 25.0)).abs() < 1e-9,
            "{seconds}"
        );
        // The four frame rates.
        for (kind, rate) in [(0u8, 24.0), (1, 25.0), (2, 30_000.0 / 1_001.0), (3, 30.0)] {
            let b = [
                0xF0,
                0x7F,
                0x7F,
                0x06,
                0x44,
                0x06,
                0x01,
                kind << 5,
                0,
                1,
                10,
                0,
                0xF7,
            ];
            let Some(Mmc::Locate { seconds }) = parse_mmc(&b) else {
                panic!("a locate");
            };
            assert!(
                (seconds - (1.0 + 10.0 / rate)).abs() < 1e-9,
                "{kind}: {seconds}"
            );
        }
        // The start of the song.
        let zero = [
            0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 0, 0, 0, 0, 0xF7,
        ];
        assert_eq!(parse_mmc(&zero), Some(Mmc::Locate { seconds: 0.0 }));
    }

    #[test]
    fn what_is_not_a_usable_command_is_nothing() {
        assert_eq!(parse_mmc(&[]), None);
        assert_eq!(parse_mmc(&[0xF0, 0xF7]), None);
        // Not real time, not a command, not a command we have.
        assert_eq!(parse_mmc(&[0xF0, 0x7E, 0x7F, 0x06, 0x02, 0xF7]), None);
        assert_eq!(parse_mmc(&[0xF0, 0x7F, 0x7F, 0x07, 0x02, 0xF7]), None);
        assert_eq!(parse_mmc(&simple(0x0D)), None);
        // A stop with a stray data byte, or a data byte with the high bit set.
        assert_eq!(parse_mmc(&[0xF0, 0x7F, 0x7F, 0x06, 0x01, 0x05, 0xF7]), None);
        assert_eq!(parse_mmc(&[0xF0, 0x7F, 0x7F, 0x06, 0x01, 0x85, 0xF7]), None);
        // A channel message is not SysEx.
        assert_eq!(parse_mmc(&[0x90, 60, 100]), None);
        // Locates cut short or with impossible times.
        for bad in [
            vec![0xF0, 0x7F, 0x7F, 0x06, 0x44, 0xF7],
            vec![0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 0, 0, 0, 0xF7],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x02, 0, 0, 0, 0, 0, 0xF7,
            ],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 60, 0, 0, 0, 0xF7,
            ],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 0, 60, 0, 0, 0xF7,
            ],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 24, 0, 0, 0, 0, 0xF7,
            ],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 0, 0, 24, 0, 0xF7,
            ],
            vec![
                0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 0, 0, 0, 0, 100, 0xF7,
            ],
        ] {
            assert_eq!(parse_mmc(&bad), None, "{bad:02X?}");
        }
    }

    #[test]
    fn no_input_makes_the_parser_panic() {
        // Every two-byte prefix of a locate, with every second byte.
        let base = [
            0xF0, 0x7F, 0x7F, 0x06, 0x44, 0x06, 0x01, 1, 2, 3, 4, 5, 0xF7,
        ];
        for cut in 0..=base.len() {
            for fill in [0u8, 0x7F, 0x80, 0xFF] {
                let mut b = base[..cut].to_vec();
                if let Some(last) = b.last_mut() {
                    *last = fill;
                }
                let _ = parse_mmc(&b);
            }
        }
    }
}
