//! Twist calibration: measures how far this user's wrist turns and how much a still hand shakes
//! in this camera, and sets the twist settings from that.
//!
//! The user pinches and keeps pinching through three steps, each told in the preview box and
//! the status line:
//!
//! 1. **Hold still** for [`STILL_SECS`]: the spread of the twist angle is the jitter of this
//!    hand in this light, and sets the dead zone (`0.75 × jitter + 0.3°`, within the dead-zone
//!    limits) so a still hand does not drift and a steady one stays sharp.
//! 2. **Twist right** as far as is comfortable and hold: a step ends when the angle has stayed
//!    within [`HOLD_SPREAD`] (or twice the jitter) for [`HOLD_SECS`], at least [`MIN_TWIST`] from
//!    the start.
//! 3. **Twist left** the same way, at least `2 × MIN_TWIST` from the right end.
//!
//! The span from the left end to the right end becomes the twist for the full range (within the
//! full-turn limits), and a right twist that reads as a left one sets `reverse`. Letting go in the
//! middle starts again from step 1; [`TIMEOUT_SECS`] without finishing changes nothing. While it
//! runs a pinch turns no knob. The angle is the tracker's twist since the pinch began, before
//! the `reverse` setting, so a calibration never depends on the one before it.

use super::*;
use crate::keymap_store::{MAX_DEAD_ZONE, MAX_FULL_TURN, MIN_DEAD_ZONE, MIN_FULL_TURN};

/// How long the hand is held still, in seconds (after [`SETTLE_SECS`]).
pub const STILL_SECS: f64 = 1.5;
/// What is not measured at the start of a pinch while the filter settles, in seconds.
pub const SETTLE_SECS: f64 = 0.3;
/// How long an end of the twist is held, in seconds.
pub const HOLD_SECS: f64 = 0.8;
/// The most the angle may wander while an end is held, in degrees (or twice the jitter).
pub const HOLD_SPREAD: f32 = 2.0;
/// The least twist from the start that counts as the right end, in degrees.
pub const MIN_TWIST: f32 = 10.0;
/// A calibration that has not finished after this many seconds stops and changes nothing.
pub const TIMEOUT_SECS: f64 = 30.0;

/// Step 1.
pub const STILL_MESSAGE: &str = "Calibration 1/3: pinch and hold your hand still";
/// Step 2.
pub const RIGHT_MESSAGE: &str =
    "Calibration 2/3: keep pinching, twist right as far as is comfortable and hold";
/// Step 3.
pub const LEFT_MESSAGE: &str =
    "Calibration 3/3: keep pinching, twist left as far as is comfortable and hold";
/// The pinch ended before the end.
pub const AGAIN_MESSAGE: &str = "Keep pinching until the end: calibration starts again";
/// Too slow.
pub const TIMEOUT_MESSAGE: &str = "Calibration timed out: nothing changed";
/// Stopped by the user (the command again) or by the camera going off.
pub const CANCEL_MESSAGE: &str = "Calibration cancelled: nothing changed";

/// The step a calibration is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationStep {
    /// Hold the pinched hand still.
    Still,
    /// Twist right and hold.
    Right,
    /// Twist left and hold.
    Left,
}

impl CalibrationStep {
    fn message(self) -> &'static str {
        match self {
            CalibrationStep::Still => STILL_MESSAGE,
            CalibrationStep::Right => RIGHT_MESSAGE,
            CalibrationStep::Left => LEFT_MESSAGE,
        }
    }
}

/// What a calibration measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationResult {
    /// The spread of a still hand's twist, in degrees.
    pub jitter: f32,
    /// The right end, in degrees from where the pinch began.
    pub right: f32,
    /// The left end, in degrees from where the pinch began.
    pub left: f32,
}

impl CalibrationResult {
    /// The twist for the full range: the span between the ends, in whole degrees.
    pub fn full_turn(&self) -> u16 {
        ((self.right - self.left).abs().round() as u16).clamp(MIN_FULL_TURN, MAX_FULL_TURN)
    }

    /// The dead zone, in tenths of a degree.
    pub fn dead_zone(&self) -> u16 {
        (((0.75 * self.jitter + 0.3) * 10.0).round() as u16).clamp(MIN_DEAD_ZONE, MAX_DEAD_ZONE)
    }

    /// Whether a right twist read as a left one.
    pub fn reverse(&self) -> bool {
        self.right < self.left
    }
}

/// A calibration in progress.
#[derive(Debug, Clone)]
pub(super) struct Calibration {
    step: CalibrationStep,
    /// The hand being measured (the first one pinched).
    hand: Option<HandId>,
    /// When the pinch of this run began (frame time).
    pinched_at: f64,
    /// When the calibration saw its first frame.
    began: Option<f64>,
    /// `(time, degrees)` of the current step.
    samples: Vec<(f64, f32)>,
    jitter: f32,
    right: f32,
    /// The preview switch before the calibration (it is on while one runs).
    preview_was: bool,
}

impl Calibration {
    fn restart(&mut self) {
        self.step = CalibrationStep::Still;
        self.hand = None;
        self.samples.clear();
    }

    /// The angle held at the end of the samples: `Some(mean)` when the last [`HOLD_SECS`] stayed
    /// within the spread allowed.
    fn held(&self, now: f64) -> Option<f32> {
        let first = self.samples.first()?.0;
        if now - first < HOLD_SECS {
            return None;
        }
        let window: Vec<f32> = self
            .samples
            .iter()
            .filter(|(t, _)| *t >= now - HOLD_SECS)
            .map(|(_, a)| *a)
            .collect();
        if window.len() < 2 {
            return None;
        }
        let (lo, hi) = spread(&window);
        (hi - lo <= HOLD_SPREAD.max(2.0 * self.jitter))
            .then(|| window.iter().sum::<f32>() / window.len() as f32)
    }
}

fn spread(values: &[f32]) -> (f32, f32) {
    values
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        })
}

impl<S: EngineSink> AppCore<S> {
    /// The `camera.calibrate` command: starts a twist calibration (turning the camera on), or
    /// cancels the one running.
    pub fn camera_calibrate(&mut self) {
        if self.vision.calibration.is_some() {
            self.camera_calibration_end(Some(CANCEL_MESSAGE));
            return;
        }
        self.end_pinch();
        let preview_was = self.vision.preview_on;
        self.vision.calibration = Some(Calibration {
            step: CalibrationStep::Still,
            hand: None,
            pinched_at: 0.0,
            began: None,
            samples: Vec::new(),
            jitter: 0.0,
            right: 0.0,
            preview_was,
        });
        if !self.vision.preview_on {
            self.vision.preview_on = true;
            self.sync_camera_preview();
        }
        self.camera_on();
        self.vision.message = Some(STILL_MESSAGE.to_owned());
        self.report(STILL_MESSAGE);
    }

    /// The step of the calibration running, if one is.
    pub fn camera_calibration_step(&self) -> Option<CalibrationStep> {
        self.vision.calibration.as_ref().map(|c| c.step)
    }

    /// The settings a calibration just made, once: the shell saves them to `workflow.json`.
    pub fn take_camera_calibrated(&mut self) -> Option<CameraPrefs> {
        self.vision.calibrated.take()
    }

    /// Ends the calibration running (nothing changes), saying `why`.
    pub(super) fn camera_calibration_end(&mut self, why: Option<&str>) {
        let Some(cal) = self.vision.calibration.take() else {
            return;
        };
        if self.vision.preview_on != cal.preview_was {
            self.vision.preview_on = cal.preview_was;
            self.sync_camera_preview();
        }
        if let Some(why) = why {
            self.vision.message = Some(why.to_owned());
            self.report(why);
        }
    }

    /// One frame of the tracker during a calibration.
    pub(super) fn camera_calibration_frame(&mut self, report: &FrameReport) {
        let allowed = self.vision.prefs.hand;
        let Some(cal) = &mut self.vision.calibration else {
            return;
        };
        let now = report.t;
        let began = *cal.began.get_or_insert(now);
        if now - began > TIMEOUT_SECS {
            self.camera_calibration_end(Some(TIMEOUT_MESSAGE));
            return;
        }
        let pinched = |h: &&HandState| h.present && h.pinch;
        let hand = match cal.hand {
            Some(id) => report.hands.iter().find(|h| h.id == id).filter(pinched),
            None => report
                .hands
                .iter()
                .filter(pinched)
                .find(|h| allowed.allows(h.handedness.letter())),
        };
        let Some(hand) = hand else {
            if cal.hand.is_some() {
                // Let go in the middle: from the start again.
                cal.restart();
                self.vision.message = Some(AGAIN_MESSAGE.to_owned());
                self.report(AGAIN_MESSAGE);
            }
            return;
        };
        if cal.hand.is_none() {
            cal.hand = Some(hand.id);
            cal.pinched_at = now;
            self.vision.message = Some(STILL_MESSAGE.to_owned());
        }
        if now - cal.pinched_at < SETTLE_SECS {
            return;
        }
        let angle = hand.twist_angle.to_degrees();
        cal.samples.push((now, angle));
        let next = match cal.step {
            CalibrationStep::Still => {
                let first = cal.samples[0].0;
                if now - first < STILL_SECS {
                    return;
                }
                let angles: Vec<f32> = cal.samples.iter().map(|(_, a)| *a).collect();
                let (lo, hi) = spread(&angles);
                cal.jitter = hi - lo;
                CalibrationStep::Right
            }
            CalibrationStep::Right => match cal.held(now) {
                Some(right) if right.abs() >= MIN_TWIST => {
                    cal.right = right;
                    CalibrationStep::Left
                }
                _ => return,
            },
            CalibrationStep::Left => match cal.held(now) {
                Some(left)
                    if (left - cal.right).abs() >= 2.0 * MIN_TWIST
                        && (left - cal.right).signum() == -cal.right.signum() =>
                {
                    let result = CalibrationResult {
                        jitter: cal.jitter,
                        right: cal.right,
                        left,
                    };
                    self.camera_calibration_done(result);
                    return;
                }
                _ => return,
            },
        };
        cal.step = next;
        cal.samples.clear();
        let text = next.message();
        self.vision.message = Some(text.to_owned());
        self.report(text);
    }

    fn camera_calibration_done(&mut self, result: CalibrationResult) {
        let mut prefs = self.vision.prefs.clone();
        prefs.full_turn = result.full_turn();
        prefs.dead_zone = result.dead_zone();
        prefs.reverse = result.reverse();
        let text = format!(
            "Calibration done: {turn}\u{b0} for the full range, dead zone {dead}\u{b0}",
            turn = prefs.full_turn,
            dead = format_tenths(prefs.dead_zone),
        );
        // The preview goes back first, so the prefs below keep the user's preview switch.
        self.camera_calibration_end(None);
        prefs.preview = self.vision.prefs.preview;
        self.set_camera_prefs(prefs.clone());
        self.vision.calibrated = Some(prefs);
        self.vision.message = Some(text.clone());
        self.report(text);
    }
}

/// Tenths of a degree as text: `15` is `1.5`.
pub fn format_tenths(tenths: u16) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}
