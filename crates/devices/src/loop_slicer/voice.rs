//! One loop-slicer voice: one hit of one slice (see the module docs of `loop_slicer`).

use super::{Shared, slice_key_ok};
use crate::sampler::{tail_gain, zone_region};
use atomik_core::db_to_gain;
use atomik_device_sdk::{LoopMode, SampleMap, TriggerMode, Zone, velocity_to_7bit};
use atomik_dsp::{LoopKind, PlayRegion, Playhead, balance_gains, pan_gains_mono};

/// The decay envelope counts as finished (the voice goes idle) below this level (-80 dB).
pub(super) const DECAY_FLOOR: f32 = 1.0e-4;
/// Samples between pitch updates.
const CONTROL_INTERVAL: u32 = 16;

/// Which map a voice reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    Current,
    Draining,
}

/// A hit waiting for its voice's steal fade to end.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pending {
    pub key: u8,
    pub velocity: f32,
    pub zone: u32,
}

/// Placeholder zone of an idle voice.
const IDLE_ZONE: Zone = Zone {
    buffer: 0,
    key_lo: 0,
    key_hi: 0,
    vel_lo: 1,
    vel_hi: 1,
    root_key: 60,
    transpose: 0,
    tune_cents: 0.0,
    gain_db: 0.0,
    pan: 0.0,
    start: 0,
    end: 0,
    loop_mode: LoopMode::Off,
    loop_start: 0,
    loop_end: 0,
    crossfade: 0,
    trigger: TriggerMode::Gate,
    reverse: false,
};

/// One preallocated voice.
#[derive(Debug, Clone)]
pub(super) struct Voice {
    pub active: bool,
    pub key: u8,
    pub order: u64,
    pub slot: Slot,
    pub zone_idx: u32,
    pub zone: Zone,
    head: Playhead,
    stereo: bool,
    zone_gain: f32,
    pan_l: f32,
    pan_r: f32,
    vel_gain: f32,
    /// Mode snapshot at the hit: gate voices hold while `held`.
    gate: bool,
    held: bool,
    /// Samples of the attack ramp played so far.
    att_pos: u32,
    /// Exponential decay level (1 until the decay starts).
    decay: f32,
    ratio: f64,
    countdown: u32,
    pub pending: Option<Pending>,
    /// Samples left of a fade-out (0: not fading) and its length.
    pub fade_left: u32,
    fade_len: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            active: false,
            key: 0,
            order: 0,
            slot: Slot::Current,
            zone_idx: 0,
            zone: IDLE_ZONE,
            head: Playhead::new(PlayRegion::new(0, 0, LoopKind::Off, 0, 0, 0, 0)),
            stereo: false,
            zone_gain: 1.0,
            pan_l: 1.0,
            pan_r: 1.0,
            vel_gain: 1.0,
            gate: false,
            held: false,
            att_pos: 0,
            decay: 1.0,
            ratio: 1.0,
            countdown: 0,
            pending: None,
            fade_left: 0,
            fade_len: 1.0,
        }
    }

    /// Stops the voice at once (no fade) and drops a pending hit.
    pub fn kill(&mut self) {
        self.active = false;
        self.pending = None;
        self.fade_left = 0;
    }

    fn take_zone(&mut self, map: &SampleMap, zi: usize) -> bool {
        let Some(&zone) = map.zones().get(zi) else {
            return false;
        };
        self.zone = zone;
        self.zone_idx = zi as u32;
        self.stereo = map.buffer_of(&zone).data.len() >= 2;
        self.zone_gain = db_to_gain(zone.gain_db);
        (self.pan_l, self.pan_r) = if self.stereo {
            balance_gains(zone.pan)
        } else {
            pan_gains_mono(zone.pan)
        };
        self.countdown = 0;
        true
    }

    fn region(&self, map: &SampleMap) -> PlayRegion {
        zone_region(&self.zone, map.buffer_of(&self.zone).frames)
    }

    /// Starts a hit of slice `key` on zone `zi` from silence.
    pub fn start(&mut self, map: &SampleMap, zi: usize, key: u8, velocity: f32, sh: &Shared) {
        if !self.take_zone(map, zi) {
            self.kill();
            return;
        }
        let velocity = if velocity.is_nan() {
            0.0
        } else {
            velocity.clamp(0.0, 1.0)
        };
        self.key = key;
        self.slot = Slot::Current;
        self.pending = None;
        self.vel_gain = 1.0 - sh.velocity_amount * (1.0 - velocity);
        self.gate = sh.gate;
        self.held = true;
        self.att_pos = 0;
        self.decay = 1.0;
        self.head = Playhead::new(self.region(map));
        self.fade_left = 0;
        self.active = true;
    }

    /// Moves the voice onto zone `zi` of `map` (same buffer): adopts the zone's parameters and
    /// keeps its position.
    pub fn rebind(&mut self, map: &SampleMap, zi: usize) {
        if !self.take_zone(map, zi) {
            self.kill();
            return;
        }
        let (pos, frac) = self.head.position();
        self.head = Playhead::at(self.region(map), pos, frac);
        if self.head.finished() {
            self.kill();
        }
    }

    /// Starts a fade-out of `samples` (a voice that already fades keeps its fade).
    pub fn start_fade(&mut self, samples: u32) {
        if self.fade_left == 0 {
            let n = samples.max(1);
            self.fade_len = n as f32;
            self.fade_left = n;
        }
    }

    pub fn start_drain(&mut self, samples: u32) {
        self.slot = Slot::Draining;
        self.start_fade(samples);
    }

    /// Note-off of a gate voice: the decay starts.
    pub fn release(&mut self) {
        if self.gate {
            self.held = false;
        }
    }

    pub fn is_fading(&self) -> bool {
        self.fade_left > 0
    }

    /// Adds this voice into `out` (aligned slices). `own` is the map the voice plays from,
    /// `current` the device's current map (where a pending hit starts); `cv` is the pitch CV.
    pub fn render(
        &mut self,
        own: &SampleMap,
        current: Option<&SampleMap>,
        out: [&mut [f32]; 2],
        cv: &[f32],
        sh: &Shared,
    ) {
        let [out_l, out_r] = out;
        let mut map = own;
        for n in 0..out_l.len() {
            if self.countdown == 0 {
                self.control(cv[n], sh);
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
            let data = &map.buffer_of(&self.zone).data;
            let xl = self.head.read(&data[0]);
            let xr = if self.stereo {
                self.head.read(&data[1])
            } else {
                xl
            };
            let attacking = self.att_pos < sh.attack;
            let att = if attacking {
                self.att_pos += 1;
                self.att_pos as f32 / sh.attack as f32
            } else {
                1.0
            };
            let env = att * self.decay;
            let fade = if self.fade_left > 0 {
                self.fade_left as f32 / self.fade_len
            } else {
                1.0
            };
            let tail = tail_gain(&self.head, &self.zone, sh.sample_rate);
            let g = env * self.zone_gain * self.vel_gain * fade * tail;
            out_l[n] += xl * self.pan_l * g;
            out_r[n] += xr * self.pan_r * g;
            self.head.advance(self.ratio);
            let mut ended = self.head.finished();
            // Gate: hold until released. One-shot: decay once the attack is over.
            let decaying = if self.gate { !self.held } else { !attacking };
            if decaying {
                self.decay *= sh.decay_coef;
                ended |= self.decay < DECAY_FLOOR;
            }
            if self.fade_left > 0 {
                self.fade_left -= 1;
                ended |= self.fade_left == 0;
            }
            if ended {
                self.active = false;
                self.fade_left = 0;
                let started = self.pending.take().and_then(|pe| {
                    let m = current?;
                    let z = m.zones().get(pe.zone as usize)?;
                    (slice_key_ok(z, pe.key) && z.contains(pe.key, velocity_to_7bit(pe.velocity)))
                        .then_some((m, pe))
                });
                match started {
                    Some((m, pe)) => {
                        self.start(m, pe.zone as usize, pe.key, pe.velocity, sh);
                        map = m;
                    }
                    None => return,
                }
            }
        }
    }

    /// Control-rate update of the playback ratio.
    fn control(&mut self, cv: f32, sh: &Shared) {
        let z = &self.zone;
        let semis = (i32::from(self.key) - i32::from(z.root_key) + i32::from(z.transpose)) as f32;
        let exponent = (semis + sh.tune_semi) / 12.0
            + z.tune_cents / 1200.0
            + if cv.is_finite() {
                cv.clamp(-1.0, 1.0)
            } else {
                0.0
            } * sh.cv_pitch
                / 12.0;
        self.ratio = f64::from(exponent.exp2());
    }
}
