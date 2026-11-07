//! What a device hears: the audio that arrives at its stereo input, rendered offline.
//!
//! The render is the project bounce ([`crate::project`]) with **one tap** on the flat rack, as a
//! freeze takes one ([`crate::freeze`]): the outputs that feed the device's `in_l` and `in_r` are
//! cabled to two more interface outputs, and those two channels are the result. Everything
//! else plays as in a bounce, so the capture is exactly what the device receives when the song
//! plays (the devices in front of it, sends and returns, the sidechains of other tracks), and
//! nothing about the render depends on the tap. The device itself is built and runs like any
//! other, so its own settings can never change what it hears.
//!
//! The render starts at the song's first frame and stops at `to_frame`; the result holds the
//! frames `from_frame..to_frame`, the same samples a playback from the start would give.
//! Latency compensation is not undone: the capture is what the device receives, at the time it
//! receives it. A device with only one input cabled hears that side on both channels. The render
//! can be watched and stopped through a [`RenderControl`]; a stopped render returns `None`.

use crate::plugin_pass::RenderControl;
use crate::project::{Core, Extra, PluginBounce, ProjectOptions, render_core};
use crate::{RenderError, Rendered};
use atomik_device_sdk::Registry;
use atomik_model::{DeviceKey, RackDoc, SocketRef};
use std::path::Path;

/// The audio that reached a device's input.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInput {
    /// The project's sample rate.
    pub sample_rate: u32,
    /// The left input.
    pub left: Vec<f32>,
    /// The right input.
    pub right: Vec<f32>,
}

/// The outputs that feed `device`'s `in_l` and `in_r` in the flat rack of `doc`. A side with no
/// cable repeats the other; `None` when neither is cabled.
pub fn input_sources(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
) -> Option<[SocketRef; 2]> {
    let flat = doc.expand(registry);
    let from = |socket: &str| {
        flat.cables
            .iter()
            .find(|c| &c.to.device == device && c.to.socket == socket)
            .map(|c| c.from.clone())
    };
    match (from("in_l"), from("in_r")) {
        (Some(l), Some(r)) => Some([l, r]),
        (Some(l), None) => Some([l.clone(), l]),
        (None, Some(r)) => Some([r.clone(), r]),
        (None, None) => None,
    }
}

/// Renders `doc` up to `to_frame` and returns the frames `from_frame..to_frame` of the audio at
/// the input of `device` (module docs). `bundle_root` is the folder the project's audio lives in,
/// as for a bounce.
///
/// # Errors
/// Nothing is cabled to the device's input, the span is empty, or the bounce of the document
/// fails.
pub fn render_device_input(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    device: &DeviceKey,
    (from_frame, to_frame): (u64, u64),
    control: RenderControl<'_>,
) -> Result<Option<DeviceInput>, RenderError> {
    let fail = |m: String| RenderError::Project(m);
    if from_frame >= to_frame {
        return Err(fail(
            "there is nothing to render in an empty span".to_owned(),
        ));
    }
    let Some(tap) = input_sources(doc, registry, device) else {
        return Err(fail(format!("nothing is cabled to the input of {device}")));
    };
    let extra = Extra {
        taps: vec![tap],
        frames: Some(usize::try_from(to_frame).map_err(|e| fail(e.to_string()))?),
        probes: Vec::new(),
        meter_probes: Vec::new(),
        note_taps: Vec::new(),
        control,
    };
    let Some(Core {
        rendered: Rendered { mut channels, .. },
        ..
    }) = render_core(doc, registry, (options, bundle_root), plugins, &extra)?
    else {
        return Ok(None);
    };
    // The tap is the pair of channels after the document's.
    let base = usize::from(doc.output_channels);
    let right = channels.remove(base + 1);
    let left = channels.remove(base);
    let from = usize::try_from(from_frame).unwrap_or(usize::MAX);
    let to = usize::try_from(to_frame).unwrap_or(usize::MAX);
    let slice = |c: Vec<f32>| -> Vec<f32> {
        let end = c.len().min(to);
        c[from.min(end)..end].to_vec()
    };
    Ok(Some(DeviceInput {
        sample_rate: doc.sample_rate,
        left: slice(left),
        right: slice(right),
    }))
}
