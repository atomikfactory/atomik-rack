//! The plugin device's rack panel: a hand-made 2 U panel for every
//! third-party plugin device.
//!
//! ```text
//! PLUGIN               PINNED
//! +--------------+     (knob) (knob) (knob) (knob) (knob) (knob) (knob) (knob)
//! | Name    CLAP |     (knob) (knob) ...        (the pins, up to 16 in two rows)
//! | vendor v1.2  |
//! | RUNNING 128fr|
//! | detail       |
//! +--------------+
//! [POWER] [BYPASS]
//! [OPEN GUI][PARAMS]
//! [TO CH] [AUDIO THR]
//! [RELOAD][RELINK]
//! ```
//!
//! * The knobs are the device's **pinned** params ([`featured_pins`]: the saved pins, else the
//!   first eight params of the plugin), so a drag, a double-click (reset) and the wheel are
//!   ordinary `SetParam`s on the key `p<id>`; a stepped 0/1 param is a toggle. Shift +
//!   right-click on a knob unpins it (`PluginCommand::SetPinned`).
//! * POWER, BYPASS and AUDIO THR (RUN ON AUDIO THREAD) are the document's `powered`, the
//!   host `bypass` param and `run_on_audio_thread`. OPEN GUI shows only when the app said the
//!   plugin has an editor ([`crate::RackView::set_plugin_status`]); PARAMS opens the parameter
//!   pane; TO CH (instruments only) adds the console channel; RELOAD recreates the instance;
//!   RELINK (a stand-in only) points the device at another installed plugin.
//! * The status lines show the state (RUNNING, SLEEPING, STOPPED, RESTARTING, CRASHED from the
//!   meter values), the reported latency and the dropped-event count, and MISSING, CRASHED or
//!   FAILED with the reason (and the saved path hint of a stand-in).
//!
//! This module holds the geometry and the pure helpers; the rack view drives the gestures
//! (`rack/plugin_input.rs`) and `paint_panel` draws the extras.

use crate::autolayout::{CONTENT_X, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::{button_sized, fit_label};
use crate::plugin_data::{
    METER_DROPPED, METER_LATENCY, METER_STATE, PluginDeviceStatus, format_badge,
};
use crate::seq_panel::{back_layout, place_controls};
use crate::theme::{Theme, mix};
use atomik_device_sdk::{ControlKind, DeviceDescriptor, PluginKindHint};
use atomik_model::{PluginDoc, RackDevice};
use vello::kurbo::{Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the plugin panel in rack units.
pub const PLUGIN_UNITS: u8 = 2;
/// Most knobs a panel shows (two rows of eight).
pub const MAX_FEATURED: usize = 16;
/// Knobs a plugin gets when nothing is pinned.
pub const DEFAULT_FEATURED: usize = 8;
const LEFT_W: f64 = 108.0;
const KNOBS_X: f64 = CONTENT_X.0 + LEFT_W + 8.0;
const COL_W: f64 = 64.0;
const COLS: usize = 8;
const INFO_H: f64 = 44.0;
const BTN_H: f64 = 18.0;
const BTN_PITCH: f64 = 21.0;
const BTN_W: f64 = 52.0;

/// A button of the plugin panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginButton {
    /// The panel's power switch (`PluginCommand::SetPower`).
    Power,
    /// The host `bypass` param.
    Bypass,
    /// RUN ON AUDIO THREAD (`PluginCommand::SetRunMode`).
    AudioThread,
    /// Open the plugin's editor window.
    OpenGui,
    /// Open the parameter pane.
    Params,
    /// Add the console channel (instruments).
    ToChannel,
    /// Recreate the instance.
    Reload,
    /// Point a stand-in at another installed plugin.
    Relink,
}

impl PluginButton {
    /// Every button, in panel order (left to right, top to bottom).
    pub const ALL: [Self; 8] = [
        Self::Power,
        Self::Bypass,
        Self::OpenGui,
        Self::Params,
        Self::ToChannel,
        Self::AudioThread,
        Self::Reload,
        Self::Relink,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Power => "POWER",
            Self::Bypass => "BYPASS",
            Self::AudioThread => "AUDIO THR",
            Self::OpenGui => "OPEN GUI",
            Self::Params => "PARAMS",
            Self::ToChannel => "TO CH",
            Self::Reload => "RELOAD",
            Self::Relink => "RELINK",
        }
    }
}

/// Geometry of the plugin panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginPanel {
    /// Title strip of the left block.
    pub title: Rect,
    /// The info window (name, vendor, status).
    pub info: Rect,
    /// POWER.
    pub power: Rect,
    /// BYPASS.
    pub bypass: Rect,
    /// AUDIO THR.
    pub audio_thread: Rect,
    /// OPEN GUI.
    pub open_gui: Rect,
    /// PARAMS.
    pub params: Rect,
    /// TO CH.
    pub to_channel: Rect,
    /// RELOAD.
    pub reload: Rect,
    /// RELINK.
    pub relink: Rect,
}

impl PluginPanel {
    /// The rectangle of a button.
    pub fn button(&self, b: PluginButton) -> Rect {
        match b {
            PluginButton::Power => self.power,
            PluginButton::Bypass => self.bypass,
            PluginButton::AudioThread => self.audio_thread,
            PluginButton::OpenGui => self.open_gui,
            PluginButton::Params => self.params,
            PluginButton::ToChannel => self.to_channel,
            PluginButton::Reload => self.reload,
            PluginButton::Relink => self.relink,
        }
    }
}

/// The param ids shown as knobs: the saved pins that the plugin still has (in order, at most
/// [`MAX_FEATURED`]), or, when nothing is pinned, the first [`DEFAULT_FEATURED`] params of its
/// layout.
pub fn featured_pins(doc: &PluginDoc) -> Vec<u32> {
    let has = |id: u32| doc.layout.params.iter().any(|p| p.id == id);
    if doc.pinned.is_empty() {
        return doc
            .layout
            .params
            .iter()
            .take(DEFAULT_FEATURED)
            .map(|p| p.id)
            .collect();
    }
    doc.pinned
        .iter()
        .copied()
        .filter(|id| has(*id))
        .take(MAX_FEATURED)
        .collect()
}

/// The param key of a plugin param id.
pub fn param_key(id: u32) -> String {
    format!("p{id}")
}

/// The plugin param id of a key `p<id>` (canonical decimal).
pub fn param_id_of(key: &str) -> Option<u32> {
    let id: u32 = key.strip_prefix('p')?.parse().ok()?;
    (param_key(id) == key).then_some(id)
}

/// The panel of a plugin device (two rack units, see the module documentation); the back is
/// the generic socket layout. `doc` supplies the pins (see [`featured_pins`]).
pub fn plugin_layout(desc: &DeviceDescriptor, doc: &PluginDoc) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let keys: Vec<String> = featured_pins(doc)
        .into_iter()
        .map(param_key)
        .filter(|k| desc.params.iter().any(|p| p.key == k))
        .collect();
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let mut front = place_controls(desc, &key_refs, KNOBS_X, COL_W, COLS, top);
    for pc in &mut front {
        let p = &desc.params[pc.param];
        pc.kind = if p.discrete && (p.max - p.min - 1.0).abs() < 1e-6 {
            ControlKind::Toggle
        } else {
            ControlKind::Knob
        };
    }
    let right = KNOBS_X + COLS as f64 * COL_W;
    let info = Rect::new(x0, top + 2.0, x0 + LEFT_W, top + 2.0 + INFO_H);
    let btn = |row: usize, col: usize| {
        let x = x0 + col as f64 * (BTN_W + 4.0);
        let y = info.y1 + 4.0 + row as f64 * BTN_PITCH;
        Rect::new(x, y, x + BTN_W, y + BTN_H)
    };
    let panel = PluginPanel {
        title: Rect::new(x0, MARGIN_TOP, x0 + LEFT_W, MARGIN_TOP + TITLE_H),
        info,
        power: btn(0, 0),
        bypass: btn(0, 1),
        open_gui: btn(1, 0),
        params: btn(1, 1),
        to_channel: btn(2, 0),
        audio_thread: btn(2, 1),
        reload: btn(3, 0),
        relink: btn(3, 1),
    };
    let front_sections = vec![
        Section {
            label: "PLUGIN".to_string(),
            rect: panel.title,
        },
        Section {
            label: "PINNED".to_string(),
            rect: Rect::new(KNOBS_X, MARGIN_TOP, right, MARGIN_TOP + TITLE_H),
        },
    ];
    let (units, back, back_sections) = back_layout(desc, PLUGIN_UNITS);
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: Some(panel),
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ pure helpers

/// Which buttons the panel shows (and answers) for a device in this state.
pub fn button_visible(b: PluginButton, doc: &PluginDoc, status: &PluginDeviceStatus) -> bool {
    match b {
        PluginButton::OpenGui => status.has_editor && status.missing.is_none(),
        PluginButton::ToChannel => doc.layout.kind_hint == PluginKindHint::Instrument,
        PluginButton::Relink => status.missing.is_some(),
        _ => true,
    }
}

/// Why a SLEEPING plugin sleeps and what wakes it (the detail line).
pub const SLEEPING_REASON: &str = "idle, a note or signal wakes it";

/// The state word of the meter value at [`METER_STATE`].
pub fn state_label(state: f32) -> &'static str {
    match state.round() as i32 {
        1 => "SLEEPING",
        2 => "STOPPED",
        3 => "FAILED",
        4 => "RESTARTING",
        5 => "CRASHED",
        _ => "RUNNING",
    }
}

/// What the status lines of the info window say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLines {
    /// The state word (MISSING, FAILED, LOADING or a [`state_label`]).
    pub state: String,
    /// The state is an error (drawn in the warning colour).
    pub error: bool,
    /// The reported latency, `N fr`, when the plugin has any.
    pub latency: Option<String>,
    /// The last line: the reason of MISSING or FAILED, else the dropped-event count when there
    /// are some.
    pub detail: String,
}

/// A multi-line reason on one line: the non-empty lines joined with ` | `.
fn one_line(reason: &str) -> String {
    reason
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The status lines of a device (MISSING beats CRASHED beats FAILED beats LOADING beats the
/// meter state).
pub fn status_lines(status: &PluginDeviceStatus, meters: &[f32]) -> StatusLines {
    let meter = |i: usize| meters.get(i).copied().unwrap_or(0.0);
    let latency =
        (meter(METER_LATENCY) >= 1.0).then(|| format!("{} fr", meter(METER_LATENCY) as u64));
    if let Some(r) = &status.missing {
        return StatusLines {
            state: "MISSING".into(),
            error: true,
            latency: None,
            detail: r.clone(),
        };
    }
    if status.crashed.is_some() || meter(METER_STATE).round() as i32 == 5 {
        return StatusLines {
            state: "CRASHED".into(),
            error: true,
            latency,
            // One line: the exit code first, then what the process wrote last.
            detail: status.crashed.as_deref().map(one_line).unwrap_or_default(),
        };
    }
    let failed_meter = meter(METER_STATE).round() as i32 == 3;
    if status.failed.is_some() || failed_meter {
        return StatusLines {
            state: "FAILED".into(),
            error: true,
            latency,
            detail: status.failed.clone().unwrap_or_default(),
        };
    }
    if let Some(why) = &status.waiting {
        return StatusLines {
            state: "NO AUDIO".into(),
            error: true,
            latency,
            detail: format!("waiting for the audio engine ({why})"),
        };
    }
    if status.loading {
        return StatusLines {
            state: "LOADING".into(),
            error: false,
            latency: None,
            detail: "creating the plugin, this can take a few seconds".into(),
        };
    }
    let dropped = meter(METER_DROPPED) as u64;
    StatusLines {
        state: state_label(meter(METER_STATE)).into(),
        error: false,
        latency,
        detail: if dropped > 0 {
            format!("{dropped} dropped")
        } else if state_label(meter(METER_STATE)) == "SLEEPING" {
            // The device only reports SLEEPING when it really is asleep: no process calls, the
            // plugin told to stop. The reason says what wakes it.
            SLEEPING_REASON.into()
        } else {
            String::new()
        },
    }
}

// ------------------------------------------------------------------ painting

/// What the panel draws besides the document.
#[derive(Debug, Clone, Copy)]
pub struct PluginInfo<'a> {
    /// The app's view of the live instance.
    pub status: &'a PluginDeviceStatus,
    /// The device's meter values (see [`crate::plugin_data::METER_STATE`]).
    pub meters: &'a [f32],
    /// The console already has a channel fed by this device (TO CH is dim).
    pub has_channel: bool,
}

fn warn_color(theme: &Theme) -> Color {
    mix(theme.text, Color::from_rgba8(255, 96, 80, 255), 0.8)
}

/// A plugin panel button: the panel's button face with narrow text.
fn panel_button<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, label: &str, enabled: bool) {
    button_sized(c, theme, r, label, enabled, 7.2, 0.3);
}

pub(crate) fn led_button<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    label: &str,
    on: Option<bool>,
    enabled: bool,
    accent: Color,
) {
    panel_button(c, theme, r, label, enabled);
    if let Some(on) = on {
        let led = pt(r.x0 + 5.5, r.y0 + 5.5);
        if on {
            c.fill_with(Circle::new(led, 4.5), accent.with_alpha(0.25));
            c.fill_with(Circle::new(led, 2.2), mix(accent, Color::WHITE, 0.25));
        } else {
            c.fill_with(Circle::new(led, 2.2), theme.led_off);
        }
    }
}

/// Draws the plugin panel's extras: the info window and the buttons.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &PluginPanel,
    device: &RackDevice,
    info: &PluginInfo<'_>,
    accent: Color,
) {
    let Some(doc) = device.plugin.as_deref() else {
        return;
    };
    let lines = status_lines(info.status, info.meters);
    c.fill_with(RoundedRect::from_rect(panel.info, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(panel.info.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let x = panel.info.x0 + 6.0;
    let w = panel.info.width() - 12.0;
    let y0 = panel.info.y0;
    // Name and the format badge.
    let badge = format_badge(doc.format);
    c.text(
        &TextRun::new(
            fit_label(doc.layout.name.as_str(), w - 26.0, 9.5),
            pt(x, y0 + 10.0),
            9.5,
            theme.text,
        )
        .weight(700),
    );
    c.text(
        &TextRun::new(badge, pt(panel.info.x1 - 6.0, y0 + 9.5), 6.5, accent)
            .align(Align::End)
            .weight(700)
            .spacing(0.6),
    );
    // Vendor and version, or the saved path of a stand-in.
    let second = if info.status.missing.is_some() {
        doc.path_hint
            .clone()
            .unwrap_or_else(|| doc.layout.vendor.clone())
    } else if doc.layout.version.is_empty() {
        doc.layout.vendor.clone()
    } else {
        format!("{} {}", doc.layout.vendor, doc.layout.version)
    };
    c.text(&TextRun::new(
        fit_label(&second, w, 7.5),
        pt(x, y0 + 19.5),
        7.5,
        theme.text_muted,
    ));
    // State and latency.
    c.text(
        &TextRun::new(
            lines.state.clone(),
            pt(x, y0 + 29.0),
            7.5,
            if lines.error {
                warn_color(theme)
            } else {
                accent
            },
        )
        .weight(700)
        .spacing(0.4),
    );
    if let Some(l) = &lines.latency {
        c.text(
            &TextRun::new(
                l.clone(),
                pt(panel.info.x1 - 6.0, y0 + 29.0),
                7.5,
                theme.text_muted,
            )
            .align(Align::End),
        );
    }
    if !lines.detail.is_empty() {
        c.text(&TextRun::new(
            fit_label(&lines.detail, w, 7.5),
            pt(x, y0 + 38.5),
            7.5,
            if lines.error {
                theme.text
            } else {
                theme.text_muted
            },
        ));
    }
    // Buttons.
    let bypassed = device.params.get("bypass").is_some_and(|v| *v >= 0.5);
    for b in PluginButton::ALL {
        if !button_visible(b, doc, info.status) {
            continue;
        }
        let r = panel.button(b);
        match b {
            PluginButton::Power => {
                led_button(c, theme, r, b.label(), Some(doc.powered), true, accent)
            }
            PluginButton::Bypass => {
                led_button(c, theme, r, b.label(), Some(bypassed), true, accent)
            }
            PluginButton::AudioThread => led_button(
                c,
                theme,
                r,
                b.label(),
                Some(doc.run_on_audio_thread),
                true,
                accent,
            ),
            PluginButton::ToChannel => panel_button(c, theme, r, b.label(), !info.has_channel),
            PluginButton::Reload => panel_button(c, theme, r, b.label(), !info.status.loading),
            PluginButton::OpenGui | PluginButton::Params | PluginButton::Relink => {
                panel_button(c, theme, r, b.label(), true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_device_sdk::{Channels, PluginFormat, PluginLayout, PluginParamInfo, PluginRef};

    fn doc_with(ids: &[u32], pinned: &[u32]) -> PluginDoc {
        let layout = PluginLayout {
            name: "X".into(),
            vendor: "V".into(),
            version: "1".into(),
            kind_hint: PluginKindHint::Effect,
            main_in: Some(Channels::Stereo),
            main_out: Some(Channels::Stereo),
            sidechain: false,
            notes_in: false,
            notes_out: false,
            params: ids
                .iter()
                .map(|id| PluginParamInfo {
                    id: *id,
                    name: format!("P{id}"),
                    module: String::new(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    stepped: false,
                    steps: 0,
                })
                .collect(),
        };
        let mut d = PluginDoc::new(
            &PluginRef {
                format: PluginFormat::Clap,
                id: "x".into(),
            },
            layout,
            None,
        );
        d.pinned = pinned.to_vec();
        d
    }

    #[test]
    fn param_keys_round_trip_and_reject_non_canonical_forms() {
        assert_eq!(param_key(42), "p42");
        assert_eq!(param_id_of("p42"), Some(42));
        assert_eq!(param_id_of("p042"), None);
        assert_eq!(param_id_of("bypass"), None);
        assert_eq!(param_id_of("p"), None);
        assert_eq!(param_id_of("p4294967296"), None);
    }

    #[test]
    fn featured_pins_default_to_the_first_eight_and_drop_dead_pins() {
        let ids: Vec<u32> = (100..120).collect();
        let d = doc_with(&ids, &[]);
        assert_eq!(featured_pins(&d), (100..108).collect::<Vec<u32>>());
        let d = doc_with(&ids, &[105, 999, 101]);
        assert_eq!(featured_pins(&d), [105, 101]);
        let d = doc_with(&ids, &(100..120).collect::<Vec<u32>>());
        assert_eq!(featured_pins(&d).len(), MAX_FEATURED);
    }

    #[test]
    fn a_crashed_device_is_crashed_not_failed_and_the_reason_is_one_line() {
        let ok = PluginDeviceStatus::default();
        let l = status_lines(&ok, &[0.0, 0.0, 5.0]);
        assert_eq!((l.state.as_str(), l.error), ("CRASHED", true));
        let crashed = PluginDeviceStatus {
            crashed: Some(
                "exited with code 3
thread panicked
"
                .into(),
            ),
            failed: Some("the plugin process stopped".into()),
            ..ok.clone()
        };
        let l = status_lines(&crashed, &[0.0, 0.0, 5.0, 64.0]);
        assert_eq!(l.state, "CRASHED");
        assert_eq!(l.detail, "exited with code 3 | thread panicked");
        assert_eq!(l.latency.as_deref(), Some("64 fr"));
        let missing = PluginDeviceStatus {
            missing: Some("gone".into()),
            ..crashed
        };
        assert_eq!(status_lines(&missing, &[]).state, "MISSING");
    }

    #[test]
    fn status_priority_is_missing_failed_loading_then_the_meter_state() {
        let ok = PluginDeviceStatus::default();
        assert_eq!(status_lines(&ok, &[]).state, "RUNNING");
        assert_eq!(status_lines(&ok, &[0.0, 0.0, 2.0]).state, "STOPPED");
        assert_eq!(status_lines(&ok, &[0.0, 0.0, 4.0]).state, "RESTARTING");
        let l = status_lines(&ok, &[0.0, 0.0, 0.0, 64.0, 2.0]);
        assert_eq!(l.latency.as_deref(), Some("64 fr"));
        assert_eq!(l.detail, "2 dropped");
        let sleeping = status_lines(&ok, &[0.0, 0.0, 1.0]);
        assert_eq!(sleeping.state, "SLEEPING");
        assert_eq!(sleeping.detail, SLEEPING_REASON);
        assert_eq!(status_lines(&ok, &[0.0, 0.0, 0.0]).detail, "");
        let loading = PluginDeviceStatus {
            loading: true,
            ..ok.clone()
        };
        assert_eq!(status_lines(&loading, &[]).state, "LOADING");
        let waiting = PluginDeviceStatus {
            waiting: Some("audio is off".into()),
            ..ok.clone()
        };
        let l = status_lines(&waiting, &[]);
        assert_eq!(l.state, "NO AUDIO");
        assert!(l.detail.contains("audio is off"), "{}", l.detail);
        let failed = PluginDeviceStatus {
            failed: Some("boom".into()),
            loading: true,
            ..ok.clone()
        };
        assert_eq!(status_lines(&failed, &[]).state, "FAILED");
        assert_eq!(status_lines(&ok, &[0.0, 0.0, 3.0]).state, "FAILED");
        let missing = PluginDeviceStatus {
            missing: Some("gone".into()),
            ..failed
        };
        let l = status_lines(&missing, &[]);
        assert_eq!((l.state.as_str(), l.detail.as_str()), ("MISSING", "gone"));
    }
}
