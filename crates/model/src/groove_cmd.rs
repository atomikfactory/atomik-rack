//! Planning of the quantize and groove commands (`ApplyQuantize`, `QuantizeNotes`, `SetGroove`,
//! `DeleteGroove`, `ExtractGroove`); the arithmetic is in [`crate::groove`].

use crate::groove::{
    Groove, MAX_GROOVES, QuantizeParams, builtin_groove, extract_groove, quantize_notes,
};
use crate::history::Edit;
use crate::song::{Clip, ClipContent, ClipId, Song, SongCommand};
use crate::song_cmd::{bad, check_note, range};
use crate::{DeviceKey, ModelError, RackDoc};
use std::collections::BTreeSet;

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
    let out = quantize_notes(notes, selected, c.start - c.offset, clip.0, params, groove);
    // The same rules a SetClipContent of these notes would meet, so the step is always one the
    // document accepts (and saves).
    for n in &out.notes {
        check_note(n.tick, n.length, n.key, n.velocity).map_err(range)?;
    }
    if out.notes == *notes {
        return Ok(None);
    }
    Ok(Some(Edit::Clip {
        device,
        lane,
        old: Some(c.clone()),
        new: Some(Clip {
            content: ClipContent::Notes(out.notes),
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
        SongCommand::DeleteGroove { name } => {
            let mut new = song.grooves.clone();
            let at = new
                .iter()
                .position(|g| g.name == name)
                .ok_or_else(|| ModelError::UnknownKey(format!("groove {name}")))?;
            new.remove(at);
            Ok(Edit::Grooves {
                old: song.grooves.clone(),
                new,
            })
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
