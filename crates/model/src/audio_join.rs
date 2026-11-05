//! Joining audio clips and moving the crossfade between two of them.
//!
//! * [`join_audio`] turns consecutive pieces of one recording on one lane back into a single
//!   clip. It is the inverse of a split: the pieces must touch (or overlap), play the same file
//!   without a jump, and agree on gain, warp and mute, so the joined clip sounds exactly as the
//!   pieces did. Anything else is refused with a message that names the clips and the file.
//! * [`move_crossfade`] drags the overlap of two audio clips along the timeline: the later clip
//!   starts later (or earlier) and the earlier one ends later (or earlier) by the same number of
//!   ticks, so the overlap keeps its length and the sound outside it does not change.
//! * [`resize_crossfade`] changes the length of the overlap: the later clip stays, the earlier
//!   one ends later (a longer crossfade) or earlier (a shorter one).

use crate::audio_edit::{extend_envelope_start, join_envelopes, slice_envelope};
use crate::history::Edit;
use crate::song::{AudioClipContent, Clip, ClipContent, crossfades};
use crate::song_cmd::{bad, check_geometry, find_clip, range};
use crate::warp::{WarpSettings, source_at_tick};
use crate::{ClipId, DeviceKey, ModelError, RackDoc};

type Found<'d> = (DeviceKey, usize, &'d Clip);

fn replace(device: &DeviceKey, lane: usize, old: &Clip, new: Clip) -> Edit {
    Edit::Clip {
        device: device.clone(),
        lane,
        old: Some(old.clone()),
        new: Some(new),
    }
}

fn audio_of(c: &Clip) -> Result<&AudioClipContent, ModelError> {
    match &c.content {
        ClipContent::Audio(a) => Ok(a),
        _ => Err(bad(format!("clip {} is not an audio clip", c.id.0))),
    }
}

/// The display name of the file an asset id stands for.
fn file_name(doc: &RackDoc, asset: &str) -> String {
    doc.assets
        .get(asset)
        .map_or_else(|| "an unknown file".to_owned(), |m| m.original_name.clone())
}

/// The warp settings without the markers (clips with markers do not join).
fn plain_warp(w: &WarpSettings) -> WarpSettings {
    WarpSettings {
        markers: Vec::new(),
        ..w.clone()
    }
}

/// Plans the join of audio clips `clips` (at least two, one lane, sorted by start).
pub(crate) fn join_audio(
    doc: &RackDoc,
    clips: &[Found<'_>],
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let song = &doc.song;
    let rate = doc.sample_rate;
    let clock = song.frame_clock(rate);
    let (device, lane, first) = (&clips[0].0, clips[0].1, clips[0].2);
    for (_, _, c) in clips {
        let a = audio_of(c)?;
        if c.loop_length.is_some() {
            return Err(bad(format!(
                "clip {} loops: convert the loop to separate clips before joining",
                c.id.0
            )));
        }
        if a.reverse {
            return Err(bad(format!(
                "clip {} plays backwards: a reversed clip cannot be joined",
                c.id.0
            )));
        }
        if !a.warp.markers.is_empty() {
            return Err(bad(format!(
                "clip {} has warp markers: a clip with markers cannot be joined",
                c.id.0
            )));
        }
        if c.muted != first.muted {
            return Err(bad("mute or unmute the clips alike before joining them"));
        }
    }
    let mut run = first.clone();
    let mut acc = audio_of(first)?.clone();
    let mut end = first.end();
    for (_, _, next) in &clips[1..] {
        let b = audio_of(next)?;
        if b.asset != acc.asset {
            return Err(bad(format!(
                "clips {} and {} play different files ('{}' and '{}'): only pieces of one file can be \
                 joined; bounce them in place to get one clip",
                run.id.0,
                next.id.0,
                file_name(doc, &acc.asset),
                file_name(doc, &b.asset)
            )));
        }
        if next.start > end {
            return Err(bad(format!(
                "clip {} does not touch the clip before it: move it against the previous one first",
                next.id.0
            )));
        }
        if b.gain_db.to_bits() != acc.gain_db.to_bits()
            || plain_warp(&b.warp) != plain_warp(&acc.warp)
        {
            return Err(bad(format!(
                "clips {} and {} differ in gain or warp: give them the same settings first",
                run.id.0, next.id.0
            )));
        }
        let asset_frames = doc.assets.get(&acc.asset).map_or(u64::MAX, |m| m.frames);
        let delta = clock.at(next.start).saturating_sub(clock.at(run.start));
        let (expected, _) = acc.piece_from(song, rate, run.start, next.start, delta, asset_frames);
        if expected.abs_diff(b.source_offset) > 1 {
            return Err(bad(format!(
                "clip {} does not continue clip {} in '{}': joining would change what plays",
                next.id.0,
                run.id.0,
                file_name(doc, &acc.asset)
            )));
        }
        if b.fade_in > 0 {
            return Err(bad(format!(
                "clip {} fades in at the join: set its fade in to 0 first",
                next.id.0
            )));
        }
        if next.end() > end {
            if acc.fade_out > 0 {
                return Err(bad(format!(
                    "clip {} fades out at the join: set its fade out to 0 first",
                    run.id.0
                )));
            }
            acc.envelope = join_envelopes(&acc.envelope, &b.envelope, delta)
                .ok_or_else(|| bad("the joined gain envelope would have too many points"))?;
            acc.fade_out = b.fade_out;
            end = next.end();
            run.length = end - run.start;
        } else if b.fade_out > 0 {
            return Err(bad(format!(
                "clip {} fades out inside the joined clip: set its fade out to 0 first",
                next.id.0
            )));
        }
    }
    let joined = Clip {
        length: end - first.start,
        offset: 0,
        content: ClipContent::Audio(acc),
        ..first.clone()
    };
    check_geometry(joined.start, joined.length, 0, None).map_err(range)?;
    let mut edits: Vec<Edit> = clips[1..]
        .iter()
        .map(|(d, l, c)| Edit::Clip {
            device: d.clone(),
            lane: *l,
            old: Some((*c).clone()),
            new: None,
        })
        .collect();
    edits.push(replace(device, lane, first, joined));
    Ok((Edit::Compound(edits), Some(first.id)))
}

/// Plans `MoveCrossfade`: the overlap that audio clip `later` makes with the clip before it
/// moves by `delta` ticks.
pub(crate) fn move_crossfade(
    doc: &RackDoc,
    later: ClipId,
    delta: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if delta == 0 {
        return Err(bad("the crossfade does not move"));
    }
    let (device, lane, lc) = find_clip(doc, later)?;
    let song = &doc.song;
    let rate = doc.sample_rate;
    let zone = song
        .track(&device)
        .and_then(|t| t.lanes.get(lane))
        .and_then(|l| crossfades(l).into_iter().find(|z| z.later == later))
        .ok_or_else(|| {
            bad(format!(
                "clip {} does not crossfade with a clip before it",
                later.0
            ))
        })?;
    let (_, _, ec) = find_clip(doc, zone.earlier)?;
    let (la, ea) = (audio_of(lc)?, audio_of(ec)?);
    if la.reverse || ea.reverse {
        return Err(bad("the crossfade of a reversed clip cannot be moved"));
    }
    if lc.loop_length.is_some() || ec.loop_length.is_some() {
        return Err(bad("the crossfade of a looped clip cannot be moved"));
    }
    if zone.end != ec.end() {
        return Err(bad(
            "the clip before runs past the crossfade: only a crossfade where it ends can move",
        ));
    }
    let (new_start, new_end) = (lc.start + delta, zone.end + delta);
    if new_start <= ec.start || new_end >= lc.end() {
        return Err(range(
            "the crossfade cannot move that far: it has to stay inside both clips",
        ));
    }
    let clock = song.frame_clock(rate);
    let asset_frames = doc.assets.get(&la.asset).map_or(u64::MAX, |m| m.frames);
    // The later clip starts at the new tick and plays the same audio at every tick.
    let mut later_audio = la.clone();
    if delta > 0 {
        let plain = clock.at(new_start) - clock.at(lc.start);
        let (offset, markers) = la.piece_from(song, rate, lc.start, new_start, plain, asset_frames);
        later_audio.source_offset = offset;
        later_audio.warp.markers = markers;
        later_audio.envelope = slice_envelope(&la.envelope, plain, u64::MAX);
    } else {
        let plain = clock.at(lc.start) - clock.at(new_start);
        let no_audio = || bad("the clip has no audio before its start to crossfade over");
        if la.warp.enabled {
            let at = source_at_tick(song, rate, lc.start, la, new_start).round();
            if at < 0.0 {
                return Err(no_audio());
            }
            later_audio.source_offset = at as u64;
            later_audio.warp.markers =
                la.warp
                    .markers_after(delta, la.source_offset, later_audio.source_offset);
        } else {
            later_audio.source_offset = la.source_offset.checked_sub(plain).ok_or_else(no_audio)?;
        }
        later_audio.envelope = extend_envelope_start(&la.envelope, plain)
            .ok_or_else(|| bad("the gain envelope would have too many points"))?;
    }
    // The earlier clip must still have audio to play where it now reaches.
    if delta > 0 {
        let earlier_frames = doc.assets.get(&ea.asset).map_or(u64::MAX, |m| m.frames);
        let needed = source_at_tick(song, rate, ec.start, ea, new_end);
        if needed > earlier_frames as f64 + 1.0 {
            return Err(bad("the clip before has no audio left after its end"));
        }
    }
    let new_later = Clip {
        start: new_start,
        length: lc.end() - new_start,
        offset: 0,
        content: ClipContent::Audio(later_audio),
        ..lc.clone()
    };
    let new_earlier = Clip {
        length: new_end - ec.start,
        ..ec.clone()
    };
    for c in [&new_later, &new_earlier] {
        check_geometry(c.start, c.length, 0, None).map_err(range)?;
    }
    Ok((
        Edit::Compound(vec![
            replace(&device, lane, ec, new_earlier),
            replace(&device, lane, lc, new_later),
        ]),
        Some(later),
    ))
}

/// Plans `ResizeCrossfade`: the overlap that audio clip `later` makes with the clip before it
/// grows by `delta` ticks (shrinks for a negative one). The later clip stays where it is and
/// the clip before ends `delta` ticks later, so only the earlier clip's tail changes length.
pub(crate) fn resize_crossfade(
    doc: &RackDoc,
    later: ClipId,
    delta: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    if delta == 0 {
        return Err(bad("the crossfade keeps its length"));
    }
    let (device, lane, lc) = find_clip(doc, later)?;
    let song = &doc.song;
    let zone = song
        .track(&device)
        .and_then(|t| t.lanes.get(lane))
        .and_then(|l| crossfades(l).into_iter().find(|z| z.later == later))
        .ok_or_else(|| {
            bad(format!(
                "clip {} does not crossfade with a clip before it",
                later.0
            ))
        })?;
    let (_, _, ec) = find_clip(doc, zone.earlier)?;
    let ea = audio_of(ec)?;
    audio_of(lc)?;
    if ea.reverse {
        return Err(bad("the crossfade of a reversed clip cannot be resized"));
    }
    if lc.loop_length.is_some() || ec.loop_length.is_some() {
        return Err(bad("the crossfade of a looped clip cannot be resized"));
    }
    if zone.end != ec.end() {
        return Err(bad(
            "the clip before runs past the crossfade: only a crossfade where it ends can be resized",
        ));
    }
    let new_end = zone.end + delta;
    if new_end <= lc.start || new_end >= lc.end() {
        return Err(range(
            "the crossfade cannot be that long or that short: it has to stay inside the later clip",
        ));
    }
    // A longer crossfade needs audio left in the earlier clip's file.
    if delta > 0 {
        let frames = doc.assets.get(&ea.asset).map_or(u64::MAX, |m| m.frames);
        let needed = source_at_tick(song, doc.sample_rate, ec.start, ea, new_end);
        if needed > frames as f64 + 1.0 {
            return Err(bad("the clip before has no audio left after its end"));
        }
    }
    let new_earlier = Clip {
        length: new_end - ec.start,
        ..ec.clone()
    };
    check_geometry(new_earlier.start, new_earlier.length, 0, None).map_err(range)?;
    Ok((
        Edit::Compound(vec![replace(&device, lane, ec, new_earlier)]),
        Some(later),
    ))
}
