//! Row 3 of the preset matrix: `lossy`, `swirl`, `torn-cone`, `tin-robot` (indices 8 to 11), all
//! on the generic rack. The stage lists are the topology (code); the sends are in
//! `data/character-routes.csv` and every number in `data/character-voicing.csv`.
//!
//! | Idx | Preset | Signal flow |
//! |---|---|---|
//! | 8 | `lossy` | top bell lift, **spectral stage** (gate with hold, coarse level quantiser, seeded dropouts), top bell dip, resonant low-pass edge; **long latency class** |
//! | 9 | `swirl` | six-pole phaser, slow pitch wobble, rotating auto-pan whose rate follows the phaser, feedback echo |
//! | 10 | `torn-cone` | high-pass, oversampled hard asymmetric clip, resonant cone peak, low-pass, rattle keyed by the envelope, output compressor; mono |
//! | 11 | `tin-robot` | short comb resonance with a seeded slow detune per channel, band from a high-pass and a low-pass, two formant-like peaks, ring modulation |

use super::chain::{PresetChain, Routing};
use super::rack::{Kind, RackChain, RackDef, StageDef};
use atomik_dsp::ShaperKind;

const fn st(name: &'static str, kind: Kind) -> StageDef {
    StageDef { name, kind }
}

/// `lossy`: poorly coded and decoded audio (the long latency class).
pub static LOSSY: RackDef = RackDef {
    stages: &[
        st("bell1", Kind::Peak),
        st("spec", Kind::Spectral),
        st("bell2", Kind::Peak),
        st("lp", Kind::LowPass),
    ],
    routing: Routing::Stereo,
};

/// `swirl`: psychedelic phasing, wobble, rotating pan and echo.
pub static SWIRL: RackDef = RackDef {
    stages: &[
        st("ph", Kind::Phaser),
        st("wob", Kind::Wobble),
        st("pan", Kind::AutoPan),
        st("echo", Kind::Echo),
    ],
    routing: Routing::Stereo,
};

/// `torn-cone`: a broken speaker.
pub static TORN_CONE: RackDef = RackDef {
    stages: &[
        st("hp", Kind::HighPass),
        st("sat", Kind::Sat(ShaperKind::HardAsymmetric)),
        st("cone", Kind::Peak),
        st("lp", Kind::LowPass),
        st("rattle", Kind::Rattle),
        st("comp", Kind::Comp),
    ],
    routing: Routing::MonoSum,
};

/// `tin-robot`: hollow and robotic.
pub static TIN_ROBOT: RackDef = RackDef {
    stages: &[
        st("comb", Kind::Comb),
        st("hp", Kind::HighPass),
        st("lp", Kind::LowPass),
        st("f1", Kind::Peak),
        st("f2", Kind::Peak),
        st("ring", Kind::RingMod),
    ],
    routing: Routing::Stereo,
};

/// The rack definitions of this row with their preset indices.
#[cfg(test)]
pub fn racks() -> [(usize, &'static RackDef); 4] {
    [(8, &LOSSY), (9, &SWIRL), (10, &TORN_CONE), (11, &TIN_ROBOT)]
}

/// The chain of preset `idx` (8 to 11).
pub fn build(idx: usize) -> Box<dyn PresetChain> {
    let def = match idx {
        8 => &LOSSY,
        9 => &SWIRL,
        10 => &TORN_CONE,
        _ => &TIN_ROBOT,
    };
    Box::new(RackChain::new(def))
}
