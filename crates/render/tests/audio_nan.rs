//! A bundle file with NaN or infinite samples (a file placed by hand; this program never writes
//! one) bounces to finite audio, and the bounce says how many samples it read as silence.

use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects")
}

fn options() -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

/// Copies the audio-demo project and its bundle into a fresh folder.
fn copy_demo(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("atomik-render-nan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let audio = dir.join("audio-demo/Audio");
    std::fs::create_dir_all(&audio).unwrap();
    std::fs::copy(
        examples().join("audio-demo.atomik"),
        dir.join("audio-demo.atomik"),
    )
    .unwrap();
    let mut wav = None;
    for entry in std::fs::read_dir(examples().join("audio-demo/Audio")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "wav") {
            let to = audio.join(path.file_name().unwrap());
            std::fs::copy(&path, &to).unwrap();
            wav = Some(to);
        }
    }
    (dir.join("audio-demo.atomik"), wav.expect("the demo asset"))
}

/// Offset of the first sample of a RIFF file written by this program.
fn data_start(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("a data chunk")
        + 8
}

#[test]
fn a_bundle_file_with_non_finite_samples_bounces_finite_and_reports_the_count() {
    let registry = default_registry();
    let (project, wav) = copy_demo("poisoned");

    let clean = render_project_file(&project, &registry, &options()).unwrap();
    assert!(
        clean
            .warnings
            .iter()
            .all(|w| !w.path.starts_with("assets."))
    );

    // Poison 11 samples of the asset: ten NaNs and an infinity.
    let mut bytes = std::fs::read(&wav).unwrap();
    let start = data_start(&bytes);
    for i in 0..10 {
        let at = start + (100 + i) * 4;
        bytes[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    }
    let at = start + 5000 * 4;
    bytes[at..at + 4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    std::fs::write(&wav, &bytes).unwrap();

    let r = render_project_file(&project, &registry, &options()).unwrap();
    assert!(
        r.channels.iter().flatten().all(|s| s.is_finite()),
        "the bounce is finite"
    );
    let reported: Vec<String> = r
        .warnings
        .iter()
        .filter(|w| w.path.starts_with("assets."))
        .map(ToString::to_string)
        .collect();
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(
        reported[0].contains("11 NaN or infinite samples"),
        "{reported:?}"
    );
    std::fs::remove_dir_all(project.parent().unwrap()).unwrap();
}
