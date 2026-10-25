//! Control-surface drivers: plain Rust, no I/O, no model.
//!
//! A driver turns the bytes a surface sends into [`SurfaceAction`]s (what the user did, in the
//! units of the controls) and turns a [`SurfaceState`] (what the project looks like) into the
//! bytes that make the surface show it (motor faders, LEDs, rings, displays). The app owns the
//! ports, the model and the thread: it applies the actions as ordinary commands, builds the
//! state from the document, throttles the calls to [`SurfaceDriver::feedback`] and sends what
//! comes back. Because a driver only ever sees values, every one of them is unit-tested
//! without hardware (`mcu` has the exact bytes of every message it sends).
//!
//! Strips are numbered **in console order** (channels, then output buses, then VCA groups; the
//! track folders of the arrangement do not reorder the console), the same list the console
//! view draws. The driver owns the bank (which eight strips the surface shows).

pub mod mcu;

pub use mcu::Mcu;

/// What kind of console strip a surface strip stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StripKind {
    /// An instrument or audio channel.
    #[default]
    Channel,
    /// An output bus.
    Bus,
    /// A VCA group (its fader is the group's offset; it has no pan, record or select).
    Vca,
}

/// One console strip as a surface shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StripState {
    /// The name on the scribble strip.
    pub name: String,
    /// Channel, bus or VCA group.
    pub kind: StripKind,
    /// The fader position, 0..=1 (the position of the console's own fader).
    pub fader: f32,
    /// The fader as text for the lower display row (`-6.0 dB`).
    pub fader_text: String,
    /// The pan, -1..=1, when the strip has one.
    pub pan: Option<f32>,
    /// The pan as text (`L20`, `C`, `R35`).
    pub pan_text: String,
    /// The strip is muted.
    pub mute: bool,
    /// The strip is soloed.
    pub solo: bool,
    /// The strip's track is armed for recording.
    pub armed: bool,
    /// The strip is selected.
    pub selected: bool,
}

/// The master fader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MasterState {
    /// The fader position, 0..=1.
    pub fader: f32,
}

/// The transport as the surface shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportLeds {
    /// Playing.
    pub playing: bool,
    /// Recording.
    pub recording: bool,
    /// The loop is on.
    pub looping: bool,
}

/// The play position in the two readouts a surface can show.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PositionState {
    /// Bar, 1-based.
    pub bar: u32,
    /// Beat in the bar, 1-based.
    pub beat: u32,
    /// Sixteenth in the beat, 1-based.
    pub sixteenth: u32,
    /// Ticks in the sixteenth, 0..=239.
    pub ticks: u32,
    /// Seconds from the start of the song.
    pub seconds: f64,
}

/// Everything a surface shows: the strips in console order, the master and the transport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceState {
    /// Every strip, in console order.
    pub strips: Vec<StripState>,
    /// The master fader, when the project has a master.
    pub master: Option<MasterState>,
    /// The transport lamps.
    pub transport: TransportLeds,
    /// The play position.
    pub position: PositionState,
}

/// A transport key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportButton {
    /// Play.
    Play,
    /// Stop.
    Stop,
    /// Record.
    Record,
    /// Rewind (one bar back).
    Rewind,
    /// Fast forward (one bar on).
    FastForward,
    /// The loop (cycle) switch.
    Cycle,
}

/// What the user did on the surface. A strip is an index into [`SurfaceState::strips`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceAction {
    /// A fader was moved to `value`, 0..=1.
    Fader {
        /// The strip.
        strip: usize,
        /// The new position.
        value: f32,
    },
    /// A finger touched or left a fader (the start and the end of an automation touch).
    Touch {
        /// The strip.
        strip: usize,
        /// Touched or released.
        touched: bool,
    },
    /// The master fader was moved.
    MasterFader(f32),
    /// A finger touched or left the master fader.
    MasterTouch(bool),
    /// A V-pot turned: `delta` is a fraction of the pan range (-1..=1 spans the whole pan).
    Pan {
        /// The strip.
        strip: usize,
        /// The turn, positive clockwise.
        delta: f32,
    },
    /// The mute key.
    ToggleMute(usize),
    /// The solo key.
    ToggleSolo(usize),
    /// The record-ready key.
    ToggleArm(usize),
    /// The select key.
    Select(usize),
    /// A transport key (pressed).
    Transport(TransportButton),
    /// The jog wheel turned by this many detents (positive is forward).
    Jog(i32),
}

/// A control-surface protocol.
pub trait SurfaceDriver: Send {
    /// A short name for the settings and messages.
    fn name(&self) -> &'static str;

    /// The surface was just connected: appends what to send to bring it online and, since
    /// nothing is known about what it shows, everything it should show for `state`.
    fn connect(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>);

    /// Handles one message from the surface (a channel message or a whole SysEx frame): appends
    /// the user's actions to `actions` and any immediate reply to `replies`.
    fn on_message(
        &mut self,
        bytes: &[u8],
        state: &SurfaceState,
        actions: &mut Vec<SurfaceAction>,
        replies: &mut Vec<Vec<u8>>,
    );

    /// Appends the messages that make the surface show `state`, **only what differs** from what
    /// was sent before (calling it twice with the same state sends nothing the second time).
    fn feedback(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>);

    /// The surface is going away: appends what to send to let it go offline.
    fn disconnect(&mut self, out: &mut Vec<Vec<u8>>);
}

/// The drivers the settings offer, by the name saved in the settings file.
pub fn driver_by_id(id: &str) -> Option<Box<dyn SurfaceDriver>> {
    match id {
        "mcu" => Some(Box::new(Mcu::new())),
        _ => None,
    }
}

/// The ids and labels of the drivers, in the order the settings list them.
pub const DRIVERS: &[(&str, &str)] = &[("mcu", "Control surface (MCU-compatible)")];
