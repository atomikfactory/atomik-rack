//! Send to Track: the notes a track's player stack generates, rendered offline.
//!
//! The document is rendered like a bounce (every device is built, the song plays from tick 0
//! to its end plus a short tail), with a tap on the notes the track's instrument receives
//! (`atomik_engine::EngineHandle::tap_notes`). What the tap saw is turned into notes: a note on
//! starts a note at the tick of its host frame, the next off of the key (or the song end) ends
//! it. The result depends only on the document, so it is the same at any block size.

use crate::RenderError;
use crate::plugins::MissingPlugins;
use crate::project::{Core, Extra, PluginBounce, ProjectOptions, render_core};
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{DeviceKey, RackDoc};
use std::path::Path;

/// A note the player stack generated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerNote {
    /// Start, in song ticks.
    pub tick: i64,
    /// Length in ticks (at least 1).
    pub length: i64,
    /// MIDI key.
    pub key: u8,
    /// Velocity in (0, 1].
    pub velocity: f32,
}

/// Renders the notes the instrument of `track` receives from its player stack, sorted by
/// `(tick, key)`. Plugins that are not installed play as stand-ins (only the notes matter).
///
/// # Errors
/// [`RenderError::Project`] when the track does not exist, has no player stack, plays a
/// container, or the render fails.
pub fn render_player_notes(
    doc: &RackDoc,
    registry: &Registry,
    options: &ProjectOptions,
    bundle_root: Option<&Path>,
    track: &DeviceKey,
) -> Result<Vec<PlayerNote>, RenderError> {
    let fail = |m: &str| RenderError::Project(m.to_owned());
    let t = doc.song.track(track).ok_or_else(|| fail("no such track"))?;
    if doc.notes_entry(t) == &t.device {
        return Err(fail("the track has no player stack to render"));
    }
    if doc.device(track).is_none_or(|d| d.inner.is_some()) {
        return Err(fail("the track's instrument is not a plain device"));
    }
    let extra = Extra {
        note_taps: vec![track.clone()],
        ..Extra::default()
    };
    let mut plugins = PluginBounce {
        missing: MissingPlugins::StandIn,
        ..PluginBounce::default()
    };
    let Core { notes, .. } =
        render_core(doc, registry, (options, bundle_root), &mut plugins, &extra)?
            .ok_or_else(|| fail("the render was cancelled"))?;
    let clock = doc.song.frame_clock(doc.sample_rate);
    let end = doc.song.end.max(PPQ);
    let mut out = Vec::new();
    // Open notes by key: (start tick, velocity).
    let mut open: [Option<(i64, f32)>; 128] = [None; 128];
    let close =
        |out: &mut Vec<PlayerNote>, key: usize, at: i64, open: &mut [Option<(i64, f32)>; 128]| {
            if let Some((tick, velocity)) = open[key].take() {
                out.push(PlayerNote {
                    tick,
                    length: (at - tick).max(1),
                    key: key as u8,
                    velocity,
                });
            }
        };
    for (_, events) in notes {
        for (frame, bytes) in events {
            let at = clock.tick_at(frame);
            match bytes.as_slice() {
                [0x90, key, v] => {
                    let k = usize::from(key & 0x7F);
                    close(&mut out, k, at, &mut open);
                    open[k] = Some((at, f32::from(*v) / 127.0));
                }
                [0x80, key, _] => close(&mut out, usize::from(key & 0x7F), at, &mut open),
                _ => {}
            }
        }
    }
    for k in 0..128 {
        close(&mut out, k, end, &mut open);
    }
    out.sort_by_key(|n| (n.tick, n.key));
    Ok(out)
}
