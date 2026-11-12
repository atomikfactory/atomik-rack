//! The real models. They are not in the repository and the tests have no network: set
//! `ATOMIK_VISION_MODELS` to a folder that holds the two downloaded files to run these.
//! Without it every test here prints that it was skipped and passes.

use atomik_vision::download::{HAND_LANDMARKS, PALM_DETECTOR};
use atomik_vision::models::{FrameView, HandFinder, HandModels, HandPipeline, PipelineConfig};
use std::path::PathBuf;

fn models_dir() -> Option<PathBuf> {
    match std::env::var_os("ATOMIK_VISION_MODELS") {
        Some(d)
            if PathBuf::from(&d).join(PALM_DETECTOR.name).exists()
                && PathBuf::from(&d).join(HAND_LANDMARKS.name).exists() =>
        {
            Some(PathBuf::from(d))
        }
        _ => {
            println!(
                "skipped: set ATOMIK_VISION_MODELS to the folder of the downloaded hand models"
            );
            None
        }
    }
}

/// A smooth synthetic picture: no hand in it, but not blank either.
fn gradient(n: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(n * n * 3);
    for y in 0..n {
        for x in 0..n {
            v.push(x as f32 / n as f32);
            v.push(y as f32 / n as f32);
            v.push(((x + y) % 32) as f32 / 32.0);
        }
    }
    v
}

#[test]
fn the_palm_detector_has_the_expected_output_shapes_and_is_finite_and_repeatable() {
    let Some(dir) = models_dir() else { return };
    let models = HandModels::load(&dir).expect("the palm detector and landmark model load");
    let input = gradient(192);
    let (boxes, scores) = models.detect_raw(&input).unwrap();
    assert_eq!(boxes.len(), 2016 * 18);
    assert_eq!(scores.len(), 2016);
    assert!(boxes.iter().chain(&scores).all(|v| v.is_finite()));
    let (boxes2, scores2) = models.detect_raw(&input).unwrap();
    assert_eq!(
        boxes.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        boxes2.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(
        scores.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        scores2.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn the_landmark_model_has_the_expected_output_shapes_and_is_finite_and_repeatable() {
    let Some(dir) = models_dir() else { return };
    let models = HandModels::load(&dir).unwrap();
    let input = gradient(224);
    let a = models.landmarks_raw(&input).unwrap();
    assert_eq!(a.points.len(), 63);
    assert!(a.points.iter().all(|v| v.is_finite()));
    assert!((0.0..=1.0).contains(&a.presence) && (0.0..=1.0).contains(&a.right));
    let b = models.landmarks_raw(&input).unwrap();
    assert_eq!(
        a.points.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.points.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(a.presence.to_bits(), b.presence.to_bits());
    assert_eq!(a.right.to_bits(), b.right.to_bits());
}

#[test]
fn a_frame_without_a_hand_gives_no_hand_and_no_error() {
    let Some(dir) = models_dir() else { return };
    let mut pipeline =
        HandPipeline::new(HandModels::load(&dir).unwrap(), PipelineConfig::default());
    // A grey 640 by 480 frame with a little texture.
    let mut rgb = vec![0u8; 640 * 480 * 3];
    for (i, b) in rgb.iter_mut().enumerate() {
        *b = 90 + ((i / 3 % 640 + i / 1920) % 40) as u8;
    }
    for _ in 0..2 {
        let found = pipeline
            .process(FrameView {
                width: 640,
                height: 480,
                rgb: &rgb,
            })
            .unwrap();
        assert!(found.hands.is_empty());
        assert!(
            found.detect_time > std::time::Duration::ZERO,
            "no hand followed: the detector runs"
        );
    }
}

#[test]
fn per_frame_time_is_reported() {
    let Some(dir) = models_dir() else { return };
    let models = HandModels::load(&dir).unwrap();
    let detect = gradient(192);
    let landmarks = gradient(224);
    // Warm up, then time ten runs of each (release builds only mean anything).
    let _ = models.detect_raw(&detect).unwrap();
    let _ = models.landmarks_raw(&landmarks).unwrap();
    #[allow(clippy::disallowed_methods)]
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let _ = models.detect_raw(&detect).unwrap();
    }
    let palm = start.elapsed() / 10;
    #[allow(clippy::disallowed_methods)]
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let _ = models.landmarks_raw(&landmarks).unwrap();
    }
    let lm = start.elapsed() / 10;
    println!("palm detector {palm:?}, landmark model {lm:?} per run");
}

#[test]
fn the_pinned_hashes_match_the_official_files() {
    let Some(dir) = models_dir() else { return };
    let store = atomik_vision::download::ModelStore::with_files(
        dir,
        atomik_vision::download::MODEL_FILES.to_vec(),
    );
    assert_eq!(store.status(), atomik_vision::download::Status::Ready);
}

#[test]
fn a_frame_costs_what_the_detector_and_the_crop_cost() {
    let Some(dir) = models_dir() else { return };
    let mut pipeline =
        HandPipeline::new(HandModels::load(&dir).unwrap(), PipelineConfig::default());
    let rgb = vec![100u8; 640 * 480 * 3];
    let _ = pipeline
        .process(FrameView {
            width: 640,
            height: 480,
            rgb: &rgb,
        })
        .unwrap();
    #[allow(clippy::disallowed_methods)]
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let _ = pipeline
            .process(FrameView {
                width: 640,
                height: 480,
                rgb: &rgb,
            })
            .unwrap();
    }
    println!(
        "a 640x480 frame with no hand (detector runs): {:?} per frame",
        start.elapsed() / 10
    );
}
