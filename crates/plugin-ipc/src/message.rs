//! The messages of the control channel, and the serde mirrors of the host types they carry.
//!
//! Requests carry an `id` and the answer echoes it; notifications have none. The variant is
//! named by the `"t"` field. [`WireLayout`] reuses the field names (and the text values) of the
//! scan JSON, so a layout reads the same everywhere.

use atomik_device_sdk::{Channels, PluginKindHint, PluginLayout, PluginParamInfo};
use atomik_plugin_host::{BackendError, LogLevel, NativeParent, ParamPhase};
use serde::{Deserialize, Serialize};

/// Parent to child.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ToChild {
    /// First message: the protocol the parent speaks.
    Hello {
        /// [`crate::PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// Creates the plugin named on the command line.
    Instantiate {
        /// Request id.
        id: u64,
    },
    /// Activates the instance.
    Activate {
        /// Request id.
        id: u64,
        /// Session sample rate.
        sample_rate: f64,
        /// Block size (always 64).
        frames: u32,
        /// Offline bounce rather than live.
        offline: bool,
    },
    /// Deactivates the instance.
    Deactivate {
        /// Request id.
        id: u64,
    },
    /// Saves the plugin state; the answer is [`FromChild::State`].
    CaptureState {
        /// Request id.
        id: u64,
    },
    /// Restores a saved state; the blob follows the frame.
    RestoreState {
        /// Request id.
        id: u64,
    },
    /// Asks for the display text of a param value.
    ParamText {
        /// Request id.
        id: u64,
        /// The plugin's param id.
        param: u32,
        /// The plain value.
        plain: f64,
    },
    /// Asks for the latency now.
    LatencyFrames {
        /// Request id.
        id: u64,
    },
    /// Asks for the params a panel should pin by default.
    DefaultPins {
        /// Request id.
        id: u64,
    },
    /// Asks whether the param table changed (and clears the flag).
    TakeLayoutChanged {
        /// Request id.
        id: u64,
    },
    /// Opens (or raises) the plugin's own editor window.
    OpenGui {
        /// Request id.
        id: u64,
        /// The window to own it, when the platform allows.
        owner: Option<WireParent>,
        /// Display scale.
        scale: f64,
    },
    /// Closes the editor window.
    CloseGui {
        /// Request id.
        id: u64,
    },
    /// Sends every notification the plugin has posted so far, then replies `Ok`. A bounce asks
    /// for it after each batch, so a plugin callback (a latency change, a restart request)
    /// reaches the host before the next batch, as it does in process, and never depends on time.
    Flush {
        /// Request id.
        id: u64,
    },
    /// Ends the child.
    Shutdown,
}

/// Child to parent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum FromChild {
    /// First message: the protocol the child speaks and its process id.
    Hello {
        /// [`crate::PROTOCOL_VERSION`].
        protocol: u32,
        /// The child's process id.
        pid: u32,
    },
    /// The request succeeded and has no result.
    Ok {
        /// The request's id.
        id: u64,
    },
    /// The request failed.
    Err {
        /// The request's id.
        id: u64,
        /// Which [`BackendError`] variant.
        kind: WireErrorKind,
        /// The error's text.
        text: String,
    },
    /// The plugin was created.
    Instantiated {
        /// The request's id.
        id: u64,
        /// What the plugin looks like.
        layout: WireLayout,
        /// The plugin has an editor.
        has_gui: bool,
        /// Latency now, in frames.
        latency: u32,
    },
    /// The instance is active.
    Activated {
        /// The request's id.
        id: u64,
        /// Latency now, in frames.
        latency: u32,
    },
    /// The plugin state; the blob follows the frame.
    State {
        /// The request's id.
        id: u64,
    },
    /// The display text of a param value.
    Text {
        /// The request's id.
        id: u64,
        /// The text, when the plugin has one.
        text: Option<String>,
    },
    /// The latency now.
    Latency {
        /// The request's id.
        id: u64,
        /// Frames.
        frames: u32,
    },
    /// The params to pin by default.
    Pins {
        /// The request's id.
        id: u64,
        /// Param ids.
        pins: Vec<u32>,
    },
    /// Whether the param table changed, and the new table when it did.
    LayoutChanged {
        /// The request's id.
        id: u64,
        /// The new layout; `None` when nothing changed.
        layout: Option<WireLayout>,
    },
    /// The editor window is open.
    GuiOpened {
        /// The request's id.
        id: u64,
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// Notification: a param was edited in the plugin's own editor.
    ParamEdit {
        /// The plugin's param id.
        param: u32,
        /// The plain value.
        value: f64,
        /// Which part of the gesture.
        phase: WirePhase,
    },
    /// Notification: the plugin's state changed.
    StateDirty,
    /// Notification: the latency changed.
    LatencyChanged,
    /// Notification: the param table changed.
    RescanParams,
    /// Notification: the plugin asks to be restarted.
    RequestRestart,
    /// Notification: the plugin resized its editor.
    GuiResize {
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// Notification: the editor window was closed.
    GuiClosed,
    /// Notification: the plugin asks to show its editor.
    GuiRequestShow,
    /// Notification: the plugin asks to hide its editor.
    GuiRequestHide,
    /// Notification: a log line of the plugin.
    Log {
        /// Severity.
        level: WireLevel,
        /// The line.
        text: String,
    },
}

/// The window an editor is owned by ([`NativeParent`] on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireParent {
    /// A Windows `HWND`.
    Win32(u64),
    /// A macOS `NSView`.
    Cocoa(u64),
    /// An X11 window id.
    X11(u32),
}

impl From<NativeParent> for WireParent {
    fn from(p: NativeParent) -> Self {
        match p {
            NativeParent::Win32(h) => Self::Win32(h as u64),
            NativeParent::Cocoa(h) => Self::Cocoa(h as u64),
            NativeParent::X11(w) => Self::X11(w),
        }
    }
}

impl From<WireParent> for NativeParent {
    fn from(p: WireParent) -> Self {
        match p {
            WireParent::Win32(h) => Self::Win32(h as usize),
            WireParent::Cocoa(h) => Self::Cocoa(h as usize),
            WireParent::X11(w) => Self::X11(w),
        }
    }
}

/// Which [`BackendError`] variant failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireErrorKind {
    /// [`BackendError::NotAPlugin`].
    NotAPlugin,
    /// [`BackendError::UnknownPlugin`].
    UnknownPlugin,
    /// [`BackendError::Unsupported`].
    Unsupported,
    /// [`BackendError::Failed`].
    Failed,
}

impl WireErrorKind {
    /// The kind and the text (without the variant's prefix) of an error.
    pub fn of(error: &BackendError) -> (Self, String) {
        match error {
            BackendError::NotAPlugin(t) => (Self::NotAPlugin, t.clone()),
            BackendError::UnknownPlugin(t) => (Self::UnknownPlugin, t.clone()),
            BackendError::Unsupported(t) => (Self::Unsupported, t.clone()),
            BackendError::Failed(t) => (Self::Failed, t.clone()),
        }
    }
}

/// The [`BackendError`] a child's `Err` message stands for.
pub fn into_backend_error(kind: WireErrorKind, text: String) -> BackendError {
    match kind {
        WireErrorKind::NotAPlugin => BackendError::NotAPlugin(text),
        WireErrorKind::UnknownPlugin => BackendError::UnknownPlugin(text),
        WireErrorKind::Unsupported => BackendError::Unsupported(text),
        WireErrorKind::Failed => BackendError::Failed(text),
    }
}

/// [`ParamPhase`] on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePhase {
    /// The user grabbed the control.
    Begin,
    /// The value moved.
    Change,
    /// The user let go.
    End,
}

impl From<ParamPhase> for WirePhase {
    fn from(p: ParamPhase) -> Self {
        match p {
            ParamPhase::Begin => Self::Begin,
            ParamPhase::Change => Self::Change,
            ParamPhase::End => Self::End,
        }
    }
}

impl From<WirePhase> for ParamPhase {
    fn from(p: WirePhase) -> Self {
        match p {
            WirePhase::Begin => Self::Begin,
            WirePhase::Change => Self::Change,
            WirePhase::End => Self::End,
        }
    }
}

/// [`LogLevel`] on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireLevel {
    /// Debug detail.
    Debug,
    /// Information.
    Info,
    /// Something is odd.
    Warning,
    /// Something failed.
    Error,
}

impl From<LogLevel> for WireLevel {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Debug => Self::Debug,
            LogLevel::Info => Self::Info,
            LogLevel::Warning => Self::Warning,
            LogLevel::Error => Self::Error,
        }
    }
}

impl From<WireLevel> for LogLevel {
    fn from(l: WireLevel) -> Self {
        match l {
            WireLevel::Debug => Self::Debug,
            WireLevel::Info => Self::Info,
            WireLevel::Warning => Self::Warning,
            WireLevel::Error => Self::Error,
        }
    }
}

/// [`PluginKindHint`] on the wire (the scan JSON's `kind` text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    /// A synth or sampler.
    Instrument,
    /// An audio effect.
    Effect,
    /// A note effect.
    NoteEffect,
}

/// [`Channels`] on the wire (the scan JSON's bus text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireChannels {
    /// One channel.
    Mono,
    /// Two channels.
    Stereo,
}

/// One parameter on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireParam {
    /// The plugin's own param id.
    pub id: u32,
    /// Display name.
    pub name: String,
    /// Module (group) path.
    #[serde(default)]
    pub module: String,
    /// Minimum plain value.
    pub min: f64,
    /// Maximum plain value.
    pub max: f64,
    /// Default value.
    pub default: f64,
    /// Discrete param.
    #[serde(default)]
    pub stepped: bool,
    /// Step count of a stepped param.
    #[serde(default)]
    pub steps: u32,
}

/// A [`PluginLayout`] on the wire: the field names of the scan JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireLayout {
    /// Plugin name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Version string.
    pub version: String,
    /// The plugin's own category.
    pub kind: WireKind,
    /// Main input bus.
    pub main_in: Option<WireChannels>,
    /// Main output bus.
    pub main_out: Option<WireChannels>,
    /// A sidechain bus exists.
    pub sidechain: bool,
    /// The plugin accepts notes.
    pub notes_in: bool,
    /// The plugin emits notes.
    pub notes_out: bool,
    /// The params, in the plugin's order.
    pub params: Vec<WireParam>,
}

/// JSON cannot carry NaN or infinity; the scanner makes the same repair.
fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

impl From<&PluginLayout> for WireLayout {
    fn from(l: &PluginLayout) -> Self {
        Self {
            name: l.name.clone(),
            vendor: l.vendor.clone(),
            version: l.version.clone(),
            kind: match l.kind_hint {
                PluginKindHint::Instrument => WireKind::Instrument,
                PluginKindHint::Effect => WireKind::Effect,
                PluginKindHint::NoteEffect => WireKind::NoteEffect,
            },
            main_in: l.main_in.map(WireChannels::from),
            main_out: l.main_out.map(WireChannels::from),
            sidechain: l.sidechain,
            notes_in: l.notes_in,
            notes_out: l.notes_out,
            params: l
                .params
                .iter()
                .map(|p| {
                    let min = finite(p.min, 0.0);
                    WireParam {
                        id: p.id,
                        name: p.name.clone(),
                        module: p.module.clone(),
                        min,
                        max: finite(p.max, min + 1.0),
                        default: finite(p.default, min),
                        stepped: p.stepped,
                        steps: p.steps,
                    }
                })
                .collect(),
        }
    }
}

impl From<WireLayout> for PluginLayout {
    fn from(w: WireLayout) -> Self {
        Self {
            name: w.name,
            vendor: w.vendor,
            version: w.version,
            kind_hint: match w.kind {
                WireKind::Instrument => PluginKindHint::Instrument,
                WireKind::Effect => PluginKindHint::Effect,
                WireKind::NoteEffect => PluginKindHint::NoteEffect,
            },
            main_in: w.main_in.map(Channels::from),
            main_out: w.main_out.map(Channels::from),
            sidechain: w.sidechain,
            notes_in: w.notes_in,
            notes_out: w.notes_out,
            params: w
                .params
                .into_iter()
                .map(|p| PluginParamInfo {
                    id: p.id,
                    name: p.name,
                    module: p.module,
                    min: p.min,
                    max: p.max,
                    default: p.default,
                    stepped: p.stepped,
                    steps: p.steps,
                })
                .collect(),
        }
    }
}

impl From<Channels> for WireChannels {
    fn from(c: Channels) -> Self {
        match c {
            Channels::Mono => Self::Mono,
            Channels::Stereo => Self::Stereo,
        }
    }
}

impl From<WireChannels> for Channels {
    fn from(c: WireChannels) -> Self {
        match c {
            WireChannels::Mono => Self::Mono,
            WireChannels::Stereo => Self::Stereo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{read_frame, write_frame};
    use atomik_device_sdk::{Channels, PluginKindHint, PluginLayout, PluginParamInfo};
    use atomik_plugin_host::{BackendError, LogLevel, NativeParent, ParamPhase};
    use std::io::Cursor;

    fn layout() -> PluginLayout {
        PluginLayout {
            name: "Gain".to_string(),
            vendor: "Atomik".to_string(),
            version: "1.2".to_string(),
            kind_hint: PluginKindHint::NoteEffect,
            main_in: Some(Channels::Mono),
            main_out: Some(Channels::Stereo),
            sidechain: true,
            notes_in: true,
            notes_out: false,
            params: vec![
                PluginParamInfo {
                    id: 7,
                    name: "Drive".to_string(),
                    module: "Main/Stage".to_string(),
                    min: -12.0,
                    max: 12.5,
                    default: 0.125,
                    stepped: false,
                    steps: 0,
                },
                PluginParamInfo {
                    id: u32::MAX,
                    name: "Mode".to_string(),
                    module: String::new(),
                    min: 0.0,
                    max: 3.0,
                    default: 1.0,
                    stepped: true,
                    steps: 3,
                },
            ],
        }
    }

    fn round_trip<T>(m: &T, blob: Option<&[u8]>) -> (T, Option<Vec<u8>>)
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, m, blob).unwrap();
        read_frame(&mut Cursor::new(bytes)).unwrap()
    }

    fn every_to_child() -> Vec<(ToChild, Option<Vec<u8>>)> {
        vec![
            (ToChild::Hello { protocol: 1 }, None),
            (ToChild::Instantiate { id: 3 }, None),
            (
                ToChild::Activate {
                    id: 3,
                    sample_rate: 44_100.0,
                    frames: 64,
                    offline: true,
                },
                None,
            ),
            (ToChild::Deactivate { id: 3 }, None),
            (ToChild::CaptureState { id: 3 }, None),
            (ToChild::RestoreState { id: 3 }, Some(vec![1, 2, 3, 0, 255])),
            (
                ToChild::ParamText {
                    id: 3,
                    param: 9,
                    plain: -0.1,
                },
                None,
            ),
            (ToChild::LatencyFrames { id: 3 }, None),
            (ToChild::DefaultPins { id: 3 }, None),
            (ToChild::TakeLayoutChanged { id: 3 }, None),
            (
                ToChild::OpenGui {
                    id: 3,
                    owner: Some(WireParent::Win32(0xdead_beef)),
                    scale: 1.5,
                },
                None,
            ),
            (
                ToChild::OpenGui {
                    id: 3,
                    owner: None,
                    scale: 1.0,
                },
                None,
            ),
            (ToChild::CloseGui { id: 3 }, None),
            (ToChild::Flush { id: 3 }, None),
            (ToChild::Shutdown, None),
        ]
    }

    fn every_from_child() -> Vec<(FromChild, Option<Vec<u8>>)> {
        let wire = WireLayout::from(&layout());
        vec![
            (
                FromChild::Hello {
                    protocol: 1,
                    pid: 4242,
                },
                None,
            ),
            (FromChild::Ok { id: 5 }, None),
            (
                FromChild::Err {
                    id: 5,
                    kind: WireErrorKind::Unsupported,
                    text: "no editor".to_string(),
                },
                None,
            ),
            (
                FromChild::Instantiated {
                    id: 5,
                    layout: wire.clone(),
                    has_gui: true,
                    latency: 32,
                },
                None,
            ),
            (FromChild::Activated { id: 5, latency: 64 }, None),
            (FromChild::State { id: 5 }, Some(vec![9; 100])),
            (
                FromChild::Text {
                    id: 5,
                    text: Some("-3.0 dB".to_string()),
                },
                None,
            ),
            (FromChild::Text { id: 5, text: None }, None),
            (FromChild::Latency { id: 5, frames: 17 }, None),
            (
                FromChild::Pins {
                    id: 5,
                    pins: vec![1, 2, 99],
                },
                None,
            ),
            (
                FromChild::LayoutChanged {
                    id: 5,
                    layout: Some(wire),
                },
                None,
            ),
            (
                FromChild::LayoutChanged {
                    id: 5,
                    layout: None,
                },
                None,
            ),
            (
                FromChild::GuiOpened {
                    id: 5,
                    w: 800,
                    h: 600,
                },
                None,
            ),
            (
                FromChild::ParamEdit {
                    param: 4,
                    value: 0.333_333_333_333_333_3,
                    phase: WirePhase::Change,
                },
                None,
            ),
            (FromChild::StateDirty, None),
            (FromChild::LatencyChanged, None),
            (FromChild::RescanParams, None),
            (FromChild::RequestRestart, None),
            (FromChild::GuiResize { w: 1, h: 2 }, None),
            (FromChild::GuiClosed, None),
            (FromChild::GuiRequestShow, None),
            (FromChild::GuiRequestHide, None),
            (
                FromChild::Log {
                    level: WireLevel::Warning,
                    text: "careful".to_string(),
                },
                None,
            ),
        ]
    }

    #[test]
    fn every_message_to_the_child_round_trips() {
        for (msg, blob) in every_to_child() {
            let (back, back_blob) = round_trip(&msg, blob.as_deref());
            assert_eq!(back, msg);
            assert_eq!(back_blob, blob);
        }
    }

    #[test]
    fn every_message_from_the_child_round_trips() {
        for (msg, blob) in every_from_child() {
            let (back, back_blob) = round_trip(&msg, blob.as_deref());
            assert_eq!(back, msg);
            assert_eq!(back_blob, blob);
        }
    }

    #[test]
    fn the_variant_is_named_by_the_t_field() {
        let json = serde_json::to_value(ToChild::Shutdown).unwrap();
        assert_eq!(json, serde_json::json!({"t": "Shutdown"}));
        let json = serde_json::to_value(FromChild::Hello {
            protocol: 1,
            pid: 2,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"t": "Hello", "protocol": 1, "pid": 2})
        );
        let json = serde_json::to_value(FromChild::Ok { id: 9 }).unwrap();
        assert_eq!(json, serde_json::json!({"t": "Ok", "id": 9}));
    }

    #[test]
    fn a_param_edit_keeps_every_bit_of_its_value() {
        for value in [
            0.1,
            1.0 / 3.0,
            f64::MIN_POSITIVE,
            1e300,
            -0.0,
            123_456.789_012_345_6,
        ] {
            let msg = FromChild::ParamEdit {
                param: 1,
                value,
                phase: WirePhase::Begin,
            };
            let (back, _) = round_trip(&msg, None);
            match back {
                FromChild::ParamEdit { value: v, .. } => assert_eq!(v.to_bits(), value.to_bits()),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_layout_survives_the_wire_unchanged() {
        let original = layout();
        let wire = WireLayout::from(&original);
        let (back, _) = round_trip(&wire, None);
        assert_eq!(PluginLayout::from(back), original);
    }

    #[test]
    fn the_layout_keeps_the_field_names_of_the_scan_json() {
        let json = serde_json::to_value(WireLayout::from(&layout())).unwrap();
        for key in [
            "name",
            "vendor",
            "version",
            "kind",
            "main_in",
            "main_out",
            "sidechain",
            "notes_in",
            "notes_out",
            "params",
        ] {
            assert!(json.get(key).is_some(), "{key} is missing");
        }
        assert_eq!(json["kind"], "note_effect");
        assert_eq!(json["main_in"], "mono");
        assert_eq!(json["main_out"], "stereo");
        let param = &json["params"][0];
        for key in [
            "id", "name", "module", "min", "max", "default", "stepped", "steps",
        ] {
            assert!(param.get(key).is_some(), "param {key} is missing");
        }
    }

    #[test]
    fn a_layout_without_buses_round_trips() {
        let mut l = layout();
        l.main_in = None;
        l.main_out = None;
        l.kind_hint = PluginKindHint::Instrument;
        let (back, _) = round_trip(&WireLayout::from(&l), None);
        assert_eq!(PluginLayout::from(back), l);
    }

    #[test]
    fn a_param_that_is_not_finite_is_made_finite_before_it_is_sent() {
        let mut l = layout();
        l.params[0].min = f64::NAN;
        l.params[0].max = f64::INFINITY;
        l.params[0].default = f64::NEG_INFINITY;
        let wire = WireLayout::from(&l);
        let (back, _) = round_trip(&wire, None);
        let p = &PluginLayout::from(back).params[0];
        assert!(p.min.is_finite() && p.max.is_finite() && p.default.is_finite());
        assert!(p.max > p.min);
    }

    #[test]
    fn an_unknown_plugin_kind_is_refused_when_it_is_read() {
        let mut json = serde_json::to_value(WireLayout::from(&layout())).unwrap();
        json["kind"] = serde_json::json!("synth");
        assert!(serde_json::from_value::<WireLayout>(json).is_err());
    }

    #[test]
    fn a_native_parent_survives_the_wire() {
        for p in [
            NativeParent::Win32(0x1234_5678),
            NativeParent::Cocoa(usize::MAX >> 4),
            NativeParent::X11(0xffff_ffff),
        ] {
            let wire = WireParent::from(p);
            let (back, _) = round_trip(&wire, None);
            assert_eq!(NativeParent::from(back), p);
        }
    }

    #[test]
    fn every_backend_error_survives_the_wire() {
        for e in [
            BackendError::NotAPlugin("a.clap".to_string()),
            BackendError::UnknownPlugin("x.y".to_string()),
            BackendError::Unsupported("no editor".to_string()),
            BackendError::Failed("boom".to_string()),
        ] {
            let (kind, text) = WireErrorKind::of(&e);
            let (kind, _) = round_trip(&kind, None);
            assert_eq!(into_backend_error(kind, text), e);
        }
    }

    #[test]
    fn phases_and_levels_convert_both_ways() {
        for p in [ParamPhase::Begin, ParamPhase::Change, ParamPhase::End] {
            assert_eq!(ParamPhase::from(WirePhase::from(p)), p);
        }
        for l in [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            assert_eq!(LogLevel::from(WireLevel::from(l)), l);
        }
    }
}
