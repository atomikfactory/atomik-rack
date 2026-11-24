//! The limits that follow the machine's memory: the undo history's budget. A pure function of the
//! physical memory the platform layer reports ([`crate::platform::memory`]); a machine with 8 GiB
//! or more, or one that does not tell, gets the limit that was always there.

use atomik_model::MAX_UNDO_BYTES;

const GIB: u64 = 1 << 30;

/// Below this much physical memory the undo history keeps a smaller budget.
const SMALL_MACHINE: u64 = 8 * GIB;

/// The undo budget, in bytes, on a machine with less than [`SMALL_MACHINE`].
const SMALL_UNDO_BYTES: usize = 128 << 20;

/// The undo history's memory limit for a machine with `total` bytes of physical memory: 128 MiB
/// under 8 GiB, [`MAX_UNDO_BYTES`] from there on and when the memory is not known.
pub fn undo_bytes_for(total: Option<u64>) -> usize {
    match total {
        Some(t) if t < SMALL_MACHINE => SMALL_UNDO_BYTES,
        _ => MAX_UNDO_BYTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_budget_follows_the_memory() {
        assert_eq!(undo_bytes_for(None), MAX_UNDO_BYTES);
        assert_eq!(undo_bytes_for(Some(4 * GIB)), 128 << 20);
        assert_eq!(undo_bytes_for(Some(8 * GIB - 1)), 128 << 20);
        assert_eq!(undo_bytes_for(Some(8 * GIB)), MAX_UNDO_BYTES);
        assert_eq!(undo_bytes_for(Some(64 * GIB)), MAX_UNDO_BYTES);
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
