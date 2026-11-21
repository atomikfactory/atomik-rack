//! The free-space check made when an audio take starts.
//!
//! A take streams to disk as 32-bit float, and when it ends every lap is stored once more under
//! its content hash before the in-progress file is deleted, so a take needs about twice its own
//! size on the volume of the project's `Audio` folder. The check warns, it never refuses: the
//! number the operating system gives is an estimate (quotas, other programs writing), and a take
//! that does fill the disk stops at once and keeps what it has (see `audio_writer`).

use atomik_ui::i18n;
use std::path::Path;

/// Minutes of the take's own audio the volume should have room for, twice over (the file being
/// written and its stored copy), before the warning shows. Ten minutes covers a song-length take
/// with room to spare and is 230 MB at 48 kHz stereo, a small fraction of the disks the app is
/// installed on, so the warning is rare and means it.
pub const FREE_SPACE_MINUTES: u64 = 10;

/// The free bytes for a normal user on the volume holding `dir`, or `None` when the system does
/// not say.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    fs4::available_space(dir).ok()
}

/// The bytes [`FREE_SPACE_MINUTES`] of a take need: `channels` is the sum over the take's files,
/// each sample is 4 bytes, and the factor of two is the stored copy.
pub fn needed_bytes(sample_rate: u32, channels: usize) -> u64 {
    FREE_SPACE_MINUTES * 60 * u64::from(sample_rate) * channels as u64 * 4 * 2
}

/// A size in whole megabytes, or gigabytes from 10 000 of them, as the notice says it.
fn size_text(bytes: u64) -> String {
    let mb = bytes / 1_000_000;
    if mb >= 10_000 {
        format!("{} GB", mb / 1_000)
    } else {
        format!("{mb} MB")
    }
}

/// The notice for a volume with `free` bytes when `needed` are wanted: `None` when there is room
/// or the free space is not known.
pub fn low_space_notice(free: Option<u64>, needed: u64) -> Option<String> {
    let free = free.filter(|f| *f < needed)?;
    Some(i18n::tf(
        "low disk space for recording: {free} free where this project's audio is written, and {needed} are wanted for {minutes} minutes of this take; the recording stops by itself if the disk fills up",
        &[
            ("free", &size_text(free)),
            ("needed", &size_text(needed)),
            ("minutes", &FREE_SPACE_MINUTES.to_string()),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_is_ten_minutes_twice_over() {
        // 48 kHz stereo: 10 min x 60 x 48 000 x 2 channels x 4 bytes = 230 400 000, twice.
        assert_eq!(needed_bytes(48_000, 2), 460_800_000);
        assert_eq!(needed_bytes(48_000, 4), 2 * needed_bytes(48_000, 2));
    }

    #[test]
    fn only_a_volume_below_the_floor_warns() {
        let needed = needed_bytes(48_000, 2);
        assert!(low_space_notice(Some(needed - 1), needed).is_some());
        assert!(low_space_notice(Some(needed), needed).is_none());
        assert!(
            low_space_notice(None, needed).is_none(),
            "unknown is not low"
        );
        let text = low_space_notice(Some(100_000_000), needed).unwrap_or_default();
        assert!(text.contains("100 MB") && text.contains("460 MB"), "{text}");
    }

    #[test]
    fn the_system_answers_for_a_folder_that_exists() {
        assert!(free_bytes(&std::env::temp_dir()).is_some_and(|b| b > 0));
    }
}
