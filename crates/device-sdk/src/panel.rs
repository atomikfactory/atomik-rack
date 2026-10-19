//! Optional panel layouts for device front and back panels.
//!
//! Coordinates are logical units at 1× scale. The rack is [`RACK_WIDTH`] units wide and a
//! device is `height_units ×` [`RACK_UNIT`] units tall. `(x, y)` is the centre of the control.
//! A device without a layout gets an automatically generated one from its descriptor.

/// Rack width in logical units.
pub const RACK_WIDTH: f32 = 760.0;
/// Height of one rack unit in logical units.
pub const RACK_UNIT: f32 = 76.0;

/// How a parameter is presented on the front panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// Rotary knob.
    Knob,
    /// Vertical fader.
    Fader,
    /// Latching on/off button.
    Toggle,
    /// Choice selector (for `Unit::Choice` params).
    Choice,
    /// Read-only numeric display.
    Display,
}

/// One control on the front panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelControl {
    /// Param key in the device descriptor.
    pub param: &'static str,
    /// Presentation.
    pub kind: ControlKind,
    /// Centre x in logical units.
    pub x: f32,
    /// Centre y in logical units (from the panel top).
    pub y: f32,
}

/// One socket on the back panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelSocket {
    /// Socket key in the device descriptor.
    pub socket: &'static str,
    /// Centre x in logical units.
    pub x: f32,
    /// Centre y in logical units (from the panel top).
    pub y: f32,
}

/// A hand-made layout for one device type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelLayout {
    /// Device type id this layout belongs to.
    pub type_id: &'static str,
    /// Panel height in rack units (1..=8).
    pub height_units: u8,
    /// Front-panel controls.
    pub front: &'static [PanelControl],
    /// Back-panel sockets.
    pub back: &'static [PanelSocket],
}
