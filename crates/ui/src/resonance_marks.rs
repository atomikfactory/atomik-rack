//! What the dynamic EQ panel shows of the resonance finder: the resonances found in the sound
//! at the device's input (the markers on the plot) and whether a search is running.
//!
//! The app fills it ([`crate::RackView::set_resonances`]); the panel only draws it and turns the
//! FIND and CUT buttons into [`crate::UiAction::FindResonances`] and
//! [`crate::UiAction::CutResonances`].

/// One resonance, as the panel draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResonanceMark {
    /// Centre frequency, Hz.
    pub freq_hz: f32,
    /// Estimated Q.
    pub q: f32,
    /// How far the peak stands above the trend, dB.
    pub prominence_db: f32,
}

/// The finder's state for one dynamic EQ.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResonanceView {
    /// The resonances of the last search, strongest first.
    pub marks: Vec<ResonanceMark>,
    /// A search is running.
    pub busy: bool,
}
