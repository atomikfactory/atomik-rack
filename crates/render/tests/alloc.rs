//! On a real patch: rendering the built slice demo never allocates on the render path.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

#[test]
fn render_slice_demo_is_allocation_free() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let patch = Patch::from_json(&text).unwrap();
    for host_block in [64, 333] {
        let built = build(&patch, &atomik_render::default_registry()).unwrap();
        assert!(!built.notes.is_empty());
        let job = OfflineJob {
            frames: built.frames,
            host_block,
            notes: &built.notes,
            params: &[],
            inputs: &[],
        };
        let out = render_offline_with(built.plan, built.config, &job, |render| {
            assert_no_alloc(render)
        });
        assert_eq!(out.len(), 2);
        assert!(out.iter().flatten().any(|x| x.abs() > 1e-3));
    }
}
