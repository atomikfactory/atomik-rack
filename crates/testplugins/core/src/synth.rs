//! The test instrument: eight voices of sine or saw with the MIDI and expression inputs of
//!

use crate::{BLOCK, Block, Event, ExprKind, TimedEvent, decode_state, encode_state};

const TAG: [u8; 4] = *b"ATS1";
const ATTACK_SECONDS: f64 = 0.002;
const RELEASE_SECONDS: f64 = 0.010;
const VIBRATO_HZ: f64 = 5.0;

#[derive(Debug, Clone, Copy)]
struct Voice {
    active: bool,
    releasing: bool,
    key: u8,
    id: i32,
    phase: f64,
    env: f32,
    velocity: f32,
    tuning: f32,
    pressure: f32,
    volume: f32,
    order: u64,
}

impl Voice {
    const IDLE: Voice = Voice {
        active: false,
        releasing: false,
        key: 0,
        id: -1,
        phase: 0.0,
        env: 0.0,
        velocity: 0.0,
        tuning: 0.0,
        pressure: 0.0,
        volume: 1.0,
        order: 0,
    };

    /// Does an event addressed by (`key`, `id`) mean this voice? By id when it has one.
    fn is(&self, key: u8, id: i32) -> bool {
        self.active
            && if id >= 0 {
                self.id == id
            } else {
                self.key == key
            }
    }
}

/// An eight-voice polyphonic test instrument, mono source on both channels.
#[derive(Debug, Clone)]
pub struct TestSynth {
    sample_rate: f64,
    wave: f64,
    level: f64,
    voices: [Voice; Self::VOICES],
    bend: f32,
    wheel: f32,
    pressure: f32,
    lfo: f64,
    counter: u64,
}

impl TestSynth {
    /// Waveform, stepped: 0 sine, 1 saw.
    pub const WAVE: u32 = 1;
    /// Output level, 0 to 1, default 0.5.
    pub const LEVEL: u32 = 2;
    /// Number of voices.
    pub const VOICES: usize = 8;

    /// A silent synth at `sample_rate`.
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            wave: 0.0,
            level: 0.5,
            voices: [Voice::IDLE; Self::VOICES],
            bend: 0.0,
            wheel: 0.0,
            pressure: 0.0,
            lfo: 0.0,
            counter: 0,
        }
    }

    /// Sets a param (clamped; unknown ids and NaN are ignored).
    pub fn set_param(&mut self, id: u32, value: f64) {
        if value.is_nan() {
            return;
        }
        match id {
            Self::WAVE => self.wave = value.clamp(0.0, 1.0).round(),
            Self::LEVEL => self.level = value.clamp(0.0, 1.0),
            _ => {}
        }
    }

    /// A param value (0 for an unknown id).
    pub fn param(&self, id: u32) -> f64 {
        match id {
            Self::WAVE => self.wave,
            Self::LEVEL => self.level,
            _ => 0.0,
        }
    }

    /// True when no voice sounds or releases: the plugin may answer "sleep".
    pub fn is_idle(&self) -> bool {
        self.voices.iter().all(|v| !v.active)
    }

    /// Voices that sound or release.
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Silences everything and resets the controllers.
    pub fn reset(&mut self) {
        self.voices = [Voice::IDLE; Self::VOICES];
        self.bend = 0.0;
        self.wheel = 0.0;
        self.pressure = 0.0;
    }

    /// The state as bytes.
    pub fn save_state(&self) -> Vec<u8> {
        encode_state(TAG, &[self.wave, self.level])
    }

    /// Restores a saved state; false (and no change) if the bytes are not one.
    pub fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some([wave, level]) = decode_state::<2>(TAG, bytes) else {
            return false;
        };
        self.set_param(Self::WAVE, wave);
        self.set_param(Self::LEVEL, level);
        true
    }

    fn note_on(&mut self, key: u8, velocity: f32, id: i32) {
        self.counter += 1;
        let slot = self
            .voices
            .iter()
            .position(|v| !v.active)
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, v)| v.order)
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        self.voices[slot] = Voice {
            active: true,
            key,
            id,
            velocity,
            order: self.counter,
            ..Voice::IDLE
        };
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::NoteOn { key, velocity, id } => self.note_on(key, velocity.clamp(0.0, 1.0), id),
            Event::NoteOff { key, id } => {
                if let Some(v) = self
                    .voices
                    .iter_mut()
                    .find(|v| v.is(key, id) && !v.releasing)
                {
                    v.releasing = true;
                }
            }
            Event::PitchBend(b) => self.bend = b.clamp(-1.0, 1.0),
            Event::Cc {
                controller: 1,
                value,
            } => self.wheel = value.clamp(0.0, 1.0),
            Event::Cc {
                controller: 123, ..
            } => {
                for v in self.voices.iter_mut().filter(|v| v.active) {
                    v.releasing = true;
                }
            }
            Event::Cc { .. } | Event::Param { .. } => {}
            Event::ChannelPressure(p) => self.pressure = p.clamp(0.0, 1.0),
            Event::PolyPressure { key, id, value } => self.expr(key, id, ExprKind::Pressure, value),
            Event::Expr {
                key,
                id,
                kind,
                value,
            } => self.expr(key, id, kind, value),
        }
    }

    fn expr(&mut self, key: u8, id: i32, kind: ExprKind, value: f32) {
        for v in self.voices.iter_mut().filter(|v| v.is(key, id)) {
            match kind {
                ExprKind::Tuning => v.tuning = value.clamp(-48.0, 48.0),
                ExprKind::Pressure => v.pressure = value.clamp(0.0, 1.0),
                ExprKind::Volume => v.volume = value.clamp(0.0, 1.0),
                ExprKind::Slide | ExprKind::Pan => {}
            }
        }
    }

    /// Renders one block; `events` are in non-decreasing offset order and apply at their frame.
    pub fn process(&mut self, events: &[TimedEvent], out: &mut [Block; 2]) {
        let attack = (1.0 / (ATTACK_SECONDS * self.sample_rate)) as f32;
        let release = (1.0 / (RELEASE_SECONDS * self.sample_rate)) as f32;
        let mut next = 0;
        let [left, right] = out;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            while let Some(e) = events
                .get(next)
                .filter(|e| usize::from(e.offset).min(BLOCK - 1) <= i)
            {
                self.apply(e.event);
                next += 1;
            }
            let vibrato = f64::from(self.wheel) * 0.5 * (std::f64::consts::TAU * self.lfo).sin();
            // The vibrato runs only while a voice sounds, so a block with no voice and no event
            // changes nothing: skipping it (a plugin asleep) or processing it (an instrument
            // the host keeps warm for a second) leaves the same state.
            if self.voices.iter().any(|v| v.active) {
                self.lfo = (self.lfo + VIBRATO_HZ / self.sample_rate).fract();
            }
            let bend = f64::from(self.bend) * 2.0;
            let saw = self.wave >= 0.5;
            let mut mix = 0.0f32;
            for v in self.voices.iter_mut().filter(|v| v.active) {
                let semis = f64::from(v.key) - 69.0 + bend + f64::from(v.tuning) + vibrato;
                let freq = 440.0 * (semis / 12.0).exp2();
                v.phase = (v.phase + freq / self.sample_rate).fract();
                if v.releasing {
                    v.env -= release;
                    if v.env <= 0.0 {
                        v.env = 0.0;
                        v.active = false;
                        continue;
                    }
                } else if v.env < 1.0 {
                    v.env = (v.env + attack).min(1.0);
                }
                let osc = if saw {
                    (2.0 * v.phase - 1.0) as f32
                } else {
                    (std::f64::consts::TAU * v.phase).sin() as f32
                };
                let p = v.pressure.max(self.pressure);
                mix += osc * v.velocity * v.env * (0.5 + 0.5 * p) * v.volume;
            }
            let s = mix * self.level as f32;
            *l = s;
            *r = s;
        }
        for e in &events[next..] {
            self.apply(e.event);
        }
    }
}
