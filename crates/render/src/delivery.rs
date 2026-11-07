//! The master of a song measured for the delivery targets: the loudness report
//! ([`crate::loudness`]) of the master section as the export would write it.
//!
//! The master section is rendered as a stem ([`crate::stems`]) by the same deterministic pass that
//! exports the stems, from the start of the song to its End with no tail, and measured as soon as
//! the pass ends (integrated loudness, loudness range, true peak and sample peak), so the audio is
//! not kept. The numbers are those of an export of the same span: the same device chain, the same
//! meter, no dither and no sample rate conversion.

use crate::RenderError;
use crate::export::ExportSpan;
use crate::loudness::{LoudnessReport, measure};
use crate::plugin_pass::RenderControl;
use crate::project::{PluginBounce, ProjectOptions};
use crate::stems::{Stem, StemRequest, StemStrip, render_stems};
use atomik_device_sdk::Registry;
use atomik_model::{RackDoc, StemKind};
use std::path::Path;

/// Renders the master of `doc` over the whole song and measures it. A stopped render returns
/// `None`; `bundle_root` is the folder the project's audio lives in, as for a bounce.
///
/// # Errors
/// The document has no master section, or the bounce of the document fails.
pub fn measure_master(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    control: RenderControl<'_>,
) -> Result<Option<LoudnessReport>, RenderError> {
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
    let mut report = None;
    let mut take = |stem: Stem| {
        report = Some(measure(&stem.audio));
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
    Ok(report)
}
