//! Insert and remove time: shifting the whole arrangement after a point, or closing a gap in it.
//!
//! [`insert_time`] opens `length` ticks at `at`: a clip that straddles `at` is split there and
//! its right half moves, every clip that starts at or after `at` moves, and so do the take
//! groups, the markers and ranges, the tempo and meter changes, the loop and the song End.
//! [`remove_time`] cuts `[at, at + length)` out and closes the gap: the parts of clips inside
//! it go (a clip that spans it keeps both sides, cut the way a split cuts them), everything
//! after it moves back, and the tempo and meter in effect after the gap carry on from `at`.
//! Both are one `Edit::Compound`, so one undo step.
//!
//! Anything that starts exactly on `at` moves (a marker, a tempo or meter change, a range or
//! loop start) except the song's first tempo and meter, which are the song's own. A note that
//! crosses `at` stays whole in the left half of its clip on insert (with the song's note tails;
//! without them it ends at the cut, as it always did) and the right half does not play it
//! again; on remove a note that would ring into the removed time ends where the cut starts.
//! An automation clip is cut like any other: each half plays the curve the clip had at its
//! edge, so the curve stays continuous on insert and joins with the value from the end of the
//! range on remove.
//!
//! What stays consistent, and what is refused:
//!
//! * a take group is moved or (inside a removed range) dropped as a whole; one that straddles
//!   an edge of the range is refused, because splitting a stack of takes has no single answer;
//! * with a meter change after the point, the range must start and end on bar lines (the meter
//!   map counts bars); with none, any tick will do;
//! * a project with a frozen track is refused (the render is tied to the tempo map);
//! * a tempo ramp that crosses the point is stretched (insert) or cut and restarted at `at`
//!   from the value it had at the end of the range (remove).

use crate::history::Edit;
use crate::song::{Clip, ClipContent, Marker, MeterEvent, Note, RangeMarker, TempoEvent};
use crate::song_cmd::{check_end, check_geometry, split_pair};
use crate::takes::TakeGroup;
use crate::{ClipId, ModelError, RackDoc};

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

/// Checks the range and the frozen state shared by both commands.
fn check_common(doc: &RackDoc, at: i64, length: i64) -> Result<(), ModelError> {
    if at < 0 || length <= 0 {
        return Err(bad(
            "the range needs a start of at least 0 and a length above 0",
        ));
    }
    if at.checked_add(length).is_none() {
        return Err(bad("the range is out of bounds"));
    }
    if doc.song.tracks.iter().any(|t| t.frozen.is_some()) {
        return Err(ModelError::Frozen(
            "a track is frozen: unfreeze it to insert or remove time".to_owned(),
        ));
    }
    Ok(())
}

/// Allocates clip ids from the song's next free id.
struct Ids {
    first: u64,
    next: u64,
}

impl Ids {
    fn new(doc: &RackDoc) -> Self {
        let first = doc.song.next_clip_id;
        Self { first, next: first }
    }

    fn alloc(&mut self) -> Result<ClipId, ModelError> {
        let id = ClipId(self.next);
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| bad("clip ids are exhausted"))?;
        Ok(id)
    }

    fn finish(self, edits: &mut Vec<Edit>) {
        if self.next != self.first {
            edits.push(Edit::NextClipId {
                old: self.first,
                new: self.next,
            });
        }
    }
}

fn clip_edit(device: &crate::DeviceKey, lane: usize, old: Option<Clip>, new: Option<Clip>) -> Edit {
    Edit::Clip {
        device: device.clone(),
        lane,
        old,
        new,
    }
}

fn moved(c: &Clip, by: i64) -> Result<Clip, ModelError> {
    let m = Clip {
        start: c.start + by,
        ..c.clone()
    };
    check_geometry(m.start, m.length, m.offset, m.loop_length).map_err(ModelError::OutOfRange)?;
    Ok(m)
}

/// The left piece of a clip that a removed range cut: a note that starts in it and would ring
/// on into the removed time (the song's note tails) ends where the piece does, so nothing of
/// the cut time is heard. A looped clip repeats its own content and is left alone.
fn shortened(mut left: Clip) -> Clip {
    let ClipContent::Notes(notes) = &left.content else {
        return left;
    };
    if left.loop_length.is_some() {
        return left;
    }
    let window_end = left.offset + left.length;
    let shown = |n: &Note| n.tick >= left.offset && n.tick < window_end;
    if notes
        .iter()
        .all(|n| !shown(n) || n.tick + n.length <= window_end)
    {
        return left;
    }
    let cut: Vec<Note> = notes
        .iter()
        .map(|n| Note {
            length: if shown(n) {
                n.length.min(window_end - n.tick)
            } else {
                n.length
            },
            ..*n
        })
        .collect();
    left.content = ClipContent::Notes(cut.into());
    left
}

/// The meter after `bars` whole bars are opened at `at` (`None` when `at` is past every meter
/// change, so nothing in the map moves). A change that starts on the bar line `at` itself
/// moves with the music that follows it; the opened bars take the signature before it.
fn meter_after_insert(
    doc: &RackDoc,
    at: i64,
    length: i64,
) -> Result<Option<Vec<MeterEvent>>, ModelError> {
    let table = doc.song.meter_table();
    let map = table.map();
    let b0 = map.bar_at(at);
    let on_line = map.bar_start(b0) == at;
    // The first signature (bar 1) never moves: the song starts with it.
    let moves = |e: &MeterEvent| e.bar > b0 || (on_line && e.bar == b0 && b0 > 1);
    if !doc.song.meter.iter().any(moves) {
        return Ok(None);
    }
    let bar_ticks = if on_line && b0 > 1 && doc.song.meter.iter().any(|e| e.bar == b0) {
        map.bar_ticks_at(at - 1)
    } else {
        map.bar_ticks_at(at)
    };
    if !on_line || length % bar_ticks != 0 {
        return Err(bad(
            "a meter change follows: the range must start on a bar line and be whole bars",
        ));
    }
    let bars = u32::try_from(length / bar_ticks).map_err(|_| bad("too many bars"))?;
    Ok(Some(
        doc.song
            .meter
            .iter()
            .map(|e| {
                if moves(e) {
                    MeterEvent {
                        bar: e.bar.saturating_add(bars),
                        ..*e
                    }
                } else {
                    *e
                }
            })
            .collect(),
    ))
}

/// The meter after the bars `[at, at + length)` are closed (`None` when no meter change
/// follows `at`).
fn meter_after_remove(
    doc: &RackDoc,
    at: i64,
    length: i64,
) -> Result<Option<Vec<MeterEvent>>, ModelError> {
    let table = doc.song.meter_table();
    let map = table.map();
    let b0 = map.bar_at(at);
    if !doc.song.meter.iter().any(|e| e.bar > b0) {
        return Ok(None);
    }
    let end = at + length;
    let b1 = map.bar_at(end);
    if map.bar_start(b0) != at || map.bar_start(b1) != end {
        return Err(bad(
            "a meter change follows: the range must start and end on bar lines",
        ));
    }
    let bars = b1 - b0;
    let (num, den) = map.meter_at(end);
    let mut out: Vec<MeterEvent> = Vec::new();
    for e in &doc.song.meter {
        if e.bar <= b0 {
            out.push(*e);
        } else if e.bar > b1 {
            out.push(MeterEvent {
                bar: e.bar - bars,
                ..*e
            });
        }
    }
    // The signature in effect at the end of the range carries on from its start.
    let current = out.last().map_or((num, den), |e| (e.num, e.den));
    if current != (num, den) {
        match out.last_mut() {
            Some(e) if e.bar == b0 => (e.num, e.den) = (num, den),
            _ => out.push(MeterEvent { bar: b0, num, den }),
        }
        out.sort_by_key(|e| e.bar);
    }
    Ok(Some(out))
}

/// The take groups of a lane after the move; `None` when none changes.
fn groups_after(
    groups: &[TakeGroup],
    at: i64,
    length: i64,
    insert: bool,
) -> Result<Option<Vec<TakeGroup>>, ModelError> {
    let mut out = Vec::with_capacity(groups.len());
    let mut changed = false;
    for g in groups {
        let (start, end) = g.range();
        let by = if insert {
            if end <= at {
                0
            } else if start >= at {
                length
            } else {
                return Err(bad(
                    "a take group spans the point: flatten or move it first",
                ));
            }
        } else if end <= at {
            0
        } else if start >= at + length {
            -length
        } else if start >= at && end <= at + length {
            changed = true;
            continue; // inside the removed range: the group goes with its clips
        } else {
            return Err(bad(
                "a take group crosses the edge of the range: flatten or move it first",
            ));
        };
        if by == 0 {
            out.push(g.clone());
            continue;
        }
        changed = true;
        let mut g = g.clone();
        for t in &mut g.takes {
            *t = moved(t, by)?;
        }
        for s in &mut g.comp {
            s.start += by;
            s.end += by;
        }
        out.push(g);
    }
    Ok(changed.then_some(out))
}

/// Plans `InsertTime`.
pub(crate) fn insert_time(
    doc: &RackDoc,
    at: i64,
    length: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    check_common(doc, at, length)?;
    let song = &doc.song;
    let meter = meter_after_insert(doc, at, length)?;
    let mut ids = Ids::new(doc);
    let mut edits = Vec::new();
    for track in &song.tracks {
        for (li, lane) in track.lanes.iter().enumerate() {
            if let Some(new) = groups_after(&lane.take_groups, at, length, true)? {
                edits.push(Edit::TakeGroups {
                    device: track.device.clone(),
                    lane: li,
                    old: lane.take_groups.clone(),
                    new,
                });
            }
            for c in &lane.clips {
                if c.end() <= at {
                    continue;
                }
                if c.start >= at {
                    edits.push(clip_edit(
                        &track.device,
                        li,
                        Some(c.clone()),
                        Some(moved(c, length)?),
                    ));
                    continue;
                }
                let (left, right) = split_pair(doc, c, at)?;
                let right = Clip {
                    id: ids.alloc()?,
                    ..moved(&right, length)?
                };
                edits.push(clip_edit(&track.device, li, Some(c.clone()), Some(left)));
                edits.push(clip_edit(&track.device, li, None, Some(right)));
            }
        }
    }
    ids.finish(&mut edits);

    let tempo: Vec<TempoEvent> = song
        .tempo
        .iter()
        .map(|e| {
            // A change that starts on `at` moves with the music after it (the opened time keeps
            // the tempo before it); the first change, at tick 0, is the song's own tempo.
            if e.tick >= at && e.tick > 0 {
                TempoEvent {
                    tick: e.tick + length,
                    ..*e
                }
            } else {
                *e
            }
        })
        .collect();
    if tempo != song.tempo {
        edits.push(Edit::TempoMap {
            old: song.tempo.clone(),
            new: tempo,
        });
    }
    if let Some(new) = meter {
        edits.push(Edit::MeterMap {
            old: song.meter.clone(),
            new,
        });
    }
    let markers: Vec<Marker> = song
        .markers
        .iter()
        .map(|m| Marker {
            tick: if m.tick >= at {
                m.tick + length
            } else {
                m.tick
            },
            ..m.clone()
        })
        .collect();
    if markers != song.markers {
        edits.push(Edit::Markers {
            old: song.markers.clone(),
            new: markers,
        });
    }
    let ranges: Vec<RangeMarker> = song
        .ranges
        .iter()
        .map(|r| RangeMarker {
            start: if r.start >= at {
                r.start + length
            } else {
                r.start
            },
            end: if r.end > at { r.end + length } else { r.end },
            ..r.clone()
        })
        .collect();
    if ranges != song.ranges {
        edits.push(Edit::Ranges {
            old: song.ranges.clone(),
            new: ranges,
        });
    }
    let (ls, le) = song.loop_range;
    // A loop that is exactly the range stays: the new bars are what it loops.
    if (ls, le) != (at, at + length) {
        // As the ranges: a start on `at` moves, an end on `at` stays.
        let new = (
            if ls >= at { ls + length } else { ls },
            if le > at { le + length } else { le },
        );
        if new != (ls, le) {
            edits.push(Edit::Loop {
                old: (song.loop_range, song.loop_enabled),
                new: (new, song.loop_enabled),
            });
        }
    }
    if song.end > at {
        let end = song.end + length;
        check_end(end).map_err(ModelError::OutOfRange)?;
        edits.push(Edit::SongEnd {
            old: song.end,
            new: end,
        });
    }
    if edits.is_empty() {
        return Err(bad("there is nothing after the point to move"));
    }
    Ok((Edit::Compound(edits), None))
}

/// Plans `RemoveTime`.
pub(crate) fn remove_time(
    doc: &RackDoc,
    at: i64,
    length: i64,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    check_common(doc, at, length)?;
    let song = &doc.song;
    let cut_end = at + length;
    let meter = meter_after_remove(doc, at, length)?;
    let mut ids = Ids::new(doc);
    let mut edits = Vec::new();
    for track in &song.tracks {
        for (li, lane) in track.lanes.iter().enumerate() {
            if let Some(new) = groups_after(&lane.take_groups, at, length, false)? {
                edits.push(Edit::TakeGroups {
                    device: track.device.clone(),
                    lane: li,
                    old: lane.take_groups.clone(),
                    new,
                });
            }
            for c in &lane.clips {
                let put =
                    |old: Option<Clip>, new: Option<Clip>| clip_edit(&track.device, li, old, new);
                if c.end() <= at {
                    continue;
                }
                if c.start >= cut_end {
                    edits.push(put(Some(c.clone()), Some(moved(c, -length)?)));
                    continue;
                }
                let starts_before = c.start < at;
                let ends_after = c.end() > cut_end;
                match (starts_before, ends_after) {
                    (false, false) => edits.push(put(Some(c.clone()), None)),
                    (true, false) => {
                        let (left, _) = split_pair(doc, c, at)?;
                        edits.push(put(Some(c.clone()), Some(shortened(left))));
                    }
                    (false, true) => {
                        let (_, right) = split_pair(doc, c, cut_end)?;
                        edits.push(put(Some(c.clone()), Some(moved(&right, -length)?)));
                    }
                    (true, true) => {
                        let (left, rest) = split_pair(doc, c, at)?;
                        let (_, right) = split_pair(doc, &rest, cut_end)?;
                        let right = Clip {
                            id: ids.alloc()?,
                            ..moved(&right, -length)?
                        };
                        edits.push(put(Some(c.clone()), Some(shortened(left))));
                        edits.push(put(None, Some(right)));
                    }
                }
            }
        }
    }
    ids.finish(&mut edits);

    // Tempo: what is in effect at the end of the range carries on from `at`.
    let table = song.tempo_table();
    let ramping = song
        .tempo
        .iter()
        .rev()
        .find(|e| e.tick < cut_end)
        .is_some_and(|e| e.ramp)
        && song.tempo.iter().any(|e| e.tick > cut_end);
    let mut tempo: Vec<TempoEvent> = Vec::new();
    for e in &song.tempo {
        if e.tick < at {
            tempo.push(*e);
        } else if e.tick >= cut_end {
            tempo.push(TempoEvent {
                tick: e.tick - length,
                ..*e
            });
        }
    }
    if !tempo.iter().any(|e| e.tick == at) {
        let bpm = table.map().bpm(cut_end as f64);
        // No event is needed when the tempo before the range already is the one after it.
        let held = tempo
            .iter()
            .rev()
            .find(|e| e.tick < at)
            .is_some_and(|e| !e.ramp && !ramping && (e.bpm - bpm).abs() < 1e-9);
        if !held {
            tempo.push(TempoEvent {
                tick: at,
                bpm,
                ramp: ramping,
            });
            tempo.sort_by_key(|e| e.tick);
        }
    }
    if tempo != song.tempo {
        edits.push(Edit::TempoMap {
            old: song.tempo.clone(),
            new: tempo,
        });
    }
    if let Some(new) = meter {
        edits.push(Edit::MeterMap {
            old: song.meter.clone(),
            new,
        });
    }
    let map = |t: i64| {
        if t < at {
            t
        } else if t < cut_end {
            at
        } else {
            t - length
        }
    };
    let markers: Vec<Marker> = song
        .markers
        .iter()
        .filter(|m| m.tick < at || m.tick >= cut_end)
        .map(|m| Marker {
            tick: map(m.tick),
            ..m.clone()
        })
        .collect();
    if markers != song.markers {
        edits.push(Edit::Markers {
            old: song.markers.clone(),
            new: markers,
        });
    }
    let ranges: Vec<RangeMarker> = song
        .ranges
        .iter()
        .map(|r| RangeMarker {
            start: map(r.start),
            end: map(r.end),
            ..r.clone()
        })
        .filter(|r| r.start < r.end)
        .collect();
    if ranges != song.ranges {
        edits.push(Edit::Ranges {
            old: song.ranges.clone(),
            new: ranges,
        });
    }
    let (ls, le) = song.loop_range;
    let new_loop = (map(ls), map(le));
    // A loop the cut empties stays as it was.
    if new_loop.0 < new_loop.1 && new_loop != (ls, le) {
        edits.push(Edit::Loop {
            old: (song.loop_range, song.loop_enabled),
            new: (new_loop, song.loop_enabled),
        });
    }
    if song.end > at {
        let floor = song.bar_ticks().min(song.end);
        let end = map(song.end).max(floor);
        if end != song.end {
            check_end(end).map_err(ModelError::OutOfRange)?;
            edits.push(Edit::SongEnd {
                old: song.end,
                new: end,
            });
        }
    }
    if edits.is_empty() {
        return Err(bad("there is nothing in the range or after it"));
    }
    Ok((Edit::Compound(edits), None))
}
