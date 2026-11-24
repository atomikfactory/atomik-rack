//! The limits that follow the machine's memory: the undo history's budget, and how large an
//! import may be. Pure functions of the physical memory the platform layer reports
//! ([`crate::platform::memory`]); a machine with 16 GiB or more, or one that does not tell,
//! gets the limits that were always there.

use atomik_audio_file::{DEFAULT_MAX_SAMPLES, ImportLimits};
use atomik_model::MAX_UNDO_BYTES;

const GIB: u64 = 1 << 30;

/// Below this much physical memory the undo history keeps a smaller budget.
const SMALL_MACHINE: u64 = 8 * GIB;

/// The undo budget, in bytes, on a machine with less than [`SMALL_MACHINE`].
const SMALL_UNDO_BYTES: usize = 128 << 20;

/// Memory from which an import has the whole of its default cap (4 GiB is a quarter of it).
const FULL_CAP_MACHINE: u64 = 16 * GIB;

/// The undo history's memory limit for a machine with `total` bytes of physical memory: 128 MiB
/// under 8 GiB, [`MAX_UNDO_BYTES`] from there on and when the memory is not known.
pub fn undo_bytes_for(total: Option<u64>) -> usize {
    match total {
        Some(t) if t < SMALL_MACHINE => SMALL_UNDO_BYTES,
        _ => MAX_UNDO_BYTES,
    }
}

/// What an import may decode on a machine with `total` bytes of physical memory. The decoded
/// size (frames x channels x 4 bytes) may not exceed a quarter of the memory, and never the
/// default cap of 4 GiB, so a machine with 16 GiB or more has exactly the default. Below 16 GiB a
/// file that has to be converted to the session's rate counts twice (the conversion holds a
/// second copy). An unknown size leaves the default.
pub fn import_limits_for(total: Option<u64>) -> ImportLimits {
    let Some(total) = total.filter(|t| *t < FULL_CAP_MACHINE) else {
        return ImportLimits::default();
    };
    let quarter_samples = (total / 4 / 4).min(DEFAULT_MAX_SAMPLES);
    ImportLimits {
        max_samples: quarter_samples,
        max_samples_resampled: quarter_samples / 2,
    }
}

/// A size for a message: binary units, one decimal ("3.2 GB", "640 MB").
pub fn size_text(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MB", bytes.div_ceil(1 << 20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_gigabytes_or_megabytes() {
        assert_eq!(size_text(3 * GIB + GIB / 5), "3.2 GB");
        assert_eq!(size_text(GIB), "1.0 GB");
        assert_eq!(size_text(640 << 20), "640 MB");
        assert_eq!(size_text(1), "1 MB");
    }

    #[test]
    fn undo_budget_follows_the_memory() {
        assert_eq!(undo_bytes_for(None), MAX_UNDO_BYTES);
        assert_eq!(undo_bytes_for(Some(4 * GIB)), 128 << 20);
        assert_eq!(undo_bytes_for(Some(8 * GIB - 1)), 128 << 20);
        assert_eq!(undo_bytes_for(Some(8 * GIB)), MAX_UNDO_BYTES);
        assert_eq!(undo_bytes_for(Some(64 * GIB)), MAX_UNDO_BYTES);
    }

    #[test]
    fn import_limit_is_a_quarter_of_the_memory_and_never_above_the_default() {
        let default = ImportLimits::default();
        assert_eq!(import_limits_for(None), default);
        assert_eq!(import_limits_for(Some(16 * GIB)), default);
        assert_eq!(import_limits_for(Some(256 * GIB)), default);
        // 8 GiB: a quarter is 2 GiB of f32, 512 Mi samples; converting counts twice.
        let eight = import_limits_for(Some(8 * GIB));
        assert_eq!(eight.max_samples, (2 * GIB / 4));
        assert_eq!(eight.max_samples_resampled, GIB / 4);
        // 4 GiB: 1 GiB of f32.
        let four = import_limits_for(Some(4 * GIB));
        assert_eq!(four.max_samples, GIB / 4);
        assert_eq!(four.max_samples_resampled, GIB / 8);
        // Just under 16 GiB is below the default cap, not above it.
        assert!(import_limits_for(Some(16 * GIB - 1)).max_samples <= default.max_samples);
    }
}

#[cfg(test)]
mod core_tests {
    use crate::AppCore;
    use crate::platform::memory::MemoryInfo;
    use crate::sink::RecordingSink;
    use atomik_model::{MAX_UNDO_BYTES, MAX_UNDO_STEPS, new_project_doc, save_project};

    const GIB: u64 = 1 << 30;

    fn machine(total: u64) -> MemoryInfo {
        MemoryInfo {
            total: Some(total),
            available: Some(total / 2),
        }
    }

    fn limits(c: &AppCore<RecordingSink>) -> (usize, usize) {
        c.document().undo_limits()
    }

    #[test]
    fn a_document_made_by_each_path_gets_the_budget_of_the_machine() {
        let json = save_project(&new_project_doc());
        for (total, bytes) in [(4 * GIB, 128 << 20), (16 * GIB, MAX_UNDO_BYTES)] {
            let mut c = AppCore::new(RecordingSink::new());
            // Made by the constructor: a unit test does not read the machine.
            assert_eq!(limits(&c), (MAX_UNDO_STEPS, MAX_UNDO_BYTES));
            c.set_memory_info(machine(total));
            assert_eq!(limits(&c), (MAX_UNDO_STEPS, bytes), "the open document");
            // New song, open and close each make a document through `replace`.
            c.new_document();
            assert_eq!(limits(&c), (MAX_UNDO_STEPS, bytes), "new song");
            c.open_str(&json, None).unwrap();
            assert_eq!(limits(&c), (MAX_UNDO_STEPS, bytes), "open");
        }
    }

    #[test]
    fn a_machine_that_tells_nothing_keeps_the_default() {
        let mut c = AppCore::new(RecordingSink::new());
        c.set_memory_info(MemoryInfo::UNKNOWN);
        c.new_document();
        assert_eq!(limits(&c), (MAX_UNDO_STEPS, MAX_UNDO_BYTES));
    }
}
