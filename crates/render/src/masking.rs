//! The band maps of the tracks of a song, for the masking analysis ([`atomik_dsp::masking`]).
//!
//! Each strip is rendered as a stem ([`crate::stems`]), by the same deterministic pass that exports
//! the stems, so a track is analysed as it sounds in the mix (its fader, pan and inserts applied;
//! a muted strip is silent) and lined up with the others. Each stem is turned into its
//! [`BandMap`] as soon as its pass ends and the audio is dropped, so a long song with many tracks
//! costs a band map per track, not the audio.
//!
//! The song is analysed from the start to its End with no tail, or over a range of ticks (a time
//! selection). The maps of one request all have the same length, so they can be compared.

use crate::RenderError;
use crate::export::ExportSpan;
use crate::plugin_pass::RenderControl;
use crate::project::{PluginBounce, ProjectOptions};
use crate::stems::{Stem, StemRequest, StemStrip, render_stems};
use atomik_device_sdk::Registry;
use atomik_dsp::masking::{BandMap, analyse};
use atomik_model::{RackDoc, StemSource};
use std::path::Path;

/// What to analyse.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskRequest {
    /// The range: the song up to its End, the loop, or a range of ticks.
    pub span: ExportSpan,
    /// The strips to analyse, in the order the maps come back ([`atomik_model::stem_sources`]).
    pub sources: Vec<StemSource>,
}

/// One analysed strip.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMap {
    /// The strip.
    pub source: StemSource,
    /// Its band map.
    pub map: BandMap,
}

/// Renders the strips of `request` and returns their band maps in the order of the request. A
/// stopped render returns `None`. `bundle_root` is the folder the project's audio lives in, as for
/// a bounce.
///
/// # Errors
/// The request is empty or out of range, or the bounce of the document fails.
pub fn render_maps(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    request: &MaskRequest,
    control: RenderControl<'_>,
) -> Result<Option<Vec<TrackMap>>, RenderError> {
    let stems = StemRequest {
        span: request.span,
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources: request.sources.clone(),
    };
    let mut maps: Vec<(usize, TrackMap)> = Vec::new();
    let mut take = |stem: Stem| {
        let planes = &stem.audio.channels;
        let right = planes.get(1).map_or(&[][..], Vec::as_slice);
        let map = analyse(&planes[0], right, stem.audio.sample_rate);
        maps.push((
            stem.index,
            TrackMap {
                source: stem.source,
                map,
            },
        ));
        Ok(())
    };
    let done = render_stems(
        doc,
        registry,
        (options, bundle_root),
        plugins,
        &stems,
        (control, &mut take),
    )?;
    if done.is_none() {
        return Ok(None);
    }
    maps.sort_by_key(|(i, _)| *i);
    Ok(Some(maps.into_iter().map(|(_, m)| m).collect()))
}
