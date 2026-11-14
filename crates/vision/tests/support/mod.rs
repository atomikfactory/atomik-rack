//! A parametric hand for the tests: no camera and no recording, just geometry. A hand is built
//! upright in its own units (the wrist to the middle knuckle is 1), turned, scaled, moved and
//! given a little noise. The same generator writes the fixtures in `tests/fixtures`.
#![allow(dead_code, missing_docs)]

use atomik_vision::{HandObservation, Handedness, LANDMARKS, Observation, Point3};
use std::f32::consts::{FRAC_PI_2, PI};

pub const FPS: f64 = 30.0;
pub const ASPECT: f32 = 4.0 / 3.0;

/// The upright open hand in hand units: x to the right, y up.
const BASE: [(f32, f32); LANDMARKS] = [
    (0.0, 0.0),    // wrist
    (-0.25, 0.25), // thumb cmc
    (-0.45, 0.45), // thumb mcp
    (-0.62, 0.65), // thumb ip
    (-0.8, 0.8),   // thumb tip (replaced when pinching)
    (-0.3, 0.95),  // index mcp
    (-0.34, 1.3),  // index pip
    (-0.36, 1.55), // index dip
    (-0.38, 1.75), // index tip
    (0.0, 1.0),    // middle mcp
    (0.0, 1.4),
    (0.0, 1.68),
    (0.0, 1.9),
    (0.27, 0.93), // ring mcp
    (0.3, 1.28),
    (0.32, 1.52),
    (0.33, 1.7),
    (0.52, 0.8), // pinky mcp
    (0.58, 1.05),
    (0.61, 1.22),
    (0.63, 1.35),
];

/// A deterministic noise source (a small xorshift), so fixtures never change by themselves.
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    /// A number in -1 to 1.
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) * 2.0 - 1.0
    }
}

/// One hand to draw.
#[derive(Clone, Copy)]
pub struct Spec {
    pub handedness: Handedness,
    /// The palm in normalised image coordinates.
    pub center: (f32, f32),
    /// Wrist to middle knuckle, in frame heights.
    pub size: f32,
    /// The turn, counter-clockwise in the image positive (the twist of the recogniser).
    pub angle: f32,
    /// Thumb tip to index tip over the hand size.
    pub pinch_ratio: f32,
    pub score: f32,
    /// Noise on every landmark, in frame heights.
    pub noise: f32,
    /// How far the fingers are folded into the palm: 0 open, 1 a fist.
    pub curl: f32,
    /// Which of the index, middle, ring and little fingers the curl folds (all by default).
    pub folded: [bool; 4],
    /// The thumb stands up above the fist.
    pub thumb_up: bool,
}

impl Spec {
    pub fn right() -> Self {
        Self {
            handedness: Handedness::Right,
            center: (0.5, 0.55),
            size: 0.3,
            angle: 0.0,
            pinch_ratio: 1.0,
            score: 0.95,
            noise: 0.0,
            curl: 0.0,
            folded: [true; 4],
            thumb_up: false,
        }
    }
    pub fn left() -> Self {
        Self {
            handedness: Handedness::Left,
            ..Self::right()
        }
    }
}

fn round(v: f32) -> f32 {
    (v * 1e5).round() / 1e5
}

/// The landmarks of one hand.
pub fn hand(spec: &Spec, noise: &mut Noise) -> HandObservation {
    let mut pts = BASE;
    // The thumb tip sits `pinch_ratio` hand sizes from the index tip, on the line to the open
    // thumb tip.
    let (ix, iy) = BASE[8];
    let (tx, ty) = BASE[4];
    let (dx, dy) = (tx - ix, ty - iy);
    let len = (dx * dx + dy * dy).sqrt();
    pts[4] = (
        ix + dx / len * spec.pinch_ratio,
        iy + dy / len * spec.pinch_ratio,
    );
    // A curled finger folds its three upper joints back towards the wrist, the tip furthest.
    if spec.curl > 0.0 {
        for (n, mcp) in [5usize, 9, 13, 17].into_iter().enumerate() {
            if !spec.folded[n] {
                continue;
            }
            let (mx, my) = BASE[mcp];
            for (joint, back) in [(mcp + 1, 0.0), (mcp + 2, 0.2), (mcp + 3, 0.45)] {
                let folded = (mx, my * (1.0 - back) - 0.05 + 0.1 * back);
                let open = BASE[joint];
                pts[joint] = (
                    open.0 + (folded.0 - open.0) * spec.curl,
                    open.1 + (folded.1 - open.1) * spec.curl,
                );
            }
        }
    }
    // A thumb up: the thumb stands straight above the knuckles.
    if spec.thumb_up {
        pts[2] = (-0.4, 0.5);
        pts[3] = (-0.45, 1.0);
        pts[4] = (-0.5, 1.5);
    }
    // Where the palm is: the mean of the wrist and the four knuckles.
    let (mut px, mut py) = (0.0, 0.0);
    for i in [0, 5, 9, 13, 17] {
        px += pts[i].0 / 5.0;
        py += pts[i].1 / 5.0;
    }
    let (s, c) = spec.angle.sin_cos();
    let mut landmarks = [Point3::default(); LANDMARKS];
    for (i, (x, y)) in pts.iter().enumerate() {
        let (x, y) = (x - px, y - py);
        let (rx, ry) = (x * c - y * s, x * s + y * c);
        let nx = noise.next() * spec.noise;
        let ny = noise.next() * spec.noise;
        landmarks[i] = Point3::new(
            round(spec.center.0 + rx * spec.size / ASPECT + nx / ASPECT),
            round(spec.center.1 - ry * spec.size + ny),
            0.0,
        );
    }
    HandObservation {
        landmarks,
        handedness: spec.handedness,
        score: spec.score,
    }
}

/// A sequence of frames at 30 fps: `build(frame, t)` says which hands are in view.
pub fn sequence(frames: usize, mut build: impl FnMut(usize, f64) -> Vec<Spec>) -> Vec<Observation> {
    let mut noise = Noise::new(0x9E37_79B9_7F4A_7C15);
    (0..frames)
        .map(|i| {
            let t = i as f64 / FPS;
            let hands = build(i, t).iter().map(|s| hand(s, &mut noise)).collect();
            Observation {
                t,
                aspect: ASPECT,
                hands,
            }
        })
        .collect()
}

pub fn lerp(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f.clamp(0.0, 1.0)
}

/// Open, pinch, hold, turn 90 degrees, hold, open: the whole gesture.
pub fn pinch_twist() -> Vec<Observation> {
    sequence(90, |_, t| {
        let t = t as f32;
        let ratio = if t < 0.5 {
            1.0
        } else if t < 0.7 {
            lerp(1.0, 0.05, (t - 0.5) / 0.2)
        } else if t < 2.5 {
            0.05
        } else if t < 2.7 {
            lerp(0.05, 1.0, (t - 2.5) / 0.2)
        } else {
            1.0
        };
        let angle = lerp(0.0, FRAC_PI_2, (t - 1.0) / 1.0);
        vec![Spec {
            pinch_ratio: ratio,
            angle,
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// A pinched hand held still with the jitter of a real model.
pub fn still_jitter() -> Vec<Observation> {
    sequence(90, |_, _| {
        vec![Spec {
            pinch_ratio: 0.08,
            angle: 0.3,
            noise: 0.0015,
            ..Spec::right()
        }]
    })
}

/// A pinched hand turned across the jump of the angle at 180 degrees. Upright is 90 degrees
/// of image angle, so a twist of 75 to 105 degrees goes from 165 to 195.
pub fn cross_180(forward: bool) -> Vec<Observation> {
    let (from, to) = (75f32.to_radians(), 105f32.to_radians());
    sequence(60, |_, t| {
        let f = (t as f32 - 0.3) / 1.0;
        let angle = if forward {
            lerp(from, to, f)
        } else {
            lerp(to, from, f)
        };
        vec![Spec {
            pinch_ratio: 0.08,
            angle,
            noise: 0.0005,
            ..Spec::right()
        }]
    })
}

/// The thumb and index finger hover between the two thresholds.
pub fn threshold_hover() -> Vec<Observation> {
    sequence(100, |_, t| {
        let t = t as f32;
        let wobble = 0.5 + 0.5 * (t * 2.0 * PI * 1.3).sin();
        let ratio = if t < 1.0 {
            lerp(0.28, 0.39, wobble) // never reaches the start threshold
        } else if t < 1.2 {
            0.1
        } else if t < 2.4 {
            lerp(0.27, 0.39, wobble) // never reaches the end threshold
        } else if t < 2.6 {
            0.7
        } else {
            1.0
        };
        vec![Spec {
            pinch_ratio: ratio,
            noise: 0.0004,
            ..Spec::right()
        }]
    })
}

/// Two hands that pinch at different times and turn in opposite directions; the order in which
/// they are listed changes between frames.
pub fn two_hands() -> Vec<Observation> {
    sequence(90, |i, t| {
        let t = t as f32;
        let left = Spec {
            center: (0.28, 0.55),
            pinch_ratio: if (0.4..2.4).contains(&t) { 0.06 } else { 1.0 },
            angle: lerp(0.0, 40f32.to_radians(), (t - 0.8) / 1.0),
            noise: 0.0006,
            ..Spec::left()
        };
        let right = Spec {
            center: (0.72, 0.5),
            pinch_ratio: if (0.9..2.6).contains(&t) { 0.06 } else { 1.0 },
            angle: lerp(0.0, -60f32.to_radians(), (t - 1.2) / 1.0),
            noise: 0.0006,
            ..Spec::right()
        };
        if i % 3 == 0 {
            vec![right, left]
        } else {
            vec![left, right]
        }
    })
}

/// A fist, then an open palm held for a while, then a fist again.
pub fn open_palm() -> Vec<Observation> {
    sequence(90, |_, t| {
        let open = (0.4..1.8).contains(&t);
        vec![Spec {
            curl: if open { 0.0 } else { 1.0 },
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// The palm opens for 200 ms only: a flash that must not count.
pub fn open_flash() -> Vec<Observation> {
    sequence(60, |_, t| {
        let open = (0.4..0.6).contains(&t);
        vec![Spec {
            curl: if open { 0.0 } else { 1.0 },
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// A fist held, then the hand opens.
pub fn fist() -> Vec<Observation> {
    sequence(60, |_, t| {
        vec![Spec {
            curl: if t < 1.2 { 1.0 } else { 0.0 },
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// A thumbs up held, then the fingers open.
pub fn thumbs_up() -> Vec<Observation> {
    sequence(60, |_, t| {
        let up = t < 1.2;
        vec![Spec {
            curl: if up { 1.0 } else { 0.0 },
            thumb_up: up,
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// The index finger up (the others folded), held, then the hand opens.
pub fn point() -> Vec<Observation> {
    sequence(60, |_, t| {
        vec![Spec {
            curl: if t < 1.2 { 1.0 } else { 0.0 },
            folded: [false, true, true, true],
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// The index and middle fingers up (the others folded), held, then the hand opens.
pub fn peace() -> Vec<Observation> {
    sequence(60, |_, t| {
        vec![Spec {
            curl: if t < 1.2 { 1.0 } else { 0.0 },
            folded: [false, false, true, true],
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// An open hand that waits, then sweeps across the frame in 300 ms (`to_left`: the palm goes
/// to the left of the raw image), then rests.
pub fn swipe(to_left: bool) -> Vec<Observation> {
    sequence(60, |_, t| {
        let t = t as f32;
        let (a, b) = if to_left { (0.8, 0.25) } else { (0.25, 0.8) };
        let f = (t - 0.5) / 0.3;
        vec![Spec {
            center: (lerp(a, b, f), 0.55),
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// The same sweep, but over two seconds: a drift, not a swipe.
pub fn drift() -> Vec<Observation> {
    sequence(90, |_, t| {
        let f = (t as f32 - 0.2) / 2.4;
        vec![Spec {
            center: (lerp(0.8, 0.25, f), 0.55),
            noise: 0.0008,
            ..Spec::right()
        }]
    })
}

/// Both hands pinch (the left at 0.3 s, the right at 0.5 s), move apart and up
/// between 1.0 and 1.6 s while pinched, and open at 2 s.
pub fn two_hand_pad() -> Vec<Observation> {
    sequence(90, |_, t| {
        let t = t as f32;
        let f = (t - 1.0) / 0.6;
        let left = Spec {
            center: (lerp(0.4, 0.2, f), lerp(0.6, 0.4, f)),
            pinch_ratio: if (0.3..2.0).contains(&t) { 0.06 } else { 1.0 },
            noise: 0.0004,
            ..Spec::left()
        };
        let right = Spec {
            center: (lerp(0.6, 0.8, f), lerp(0.6, 0.4, f)),
            pinch_ratio: if (0.5..2.0).contains(&t) { 0.06 } else { 1.0 },
            noise: 0.0004,
            ..Spec::right()
        };
        vec![left, right]
    })
}

/// Every fixture: file name and frames.
pub fn fixtures() -> Vec<(&'static str, Vec<Observation>)> {
    vec![
        ("pinch_twist.json", pinch_twist()),
        ("still_jitter.json", still_jitter()),
        ("cross_180.json", cross_180(true)),
        ("threshold_hover.json", threshold_hover()),
        ("two_hands.json", two_hands()),
        ("open_palm.json", open_palm()),
        ("open_flash.json", open_flash()),
        ("fist.json", fist()),
        ("thumbs_up.json", thumbs_up()),
        ("point.json", point()),
        ("peace.json", peace()),
        ("swipe_left.json", swipe(true)),
        ("swipe_right.json", swipe(false)),
        ("drift.json", drift()),
        ("two_hand_pad.json", two_hand_pad()),
    ]
}

pub fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Loads a fixture; with `ATOMIK_UPDATE_GOLDEN=1` the checked-in file is rewritten first.
pub fn load(name: &str) -> Vec<Observation> {
    let path = fixture_dir().join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        let frames = fixtures()
            .into_iter()
            .find(|f| f.0 == name)
            .expect("a known fixture")
            .1;
        std::fs::write(&path, serde_json::to_string(&frames).unwrap() + "\n").unwrap();
    }
    let text = std::fs::read_to_string(&path)
        .expect("the fixture exists (run with ATOMIK_UPDATE_GOLDEN=1)");
    serde_json::from_str(&text).expect("the fixture parses")
}
