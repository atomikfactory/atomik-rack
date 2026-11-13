//! The gesture recogniser: plain code, no model. It takes the 21 landmarks of each hand in each
//! frame and says whether the thumb and index finger are pinched and how far the wrist has
//! turned since the pinch began, as a knob would turn.
//!
//! # The rules
//!
//! * **Pinch.** The distance from the thumb tip to the index tip, divided by the size of the
//!   hand (the wrist to the knuckle of the middle finger). It starts below
//!   [`GestureConfig::pinch_on`] (0.25) and ends above [`GestureConfig::pinch_off`] (0.40): the
//!   gap between the two is the hysteresis, so a hand that hovers near one number does not
//!   flicker. The two numbers were chosen on the synthetic hands of the tests (the thumb and
//!   finger of a closed pinch are about 0.05 apart, an open hand 1.0) and wait for hand checks
//!   with a real camera.
//! * **Twist.** The angle of the wrist-to-knuckle vector, measured in a square unit (the width
//!   is multiplied by the aspect ratio of the frame), unwrapped so it never jumps at 180
//!   degrees, and smoothed by a One Euro filter. A positive twist turns clockwise as the user
//!   sees their own mirrored image. The twist of a pinch is measured from the angle at the
//!   moment it began (a ratchet: open the hand, turn it back and pinch again, and the next
//!   twist starts from zero again). A [`GestureEvent::Twist`] carries the change since the last
//!   one, and only once it is larger than the dead zone (1.5 degrees), so a still hand does not
//!   drift.
//! * **Open palm.** All four fingers extended (each tip further from the wrist than its middle
//!   joint and well past its knuckle), the thumb tip clear of the index tip
//!   ([`GestureConfig::open_spread`]) and no pinch, held for [`GestureConfig::open_hold`] (300 ms):
//!   [`GestureEvent::OpenPalmStart`]. Any finger folding, or a pinch, ends it. Like the pinch
//!   numbers these wait for hand checks with a real camera.
//! * **Poses.** Four more held shapes, each from which fingers are extended or curled (a finger
//!   is curled when its tip is no further from the wrist than its knuckle, with a little room):
//!   a **fist** (all four curled), a **thumbs up** (all four curled and the thumb tip the highest
//!   point of the hand, well above the knuckles, wherever the hand is turned), a **point** (the
//!   index finger extended, the other three curled) and a **peace** sign (index and middle
//!   extended, ring and little curled). A pose counts after [`GestureConfig::pose_hold`] (300 ms)
//!   and ends when the shape goes; a pinch is never a pose. The thumb is free in a point and a
//!   peace, and a pose changes to another one only after the first has ended. Like the pinch
//!   numbers these wait for hand checks with a real camera.
//! * **Swipe.** The palm (not pinched) moves at least [`GestureConfig::swipe_distance`] (half a
//!   frame height) along the picture within [`GestureConfig::swipe_window`] (400 ms) and mostly
//!   sideways: one [`GestureEvent::Swipe`], then [`GestureConfig::swipe_lockout`] before the
//!   next, so a stroke is one swipe whichever way the hand comes back. The direction is the one
//!   the user sees in the mirrored preview.
//! * **Pad.** Both hands pinched: [`Update::pad`] is the midpoint of the two palms and the
//!   distance between them (in frame heights); [`GestureEvent::PadStart`] and
//!   [`GestureEvent::PadEnd`] frame it.
//! * **Loss.** A hand that is not in a frame keeps its pinch for [`GestureConfig::loss_tolerance`]
//!   (150 ms); after that the pinch and the open palm end. A hand the model is unsure about
//!   ([`GestureConfig::min_score`]) counts as not there.
//! * **Two hands** are tracked one by one. A hand keeps its [`HandId`] from frame to frame by the
//!   model's handedness and the nearest palm.
//!
//! The recogniser holds no clock: time is the `t` of each [`Observation`], so the same input gives
//! the same output.

mod one_euro;

use crate::hand::{HandObservation, Handedness, LANDMARKS, Observation, joint};
use one_euro::OneEuro;
pub use one_euro::OneEuroParams;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// The identity of a hand while the program follows it (the first hand is 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HandId(pub u32);

/// What the recogniser sees of one hand in one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandState {
    /// Which hand this is, for as long as it is followed.
    pub id: HandId,
    /// Whether the hand is in this frame (false during the short tolerance after it left).
    pub present: bool,
    /// The model's label of the hand.
    pub handedness: Handedness,
    /// Whether thumb and index finger are pinched (with hysteresis).
    pub pinch: bool,
    /// 1 when the fingers touch, 0 when they are at the end threshold or further.
    pub pinch_strength: f32,
    /// The turn since the pinch began, in radians, smoothed (0 when not pinched). Positive is
    /// clockwise as the user sees the mirrored image.
    pub twist_angle: f32,
    /// The middle of the palm in normalised image coordinates.
    pub palm_center: (f32, f32),
    /// The length of the palm along the hand, in frame heights.
    pub palm_height: f32,
    /// Wrist to the knuckle of the middle finger, in frame heights.
    pub hand_size: f32,
    /// How near the hand is: the smoothed hand size in frame heights (a closer hand is bigger).
    pub depth_proxy: f32,
    /// Whether the palm is open: every finger extended and held for [`GestureConfig::open_hold`].
    pub open_palm: bool,
    /// The pose held for [`GestureConfig::pose_hold`], if any.
    pub pose: Option<Pose>,
}

/// A held shape of the hand (the open palm and the pinch have events of their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Pose {
    /// Every finger curled.
    Fist,
    /// Every finger curled and the thumb standing up.
    ThumbsUp,
    /// The index finger up, the others curled.
    Point,
    /// The index and middle fingers up, the others curled.
    Peace,
}

/// Which way a swipe went, as the user sees the mirrored preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SwipeDirection {
    /// To the left of the preview.
    Left,
    /// To the right of the preview.
    Right,
}

/// Both hands pinched: where they are and how far apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pad {
    /// The middle of the two palms, from the left of the image (0 to 1; the raw image, so
    /// the preview the user sees is mirrored).
    pub x: f32,
    /// The middle of the two palms, from the top of the image (0 to 1).
    pub y: f32,
    /// The distance between the palms in frame heights.
    pub spread: f32,
}

/// A change in a gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureEvent {
    /// The thumb and index finger came together.
    PinchStart {
        /// The hand.
        hand: HandId,
        /// The model's label of the hand.
        handedness: Handedness,
    },
    /// The wrist turned while pinched.
    Twist {
        /// The hand.
        hand: HandId,
        /// The change since the last twist (or since the pinch began), in radians.
        delta_radians: f32,
    },
    /// The fingers opened, or the hand was lost for too long.
    PinchEnd {
        /// The hand.
        hand: HandId,
    },
    /// The palm has been open (every finger extended, not pinched) for the hold time.
    OpenPalmStart {
        /// The hand.
        hand: HandId,
        /// The model's label of the hand.
        handedness: Handedness,
    },
    /// The fingers closed or the thumb came to the index finger, or the hand was lost for too
    /// long.
    OpenPalmEnd {
        /// The hand.
        hand: HandId,
    },
    /// A pose has been held for the hold time.
    PoseStart {
        /// The hand.
        hand: HandId,
        /// The model's label of the hand.
        handedness: Handedness,
        /// The pose.
        pose: Pose,
    },
    /// The pose ended (the shape changed, or the hand was lost for too long).
    PoseEnd {
        /// The hand.
        hand: HandId,
        /// The pose.
        pose: Pose,
    },
    /// The palm swept sideways fast.
    Swipe {
        /// The hand.
        hand: HandId,
        /// The model's label of the hand.
        handedness: Handedness,
        /// Which way, as the user sees the mirrored preview.
        direction: SwipeDirection,
    },
    /// Both hands are pinched: [`Update::pad`] has a value from now on.
    PadStart,
    /// One of the two hands let go or was lost.
    PadEnd,
}

/// What one frame gave.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Update {
    /// Every hand followed after this frame, by id.
    pub states: Vec<HandState>,
    /// The changes, in order.
    pub events: Vec<GestureEvent>,
    /// The pad, while both hands are pinched.
    pub pad: Option<Pad>,
}

/// The numbers of the recogniser. [`Default`] is what the program uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureConfig {
    /// The pinch starts below this (thumb to index tip over hand size).
    pub pinch_on: f32,
    /// The pinch ends above this.
    pub pinch_off: f32,
    /// A twist smaller than this, in radians, is held back.
    pub dead_zone: f32,
    /// A hand lost for longer than this many seconds ends its pinch and is forgotten.
    pub loss_tolerance: f64,
    /// A hand the model scores lower than this is ignored.
    pub min_score: f32,
    /// The palm counts as open after this many seconds with every finger extended.
    pub open_hold: f64,
    /// The thumb tip must be at least this far from the index tip (over the hand size) for the
    /// palm to count as open: a pinch, or a thumb tucked against the finger, is not an open palm.
    pub open_spread: f32,
    /// A pose counts after this many seconds.
    pub pose_hold: f64,
    /// A swipe is at least this far (in frame heights).
    pub swipe_distance: f64,
    /// A swipe is made within this many seconds.
    pub swipe_window: f64,
    /// No other swipe for this many seconds after one.
    pub swipe_lockout: f64,
    /// The farthest a palm may move between two frames and still be the same hand.
    pub match_radius: f32,
    /// The filter of the angle.
    pub angle_filter: OneEuroParams,
    /// The filter of positions and size.
    pub position_filter: OneEuroParams,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            pinch_on: 0.25,
            pinch_off: 0.40,
            dead_zone: 1.5f32.to_radians(),
            loss_tolerance: 0.15,
            min_score: 0.5,
            open_hold: 0.3,
            open_spread: 0.6,
            pose_hold: 0.3,
            swipe_distance: 0.5,
            swipe_window: 0.4,
            swipe_lockout: 0.4,
            match_radius: 0.4,
            angle_filter: OneEuroParams {
                min_cutoff: 1.5,
                beta: 1.0,
                d_cutoff: 1.0,
            },
            position_filter: OneEuroParams {
                min_cutoff: 2.0,
                beta: 2.0,
                d_cutoff: 1.0,
            },
        }
    }
}

/// What is measured on one hand in one frame, before any smoothing.
struct Measure {
    angle: f64,
    size: f64,
    ratio: f64,
    palm: (f32, f32),
    palm_height: f64,
    /// Every one of the four fingers is extended.
    extended: bool,
    /// The candidate pose of this frame (no pinch is looked at here).
    pose: Option<Pose>,
    /// The palm in one unit (the width scaled by the aspect ratio), for the swipe.
    palm_unit: (f64, f64),
}

/// What one finger looks like.
#[derive(Clone, Copy, PartialEq)]
enum Finger {
    Extended,
    Curled,
    Between,
}

/// The four fingers' joints: knuckle, middle joint and tip.
const FINGERS: [[usize; 3]; 4] = [[5, 6, 8], [9, 10, 12], [13, 14, 16], [17, 18, 20]];

fn measure(hand: &HandObservation, aspect: f32) -> Option<Measure> {
    let a = f64::from(aspect);
    let p = &hand.landmarks;
    // In one unit: the width scaled by the aspect ratio, the height as it is.
    let at = |i: usize| (f64::from(p[i].x) * a, f64::from(p[i].y));
    let (wx, wy) = at(joint::WRIST);
    let (mx, my) = at(joint::MIDDLE_MCP);
    let (dx, dy) = (mx - wx, my - wy);
    let size = dx.hypot(dy);
    if !size.is_finite() || size < 1e-4 {
        return None;
    }
    let (tx, ty) = at(joint::THUMB_TIP);
    let (ix, iy) = at(joint::INDEX_TIP);
    let ratio = (tx - ix).hypot(ty - iy) / size;
    // The image y grows downwards: flip it so the angle is the usual one.
    let angle = (-dy).atan2(dx);
    let mut palm = (0.0f32, 0.0f32);
    let mut along = (f64::MAX, f64::MIN);
    let (ux, uy) = (dx / size, dy / size);
    for i in [
        joint::WRIST,
        joint::INDEX_MCP,
        joint::MIDDLE_MCP,
        joint::RING_MCP,
        joint::PINKY_MCP,
    ] {
        palm.0 += p[i].x / 5.0;
        palm.1 += p[i].y / 5.0;
        let (x, y) = at(i);
        let s = (x - wx) * ux + (y - wy) * uy;
        along = (along.0.min(s), along.1.max(s));
    }
    // A finger is extended when its tip is further from the wrist than its middle joint and
    // well past its knuckle: a curled finger folds the tip back.
    let from_wrist = |i: usize| {
        let (x, y) = at(i);
        (x - wx).hypot(y - wy)
    };
    let fingers = FINGERS.map(|[mcp, pip, tip]| {
        let tip = from_wrist(tip);
        if tip > from_wrist(pip) * 1.05 && tip > from_wrist(mcp) * 1.3 {
            Finger::Extended
        } else if tip < from_wrist(mcp) * 1.1 {
            Finger::Curled
        } else {
            Finger::Between
        }
    });
    let extended = fingers.iter().all(|f| *f == Finger::Extended);
    // The thumb stands up when its tip is the highest point of the hand by a good margin.
    let top = [
        0usize, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ]
    .iter()
    .map(|i| at(*i).1)
    .fold(f64::MAX, f64::min);
    let thumb_up = at(joint::THUMB_TIP).1 < top - 0.35 * size;
    use Finger::{Curled as C, Extended as E};
    let pose = match fingers {
        [C, C, C, C] if thumb_up => Some(Pose::ThumbsUp),
        [C, C, C, C] => Some(Pose::Fist),
        [E, C, C, C] => Some(Pose::Point),
        [E, E, C, C] => Some(Pose::Peace),
        _ => None,
    };
    let palm_unit = {
        let mut u = (0.0, 0.0);
        for i in [
            joint::WRIST,
            joint::INDEX_MCP,
            joint::MIDDLE_MCP,
            joint::RING_MCP,
            joint::PINKY_MCP,
        ] {
            let (x, y) = at(i);
            u = (u.0 + x / 5.0, u.1 + y / 5.0);
        }
        u
    };
    Some(Measure {
        angle,
        size,
        ratio,
        palm,
        palm_height: along.1 - along.0,
        extended,
        pose,
        palm_unit,
    })
}

fn wrap(a: f64) -> f64 {
    let mut a = a % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    } else if a < -PI {
        a += 2.0 * PI;
    }
    a
}

/// One hand being followed.
struct Track {
    id: HandId,
    handedness: Handedness,
    last_seen: f64,
    present: bool,
    palm: (f32, f32),
    raw_angle: f64,
    unwrapped: f64,
    angle: OneEuro,
    x: OneEuro,
    y: OneEuro,
    size: OneEuro,
    pinching: bool,
    base: f64,
    emitted: f64,
    /// Since when every finger has been extended, while they are.
    open_since: Option<f64>,
    palm_open: bool,
    /// The candidate pose of the last frames and since when it has been the same.
    pose_since: Option<(Pose, f64)>,
    pose: Option<Pose>,
    /// The palm in one unit over the last [`GestureConfig::swipe_window`].
    trail: Vec<(f64, f64, f64)>,
    /// No swipe before this time.
    swipe_after: f64,
    last: HandState,
}

impl Track {
    fn new(id: HandId, h: &HandObservation, m: &Measure, t: f64, cfg: &GestureConfig) -> Self {
        Self {
            id,
            handedness: h.handedness,
            last_seen: t,
            present: true,
            palm: m.palm,
            raw_angle: m.angle,
            unwrapped: m.angle,
            angle: OneEuro::new(cfg.angle_filter),
            x: OneEuro::new(cfg.position_filter),
            y: OneEuro::new(cfg.position_filter),
            size: OneEuro::new(cfg.position_filter),
            pinching: false,
            base: 0.0,
            emitted: 0.0,
            open_since: None,
            palm_open: false,
            pose_since: None,
            pose: None,
            trail: Vec::new(),
            swipe_after: f64::MIN,
            last: HandState {
                id,
                present: true,
                handedness: h.handedness,
                pinch: false,
                pinch_strength: 0.0,
                twist_angle: 0.0,
                palm_center: m.palm,
                palm_height: 0.0,
                hand_size: 0.0,
                depth_proxy: 0.0,
                open_palm: false,
                pose: None,
            },
        }
    }

    /// Takes one measurement of this hand.
    fn feed(
        &mut self,
        h: &HandObservation,
        m: &Measure,
        t: f64,
        cfg: &GestureConfig,
        events: &mut Vec<GestureEvent>,
    ) {
        self.handedness = h.handedness;
        self.present = true;
        self.last_seen = t;
        self.palm = m.palm;
        self.unwrapped += wrap(m.angle - self.raw_angle);
        self.raw_angle = m.angle;
        let angle = self.angle.filter(t, self.unwrapped);
        let x = self.x.filter(t, f64::from(m.palm.0));
        let y = self.y.filter(t, f64::from(m.palm.1));
        let size = self.size.filter(t, m.size);

        let on = f64::from(cfg.pinch_on);
        let off = f64::from(cfg.pinch_off);
        if !self.pinching && m.ratio < on {
            self.pinching = true;
            self.base = angle;
            self.emitted = 0.0;
            events.push(GestureEvent::PinchStart {
                hand: self.id,
                handedness: h.handedness,
            });
        } else if self.pinching && m.ratio > off {
            self.pinching = false;
            events.push(GestureEvent::PinchEnd { hand: self.id });
        }
        let mut twist = 0.0;
        if self.pinching {
            twist = angle - self.base;
            if (twist - self.emitted).abs() >= f64::from(cfg.dead_zone) {
                events.push(GestureEvent::Twist {
                    hand: self.id,
                    delta_radians: (twist - self.emitted) as f32,
                });
                self.emitted = twist;
            }
        }
        // The palm: every finger out and the thumb clear of the index finger, held.
        let open = m.extended && m.ratio >= f64::from(cfg.open_spread) && !self.pinching;
        if open {
            let since = *self.open_since.get_or_insert(t);
            if !self.palm_open && t - since >= cfg.open_hold - 1e-9 {
                self.palm_open = true;
                events.push(GestureEvent::OpenPalmStart {
                    hand: self.id,
                    handedness: h.handedness,
                });
            }
        } else {
            self.open_since = None;
            if self.palm_open {
                self.palm_open = false;
                events.push(GestureEvent::OpenPalmEnd { hand: self.id });
            }
        }
        // The poses: the same candidate for the hold time, never while pinched.
        let candidate = if self.pinching { None } else { m.pose };
        match (candidate, self.pose_since) {
            (Some(c), Some((p, _))) if c == p => {}
            (Some(c), _) => self.pose_since = Some((c, t)),
            (None, _) => self.pose_since = None,
        }
        if self.pose.is_some()
            && self.pose != candidate
            && let Some(pose) = self.pose.take()
        {
            events.push(GestureEvent::PoseEnd {
                hand: self.id,
                pose,
            });
        }
        if self.pose.is_none()
            && let Some((pose, since)) = self.pose_since
            && t - since >= cfg.pose_hold - 1e-9
        {
            self.pose = Some(pose);
            events.push(GestureEvent::PoseStart {
                hand: self.id,
                handedness: h.handedness,
                pose,
            });
        }
        // The swipe: far and fast along the picture, never pinched, once per stroke.
        if self.pinching {
            self.trail.clear();
        } else {
            self.trail.push((t, m.palm_unit.0, m.palm_unit.1));
            self.trail.retain(|(at, _, _)| t - at <= cfg.swipe_window);
            if t >= self.swipe_after
                && let Some(&(_, x0, y0)) = self.trail.first()
            {
                let (dx, dy) = (m.palm_unit.0 - x0, m.palm_unit.1 - y0);
                if dx.abs() >= cfg.swipe_distance && dy.abs() <= dx.abs() * 0.6 {
                    // The raw image is not mirrored: a palm going to the left of it goes to
                    // the right of the preview.
                    events.push(GestureEvent::Swipe {
                        hand: self.id,
                        handedness: h.handedness,
                        direction: if dx < 0.0 {
                            SwipeDirection::Right
                        } else {
                            SwipeDirection::Left
                        },
                    });
                    self.swipe_after = t + cfg.swipe_lockout;
                    self.trail.clear();
                }
            }
        }
        self.last = HandState {
            id: self.id,
            present: true,
            handedness: h.handedness,
            pinch: self.pinching,
            pinch_strength: ((off - m.ratio) / (off - on)).clamp(0.0, 1.0) as f32,
            twist_angle: twist as f32,
            palm_center: (x as f32, y as f32),
            palm_height: m.palm_height as f32,
            hand_size: m.size as f32,
            depth_proxy: size as f32,
            open_palm: self.palm_open,
            pose: self.pose,
        };
    }
}

/// The recogniser: feed it one [`Observation`] per frame.
pub struct Recogniser {
    cfg: GestureConfig,
    tracks: Vec<Track>,
    next_id: u32,
    pad_on: bool,
}

impl Recogniser {
    /// A recogniser that follows no hand yet.
    pub fn new(cfg: GestureConfig) -> Self {
        Self {
            cfg,
            tracks: Vec::new(),
            next_id: 0,
            pad_on: false,
        }
    }

    /// Takes one frame (it may hold no hand: that is how a loss is told) and returns the state of
    /// every followed hand and the changes.
    pub fn update(&mut self, obs: &Observation) -> Update {
        let cfg = self.cfg;
        let mut events = Vec::new();
        let measured: Vec<(&HandObservation, Measure)> = obs
            .hands
            .iter()
            .filter(|h| h.score >= cfg.min_score)
            .filter_map(|h| measure(h, obs.aspect).map(|m| (h, m)))
            .collect();

        // Pair hands with tracks, the cheapest pair first: the distance between palms (in one
        // unit) plus a penalty when the model's labels differ.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (hi, (h, m)) in measured.iter().enumerate() {
            for (ti, t) in self.tracks.iter().enumerate() {
                let d = ((m.palm.0 - t.palm.0) * obs.aspect).hypot(m.palm.1 - t.palm.1);
                if d <= cfg.match_radius {
                    let penalty = if h.handedness == t.handedness {
                        0.0
                    } else {
                        0.2
                    };
                    pairs.push((d + penalty, hi, ti));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut hand_taken = vec![false; measured.len()];
        let mut track_taken = vec![false; self.tracks.len()];
        for (_, hi, ti) in pairs {
            if hand_taken[hi] || track_taken[ti] {
                continue;
            }
            hand_taken[hi] = true;
            track_taken[ti] = true;
            let (h, m) = &measured[hi];
            self.tracks[ti].feed(h, m, obs.t, &cfg, &mut events);
        }

        // The tracks that got nothing wait for their hand.
        for (t, _) in self
            .tracks
            .iter_mut()
            .zip(&track_taken)
            .filter(|(_, taken)| !**taken)
        {
            t.present = false;
            t.last.present = false;
        }

        // The hands nobody followed are new.
        for (hi, (h, m)) in measured.iter().enumerate() {
            if hand_taken[hi] {
                continue;
            }
            let id = HandId(self.next_id);
            self.next_id += 1;
            let mut track = Track::new(id, h, m, obs.t, &cfg);
            track.feed(h, m, obs.t, &cfg, &mut events);
            self.tracks.push(track);
        }

        self.tracks.retain_mut(|t| {
            if t.present || obs.t - t.last_seen <= cfg.loss_tolerance {
                return true;
            }
            if t.pinching {
                events.push(GestureEvent::PinchEnd { hand: t.id });
            }
            if t.palm_open {
                events.push(GestureEvent::OpenPalmEnd { hand: t.id });
            }
            if let Some(pose) = t.pose {
                events.push(GestureEvent::PoseEnd { hand: t.id, pose });
            }
            false
        });

        // Both hands pinched make the pad.
        let pinched: Vec<&Track> = self.tracks.iter().filter(|t| t.pinching).collect();
        let pad = (pinched.len() == 2).then(|| {
            let (a, b) = (pinched[0].last.palm_center, pinched[1].last.palm_center);
            Pad {
                x: (a.0 + b.0) / 2.0,
                y: (a.1 + b.1) / 2.0,
                spread: (f64::from(a.0 - b.0) * f64::from(obs.aspect)).hypot(f64::from(a.1 - b.1))
                    as f32,
            }
        });
        if pad.is_some() != self.pad_on {
            self.pad_on = pad.is_some();
            events.push(if self.pad_on {
                GestureEvent::PadStart
            } else {
                GestureEvent::PadEnd
            });
        }

        let mut states: Vec<HandState> = self.tracks.iter().map(|t| t.last).collect();
        states.sort_by_key(|s| s.id);
        Update {
            states,
            events,
            pad,
        }
    }

    /// Forgets every hand and returns the end of every pinch and open palm that was open (for a
    /// camera that stops or is switched).
    pub fn reset(&mut self) -> Vec<GestureEvent> {
        let mut ended = Vec::new();
        for t in &self.tracks {
            if t.pinching {
                ended.push(GestureEvent::PinchEnd { hand: t.id });
            }
            if t.palm_open {
                ended.push(GestureEvent::OpenPalmEnd { hand: t.id });
            }
            if let Some(pose) = t.pose {
                ended.push(GestureEvent::PoseEnd { hand: t.id, pose });
            }
        }
        if std::mem::take(&mut self.pad_on) {
            ended.push(GestureEvent::PadEnd);
        }
        self.tracks.clear();
        ended
    }
}

const _: () = assert!(LANDMARKS == 21);
