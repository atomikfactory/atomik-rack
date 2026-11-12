//! The vocabulary shared by the models and the gesture recogniser: what one hand looks like to
//! the program (21 landmarks, which hand, how sure) and what one camera frame yielded.

use serde::{Deserialize, Serialize};

/// How many landmarks one hand has (the wrist, then four joints for each of the five fingers).
pub const LANDMARKS: usize = 21;

/// Indices into the 21 landmarks of a hand.
pub mod joint {
    /// The wrist.
    pub const WRIST: usize = 0;
    /// The tip of the thumb.
    pub const THUMB_TIP: usize = 4;
    /// The knuckle of the index finger.
    pub const INDEX_MCP: usize = 5;
    /// The tip of the index finger.
    pub const INDEX_TIP: usize = 8;
    /// The knuckle of the middle finger.
    pub const MIDDLE_MCP: usize = 9;
    /// The knuckle of the ring finger.
    pub const RING_MCP: usize = 13;
    /// The knuckle of the little finger.
    pub const PINKY_MCP: usize = 17;
}

/// One landmark: `x` and `y` are normalised image coordinates (0 to 1 from the left and from the
/// top of the frame), `z` is the model's relative depth (the wrist is about 0, nearer is
/// smaller), in the same scale as `x`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "[f32; 3]", into = "[f32; 3]")]
pub struct Point3 {
    /// From the left edge, 0 to 1.
    pub x: f32,
    /// From the top edge, 0 to 1.
    pub y: f32,
    /// Relative depth.
    pub z: f32,
}

impl Point3 {
    /// A point.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}

impl From<[f32; 3]> for Point3 {
    fn from(p: [f32; 3]) -> Self {
        Self::new(p[0], p[1], p[2])
    }
}

impl From<Point3> for [f32; 3] {
    fn from(p: Point3) -> Self {
        [p.x, p.y, p.z]
    }
}

/// Which hand the model says it is. The camera image is not mirrored, so this is the hand as the
/// model labels it; the program uses it only to tell two hands apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Handedness {
    /// The left hand.
    Left,
    /// The right hand.
    Right,
}

impl Handedness {
    /// One letter, `L` or `R`.
    pub fn letter(self) -> char {
        match self {
            Handedness::Left => 'L',
            Handedness::Right => 'R',
        }
    }
}

/// One hand in one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandObservation {
    /// The 21 landmarks.
    pub landmarks: [Point3; LANDMARKS],
    /// Which hand.
    pub handedness: Handedness,
    /// How sure the model is that this is a hand, 0 to 1.
    pub score: f32,
}

/// Everything one frame yielded, in the order the recogniser takes it. It holds no image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Seconds since the camera started (or any steady origin).
    pub t: f64,
    /// Frame width divided by frame height, so angles and lengths can be measured in one unit.
    pub aspect: f32,
    /// The hands found (none when the hand is out of view).
    pub hands: Vec<HandObservation>,
}
