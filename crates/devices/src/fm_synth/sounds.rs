//! The factory sounds of the FM synth: a value for every param but `sound`.
//!
//! A sound is only a starting point: choosing one in the app sets all of these params in one
//! undo step (`atomik-app` expands a `sound` change into a batch of param changes) and the
//! player tweaks from there. The device itself ignores the `sound` param, it only remembers
//! which one was chosen last. Each sound lists the params that differ from the init sound (the
//! defaults of the descriptor); [`values`] fills in the rest. The values are this design's own;
//! the names describe the sound.
//!
//! | Sound | Idea |
//! |---|---|
//! | Init | a two-operator stack: operator 2 (ratio 2) modulating operator 1; the start of every patch |
//! | E. Piano | two pairs: a soft body (ratio 1 over ratio 1) and a short, bright "tine" (ratio 14 over ratio 1) that fades before the body; harder keys are brighter; a little tremolo |
//! | Bell | two pairs at inharmonic ratios (3.5, 2.76 and 5.4) with long decays and no sustain |
//! | Bass | mono with a short glide: a three-modulator stack with feedback, bright for a moment at each note |
//! | Brass | a Y routing (two modulators into one) whose index opens over 90 ms, a light vibrato; the mod wheel brightens it |
//! | Pad | two detuned pairs with slow envelopes; a slow LFO breathes the index |

use super::DESCRIPTOR;

/// The names of the sounds, in selector order.
pub const SOUND_NAMES: [&str; 6] = ["Init", "E. Piano", "Bell", "Bass", "Brass", "Pad"];

/// A factory sound.
#[derive(Debug, Clone, Copy)]
pub struct Sound {
    /// Name shown in the sound selector.
    pub name: &'static str,
    /// `(param key, plain value)` of the params that differ from the init sound.
    pub changes: &'static [(&'static str, f32)],
}

/// The sounds, in selector order.
pub const SOUNDS: [Sound; 6] = [
    Sound {
        name: "Init",
        changes: &[],
    },
    Sound {
        name: "E. Piano",
        changes: &[
            ("algorithm", 1.0),
            ("spread", 0.4),
            ("lfo.wave", 0.0),
            ("lfo.rate", 4.5),
            ("lfo.to_amp", 0.12),
            // Body: operator 2 over operator 1.
            ("op1.level", 0.8),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 2_800.0),
            ("op1.sustain", 0.12),
            ("op1.release", 350.0),
            ("op2.coarse", 1.0),
            ("op2.level", 0.32),
            ("op2.velocity", 0.7),
            ("op2.key_scale", 0.3),
            ("op2.attack", 1.0),
            ("op2.decay", 1_500.0),
            ("op2.sustain", 0.05),
            ("op2.release", 300.0),
            // Tine: operator 4 over operator 3.
            ("op3.coarse", 1.0),
            ("op3.detune", 4.0),
            ("op3.level", 0.5),
            ("op3.velocity", 0.3),
            ("op3.attack", 1.0),
            ("op3.decay", 2_400.0),
            ("op3.sustain", 0.1),
            ("op3.release", 300.0),
            ("op4.coarse", 14.0),
            ("op4.level", 0.18),
            ("op4.velocity", 0.8),
            ("op4.attack", 1.0),
            ("op4.decay", 90.0),
            ("op4.sustain", 0.0),
            ("op4.release", 100.0),
        ],
    },
    Sound {
        name: "Bell",
        changes: &[
            ("algorithm", 1.0),
            ("spread", 0.5),
            ("op1.level", 0.7),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 4_500.0),
            ("op1.sustain", 0.0),
            ("op1.release", 2_500.0),
            ("op2.coarse", 3.0),
            ("op2.fine", 0.5),
            ("op2.level", 0.55),
            ("op2.velocity", 0.6),
            ("op2.attack", 1.0),
            ("op2.decay", 2_500.0),
            ("op2.sustain", 0.0),
            ("op2.release", 1_500.0),
            ("op3.coarse", 2.0),
            ("op3.fine", 0.76),
            ("op3.level", 0.45),
            ("op3.velocity", 0.3),
            ("op3.attack", 1.0),
            ("op3.decay", 3_500.0),
            ("op3.sustain", 0.0),
            ("op3.release", 2_000.0),
            ("op4.coarse", 5.0),
            ("op4.fine", 0.4),
            ("op4.level", 0.35),
            ("op4.velocity", 0.5),
            ("op4.attack", 1.0),
            ("op4.decay", 1_200.0),
            ("op4.sustain", 0.0),
            ("op4.release", 800.0),
        ],
    },
    Sound {
        name: "Bass",
        changes: &[
            ("algorithm", 0.0),
            ("feedback", 0.3),
            ("play.mode", 1.0),
            ("play.glide", 30.0),
            ("op1.level", 0.9),
            ("op1.velocity", 0.3),
            ("op1.attack", 1.0),
            ("op1.decay", 600.0),
            ("op1.sustain", 0.8),
            ("op1.release", 120.0),
            ("op2.coarse", 1.0),
            ("op2.level", 0.5),
            ("op2.velocity", 0.6),
            ("op2.attack", 1.0),
            ("op2.decay", 300.0),
            ("op2.sustain", 0.3),
            ("op2.release", 120.0),
            ("op3.coarse", 2.0),
            ("op3.level", 0.3),
            ("op3.velocity", 0.5),
            ("op3.attack", 1.0),
            ("op3.decay", 200.0),
            ("op3.sustain", 0.1),
            ("op3.release", 100.0),
            ("op4.coarse", 1.0),
            ("op4.level", 0.4),
            ("op4.velocity", 0.4),
            ("op4.attack", 1.0),
            ("op4.decay", 150.0),
            ("op4.sustain", 0.2),
            ("op4.release", 100.0),
        ],
    },
    Sound {
        name: "Brass",
        changes: &[
            ("algorithm", 2.0),
            ("feedback", 0.2),
            ("wheel.to_index", 0.8),
            ("lfo.wave", 0.0),
            ("lfo.rate", 5.5),
            ("lfo.to_pitch", 0.12),
            ("op1.level", 0.85),
            ("op1.velocity", 0.5),
            ("op1.attack", 40.0),
            ("op1.decay", 200.0),
            ("op1.sustain", 0.85),
            ("op1.release", 150.0),
            ("op2.coarse", 1.0),
            ("op2.level", 0.5),
            ("op2.velocity", 0.8),
            ("op2.attack", 90.0),
            ("op2.decay", 250.0),
            ("op2.sustain", 0.6),
            ("op2.release", 150.0),
            ("op3.coarse", 1.0),
            ("op3.detune", 5.0),
            ("op3.level", 0.25),
            ("op3.velocity", 0.6),
            ("op3.attack", 120.0),
            ("op3.decay", 300.0),
            ("op3.sustain", 0.7),
            ("op3.release", 150.0),
            ("op4.coarse", 1.0),
            ("op4.level", 0.2),
            ("op4.velocity", 0.5),
            ("op4.attack", 100.0),
            ("op4.decay", 300.0),
            ("op4.sustain", 0.6),
            ("op4.release", 150.0),
        ],
    },
    Sound {
        name: "Pad",
        changes: &[
            ("algorithm", 1.0),
            ("spread", 0.8),
            ("lfo.wave", 1.0),
            ("lfo.rate", 0.35),
            ("lfo.to_index", 0.25),
            ("op1.detune", 7.0),
            ("op1.level", 0.6),
            ("op1.velocity", 0.1),
            ("op1.attack", 700.0),
            ("op1.decay", 1_500.0),
            ("op1.sustain", 0.8),
            ("op1.release", 1_500.0),
            ("op2.coarse", 2.0),
            ("op2.level", 0.22),
            ("op2.velocity", 0.2),
            ("op2.attack", 900.0),
            ("op2.decay", 2_000.0),
            ("op2.sustain", 0.6),
            ("op2.release", 1_500.0),
            ("op3.coarse", 1.0),
            ("op3.detune", -7.0),
            ("op3.level", 0.55),
            ("op3.velocity", 0.1),
            ("op3.attack", 900.0),
            ("op3.decay", 1_500.0),
            ("op3.sustain", 0.8),
            ("op3.release", 1_500.0),
            ("op4.coarse", 3.0),
            ("op4.level", 0.16),
            ("op4.velocity", 0.2),
            ("op4.attack", 1_200.0),
            ("op4.decay", 2_000.0),
            ("op4.sustain", 0.5),
            ("op4.release", 1_500.0),
        ],
    },
];

/// Every param of the synth but `sound` with its value for sound `index` (out of range clamps):
/// the sound's own value, or the init value for a param it leaves alone.
pub fn values(index: usize) -> Vec<(&'static str, f32)> {
    let sound = &SOUNDS[index.min(SOUNDS.len() - 1)];
    DESCRIPTOR
        .params
        .iter()
        .filter(|p| p.key != "sound")
        .map(|p| {
            let v = sound
                .changes
                .iter()
                .find(|(k, _)| *k == p.key)
                .map_or(p.default, |(_, v)| *v);
            (p.key, v)
        })
        .collect()
}
