//! Row 1 of the preset matrix: `reel`, `loudness`, `sparkle`, `low-end` (indices 0 to 3), all on
//! the generic rack. The stage lists are the topology (code); the sends are in
//! `data/character-routes.csv` and every number in `data/character-voicing.csv`.
//!
//! | Idx | Preset | Signal flow |
//! |---|---|---|
//! | 0 | `reel` | head-bump low shelf, asymmetric saturation (oversampled), low-pass, wow and flutter; pink hiss added after the wobble |
//! | 1 | `loudness` | low shelf, mid dip, high shelf, soft compressor |
//! | 2 | `sparkle` | high-pass and high shelf; a parallel exciter (band-limited by a high-pass and a low-pass, oversampled saturation, a low-pass after it) blended in by Transform |
//! | 3 | `low-end` | high-pass and a resonant low bump; a parallel mono band (40 to 120 Hz) through a fast compressor and saturation, blended in by Transform |

use super::chain::{PresetChain, Routing};
use super::rack::{Kind, RackChain, RackDef, StageDef};
use atomik_dsp::ShaperKind;

const fn st(name: &'static str, kind: Kind) -> StageDef {
    StageDef { name, kind }
}

/// `reel`: analog tape.
pub static REEL: RackDef = RackDef {
    stages: &[
        st("bump", Kind::LowShelf),
        st("sat", Kind::Sat(ShaperKind::Asymmetric)),
        st("lp", Kind::LowPass),
        st("wobble", Kind::Wobble),
        st("hiss", Kind::Hiss),
    ],
    routing: Routing::Stereo,
};

/// `loudness`: the consumer contour.
pub static LOUDNESS: RackDef = RackDef {
    stages: &[
        st("low", Kind::LowShelf),
        st("mid", Kind::Peak),
        st("high", Kind::HighShelf),
        st("comp", Kind::Comp),
    ],
    routing: Routing::Stereo,
};

/// `sparkle`: brightness with a parallel exciter.
pub static SPARKLE: RackDef = RackDef {
    stages: &[
        st("hp", Kind::HighPass),
        st("shelf", Kind::HighShelf),
        st("xhp", Kind::HighPass),
        st("xlp", Kind::LowPass),
        st("xsat", Kind::Sat(ShaperKind::SoftClip)),
        st("xpost", Kind::LowPass),
    ],
    routing: Routing::Stereo,
};

/// `low-end`: tight lows with a parallel mono band.
pub static LOW_END: RackDef = RackDef {
    stages: &[
        st("hp", Kind::HighPass),
        st("bump", Kind::Peak),
        st("bhp", Kind::HighPass),
        st("blp", Kind::LowPass),
        st("mono", Kind::Mono),
        st("comp", Kind::Comp),
        st("sat", Kind::Sat(ShaperKind::SoftClip)),
    ],
    routing: Routing::Stereo,
};

/// The rack definitions of this row with their preset indices.
#[cfg(test)]
pub fn racks() -> [(usize, &'static RackDef); 4] {
    [(0, &REEL), (1, &LOUDNESS), (2, &SPARKLE), (3, &LOW_END)]
}

/// The chain of preset `idx` (0 to 3).
pub fn build(idx: usize) -> Box<dyn PresetChain> {
    let def = match idx {
        0 => &REEL,
        1 => &LOUDNESS,
        2 => &SPARKLE,
        _ => &LOW_END,
    };
    Box::new(RackChain::new(def))
}
