//! The factory sounds of the granular sampler: a value for every param but `sound`.
//!
//! A sound is only a starting point: choosing one in the app sets all of these params in one
//! undo step (`atomik-app` expands a `sound` change into a batch of param changes) and the
//! player tweaks from there. The device itself ignores the `sound` param, it only remembers
//! which one was chosen last. Each sound lists the params that differ from the init sound (the
//! defaults of the descriptor); [`values`] fills in the rest. The sounds are made for the
//! built-in source, a vowel pad that glides ah, oh, eh, ee along the position (0 %, 33 %, 66 %
//! and 100 %), and work as well on a sample of your own. The values are this design's own; the
//! names describe the sound.
//!
//! | Sound | Idea |
//! |---|---|
//! | Init | a cloud of 90 ms Hann grains at 24 a second near the start of the sample, a little spray, the filter off |
//! | Frozen Pad | the position frozen on the "oh", long dense grains, six cents of pitch scatter, a slow swell, a low-pass that breathes with a slow LFO; the wheel opens the spray |
//! | Shimmer | an octave up, short Tukey grains with a third of them reversed and 30 cents of scatter, wide, a high-pass, a slow scan through the sample |
//! | Stutter | percussive 55 ms grains at 9 a second with no timing scatter and no spray: the same slice repeats; a square LFO jumps the position between two slices, and each note starts faster and settles |
//! | Texture | a half-speed scan through the sample with a wide spray, half the grains reversed, 45 cents of scatter, an LFO on the grain size and a filter that opens and closes with each note |

use super::DESCRIPTOR;

/// The names of the sounds, in selector order.
pub const SOUND_NAMES: [&str; 5] = ["Init", "Frozen Pad", "Shimmer", "Stutter", "Texture"];

/// A factory sound.
#[derive(Debug, Clone, Copy)]
pub struct Sound {
    /// Name shown in the sound selector.
    pub name: &'static str,
    /// `(param key, plain value)` of the params that differ from the init sound.
    pub changes: &'static [(&'static str, f32)],
}

/// The sounds, in selector order.
pub const SOUNDS: [Sound; 5] = [
    Sound {
        name: "Init",
        changes: &[],
    },
    Sound {
        name: "Frozen Pad",
        changes: &[
            ("master.level", -7.0),
            ("grain.density", 36.0),
            ("grain.size", 240.0),
            ("grain.timing", 0.6),
            ("grain.spread", 0.9),
            ("grain.limit", 64.0),
            ("pos.position", 0.3),
            ("pos.spray", 0.05),
            ("pitch.random", 6.0),
            ("env.amp.attack", 900.0),
            ("env.amp.decay", 600.0),
            ("env.amp.release", 2_200.0),
            ("velocity.amp", 0.3),
            ("filter.enabled", 1.0),
            ("filter.cutoff", 3_600.0),
            ("filter.resonance", 0.15),
            ("filter.key_track", 0.4),
            ("lfo.rate", 0.18),
            ("lfo.to_position", 0.04),
            ("lfo.to_cutoff", 0.5),
            ("wheel.to_spray", 0.6),
        ],
    },
    Sound {
        name: "Shimmer",
        changes: &[
            ("master.level", -9.0),
            ("grain.density", 60.0),
            ("grain.size", 70.0),
            ("grain.window", 1.0),
            ("grain.timing", 0.5),
            ("grain.reverse", 0.3),
            ("grain.spread", 1.0),
            ("grain.limit", 56.0),
            ("pos.position", 0.45),
            ("pos.spray", 0.25),
            ("pos.scan", 0.12),
            ("pitch.transpose", 12.0),
            ("pitch.random", 30.0),
            ("env.amp.attack", 400.0),
            ("env.amp.sustain", 0.9),
            ("env.amp.release", 1_600.0),
            ("filter.enabled", 1.0),
            ("filter.mode", 2.0),
            ("filter.cutoff", 500.0),
            ("filter.resonance", 0.1),
            ("lfo.rate", 5.2),
            ("lfo.to_pitch", 0.12),
        ],
    },
    Sound {
        name: "Stutter",
        changes: &[
            ("master.level", -5.0),
            ("grain.density", 9.0),
            ("grain.size", 55.0),
            ("grain.window", 2.0),
            ("grain.timing", 0.0),
            ("grain.spread", 0.2),
            ("grain.limit", 24.0),
            ("pos.position", 0.52),
            ("pos.spray", 0.0),
            ("env.amp.attack", 1.0),
            ("env.amp.release", 120.0),
            ("env.mod.decay", 700.0),
            ("env.mod.to_density", 1.0),
            ("lfo.wave", 3.0),
            ("lfo.rate", 2.0),
            ("lfo.to_position", 0.18),
        ],
    },
    Sound {
        name: "Texture",
        changes: &[
            ("master.level", -8.0),
            ("grain.density", 55.0),
            ("grain.size", 130.0),
            ("grain.window", 1.0),
            ("grain.timing", 0.8),
            ("grain.reverse", 0.5),
            ("grain.spread", 0.9),
            ("grain.limit", 64.0),
            ("pos.position", 0.1),
            ("pos.spray", 0.3),
            ("pos.scan", 0.4),
            ("pitch.random", 45.0),
            ("env.amp.attack", 120.0),
            ("env.amp.release", 900.0),
            ("filter.enabled", 1.0),
            ("filter.cutoff", 2_200.0),
            ("filter.resonance", 0.35),
            ("filter.env_amount", 0.5),
            ("env.mod.decay", 900.0),
            ("lfo.wave", 1.0),
            ("lfo.rate", 0.3),
            ("lfo.to_size", 0.5),
        ],
    },
];

/// Every param of the device but `sound` with its value for sound `index` (out of range clamps):
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
