//! `WorkflowCommand::ImportMidi`: new instrument tracks from a MIDI file, and optionally its
//! tempo map, meter map and markers, as **one undo step** planned on a scratch copy of the
//! document (a rejected import changes nothing).
//!
//! The file is parsed and converted outside the model (the app turns a file into
//! [`ImportedTrack`]s). Each imported track gets a new instrument of the given type, with its
//! console channel exactly as `CreateInstrument` makes it but without an armed track (the
//! track is added unarmed, so importing eight tracks does not arm eight), named after the
//! MIDI track when the name is not blank, and one clip on its note lane. The clip is validated
//! like `AddClip` (notes sorted, controller lanes canonical, ticks in range).
//!
//! A tempo or meter map replaces the song's; markers are **added** to the existing ones (new
//! ids). When a clip reaches past the song end, the end moves to the next bar line after it
//! (the song end only ever grows here).

use crate::DeviceKey;
use crate::console_cmd::Planner;
use crate::history::Edit;
use crate::song::{Clip, Marker, MeterEvent, SongCommand, TempoEvent};
use crate::song_cmd::{check_end, check_markers, check_meter_map, check_tempo_map, new_track};
use crate::workflow_create::{CreateOptions, CreateSource, create_instrument};
use crate::{Command, Document, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;

/// One track to create from a MIDI file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    /// The new device's name (a blank name keeps the instrument's own).
    pub name: String,
    /// The clip for the track's note lane (its id is ignored and allocated).
    pub clip: Clip,
}

/// What `WorkflowCommand::ImportMidi` adds to the song.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiImport {
    /// The instrument type of every new track (an instrument with a Notes input).
    pub instrument: String,
    /// The tracks, in display order.
    pub tracks: Vec<ImportedTrack>,
    /// A tempo map to replace the song's (first event at tick 0).
    pub tempo: Option<Vec<TempoEvent>>,
    /// A meter map to replace the song's (first event at bar 1).
    pub meter: Option<Vec<MeterEvent>>,
    /// Markers to add, as `(tick, name)` (names trimmed, non-empty, at most 64 characters).
    pub markers: Vec<(i64, String)>,
}

/// Plans the import (module docs).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    import: MidiImport,
) -> Result<Edit, ModelError> {
    if import.tracks.is_empty()
        && import.tempo.is_none()
        && import.meter.is_none()
        && import.markers.is_empty()
    {
        return Err(ModelError::BadSong(
            "the MIDI import has nothing in it".into(),
        ));
    }
    let mut p = Planner::new(doc, registry);
    let source = CreateSource::Type(import.instrument.clone());
    let options = CreateOptions {
        route: true,
        skip_track: true,
        disarm: Vec::new(),
    };
    let mut farthest = 0i64;
    for t in import.tracks {
        let key: DeviceKey = rules::generate_key(p.doc(), &import.instrument);
        let index = p.doc().devices.len();
        let add = create_instrument(
            p.doc(),
            registry,
            index,
            &source,
            Some(key.clone()),
            &options,
        )?;
        p.push(add);
        let name = t.name.trim();
        if !name.is_empty() {
            let old = rules::resolve(p.doc(), &key)?.name.clone();
            p.push(Edit::Name {
                key: key.clone(),
                old,
                new: name.to_owned(),
            });
        }
        let index = p.doc().song.tracks.len();
        p.push(Edit::Track {
            index,
            old: None,
            new: Some(new_track(key.clone())),
        });
        farthest = farthest.max(t.clip.end());
        let add_clip = Command::Song(SongCommand::AddClip {
            device: key,
            lane: 0,
            clip: t.clip,
        });
        p.push(Document::plan(p.doc(), add_clip, registry)?.0);
    }
    if let Some(map) = import.tempo {
        check_tempo_map(&map).map_err(ModelError::OutOfRange)?;
        p.push(Edit::TempoMap {
            old: p.doc().song.tempo.clone(),
            new: map,
        });
    }
    if let Some(map) = import.meter {
        check_meter_map(&map).map_err(ModelError::OutOfRange)?;
        p.push(Edit::MeterMap {
            old: p.doc().song.meter.clone(),
            new: map,
        });
    }
    if !import.markers.is_empty() {
        let old = p.doc().song.markers.clone();
        let first = p.doc().song.next_marker_id();
        let mut new = old.clone();
        new.extend(
            import
                .markers
                .into_iter()
                .zip(first..)
                .map(|((tick, name), id)| Marker {
                    id,
                    tick,
                    name,
                    color: None,
                }),
        );
        p.push(Edit::Markers {
            old,
            new: check_markers(new)?,
        });
    }
    let song = &p.doc().song;
    if farthest > song.end {
        let table = song.meter_table();
        let end = table.map().next_bar_line(farthest - 1).max(farthest);
        check_end(end).map_err(ModelError::OutOfRange)?;
        p.push(Edit::SongEnd {
            old: song.end,
            new: end,
        });
    }
    Ok(p.finish())
}
