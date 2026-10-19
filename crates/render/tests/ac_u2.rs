//! Full chain: a v1 patch loaded by `atomik-model` and saved as v2 is accepted by the
//! renderer, and (with the v1 song data put back: length and notes, which are not part of the
//! rack document) renders bit-identically to the v1 file.

use std::num::NonZeroUsize;
use std::path::Path;

fn example(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/patches")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn block() -> NonZeroUsize {
    NonZeroUsize::new(512).unwrap()
}

#[test]
fn model_saved_v2_is_a_valid_render_input() {
    let registry = atomik_render::default_registry();
    for name in ["slice-demo.json", "minimal.json", "slice-demo.v2.json"] {
        let doc = atomik_model::load_patch(&example(name), &registry).unwrap();
        let saved = atomik_model::save_patch(&doc);
        let patch = atomik_render::patch::Patch::from_json(&saved)
            .unwrap_or_else(|e| panic!("{name}: {e}\n{saved}"));
        assert_eq!(patch.version, 2);
        atomik_render::build::build(&patch, &registry).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn v1_through_model_as_v2_renders_bit_identically() {
    let registry = atomik_render::default_registry();
    let v1_text = example("slice-demo.json");
    let v1: serde_json::Value = serde_json::from_str(&v1_text).unwrap();
    let doc = atomik_model::load_patch(&v1_text, &registry).unwrap();
    let mut v2: serde_json::Value = serde_json::from_str(&atomik_model::save_patch(&doc)).unwrap();
    v2["length_beats"] = v1["length_beats"].clone();
    v2["notes"] = v1["notes"].clone();
    let a = atomik_render::render_json(&v1_text, &registry, block()).unwrap();
    let b = atomik_render::render_json(&v2.to_string(), &registry, block()).unwrap();
    assert_eq!(a.channels.len(), b.channels.len());
    for (x, y) in a.channels.iter().zip(&b.channels) {
        assert!(
            x.iter()
                .map(|s| s.to_bits())
                .eq(y.iter().map(|s| s.to_bits())),
            "the model-saved v2 renders differently"
        );
    }
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
}
