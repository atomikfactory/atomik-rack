//! Shared helpers of the exchange tests: a parent port and a child port over two mappings of
//! one file (the same thing two processes do), with names no other test uses.
#![allow(dead_code)] // Each test binary uses a different part.

use atomik_core::{BATCH, Batch};
use atomik_plugin_ipc::{
    ChildPort, Doorbell, DoorbellName, ParentPort, SharedBlock, block_file_name, default_block_dir,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A unique instance number inside this process.
pub fn next_instance() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The block file and doorbell name of one test instance.
pub fn names() -> (PathBuf, DoorbellName) {
    let n = next_instance();
    let pid = std::process::id();
    (
        default_block_dir().join(block_file_name(pid, n)),
        DoorbellName::for_instance(pid, n),
    )
}

/// A parent and a child over one block, both in this process.
pub fn pair() -> (ParentPort, ChildPort) {
    let (path, name) = names();
    let parent_block = SharedBlock::create(&path).unwrap();
    let child_block = SharedBlock::open(&path).unwrap();
    let parent_bell = Doorbell::create(&name, &parent_block).unwrap();
    let child_bell = Doorbell::open(&name, &child_block).unwrap();
    (
        ParentPort::new(parent_block, parent_bell),
        ChildPort::new(child_block, child_bell),
    )
}

/// A recognisable ramp: sample `i` is `scale * (i + 1) / 100`.
pub fn ramp(scale: f32) -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 100.0;
    }
    b
}
