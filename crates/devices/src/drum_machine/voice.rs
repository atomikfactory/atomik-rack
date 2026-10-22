//! One drum-machine voice: one zone of one pad hit (see the module docs of `drum_machine`).

use super::{PAD_COUNT, Pad, Shared};
use crate::sampler::{tail_gain, zone_region};
use atomik_core::BATCH;
use atomik_core::db_to_gain;
use atomik_device_sdk::{LoopMode, SampleMap, TriggerMode, Zone, velocity_to_7bit};
use atomik_dsp::{LoopKind, PlayRegion, Playhead, balance_gains, pan_gains_mono};

/// The decay envelope counts as finished (the voice goes idle) below this level (−80 dB).
pub(super) const DECAY_FLOOR: f32 = 1.0e-4;

/// Which map a voice reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    Current,
    Draining,
}

/// A hit waiting for its voice's steal fade to end.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pending {
    pub pad: u8,
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

/// Global CV modulation of one batch: the linear level gain (`10^(trim * range * cv / 20)`)
/// and the playback-ratio factor (`2^(semitones / 12)`) of every sample, each only when its
/// socket is cabled.
pub(super) struct VoiceCv {
    pub level_on: bool,
    pub level: [f32; BATCH],
    pub pitch_on: bool,
    pub pitch: [f64; BATCH],
}

impl VoiceCv {
    pub fn new() -> Self {
        Self {
            level_on: false,
            level: [1.0; BATCH],
            pitch_on: false,
            pitch: [1.0; BATCH],
        }
    }

    /// The view of a render segment that starts at batch sample `start`.
    pub fn at(&self, start: usize) -> CvView<'_> {
        CvView { cv: self, start }
    }
}

/// [`VoiceCv`] seen from the first sample of one render segment.
#[derive(Clone, Copy)]
pub(super) struct CvView<'a> {
    cv: &'a VoiceCv,
    start: usize,
}

/// Values that stay constant over one render segment.
#[derive(Debug, Clone, Copy)]
struct Segment {
    ratio: f64,
    level: f32,
    pan_l: f32,
    pan_r: f32,
}

/// One preallocated voice.
#[derive(Debug, Clone)]
pub(super) struct Voice {
    pub active: bool,
    pub pad: u8,
    pub order: u64,
    pub slot: Slot,
    pub zone_idx: u32,
    pub zone: Zone,
    head: Playhead,
    stereo: bool,
    zone_gain: f32,
    vel_gain: f32,
    /// Mode snapshot at the hit: gate voices hold while `held`.
    gate: bool,
    held: bool,
    /// Samples of the attack ramp played so far.
    att_pos: u32,
    /// Exponential decay level (1 until the decay starts).
    decay: f32,
    pub pending: Option<Pending>,
    /// Samples left of a fade-out (0: not fading) and its length.
    pub fade_left: u32,
    fade_len: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            active: false,
            pad: 0,
            order: 0,
            slot: Slot::Current,
            zone_idx: 0,
            zone: IDLE_ZONE,
            head: Playhead::new(PlayRegion::new(0, 0, LoopKind::Off, 0, 0, 0, 0)),
            stereo: false,
            zone_gain: 1.0,
            vel_gain: 1.0,
            gate: false,
            held: false,
            att_pos: 0,
            decay: 1.0,
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
        true
    }

    fn region(&self, map: &SampleMap) -> PlayRegion {
        let z = &self.zone;
        zone_region(z, map.buffer_of(z).frames)
    }

    /// Starts a hit of `pad` on zone `zi` from silence.
    pub fn start(
        &mut self,
        map: &SampleMap,
        zi: usize,
        pad: u8,
        velocity: f32,
        pads: &[Pad; PAD_COUNT],
    ) {
        if !self.take_zone(map, zi) {
            self.kill();
            return;
        }
        let velocity = if velocity.is_nan() {
            0.0
        } else {
            velocity.clamp(0.0, 1.0)
        };
        let p = &pads[usize::from(pad)];
        self.pad = pad;
        self.slot = Slot::Current;
        self.pending = None;
        self.vel_gain = 1.0 - p.velocity * (1.0 - velocity);
        self.gate = p.gate;
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

    fn segment(&self, pads: &[Pad; PAD_COUNT]) -> Segment {
        let p = &pads[usize::from(self.pad)];
        let z = &self.zone;
        let key = i32::from(super::pad_key(self.pad));
        let offset = (key - i32::from(z.root_key) + i32::from(z.transpose)) as f32;
        let exponent = (offset + p.tune + (p.fine + z.tune_cents) / 100.0) / 12.0;
        let pan = p.pan + z.pan;
        let (pan_l, pan_r) = if self.stereo {
            balance_gains(pan)
        } else {
            pan_gains_mono(pan)
        };
        Segment {
            ratio: f64::from(exponent.exp2()),
            level: p.level,
            pan_l,
            pan_r,
        }
    }

    /// Adds this voice into `out` (aligned slices). `own` is the map the voice plays from,
    /// `current` the device's current map (where a pending hit starts).
    pub fn render(
        &mut self,
        own: &SampleMap,
        current: Option<&SampleMap>,
        out: [&mut [f32]; 2],
        pads: &[Pad; PAD_COUNT],
        sh: &Shared,
        cv: &CvView<'_>,
    ) {
        let [out_l, out_r] = out;
        let mut map = own;
        let mut seg = self.segment(pads);
        for n in 0..out_l.len() {
            let data = &map.buffer_of(&self.zone).data;
            let xl = self.head.read(&data[0]);
            let xr = if self.stereo {
                self.head.read(&data[1])
            } else {
                xl
            };
            let att = if self.att_pos < sh.attack {
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
            let mut g = env * seg.level * self.zone_gain * self.vel_gain * fade * tail;
            let at = cv.start + n;
            if cv.cv.level_on {
                g *= cv.cv.level[at];
            }
            out_l[n] += xl * seg.pan_l * g;
            out_r[n] += xr * seg.pan_r * g;
            self.head.advance(if cv.cv.pitch_on {
                seg.ratio * cv.cv.pitch[at]
            } else {
                seg.ratio
            });
            let mut ended = self.head.finished();
            if !(self.gate && self.held) {
                self.decay *= pads[usize::from(self.pad)].decay_coef;
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
                    let key = super::pad_key(pe.pad);
                    (super::is_pad_zone(z, key) && z.contains(key, velocity_to_7bit(pe.velocity)))
                        .then_some((m, pe))
                });
                match started {
                    Some((m, pe)) => {
                        self.start(m, pe.zone as usize, pe.pad, pe.velocity, pads);
                        map = m;
                        seg = self.segment(pads);
                    }
                    None => return,
                }
            }
        }
    }
}
