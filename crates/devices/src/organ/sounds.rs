//! The factory sounds of the organ: a value for every param but `sound`.
//!
//! A sound is only a starting point: choosing one in the app sets all of these params in one
//! undo step (`atomik-app` expands a `sound` change into a batch of param changes) and the
//! player tweaks from there. The device itself ignores the `sound` param, it only remembers
//! which one was chosen last. Each sound lists the params that differ from the init sound (the
//! defaults of the descriptor); [`values`] fills in the rest. The registrations are written as
//! the nine drawbar positions from 16' to 1'.
//!
//! | Sound | Registration | Idea |
//! |---|---|---|
//! | Init | 888 000 000 | the three lowest footages, no percussion, no scanner |
//! | Jazz | 888 000 000 | the same bars with a fast third-harmonic percussion and a full chorus |
//! | Full | 888 888 888 | every drawbar out, a deep vibrato and a little overdrive |
//! | Rock | 888 800 000 | a bright, driven sound with a medium vibrato and a firm key click |
//! | Mellow | 008 603 000 | a soft flute-like tone with a light chorus |
//! | Church | 608 604 002 | a pipe-like stack of octaves and fifths, no scanner, hardly any click |

use super::DESCRIPTOR;

/// The names of the sounds, in selector order.
pub const SOUND_NAMES: [&str; 6] = ["Init", "Jazz", "Full", "Rock", "Mellow", "Church"];

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
        name: "Jazz",
        changes: &[
            ("perc.mode", 2.0),
            ("perc.decay", 0.0),
            ("perc.level", 1.0),
            ("scanner.type", 6.0),
            ("out.drive", 0.1),
        ],
    },
    Sound {
        name: "Full",
        changes: &[
            ("drawbar.4", 8.0),
            ("drawbar.2_2_3", 8.0),
            ("drawbar.2", 8.0),
            ("drawbar.1_3_5", 8.0),
            ("drawbar.1_1_3", 8.0),
            ("drawbar.1", 8.0),
            ("key.click", 0.2),
            ("scanner.type", 3.0),
            ("out.drive", 0.25),
            ("out.level", -3.0),
        ],
    },
    Sound {
        name: "Rock",
        changes: &[
            ("drawbar.4", 8.0),
            ("key.click", 0.5),
            ("scanner.type", 2.0),
            ("out.drive", 0.7),
            ("out.level", -3.0),
        ],
    },
    Sound {
        name: "Mellow",
        changes: &[
            ("drawbar.16", 0.0),
            ("drawbar.5_1_3", 0.0),
            ("drawbar.4", 6.0),
            ("drawbar.2", 3.0),
            ("key.click", 0.1),
            ("scanner.type", 4.0),
        ],
    },
    Sound {
        name: "Church",
        changes: &[
            ("drawbar.16", 6.0),
            ("drawbar.5_1_3", 0.0),
            ("drawbar.4", 6.0),
            ("drawbar.2_2_3", 0.0),
            ("drawbar.2", 4.0),
            ("drawbar.1", 2.0),
            ("key.click", 0.05),
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
