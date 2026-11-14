//! The master of a song as energy blocks, for the energy map ([`atomik_dsp::energy`]).
//!
//! The master section is rendered as a stem ([`crate::stems`]) by the same deterministic pass that
//! exports the stems, from the start of the song to its End with no tail, and turned into its
//! [`Blocks`] as soon as the pass ends, so the audio is not kept.

use crate::RenderError;
use crate::export::ExportSpan;
use crate::plugin_pass::RenderControl;
use crate::project::{PluginBounce, ProjectOptions};
use crate::stems::{Stem, StemRequest, StemStrip, render_stems};
use atomik_device_sdk::Registry;
use atomik_dsp::energy::{Blocks, analyse};
use atomik_model::{RackDoc, StemKind};
use std::path::Path;

/// Renders the master of `doc` over the whole song and measures it. A stopped render returns
/// `None`; `bundle_root` is the folder the project's audio lives in, as for a bounce.
///
/// # Errors
/// The song is empty, the document has no master section, or the bounce of the document fails.
pub fn render_blocks(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    control: RenderControl<'_>,
) -> Result<Option<Blocks>, RenderError> {
    let sources: Vec<_> = atomik_model::stem_sources(doc, registry)
        .into_iter()
        .filter(|s| s.kind == StemKind::Master)
        .collect();
    if sources.is_empty() {
        return Err(RenderError::Project(
            "the song has no master section to measure".to_owned(),
        ));
    }
    let request = StemRequest {
        span: ExportSpan::Song,
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources,
    };
    let mut blocks = None;
    let mut take = |stem: Stem| {
        let planes = &stem.audio.channels;
        let right = planes.get(1).map_or(&[][..], Vec::as_slice);
        blocks = Some(analyse(&planes[0], right, stem.audio.sample_rate));
        Ok(())
    };
    let done = render_stems(
        doc,
        registry,
        (options, bundle_root),
        plugins,
        &request,
        (control, &mut take),
    )?;
    if done.is_none() {
        return Ok(None);
    }
    Ok(blocks)
}

/// Renders the master of `doc` over `span` (no tail) and returns its audio at the project's rate.
/// A stopped render returns `None`; `bundle_root` is the folder the project's audio lives in, as
/// for a bounce.
///
/// # Errors
/// The span is empty or out of range, the document has no master section, or the bounce of the
/// document fails.
pub fn render_master_span(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    span: ExportSpan,
    control: RenderControl<'_>,
) -> Result<Option<atomik_audio_file::AudioData>, RenderError> {
    let sources: Vec<_> = atomik_model::stem_sources(doc, registry)
        .into_iter()
        .filter(|s| s.kind == StemKind::Master)
        .collect();
    if sources.is_empty() {
        return Err(RenderError::Project(
            "the song has no master section to measure".to_owned(),
        ));
    }
    let request = StemRequest {
        span,
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources,
    };
    let mut audio = None;
    let mut take = |stem: Stem| {
        audio = Some(stem.audio);
        Ok(())
    };
    let done = render_stems(
        doc,
        registry,
        (options, bundle_root),
        plugins,
        &request,
        (control, &mut take),
    )?;
    if done.is_none() {
        return Ok(None);
    }
    Ok(audio)
}
