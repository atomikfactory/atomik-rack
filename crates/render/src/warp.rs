//! Warped audio clips: rendering a [`WarpJob`] into a buffer the engine plays like any asset.
//!
//! A warped clip does not play its asset: the engine reads a **rendered buffer** that holds the
//! clip's whole span (the stretch and the pitch shift of [`atomik_dsp::stretch`] through the
//! clip's time map), so the audio thread does nothing new, it plays a [`SongAsset::Memory`]
//! like any other. [`render_warp`] makes one buffer; [`render_warps`] makes every buffer a
//! document needs, which is what a bounce does before it builds the song (so a bounce never
//! plays a clip dry and never depends on timing, and the same project always gives the same
//! bits). The app renders the same jobs on a background worker and builds the song from what
//! is ready ([`crate::project::song_input_with_warps`]); a clip whose buffer is not ready plays
//! its asset in real time until the worker is done.
//!
//! A [`WarpStore`] maps [`WarpJob::key`] to the finished buffer.

use atomik_dsp::stretch::{StretchMode, StretchParams, TimeMap, pitch_ratio_for_cents, stretch};
use atomik_engine::SongAsset;
use atomik_model::{RackDoc, WarpJob, WarpMode, warp_jobs};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Finished warp buffers by [`WarpJob::key`].
pub type WarpStore = BTreeMap<u64, SongAsset>;

/// The pool key of a warp buffer in `SongInput::assets` bookkeeping.
pub fn pool_key(key: u64) -> String {
    format!("warp:{key:016x}")
}

/// Frames a buffer of `job` holds, per channel pair, as 4-byte floats: what the app counts
/// against its cache budget.
pub fn job_bytes(job: &WarpJob, channels: u16) -> u64 {
    job.frames * u64::from(channels.clamp(1, 2)) * 4
}

fn stretch_mode(mode: WarpMode) -> StretchMode {
    match mode {
        WarpMode::Repitch => StretchMode::Repitch,
        WarpMode::Beats => StretchMode::Beats,
        WarpMode::Tonal => StretchMode::Tonal,
    }
}

/// Reads the frames `[first, end)` of `asset` (one or two channels), zero padded past the
/// asset's end.
fn read_span(asset: &SongAsset, first: u64, end: u64) -> Vec<Vec<f32>> {
    let want = (end - first) as usize;
    let channels = usize::from(asset.channels().clamp(1, 2));
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; want]).collect();
    match asset {
        SongAsset::Memory { data, frames, .. } => {
            let hi = end.min(*frames);
            if hi > first {
                let n = (hi - first) as usize;
                for (o, ch) in out.iter_mut().zip(data.iter()) {
                    o[..n].copy_from_slice(&ch[first as usize..hi as usize]);
                }
            }
        }
        SongAsset::Stream { frames, source, .. } => {
            let hi = end.min(*frames);
            if hi > first {
                // The reader fills one `Vec` per channel of the asset; the first two are used.
                let n = (hi - first) as usize;
                let mut buf: Vec<Vec<f32>> = (0..asset.channels()).map(|_| vec![0.0; n]).collect();
                let got = source.read(first, &mut buf).min(n);
                for (o, b) in out.iter_mut().zip(&buf) {
                    o[..got].copy_from_slice(&b[..got]);
                }
            }
        }
    }
    out
}

/// Renders the buffer of `job` from `asset`: `job.frames` frames, as many channels as the
/// asset has up to two. `None` when `cancel` returned true (nothing is kept).
///
/// Only the span of the asset the time map reads (plus a margin) is loaded, so a streamed asset
/// is read once through its source and a long asset costs only what the clip uses.
pub fn render_warp(
    job: &WarpJob,
    asset: &SongAsset,
    cancel: &dyn Fn() -> bool,
) -> Option<SongAsset> {
    let frames = usize::try_from(job.frames).ok()?;
    let (first, end) = job.source_span();
    let end = end.min(asset.frames().max(first));
    let source = read_span(asset, first, end);
    let channels = source.len();
    let knots: Vec<(f64, f64)> = job
        .knots
        .iter()
        .map(|&(out, src)| (out, src - first as f64))
        .collect();
    let rendered = match TimeMap::new(&knots) {
        Some(map) if !source.is_empty() && frames > 0 => {
            let refs: Vec<&[f32]> = source.iter().map(Vec::as_slice).collect();
            let params = StretchParams {
                mode: stretch_mode(job.mode),
                pitch_ratio: pitch_ratio_for_cents(job.transpose_cents),
                sample_rate: job.sample_rate,
            };
            stretch(&refs, &map, frames, &params, cancel)?
        }
        _ => vec![vec![0.0; frames]; channels.max(1)],
    };
    let data: Arc<[Vec<f32>]> = rendered.into();
    Some(SongAsset::memory(data))
}

/// Renders every warp job of `doc` (see [`warp_jobs`]) whose asset is in `assets`. A job whose
/// asset is missing is left out (its clip plays what a missing asset plays: nothing). Blocks
/// until all are done: the bounce path.
pub fn render_warps(doc: &RackDoc, assets: &BTreeMap<String, SongAsset>) -> WarpStore {
    let mut store = WarpStore::new();
    for (key, job) in warp_jobs(doc) {
        let Some(asset) = assets.get(&job.asset) else {
            continue;
        };
        if let Some(buffer) = render_warp(&job, asset, &|| false) {
            store.insert(key, buffer);
        }
    }
    store
}
