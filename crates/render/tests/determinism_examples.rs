//! Every example project bounces to the same bits at host blocks 64, 333 and 512 with 0 and 2
//! worker threads: the automated form of the block-and-worker step of the QA recipes. The plugin
//! demos are left out (they need a plugin installed).

use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

fn examples() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "atomik")
                && !path.to_string_lossy().contains("plugin-demo")
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects"),
        &mut files,
    );
    files.sort();
    files
}

fn bits(path: &Path, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let rendered = render_project_file(path, &default_registry(), &options)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    rendered
        .channels
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect()
}

#[test]
fn every_example_project_is_bit_identical_across_blocks_and_workers() {
    let files = examples();
    assert!(files.len() >= 10, "the examples are there: {files:?}");
    for path in files {
        let reference = bits(&path, 512, 0);
        assert!(
            !reference.is_empty(),
            "{}: renders something",
            path.display()
        );
        for block in [64, 333, 512] {
            for workers in [0, 2] {
                if (block, workers) == (512, 0) {
                    continue;
                }
                assert!(
                    bits(&path, block, workers) == reference,
                    "{}: block {block}, workers {workers} differ from block 512, workers 0",
                    path.display()
                );
            }
        }
    }
}
