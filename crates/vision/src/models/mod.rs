//! The hand models, run in pure Rust on the CPU.
//!
//! Two small neural networks (published by Google as `.tflite` files and read directly by
//! `tract`, no conversion) find hands: the palm detector looks at a whole frame, the landmark
//! model looks at a crop around one hand and gives its 21 points. Like the original pipeline, the
//! program runs the detector only when it has no hand: the landmarks of one frame frame the crop
//! of the next, so a tracked hand costs one run of the lighter model per frame.
//!
//! [`HandFinder`] is the seam the tracker uses; [`HandPipeline`] is the real one and tests use
//! fakes. The files are loaded from a folder ([`HandModels::load`]); the [download](crate::download)
//! module puts them there and checks them.

mod image;
mod landmark;
mod palm;

use crate::clock::now;
use crate::download::{HAND_LANDMARKS, PALM_DETECTOR};
use crate::hand::{HandObservation, Handedness, LANDMARKS};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tract_tflite::prelude::*;

pub use image::{Rgb, Roi};
pub use landmark::RawLandmarks;

/// Why the models could not be loaded or run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// A file could not be read or is not a model this program can run.
    #[error("cannot load {file}: {message}")]
    Load {
        /// The file.
        file: String,
        /// Why.
        message: String,
    },
    /// A run failed.
    #[error("the hand model failed: {0}")]
    Run(String),
    /// A model answered with a shape this program does not know.
    #[error("the hand model gave an unexpected output: {0}")]
    Output(String),
}

type Plan = Arc<TypedRunnableModel>;

fn load_plan(path: &Path) -> Result<Plan, ModelError> {
    let fail = |m: String| ModelError::Load {
        file: path.display().to_string(),
        message: m,
    };
    tract_tflite::tflite()
        .model_for_path(path)
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(|e| fail(format!("{e:#}")))
}

fn run_plan(plan: &Plan, side: usize, data: &[f32]) -> Result<TVec<TValue>, ModelError> {
    let input = Tensor::from_shape(&[1, side, side, 3], data)
        .map_err(|e| ModelError::Run(format!("{e:#}")))?;
    plan.run(tvec!(input.into()))
        .map_err(|e| ModelError::Run(format!("{e:#}")))
}

fn floats(v: &TValue, expected: usize, what: &str) -> Result<Vec<f32>, ModelError> {
    let view = v
        .to_plain_array_view::<f32>()
        .map_err(|e| ModelError::Output(format!("{what}: {e:#}")))?;
    if view.len() != expected {
        return Err(ModelError::Output(format!(
            "{what} has {} numbers, expected {expected}",
            view.len()
        )));
    }
    Ok(view.iter().copied().collect())
}

/// The two networks.
pub struct HandModels {
    palm: Plan,
    landmarks: Plan,
    anchors: Vec<(f32, f32)>,
}

impl std::fmt::Debug for HandModels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandModels").finish_non_exhaustive()
    }
}

impl HandModels {
    /// Loads the two files from `dir` (the names of [`MODEL_FILES`](crate::download::MODEL_FILES)).
    /// This does not check the files against their hashes: `ModelStore::verified_paths` in
    /// [`download`](crate::download) does, before the program calls this.
    pub fn load(dir: &Path) -> Result<Self, ModelError> {
        Ok(Self {
            palm: load_plan(&dir.join(PALM_DETECTOR.name))?,
            landmarks: load_plan(&dir.join(HAND_LANDMARKS.name))?,
            anchors: palm::anchors(),
        })
    }

    /// One run of the palm detector on a `192 * 192 * 3` input (values 0 to 1, rows of RGB):
    /// the raw boxes (`2016 * 18` numbers) and raw scores (2016).
    pub fn detect_raw(&self, input: &[f32]) -> Result<(Vec<f32>, Vec<f32>), ModelError> {
        let out = run_plan(&self.palm, palm::INPUT, input)?;
        if out.len() != 2 {
            return Err(ModelError::Output(format!(
                "the palm detector has {} outputs",
                out.len()
            )));
        }
        let boxes = floats(&out[0], palm::ANCHORS * palm::COORDS, "palm boxes")?;
        let scores = floats(&out[1], palm::ANCHORS, "palm scores")?;
        Ok((boxes, scores))
    }

    /// One run of the landmark model on a `224 * 224 * 3` input.
    pub fn landmarks_raw(&self, input: &[f32]) -> Result<RawLandmarks, ModelError> {
        let out = run_plan(&self.landmarks, landmark::INPUT, input)?;
        if out.len() < 3 {
            return Err(ModelError::Output(format!(
                "the landmark model has {} outputs",
                out.len()
            )));
        }
        let points = floats(&out[0], LANDMARKS * 3, "landmarks")?;
        let presence = floats(&out[1], 1, "hand presence")?[0];
        let right = floats(&out[2], 1, "handedness")?[0];
        // The model gives probabilities; accept a logit too.
        let prob = |x: f32| {
            if (0.0..=1.0).contains(&x) {
                x
            } else {
                1.0 / (1.0 + (-x).exp())
            }
        };
        Ok(RawLandmarks {
            points,
            presence: prob(presence),
            right: prob(right),
        })
    }
}

/// A frame to look at.
#[derive(Debug, Clone, Copy)]
pub struct FrameView<'a> {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height * 3` bytes, RGB.
    pub rgb: &'a [u8],
}

/// What a frame gave and what it cost.
#[derive(Debug, Clone, Default)]
pub struct FrameHands {
    /// The hands found.
    pub hands: Vec<HandObservation>,
    /// Time in the palm detector (zero when it did not run).
    pub detect_time: Duration,
    /// Time in the landmark model.
    pub landmark_time: Duration,
}

/// Something that finds hands in frames: the real models, or a fake in tests.
pub trait HandFinder: Send {
    /// Looks at one frame.
    fn process(&mut self, frame: FrameView<'_>) -> Result<FrameHands, ModelError>;
    /// Forgets the hands being followed (the camera changed).
    fn reset(&mut self);
}

/// The numbers of the pipeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipelineConfig {
    /// How many hands to follow (each costs one run of the landmark model per frame).
    pub max_hands: usize,
    /// The detector's score needed to start following a hand.
    pub detect_threshold: f32,
    /// The landmark model's presence needed to keep a hand.
    pub presence_threshold: f32,
    /// While fewer hands than `max_hands` are followed (and at least one), the detector runs
    /// once in this many frames to look for the others; with none followed it runs every frame.
    pub redetect_every: u32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_hands: 1,
            detect_threshold: 0.5,
            presence_threshold: 0.5,
            redetect_every: 8,
        }
    }
}

/// The real hand finder: the detector when no hand is followed, the landmark model on a window
/// that follows each hand.
pub struct HandPipeline {
    models: HandModels,
    cfg: PipelineConfig,
    rois: Vec<Roi>,
    frames: u32,
    scratch: Vec<f32>,
}

impl HandPipeline {
    /// A pipeline that follows no hand yet.
    pub fn new(models: HandModels, cfg: PipelineConfig) -> Self {
        Self {
            models,
            cfg,
            rois: Vec::new(),
            frames: 0,
            scratch: Vec::new(),
        }
    }

    fn run_landmarks(
        &mut self,
        img: &Rgb<'_>,
        roi: &Roi,
        time: &mut Duration,
    ) -> Result<Option<(HandObservation, Roi)>, ModelError> {
        image::crop(img, roi, landmark::INPUT, &mut self.scratch);
        let start = now();
        let raw = self.models.landmarks_raw(&self.scratch)?;
        *time += start.elapsed();
        if raw.presence < self.cfg.presence_threshold {
            return Ok(None);
        }
        let (w, h) = (img.width as f32, img.height as f32);
        let points = landmark::project(&raw.points, roi, w, h);
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Ok(None);
        }
        let next = landmark::next_roi(&points, w, h);
        let handedness = if raw.right >= 0.5 {
            Handedness::Right
        } else {
            Handedness::Left
        };
        Ok(Some((
            HandObservation {
                landmarks: points,
                handedness,
                score: raw.presence,
            },
            next,
        )))
    }

    /// The palm centre of a hand, in pixels.
    fn palm(h: &HandObservation, w: f32, ht: f32) -> (f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        for i in [0, 5, 9, 13, 17] {
            x += h.landmarks[i].x / 5.0 * w;
            y += h.landmarks[i].y / 5.0 * ht;
        }
        (x, y)
    }
}

impl HandFinder for HandPipeline {
    fn process(&mut self, frame: FrameView<'_>) -> Result<FrameHands, ModelError> {
        let img = Rgb {
            width: frame.width,
            height: frame.height,
            data: frame.rgb,
        };
        let (w, h) = (frame.width as f32, frame.height as f32);
        let mut result = FrameHands::default();
        self.frames = self.frames.wrapping_add(1);

        // The hands followed from the last frame.
        let mut next: Vec<Roi> = Vec::new();
        let rois = std::mem::take(&mut self.rois);
        for roi in rois.iter().take(self.cfg.max_hands) {
            if let Some((hand, roi)) = self.run_landmarks(&img, roi, &mut result.landmark_time)? {
                // Two windows that settled on one hand count once.
                let (px, py) = Self::palm(&hand, w, h);
                let twin = result.hands.iter().any(|o| {
                    let (ox, oy) = Self::palm(o, w, h);
                    (ox - px).hypot(oy - py) < roi.size * 0.25
                });
                if !twin {
                    result.hands.push(hand);
                    next.push(roi);
                }
            }
        }

        // Look for more when fewer than wanted are followed.
        let want_more = result.hands.len() < self.cfg.max_hands;
        let due =
            result.hands.is_empty() || self.frames.is_multiple_of(self.cfg.redetect_every.max(1));
        if want_more && due {
            let square = w.max(h);
            let letterbox = Roi {
                cx: w / 2.0,
                cy: h / 2.0,
                size: square,
                rotation: 0.0,
            };
            image::crop(&img, &letterbox, palm::INPUT, &mut self.scratch);
            let start = now();
            let (boxes, scores) = self.models.detect_raw(&self.scratch)?;
            result.detect_time = start.elapsed();
            let found = palm::suppress(
                palm::decode(
                    &boxes,
                    &scores,
                    &self.models.anchors,
                    self.cfg.detect_threshold,
                ),
                0.3,
            );
            for d in found {
                if result.hands.len() >= self.cfg.max_hands {
                    break;
                }
                let roi = palm::roi_of(&d, square, w, h);
                // A hand already followed is not found twice.
                let known = result.hands.iter().any(|o| {
                    let (ox, oy) = Self::palm(o, w, h);
                    (ox - roi.cx).hypot(oy - roi.cy) < roi.size * 0.5
                });
                if known {
                    continue;
                }
                if let Some((hand, roi)) =
                    self.run_landmarks(&img, &roi, &mut result.landmark_time)?
                {
                    result.hands.push(hand);
                    next.push(roi);
                }
            }
        }
        self.rois = next;
        Ok(result)
    }

    fn reset(&mut self) {
        self.rois.clear();
        self.frames = 0;
    }
}
