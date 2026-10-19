//! Row 4 of the preset matrix: `bit-bender`, `spin-cycle`, `ion-sweep`, `ghost-pipes` (indices 12
//! to 15), all on the generic rack.
//!
//! | Idx | Preset | Signal flow |
//! |---|---|---|
//! | 12 | `bit-bender` | bit and rate reduction at the base rate (no oversampling) with seeded stepped random modulation per channel, DC blocker; gated pops added |
//! | 13 | `spin-cycle` | drum tremolo, sloshing swept low-pass, a resonant box in parallel and a long echo wash that Transform brings in; motor hum and water bursts added |
//! | 14 | `ion-sweep` | comb flanger and resonant band-pass swept together by Transform (the sweep position), faint ring tone |
//! | 15 | `ghost-pipes` | high-pass on the excitation, twelve-mode inharmonic resonator bank |

use super::chain::{PresetChain, Routing};
use super::rack::{Kind, RackChain, RackDef, StageDef};

const fn st(name: &'static str, kind: Kind) -> StageDef {
    StageDef { name, kind }
}

/// `bit-bender`: a circuit-bent bit crusher.
pub static BIT_BENDER: RackDef = RackDef {
    stages: &[
        st("crush", Kind::Crush),
        st("dc", Kind::HighPass),
        st("pops", Kind::Crackle),
    ],
    routing: Routing::Stereo,
};

/// `spin-cycle`: a household appliance.
pub static SPIN_CYCLE: RackDef = RackDef {
    stages: &[
        st("trem", Kind::Tremolo),
        st("slosh", Kind::SweepLp),
        st("room", Kind::Room),
        st("echo", Kind::Echo),
        st("hum", Kind::Hum),
        st("water", Kind::Bursts),
    ],
    routing: Routing::Stereo,
};

/// `ion-sweep`: a sci-fi sweep.
pub static ION_SWEEP: RackDef = RackDef {
    stages: &[
        st("flange", Kind::Comb),
        st("bp", Kind::BandPass),
        st("ring", Kind::RingMod),
    ],
    routing: Routing::Stereo,
};

/// `ghost-pipes`: an eerie resonator.
pub static GHOST_PIPES: RackDef = RackDef {
    stages: &[st("hp", Kind::HighPass), st("modal", Kind::Modal)],
    routing: Routing::Stereo,
};

/// The rack definitions of this row with their preset indices.
#[cfg(test)]
pub fn racks() -> [(usize, &'static RackDef); 4] {
    [
        (12, &BIT_BENDER),
        (13, &SPIN_CYCLE),
        (14, &ION_SWEEP),
        (15, &GHOST_PIPES),
    ]
}

/// The chain of preset `idx` (12 to 15).
pub fn build(idx: usize) -> Box<dyn PresetChain> {
    let def = match idx {
        12 => &BIT_BENDER,
        13 => &SPIN_CYCLE,
        14 => &ION_SWEEP,
        _ => &GHOST_PIPES,
    };
    Box::new(RackChain::new(def))
}
