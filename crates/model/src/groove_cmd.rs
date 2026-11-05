//! Planning of the quantize and groove commands (`ApplyQuantize`, `QuantizeNotes`, `SetGroove`,
//! `RenameGroove`, `DeleteGroove`, `ExtractGroove`); the arithmetic is in [`crate::groove`].

use crate::controller_edit::follow_note_moves;
use crate::groove::{
    GROOVE_STRENGTH_MAX, Groove, MAX_GROOVES, QuantizeParams, builtin_groove, extract_groove,
    quantize_notes_in,
};
use crate::history::{Edit, Swap};
use crate::song::{Clip, ClipContent, ClipId, Song, SongCommand, TrackGroove};
use crate::song_cmd::{bad, check_note, range};
use crate::{DeviceKey, ModelError, RackDoc};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The clip `id` with its device and lane index.
fn find(doc: &RackDoc, id: ClipId) -> Result<(DeviceKey, usize, &Clip), ModelError> {
    doc.song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))
}

/// Whether a take group compiled clip `id` (its notes are rebuilt from the takes, so a quantize
/// would be lost, or worse, disagree with the comp).
fn is_compiled(doc: &RackDoc, device: &DeviceKey, lane: usize, id: ClipId) -> bool {
    doc.song.track(device).is_some_and(|t| {
        t.lanes[lane]
            .take_groups
            .iter()
            .any(|g| g.clips.contains(&id))
    })
}

/// The groove `params` names, looked up in the song and the built-in ones.
fn lookup(song: &Song, params: &QuantizeParams) -> Result<Option<Groove>, ModelError> {
    params
        .groove
        .as_deref()
        .map(|name| {
            song.groove(name)
                .ok_or_else(|| ModelError::UnknownKey(format!("groove {name}")))
        })
        .transpose()
}

/// The edit that quantizes the notes of `clip` (`selected`: only those) or `None` when no note
/// changes (or the clip is not a plain note clip).
fn clip_edit(
    doc: &RackDoc,
    clip: ClipId,
    selected: Option<&BTreeSet<usize>>,
    params: &QuantizeParams,
    groove: Option<&Groove>,
) -> Result<Option<Edit>, ModelError> {
    let (device, lane, c) = find(doc, clip)?;
    let ClipContent::Notes(notes) = &c.content else {
        return Ok(None);
    };
    if is_compiled(doc, &device, lane, clip) {
        return Ok(None);
    }
    let meter = doc.song.meter_table();
    let out = quantize_notes_in(
        notes,
        selected,
        c.start - c.offset,
        clip.0,
        params,
        groove,
        Some(meter.map()),
    );
    // The same rules a SetClipContent of these notes would meet, so the step is always one the
    // document accepts (and saves).
    for n in &out.notes {
        check_note(n.tick, n.length, n.key, n.velocity).map_err(range)?;
    }
    if out.notes == *notes {
        return Ok(None);
    }
    let controllers = if params.controllers && !c.controllers.is_empty() {
        follow_note_moves(&c.controllers, &out.moves(notes))
    } else {
        c.controllers.clone()
    };
    Ok(Some(Edit::Clip {
        device,
        lane,
        old: Some(c.clone()),
        new: Some(Clip {
            content: ClipContent::Notes(out.notes.into()),
            controllers,
            ..c.clone()
        }),
    }))
}

fn finish(edits: Vec<Edit>) -> Result<Edit, ModelError> {
    if edits.is_empty() {
        return Err(bad("quantize changes no note"));
    }
    Ok(Edit::Compound(edits))
}

/// `groove` added to, or replacing the one of the same name in, the library.
fn with_groove(song: &Song, groove: Groove) -> Result<Edit, ModelError> {
    groove.validate().map_err(range)?;
    if builtin_groove(&groove.name).is_some() {
        return Err(bad(format!(
            "{:?} is the name of a built-in groove",
            groove.name
        )));
    }
    let mut new = song.grooves.clone();
    match new.iter().position(|g| g.name == groove.name) {
        Some(i) => new[i] = groove,
        None if new.len() >= MAX_GROOVES => {
            return Err(range(format!("a song holds at most {MAX_GROOVES} grooves")));
        }
        None => new.push(groove),
    }
    Ok(Edit::Grooves {
        old: song.grooves.clone(),
        new,
    })
}

/// The edits that give every track and every note clip that names the groove `name`
/// `with(its live groove)` instead.
fn retarget(
    song: &Song,
    name: &str,
    with: impl Fn(&TrackGroove) -> Option<TrackGroove>,
) -> Vec<Edit> {
    let mut edits: Vec<Edit> = song
        .tracks
        .iter()
        .filter_map(|t| {
            let old = t.groove.clone().filter(|g| g.name == name)?;
            Some(Edit::Swap(Swap::TrackGroove {
                device: t.device.clone(),
                new: with(&old),
                old: Some(old),
            }))
        })
        .collect();
    for t in &song.tracks {
        for (lane, l) in t.lanes.iter().enumerate() {
            for c in &l.clips {
                let Some(old) = c.groove.as_deref().filter(|g| g.name == name) else {
                    continue;
                };
                edits.push(Edit::Clip {
                    device: t.device.clone(),
                    lane,
                    old: Some(c.clone()),
                    new: Some(Clip {
                        groove: with(old).map(Arc::new),
                        ..c.clone()
                    }),
                });
            }
        }
    }
    edits
}

/// The edit that gives the note clips `clips` the live groove `groove`.
fn set_clip_groove(
    doc: &RackDoc,
    clips: Vec<ClipId>,
    groove: Option<TrackGroove>,
) -> Result<Edit, ModelError> {
    if let Some(g) = &groove {
        if doc.song.groove(&g.name).is_none() {
            return Err(ModelError::UnknownKey(format!("groove {}", g.name)));
        }
        if g.strength > GROOVE_STRENGTH_MAX {
            return Err(range(format!(
                "groove strength {} is outside 0..={GROOVE_STRENGTH_MAX}",
                g.strength
            )));
        }
    }
    let ids: BTreeSet<ClipId> = clips.into_iter().collect();
    if ids.is_empty() {
        return Err(bad("no clips are selected"));
    }
    let mut edits = Vec::new();
    for id in ids {
        let (device, lane, c) = find(doc, id)?;
        if !c.content.is_notes() {
            return Err(bad(format!("clip {} is not a note clip", id.0)));
        }
        if is_compiled(doc, &device, lane, id) {
            return Err(bad(format!(
                "clip {} is compiled from takes; give the track the groove instead",
                id.0
            )));
        }
        if c.groove.as_deref() != groove.as_ref() {
            edits.push(Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    groove: groove.clone().map(Arc::new),
                    ..c.clone()
                }),
            });
        }
    }
    if edits.is_empty() {
        return Err(bad("the clips already have that groove"));
    }
    Ok(Edit::Compound(edits))
}

/// Plans one quantize or groove command.
pub(crate) fn plan(doc: &RackDoc, command: SongCommand) -> Result<Edit, ModelError> {
    let song = &doc.song;
    match command {
        SongCommand::ApplyQuantize { clips, quantize } => {
            quantize.validate().map_err(range)?;
            let groove = lookup(song, &quantize)?;
            let ids: BTreeSet<ClipId> = clips.into_iter().collect();
            let mut edits = Vec::new();
            for id in ids {
                edits.extend(clip_edit(doc, id, None, &quantize, groove.as_ref())?);
            }
            finish(edits)
        }
        SongCommand::QuantizeNotes {
            clip,
            notes,
            quantize,
        } => {
            quantize.validate().map_err(range)?;
            let groove = lookup(song, &quantize)?;
            let (_, _, c) = find(doc, clip)?;
            if let ClipContent::Notes(list) = &c.content
                && let Some(bad_index) = notes.iter().find(|&&i| i >= list.len())
            {
                return Err(range(format!(
                    "clip {} has {} notes, not note {bad_index}",
                    clip.0,
                    list.len()
                )));
            }
            if !c.content.is_notes() {
                return Err(bad(format!("clip {} is not a note clip", clip.0)));
            }
            let selected: BTreeSet<usize> = notes.into_iter().collect();
            finish(
                clip_edit(doc, clip, Some(&selected), &quantize, groove.as_ref())?
                    .into_iter()
                    .collect(),
            )
        }
        SongCommand::SetGroove(groove) => with_groove(song, groove),
        SongCommand::SetClipGroove { clips, groove } => set_clip_groove(doc, clips, groove),
        SongCommand::RenameGroove { from, to } => {
            let mut new = song.grooves.clone();
            let at = new
                .iter()
                .position(|g| g.name == from)
                .ok_or_else(|| ModelError::UnknownKey(format!("groove {from}")))?;
            if from == to {
                return Err(bad(format!("the groove is already called {to:?}")));
            }
            new[at].name = to.clone();
            new[at].validate().map_err(range)?;
            if song.groove(&to).is_some() {
                return Err(bad(format!("a groove called {to:?} exists already")));
            }
            let mut edits = vec![Edit::Grooves {
                old: song.grooves.clone(),
                new,
            }];
            edits.extend(retarget(song, &from, |g| {
                Some(TrackGroove {
                    name: to.clone(),
                    strength: g.strength,
                })
            }));
            Ok(Edit::Compound(edits))
        }
        SongCommand::DeleteGroove { name } => {
            let mut new = song.grooves.clone();
            let at = new
                .iter()
                .position(|g| g.name == name)
                .ok_or_else(|| ModelError::UnknownKey(format!("groove {name}")))?;
            new.remove(at);
            let mut edits = vec![Edit::Grooves {
                old: song.grooves.clone(),
                new,
            }];
            edits.extend(retarget(song, &name, |_| None));
            Ok(Edit::Compound(edits))
        }
        SongCommand::ExtractGroove {
            clip,
            name,
            resolution,
            slots,
        } => {
            let (_, _, c) = find(doc, clip)?;
            let ClipContent::Notes(notes) = &c.content else {
                return Err(bad(format!("clip {} is not a note clip", clip.0)));
            };
            let groove = extract_groove(&name, notes, c.start - c.offset, resolution, slots)
                .map_err(range)?;
            with_groove(song, groove)
        }
        _ => unreachable!("only quantize and groove commands are routed here"),
    }
}
