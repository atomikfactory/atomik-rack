//! "Will the bass read?": a strip of a song rendered as a stem and read by
//! [`atomik_dsp::bass_read`].
//!
//! The strip (a track's channel, or the master) is rendered by the same deterministic pass that
//! exports the stems ([`crate::stems`]), from the start of the song to its End with no tail, so a
//! track is read as it sounds in the mix (its fader, pan and inserts applied; a muted strip is
//! silent). The stem is turned into its [`Reading`] as soon as the pass ends, so the audio is not
//! kept.

use crate::RenderError;
use crate::export::ExportSpan;
use crate::plugin_pass::RenderControl;
use crate::project::{PluginBounce, ProjectOptions};
use crate::stems::{Stem, StemRequest, StemStrip, render_stems};
use atomik_device_sdk::Registry;
use atomik_dsp::bass_read::{Reading, read};
use atomik_model::{RackDoc, StemSource};
use std::path::Path;

/// Renders `source` over the whole song and reads its low notes. A stopped render returns `None`;
/// `bundle_root` is the folder the project's audio lives in, as for a bounce.
///
/// # Errors
/// The song is empty or the bounce of the document fails.
pub fn render_reading(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    source: &StemSource,
    control: RenderControl<'_>,
) -> Result<Option<Reading>, RenderError> {
    let request = StemRequest {
        span: ExportSpan::Song,
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources: vec![source.clone()],
    };
    let mut reading = None;
    let mut take = |stem: Stem| {
        let planes = &stem.audio.channels;
        let right = planes.get(1).map_or(&[][..], Vec::as_slice);
        reading = Some(read(&planes[0], right, stem.audio.sample_rate));
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
    Ok(reading)
}
