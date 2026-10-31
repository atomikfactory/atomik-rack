//! Freeze and bounce in place: rendering a track's chain to audio.
//!
//! The render is the project bounce ([`crate::project`]) with **one tap** on the flat rack: the
//! outputs of the last device of the chain ([`atomik_model::freeze_tap`]: the instrument's
//! outputs into its channel, or the last frozen insert effect's `out_l` and `out_r`) are cabled
//! to two more interface outputs, and those two channels are the result. Everything else of the
//! project plays as in a bounce (other tracks, sends, the master), so a track that is keyed or
//! modulated by another one freezes with that source playing; only the capture point differs.
//! The render starts at tick 0 like a bounce, so the devices of the chain are in exactly the
//! state they are in when the song plays, and the frozen part is a slice of the same samples:
//! the same project bounced with the track frozen differs from the unfrozen bounce only by what
//! the rest of the rack does with the render (see below), and bit for bit not at all when the
//! chain has no latency.
//!
//! * **Range.** `span` is `[start, end)` in ticks. The audio covers exactly the frames of that
//!   span, plus a **tail** when `tail_seconds` is above 0: the render goes on for up to that long
//!   past the end (reverb and delay decay) and the tail is cut where the signal has been below
//!   [`TAIL_FLOOR`] to the end, so a dry track gets no tail.
//! * **Which clips.** With `clips` set (bounce in place) the other clips of the track are removed
//!   from the document that is rendered, so only the chosen ones play, through the same chain.
//! * **Mute and solo.** The track is rendered as if it were audible: unmuted, and soloed when
//!   another track is.
//! * **Latency.** A chain whose devices report latency (look-ahead limiters, some plugins) is
//!   compensated: the tap sits in front of the delay compensation the master applies, so the
//!   render comes late by the sum of the latencies the engine published for the chain, and that
//!   many frames are cut from its start. That puts the audio where the track's signal is when
//!   it reaches the mix. A frozen track has no latency of its own, so every other track is
//!   aligned to a shorter delay: the bounce of the project with the track frozen is the
//!   unfrozen bounce earlier by the latency of the chain, and equal to it otherwise.
//! * **Plugins** are built as in a bounce, by the host in [`PluginBounce`]; a plugin that is
//!   frozen already (another track) is not built at all.
//!
//! The render can be watched and stopped through a [`RenderControl`]; a stopped render returns
//! `None`.

use crate::plugin_pass::RenderControl;
use crate::project::{Core, Extra, PluginBounce, ProjectOptions, render_core};
use crate::{RenderError, Rendered};
use atomik_audio_file::AudioData;
use atomik_device_sdk::Registry;
use atomik_model::song::ClipId;
use atomik_model::{DeviceKey, RackDoc, ticks_for_frames, track_chain};
use std::collections::BTreeSet;
use std::path::Path;

/// A tail ends where the signal stays at or below this level (about -90 dBFS) to the end of the
/// render.
pub const TAIL_FLOOR: f32 = 3.0e-5;

/// The longest tail a request can ask for, in seconds.
pub const MAX_TAIL_SECONDS: f64 = 60.0;

/// Frames rendered past the end and the tail, for the start the chain's latency cuts off (a
/// chain with more latency than this ends its render short and the missing frames are silence).
pub const LATENCY_MARGIN: u64 = 16_384;

/// What to render.
#[derive(Debug, Clone, PartialEq)]
pub struct FreezeRequest {
    /// The track's device.
    pub device: DeviceKey,
    /// How many of the channel's insert effects the render includes (`0`: the instrument alone).
    pub effects: usize,
    /// Render only these clips of the track (bounce in place), a muted note lane that holds one
    /// included; `None` renders the track, leaving its muted note lanes out.
    pub clips: Option<Vec<ClipId>>,
    /// The span to render, `[start, end)` in ticks.
    pub span: (i64, i64),
    /// Seconds of tail to render past the end of the span (`0.0`: none).
    pub tail_seconds: f64,
}

/// A finished render: stereo audio at the project's rate and the clip it plays as.
#[derive(Debug, Clone, PartialEq)]
pub struct FreezeRender {
    /// The audio: two channels, from the song frame of `start`.
    pub audio: AudioData,
    /// Song tick where the audio starts (the start of the span).
    pub start: i64,
    /// Length in ticks of the clip that plays the whole render.
    pub length: i64,
    /// Frames of the render past the end of the span (the tail that was kept).
    pub tail_frames: u64,
    /// What the render reported while it ran.
    pub warnings: Vec<crate::Warning>,
}

/// Renders `request` from `doc` (see the module docs). `bundle_root` is the folder the project's
/// audio lives in, as for a bounce.
///
/// # Errors
/// The track cannot be frozen (it is frozen already, has no channel, asks for more effects than
/// its chain has, or has an empty span), or the bounce of the document fails.
pub fn render_freeze(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    request: &FreezeRequest,
    control: RenderControl<'_>,
) -> Result<Option<FreezeRender>, RenderError> {
    let fail = |m: String| RenderError::Project(m);
    let Some(track) = doc.song.track(&request.device) else {
        return Err(fail(format!("{} has no track", request.device)));
    };
    if track.frozen.is_some() {
        return Err(fail(format!("{} is frozen already", request.device)));
    }
    let chain = track_chain(doc, registry, &request.device).map_err(fail)?;
    if request.effects > chain.effects.len() {
        return Err(fail(format!(
            "the chain of {} has {} effect(s), not {}",
            request.device,
            chain.effects.len(),
            request.effects
        )));
    }
    let (start, end) = request.span;
    if !(0 <= start && start < end) {
        return Err(fail(
            "there is nothing to render in an empty span".to_string(),
        ));
    }
    if !(0.0..=MAX_TAIL_SECONDS).contains(&request.tail_seconds) {
        return Err(fail(format!(
            "the tail must be between 0 and {MAX_TAIL_SECONDS} seconds"
        )));
    }
    let rate = doc.sample_rate;
    let start_frame = doc.song.frame_at(start, rate);
    let end_frame = doc.song.frame_at(end, rate);
    let span_frames = end_frame - start_frame;
    let tail = (request.tail_seconds * f64::from(rate)).round() as u64;

    // The document to render: the track audible, and only the chosen clips on it.
    let mut rendered_doc = doc.clone();
    let any_solo = rendered_doc.song.tracks.iter().any(|t| t.solo);
    if let Some(t) = rendered_doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == request.device)
    {
        t.muted = false;
        t.solo = any_solo;
        if let Some(keep) = &request.clips {
            let keep: BTreeSet<ClipId> = keep.iter().copied().collect();
            for lane in &mut t.lanes {
                lane.clips.retain(|c| keep.contains(&c.id));
                // The clips were chosen on purpose: a muted lane that holds one still plays.
                lane.muted &= lane.clips.is_empty();
            }
        }
    }
    let expanded = rendered_doc.expand(registry);
    let Some(tap) = atomik_model::freeze_tap(
        &expanded,
        &request.device,
        &chain.effects[..request.effects],
    ) else {
        return Err(fail(format!(
            "{} is not cabled to a channel: there is nothing to render",
            request.device
        )));
    };
    // The chain, in signal order: the instrument and the effects the render includes.
    let probes: Vec<DeviceKey> = std::iter::once(request.device.clone())
        .chain(chain.effects[..request.effects].iter().cloned())
        .collect();
    let extra = Extra {
        taps: vec![tap],
        frames: Some(
            usize::try_from(end_frame + tail + LATENCY_MARGIN).map_err(|e| fail(e.to_string()))?,
        ),
        probes,
        meter_probes: Vec::new(),
        control,
    };
    let Some(Core {
        rendered:
            Rendered {
                mut channels,
                warnings,
                ..
            },
        latencies,
        ..
    }) = render_core(
        &rendered_doc,
        registry,
        (options, bundle_root),
        plugins,
        &extra,
    )?
    else {
        return Ok(None);
    };
    let latency = u64::from(latencies.iter().sum::<u32>());
    // The tap is the pair of channels after the document's.
    let base = usize::from(doc.output_channels);
    let right = channels.remove(base + 1);
    let left = channels.remove(base);
    let slice = |c: Vec<f32>| -> Vec<f32> {
        let from = usize::try_from(start_frame + latency)
            .unwrap_or(usize::MAX)
            .min(c.len());
        c[from..].to_vec()
    };
    let (mut left, mut right) = (slice(left), slice(right));
    let span = usize::try_from(span_frames).unwrap_or(usize::MAX);
    // Whatever the chain's latency cut from the end is silence.
    let wanted = span + usize::try_from(tail).unwrap_or(usize::MAX);
    left.resize(left.len().max(span), 0.0);
    right.resize(left.len(), 0.0);
    let frames = if tail == 0 {
        span
    } else {
        // Keep the tail as far as the signal is above the floor.
        let audible = left
            .iter()
            .zip(&right)
            .rposition(|(l, r)| l.abs() > TAIL_FLOOR || r.abs() > TAIL_FLOOR)
            .map_or(0, |i| i + 1);
        audible.max(span).min(wanted).min(left.len())
    };
    left.truncate(frames);
    right.truncate(frames);
    let frames = left.len() as u64;
    let length = ticks_for_frames(&doc.song, start, frames.max(1), rate);
    let audio = AudioData::new(rate, vec![left, right]).map_err(|e| fail(e.to_string()))?;
    Ok(Some(FreezeRender {
        audio,
        start,
        length,
        tail_frames: frames.saturating_sub(span_frames),
        warnings,
    }))
}
