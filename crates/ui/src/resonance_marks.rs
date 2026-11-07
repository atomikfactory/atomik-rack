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

/// A button of the finder on the dynamic EQ plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResonanceBtn {
    /// FIND: search the sound at the input for resonances.
    Find,
    /// CUT: turn the strongest resonances into dynamic cuts.
    Cut,
}

/// Width of a finder button, px.
pub const BUTTON_W: f64 = 54.0;
/// Height of a finder button, px.
pub const BUTTON_H: f64 = 16.0;

/// The two buttons in the top right corner of the plot `plot` (panel coordinates): FIND left of
/// CUT.
pub fn buttons(plot: vello::kurbo::Rect) -> [(ResonanceBtn, vello::kurbo::Rect); 2] {
    use vello::kurbo::Rect;
    let top = plot.y0 + 5.0;
    let cut = Rect::new(plot.x1 - 6.0 - BUTTON_W, top, plot.x1 - 6.0, top + BUTTON_H);
    let find = Rect::new(cut.x0 - 4.0 - BUTTON_W, top, cut.x0 - 4.0, top + BUTTON_H);
    [(ResonanceBtn::Find, find), (ResonanceBtn::Cut, cut)]
}
