//! Atomik Rack tablature, pure and headless.
//!
//! This crate holds everything about fretted instruments that needs neither a window nor an
//! audio engine:
//!
//! * [`instrument`]: strings, tunings, capos and the built-in catalogue of instruments (tab order,
//!   re-entrant tunings, the banjo's short string, doubled courses).
//! * [`fretting`]: choosing a string and fret for pitches, notes, chords and whole sequences.
//! * [`chord`], [`transpose`]: the chord-name grammar and the key-aware transposer with capo
//!   shapes.
//! * [`voicing`]: chord shapes for any tuning, with fingers, barres, ranking and a cache.
//! * [`sheet`]: chord sheets (chords over lyrics, bracket form with directives), their writers
//!   and transposition.
//! * [`score`]: the neutral tab document (`TabScore`, tracks, notes, techniques) that every file
//!   reader and writer converts to and from.
//! * [`ascii`]: plain-text tab, read and written.
//!
//! Every parser caps its input (bytes, lines, notes) and never panics on hostile text. The crate has
//! no `unsafe` code (the workspace denies it, and a policy test keeps the word out of the pure
//! crates).

pub mod ascii;
pub mod chord;
pub mod fretting;
pub mod instrument;
pub mod score;
pub mod sheet;
pub mod transpose;
pub mod voicing;

pub use chord::{ChordError, ChordSymbol, Note};
pub use instrument::{Capo, Instrument, NamedTuning, StringDef, Tuning, catalogue};
