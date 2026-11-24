//! A rough estimate of the memory an offline render holds, so the application can say so before
//! a bounce, an export, the stems or a freeze starts on a machine that cannot hold it.
//!
//! A render loads every audio asset the song references fully (a bounce never streams), and holds
//! the whole output in memory: one buffer per output channel, as long as the song. Writing the
//! file holds a second copy of it when the file has another rate than the project or loudness
//! finishing runs, so the output counts twice. The figure is an estimate, not a promise: it
//! ignores the devices' own buffers and the pages the system can swap out.

use std::collections::BTreeSet;

use atomik_model::RackDoc;

use crate::RenderError;
use crate::export::{ExportRequest, ExportSpan, span_frames};
use crate::freeze::{FreezeRequest, LATENCY_MARGIN, MAX_TAIL_SECONDS};
use crate::stems::TAPS_PER_PASS;

/// Bytes of one decoded sample (`f32`).
const SAMPLE_BYTES: u64 = 4;

/// How many times the output is held at once (the rendered buffers and the copy made by the
/// conversion or the loudness finish before the file is written).
const OUTPUT_COPIES: u64 = 2;

/// The decoded size, in bytes, of the audio assets `doc` references that are not in memory
/// already (`resident` says which hashes are). An asset several tracks use counts once; an
/// asset the pool has no entry for counts nothing.
pub fn assets_bytes(doc: &RackDoc, resident: &dyn Fn(&str) -> bool) -> u64 {
    let wanted: BTreeSet<String> = doc.referenced_assets();
    wanted
        .iter()
        .filter(|hash| !resident(hash))
        .filter_map(|hash| doc.assets.get(hash))
        .map(|m| {
            m.frames
                .saturating_mul(u64::from(m.channels))
                .saturating_mul(SAMPLE_BYTES)
        })
        .fold(0, u64::saturating_add)
}

/// The estimate for a render of `frames` frames of `channels` channels over `assets` bytes of
/// assets: `assets + frames x channels x 4 x 2`.
pub fn estimate_bytes(assets: u64, frames: u64, channels: u64) -> u64 {
    frames
        .saturating_mul(channels)
        .saturating_mul(SAMPLE_BYTES * OUTPUT_COPIES)
        .saturating_add(assets)
}

/// The frames an export of `span` renders: the range up to its end, and the tail a song keeps.
fn rendered_frames(doc: &RackDoc, span: ExportSpan, tail_seconds: f64) -> Result<u64, RenderError> {
    let (_, to) = span_frames(doc, span)?;
    let tail = match span {
        ExportSpan::Song => {
            (tail_seconds.clamp(0.0, MAX_TAIL_SECONDS) * f64::from(doc.sample_rate)).round() as u64
        }
        ExportSpan::Loop | ExportSpan::Range(..) => 0,
    };
    Ok(to.saturating_add(tail))
}

/// The estimate for the export `request` of `doc`: the output channels of the document.
///
/// # Errors
/// The range is empty.
pub fn export_bytes(
    doc: &RackDoc,
    request: &ExportRequest,
    resident: &dyn Fn(&str) -> bool,
) -> Result<u64, RenderError> {
    let frames = rendered_frames(doc, request.span, request.tail_seconds)?;
    Ok(estimate_bytes(
        assets_bytes(doc, resident),
        frames,
        u64::from(doc.output_channels),
    ))
}

/// The estimate for the stems of `strips` strips over `span`: a pass renders the document's
/// output channels and a pair for each of at most [`TAPS_PER_PASS`] strips, and the song is
/// rendered once per pass (the assets stay loaded for the longest of them, not for each).
///
/// # Errors
/// The range is empty.
pub fn stems_bytes(
    doc: &RackDoc,
    (span, tail_seconds): (ExportSpan, f64),
    strips: usize,
    resident: &dyn Fn(&str) -> bool,
) -> Result<u64, RenderError> {
    let frames = rendered_frames(doc, span, tail_seconds)?;
    let taps = strips.clamp(1, TAPS_PER_PASS) as u64;
    Ok(estimate_bytes(
        assets_bytes(doc, resident),
        frames,
        u64::from(doc.output_channels) + 2 * taps,
    ))
}

/// The estimate for a freeze or a bounce in place: the render runs from the start of the song to
/// the end of the span with the tail and the latency margin, over the output channels and the
/// pair that is tapped.
///
/// # Errors
/// The span is empty.
pub fn freeze_bytes(
    doc: &RackDoc,
    request: &FreezeRequest,
    resident: &dyn Fn(&str) -> bool,
) -> Result<u64, RenderError> {
    let (start, end) = request.span;
    if !(0 <= start && start < end) {
        return Err(RenderError::Project(
            "there is nothing to render in an empty span".to_owned(),
        ));
    }
    let end_frame = doc.song.frame_at(end, doc.sample_rate);
    let tail = (request.tail_seconds.clamp(0.0, MAX_TAIL_SECONDS) * f64::from(doc.sample_rate))
        .round() as u64;
    let frames = end_frame
        .saturating_add(tail)
        .saturating_add(LATENCY_MARGIN);
    Ok(estimate_bytes(
        assets_bytes(doc, resident),
        frames,
        u64::from(doc.output_channels) + 2,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_registry;
    use atomik_core::PPQ;
    use atomik_model::{
        AssetMeta, Command, DeviceKey, Document, FadeCurve, SongCommand, empty_doc,
    };

    const KEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A document whose song is six beats long (three seconds at 120 beats a minute), with
    /// assets `(hash, frames, channels)` in its pool and one audio track per entry of `tracks`
    /// playing the asset named there.
    fn doc_with(assets: &[(&str, u64, u16)], tracks: &[&str]) -> RackDoc {
        let registry = default_registry();
        let mut d = Document::new(empty_doc());
        let mut apply = |c: Command| {
            d.apply(c, &registry).unwrap();
        };
        for &(hash, frames, channels) in assets {
            apply(Command::RegisterAsset {
                hash: hash.to_owned(),
                meta: AssetMeta {
                    channels,
                    sample_rate: 48_000,
                    frames,
                    original_name: "fixture.wav".to_owned(),
                },
            });
        }
        apply(Command::Song(SongCommand::SetSongEnd(6 * PPQ)));
        for (i, hash) in tracks.iter().enumerate() {
            apply(Command::Song(SongCommand::AddAudioTrack { name: None }));
            let device = apply_key(i);
            apply(Command::Song(SongCommand::AddAudioClip {
                device,
                start: 0,
                length: PPQ,
                asset: (*hash).to_owned(),
                source_offset: 0,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: FadeCurve::Linear,
            }));
        }
        d.doc().clone()
    }

    /// The device key the `n`th audio track gets.
    fn apply_key(n: usize) -> DeviceKey {
        DeviceKey(if n == 0 {
            "audio-track".to_owned()
        } else {
            format!("audio-track-{}", n + 1)
        })
    }

    fn request(span: ExportSpan) -> ExportRequest {
        ExportRequest {
            span,
            tail_seconds: 0.0,
        }
    }

    #[test]
    fn the_estimate_is_the_assets_plus_twice_the_output() {
        // 10 s of stereo at 48 kHz, and 1000 bytes of assets.
        assert_eq!(estimate_bytes(1000, 480_000, 2), 1000 + 480_000 * 2 * 4 * 2);
        assert_eq!(estimate_bytes(0, u64::MAX, 2), u64::MAX, "no overflow");
    }

    #[test]
    fn an_asset_two_tracks_play_counts_once_and_a_resident_one_not_at_all() {
        let doc = doc_with(&[(KEY_A, 1000, 2), (KEY_B, 500, 1)], &[KEY_A, KEY_A, KEY_B]);
        // A: 1000 frames x 2 channels x 4 bytes, once; B: 500 x 1 x 4.
        assert_eq!(assets_bytes(&doc, &|_| false), 8000 + 2000);
        assert_eq!(assets_bytes(&doc, &|h| h == KEY_A), 2000);
        assert_eq!(assets_bytes(&doc, &|_| true), 0);
    }

    #[test]
    fn an_export_holds_the_assets_and_twice_its_output() {
        let doc = doc_with(&[(KEY_A, 1000, 2)], &[KEY_A]);
        let frames = 3 * u64::from(doc.sample_rate);
        let want = 8000 + frames * u64::from(doc.output_channels) * 8;
        assert_eq!(
            export_bytes(&doc, &request(ExportSpan::Song), &|_| false).unwrap(),
            want
        );
        // A tail makes the song longer.
        let mut with_tail = request(ExportSpan::Song);
        with_tail.tail_seconds = 2.0;
        let longer = (3 + 2) * u64::from(doc.sample_rate) * u64::from(doc.output_channels) * 8;
        assert_eq!(
            export_bytes(&doc, &with_tail, &|h| h == KEY_A).unwrap(),
            longer
        );
    }

    #[test]
    fn stems_render_a_pair_more_for_each_strip_of_a_pass() {
        let doc = doc_with(&[(KEY_A, 1000, 2)], &[KEY_A]);
        let frames = 3 * u64::from(doc.sample_rate);
        let out = u64::from(doc.output_channels);
        let song = (ExportSpan::Song, 0.0);
        let one = stems_bytes(&doc, song, 1, &|_| true).unwrap();
        assert_eq!(one, frames * (out + 2) * 8);
        // Beyond a pass the strips are rendered pass by pass: the figure stops growing.
        let many = stems_bytes(&doc, song, 40, &|_| true).unwrap();
        assert_eq!(many, frames * (out + 2 * TAPS_PER_PASS as u64) * 8);
    }

    #[test]
    fn a_freeze_renders_to_the_end_of_the_span_with_the_margin() {
        let doc = doc_with(&[], &[]);
        let request = FreezeRequest {
            device: DeviceKey("audio-track".to_owned()),
            effects: 0,
            strip: false,
            clips: None,
            span: (0, 2 * PPQ),
            tail_seconds: 1.0,
        };
        let rate = u64::from(doc.sample_rate);
        let frames = rate + rate + LATENCY_MARGIN;
        assert_eq!(
            freeze_bytes(&doc, &request, &|_| true).unwrap(),
            frames * (u64::from(doc.output_channels) + 2) * 8
        );
        let empty = FreezeRequest {
            span: (5, 5),
            ..request
        };
        assert!(freeze_bytes(&doc, &empty, &|_| true).is_err());
    }
}
