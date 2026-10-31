//! Note tools: velocity shaping, random alteration and pitch edits over the notes of a clip, a
//! selection of them or many clips at once (`SongCommand::ApplyNoteTool`).
//!
//! Every function here is pure: the same notes and the same arguments give the same notes. The
//! random tools take a seed; their offsets are a pure function of `(seed, clip id, the note's
//! tick and key before the edit)` through [`crate::groove::hash64`], the same splitmix the
//! humanize of a quantize uses, so a bounce is reproducible and an undo then redo changes
//! nothing. Another seed is another take of the same amounts.
//!
//! A clip holds one note per `(tick, key)`. The pitch tools keep that: a note whose new key is
//! already taken at its tick keeps its old key (or, when that was taken too, the nearest free
//! one), so no note is ever dropped or stacked.

use crate::groove::{MIN_VELOCITY, hash64, signed_unit};
use crate::history::Edit;
use crate::song::{Clip, ClipContent, ClipId, Note, SongCommand};
use crate::song_cmd::{bad, check_note, range};
use crate::{DeviceKey, ModelError, RackDoc};
use std::collections::BTreeSet;

/// The largest factor of [`NoteTool::ScaleVelocity`].
pub const MAX_VELOCITY_SCALE: f32 = 4.0;
/// The widest [`NoteTool::RandomizePitch`] range, in semitones either way.
pub const MAX_PITCH_RANGE: u8 = 48;

/// One note edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteTool {
    /// Every note gets this velocity, `0..=1` (raised to the lowest velocity when 0).
    SetVelocity(f32),
    /// Every velocity is multiplied by this factor, `0..=`[`MAX_VELOCITY_SCALE`], then clamped.
    ScaleVelocity(f32),
    /// A straight line of velocity from the earliest to the latest note by start tick. `None`
    /// keeps that end note's own velocity.
    RampVelocity {
        /// The velocity at the earliest note, `0..=1`.
        start: Option<f32>,
        /// The velocity at the latest note, `0..=1`.
        end: Option<f32>,
    },
    /// Adds a random offset of up to `amount` (`0..=1` of the whole velocity range) either way.
    RandomizeVelocity {
        /// The widest offset.
        amount: f32,
        /// Another seed is another take.
        seed: u32,
    },
    /// Each note, with probability `chance` (`0..=1`), takes the key of a random note of the
    /// edited set: the pattern changes but stays on pitches it already used.
    AlterNotes {
        /// The probability of a note changing.
        chance: f32,
        /// Another seed is another take.
        seed: u32,
    },
    /// Moves each note by a random number of semitones in `-range..=range`.
    RandomizePitch {
        /// The widest move, `0..=`[`MAX_PITCH_RANGE`].
        range: u8,
        /// Another seed is another take.
        seed: u32,
    },
    /// Every note gets this key (`0..=127`); notes that would stack keep their key.
    SetPitch(u8),
}

impl NoteTool {
    /// Checks every range; `Err` says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |what: &str, v: f32| {
            if (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{what} {v} is outside 0..=1"))
            }
        };
        match *self {
            NoteTool::SetVelocity(v) => unit("velocity", v),
            NoteTool::ScaleVelocity(f) => {
                if (0.0..=MAX_VELOCITY_SCALE).contains(&f) {
                    Ok(())
                } else {
                    Err(format!(
                        "velocity scale {f} is outside 0..={MAX_VELOCITY_SCALE}"
                    ))
                }
            }
            NoteTool::RampVelocity { start, end } => {
                start.map_or(Ok(()), |v| unit("ramp start", v))?;
                end.map_or(Ok(()), |v| unit("ramp end", v))
            }
            NoteTool::RandomizeVelocity { amount, .. } => unit("randomize amount", amount),
            NoteTool::AlterNotes { chance, .. } => unit("alter chance", chance),
            NoteTool::RandomizePitch { range, .. } => {
                if range <= MAX_PITCH_RANGE {
                    Ok(())
                } else {
                    Err(format!("pitch range {range} is above {MAX_PITCH_RANGE}"))
                }
            }
            NoteTool::SetPitch(key) => {
                if key <= 127 {
                    Ok(())
                } else {
                    Err(format!("key {key} is outside 0..=127"))
                }
            }
        }
    }

    /// A short name for the undo list and logs.
    pub fn label(&self) -> &'static str {
        match self {
            NoteTool::SetVelocity(_) => "Set Velocity",
            NoteTool::ScaleVelocity(_) => "Scale Velocity",
            NoteTool::RampVelocity { .. } => "Ramp Velocity",
            NoteTool::RandomizeVelocity { .. } => "Randomize Velocity",
            NoteTool::AlterNotes { .. } => "Alter Notes",
            NoteTool::RandomizePitch { .. } => "Randomize Notes",
            NoteTool::SetPitch(_) => "Set Pitch",
        }
    }
}

fn clamp_velocity(v: f32) -> f32 {
    if v.is_nan() {
        0.8
    } else {
        v.clamp(MIN_VELOCITY, 1.0)
    }
}

/// The per-note hash input: where the note is before the edit.
fn at(n: &Note) -> u64 {
    (n.tick as u64) ^ (u64::from(n.key) << 56)
}

/// A hash as a value in `[0, 1)`.
fn unit(hash: u64) -> f64 {
    (signed_unit(hash) + 1.0) / 2.0
}

/// Gives every note of `moved` that changed key its place, keeping one note per `(tick, key)`.
/// Notes outside `edited` claim their places first, then the edited ones in order.
fn place_keys(old: &[Note], edited: &dyn Fn(usize) -> bool, wanted: &[u8]) -> Indexed {
    let mut taken: BTreeSet<(i64, u8)> = old
        .iter()
        .enumerate()
        .filter(|(i, _)| !edited(*i))
        .map(|(_, n)| (n.tick, n.key))
        .collect();
    let mut out: Vec<(Note, usize)> = old.iter().copied().zip(0..).collect();
    for (i, n) in old.iter().enumerate().filter(|(i, _)| edited(*i)) {
        let key = [wanted[i], n.key]
            .into_iter()
            .find(|k| !taken.contains(&(n.tick, *k)))
            .or_else(|| (0..=127u8).find(|k| !taken.contains(&(n.tick, *k))))
            .unwrap_or(n.key);
        taken.insert((n.tick, key));
        out[i].0.key = key;
    }
    out.sort_by_key(|(n, _)| (n.tick, n.key));
    let mut index = vec![0; out.len()];
    for (pos, (_, i)) in out.iter().enumerate() {
        index[*i] = pos;
    }
    (out.into_iter().map(|(n, _)| n).collect(), Some(index))
}

/// Notes and, when the order changed, where each input note went.
type Indexed = (Vec<Note>, Option<Vec<usize>>);

/// Applies `tool` to the notes at the indices of `selected` (every note when `None`) of the clip
/// `clip_seed`. The result is sorted by `(tick, key)`.
pub fn apply_note_tool(
    notes: &[Note],
    selected: Option<&BTreeSet<usize>>,
    clip_seed: u64,
    tool: NoteTool,
) -> Vec<Note> {
    apply_note_tool_indexed(notes, selected, clip_seed, tool).0
}

/// [`apply_note_tool`] plus where each input note went: `result.0[result.1[i]]` is input note
/// `i` (an editor keeps its selection with it).
pub fn apply_note_tool_indexed(
    notes: &[Note],
    selected: Option<&BTreeSet<usize>>,
    clip_seed: u64,
    tool: NoteTool,
) -> (Vec<Note>, Vec<usize>) {
    let (out, index) = run(notes, selected, clip_seed, tool);
    (out, index.unwrap_or_else(|| (0..notes.len()).collect()))
}

fn run(
    notes: &[Note],
    selected: Option<&BTreeSet<usize>>,
    clip_seed: u64,
    tool: NoteTool,
) -> Indexed {
    let edited = |i: usize| selected.is_none_or(|s| s.contains(&i));
    let chosen = || notes.iter().enumerate().filter(|(i, _)| edited(*i));
    let map = |f: &dyn Fn(&Note) -> Note| -> Indexed {
        let out = notes
            .iter()
            .enumerate()
            .map(|(i, n)| if edited(i) { f(n) } else { *n })
            .collect();
        (out, None)
    };
    match tool {
        NoteTool::SetVelocity(v) => map(&|n| Note {
            velocity: clamp_velocity(v),
            ..*n
        }),
        NoteTool::ScaleVelocity(f) => map(&|n| Note {
            velocity: clamp_velocity(n.velocity * f),
            ..*n
        }),
        NoteTool::RampVelocity { start, end } => {
            // Ties on the start tick: the lower key is "first" and the higher "last".
            let first = chosen()
                .min_by_key(|(_, n)| (n.tick, n.key))
                .map(|(_, n)| *n);
            let last = chosen()
                .max_by_key(|(_, n)| (n.tick, n.key))
                .map(|(_, n)| *n);
            let (Some(first), Some(last)) = (first, last) else {
                return (notes.to_vec(), None);
            };
            let from = start.unwrap_or(first.velocity);
            let to = end.unwrap_or(last.velocity);
            let span = (last.tick - first.tick) as f64;
            map(&|n| {
                let t = if span > 0.0 {
                    (n.tick - first.tick) as f64 / span
                } else {
                    0.0
                };
                let v = f64::from(from) + (f64::from(to) - f64::from(from)) * t;
                Note {
                    velocity: clamp_velocity(v as f32),
                    ..*n
                }
            })
        }
        NoteTool::RandomizeVelocity { amount, seed } => map(&|n| {
            let u = signed_unit(hash64(u64::from(seed) ^ 0xA5A5_A5A5, clip_seed, at(n)));
            Note {
                velocity: clamp_velocity(n.velocity + (u * f64::from(amount)) as f32),
                ..*n
            }
        }),
        NoteTool::AlterNotes { chance, seed } => {
            let keys: Vec<u8> = chosen()
                .map(|(_, n)| n.key)
                .collect::<BTreeSet<u8>>()
                .into_iter()
                .collect();
            if keys.len() < 2 {
                return (notes.to_vec(), None);
            }
            let wanted: Vec<u8> = notes
                .iter()
                .map(|n| {
                    let s = u64::from(seed);
                    let hit = unit(hash64(s ^ 0x5EED_0001, clip_seed, at(n))) < f64::from(chance);
                    if hit {
                        let pick = unit(hash64(s ^ 0x5EED_0002, clip_seed, at(n)));
                        keys[((pick * keys.len() as f64) as usize).min(keys.len() - 1)]
                    } else {
                        n.key
                    }
                })
                .collect();
            place_keys(notes, &edited, &wanted)
        }
        NoteTool::RandomizePitch { range, seed } => {
            let wanted: Vec<u8> = notes
                .iter()
                .map(|n| {
                    let u = signed_unit(hash64(u64::from(seed) ^ 0x5EED_0003, clip_seed, at(n)));
                    let semis = (u * f64::from(range)).round() as i32;
                    (i32::from(n.key) + semis).clamp(0, 127) as u8
                })
                .collect();
            place_keys(notes, &edited, &wanted)
        }
        NoteTool::SetPitch(key) => {
            let wanted = vec![key; notes.len()];
            place_keys(notes, &edited, &wanted)
        }
    }
}

/// The clip `id` with its device and lane index.
fn find(doc: &RackDoc, id: ClipId) -> Result<(DeviceKey, usize, &Clip), ModelError> {
    doc.song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))
}

/// Plans `SongCommand::ApplyNoteTool`: one compound edit over every plain note clip named.
pub(crate) fn plan(doc: &RackDoc, command: SongCommand) -> Result<Edit, ModelError> {
    let SongCommand::ApplyNoteTool {
        clips,
        selection,
        tool,
    } = command
    else {
        unreachable!("only the note tool command is routed here");
    };
    tool.validate().map_err(range)?;
    let ids: BTreeSet<ClipId> = clips.into_iter().collect();
    if selection.is_some() && ids.len() != 1 {
        return Err(bad("a note selection needs exactly one clip"));
    }
    let selected: Option<BTreeSet<usize>> = selection.map(|s| s.into_iter().collect());
    let mut edits = Vec::new();
    for id in ids {
        let (device, lane, c) = find(doc, id)?;
        let ClipContent::Notes(notes) = &c.content else {
            // Audio and automation clips are skipped (a selection names notes: an error).
            if selected.is_some() {
                return Err(bad(format!("clip {} is not a note clip", id.0)));
            }
            continue;
        };
        if let Some(bad_index) = selected
            .as_ref()
            .and_then(|s| s.iter().find(|&&i| i >= notes.len()))
        {
            return Err(range(format!(
                "clip {} has {} notes, not note {bad_index}",
                id.0,
                notes.len()
            )));
        }
        // The notes of a take group's compiled clip are rebuilt from the takes.
        let compiled = doc.song.track(&device).is_some_and(|t| {
            t.lanes[lane]
                .take_groups
                .iter()
                .any(|g| g.clips.contains(&id))
        });
        if compiled {
            continue;
        }
        let out = apply_note_tool(notes, selected.as_ref(), id.0, tool);
        for n in &out {
            check_note(n.tick, n.length, n.key, n.velocity).map_err(range)?;
        }
        if out == **notes {
            continue;
        }
        edits.push(Edit::Clip {
            device,
            lane,
            old: Some(c.clone()),
            new: Some(Clip {
                content: ClipContent::Notes(out.into()),
                ..c.clone()
            }),
        });
    }
    if edits.is_empty() {
        return Err(bad(format!("{} changes no note", tool.label())));
    }
    Ok(Edit::Compound(edits))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn n(tick: i64, key: u8, vel: f32) -> Note {
        Note::new(tick, 100, key, vel)
    }

    fn all() -> Option<&'static BTreeSet<usize>> {
        None
    }

    fn sel(items: &[usize]) -> BTreeSet<usize> {
        items.iter().copied().collect()
    }

    #[test]
    fn set_and_scale_velocity_clamp() {
        let notes = [n(0, 60, 0.5), n(10, 62, 0.9)];
        let out = apply_note_tool(&notes, all(), 1, NoteTool::SetVelocity(0.25));
        assert!(out.iter().all(|x| x.velocity == 0.25));
        let out = apply_note_tool(&notes, all(), 1, NoteTool::ScaleVelocity(2.0));
        assert_eq!((out[0].velocity, out[1].velocity), (1.0, 1.0));
        let out = apply_note_tool(&notes, all(), 1, NoteTool::SetVelocity(0.0));
        assert_eq!(out[0].velocity, MIN_VELOCITY);
    }

    #[test]
    fn only_the_selection_changes() {
        let notes = [n(0, 60, 0.5), n(10, 62, 0.5), n(20, 64, 0.5)];
        let s = sel(&[1]);
        let out = apply_note_tool(&notes, Some(&s), 1, NoteTool::SetVelocity(1.0));
        assert_eq!(out[0].velocity, 0.5);
        assert_eq!(out[1].velocity, 1.0);
        assert_eq!(out[2].velocity, 0.5);
    }

    #[test]
    fn ramp_runs_from_the_first_to_the_last_selected_note() {
        let notes = [
            n(0, 60, 0.2),
            n(100, 60, 0.9),
            n(200, 60, 0.4),
            n(300, 60, 0.9),
        ];
        let s = sel(&[0, 2, 3]);
        let out = apply_note_tool(
            &notes,
            Some(&s),
            1,
            NoteTool::RampVelocity {
                start: None,
                end: None,
            },
        );
        // From 0.2 (tick 0) to 0.9 (tick 300), the unselected note untouched.
        assert!((out[0].velocity - 0.2).abs() < 1e-6);
        assert_eq!(out[1].velocity, 0.9);
        assert!((out[2].velocity - (0.2 + 0.7 * 200.0 / 300.0)).abs() < 1e-5);
        assert!((out[3].velocity - 0.9).abs() < 1e-6);
        let out = apply_note_tool(
            &notes,
            all(),
            1,
            NoteTool::RampVelocity {
                start: Some(1.0),
                end: Some(0.1),
            },
        );
        assert_eq!(out[0].velocity, 1.0);
        assert!((out[3].velocity - 0.1).abs() < 1e-6);
    }

    #[test]
    fn ramp_over_one_tick_sets_the_start() {
        let notes = [n(0, 60, 0.5), n(0, 62, 0.7)];
        let out = apply_note_tool(
            &notes,
            all(),
            1,
            NoteTool::RampVelocity {
                start: Some(0.3),
                end: Some(0.9),
            },
        );
        assert!(out.iter().all(|x| x.velocity == 0.3));
    }

    #[test]
    fn random_tools_repeat_for_a_seed_and_change_with_another() {
        let notes: Vec<Note> = (0..32)
            .map(|i| n(i * 120, 48 + (i % 7) as u8, 0.5))
            .collect();
        for tool in [
            NoteTool::RandomizeVelocity {
                amount: 0.5,
                seed: 3,
            },
            NoteTool::AlterNotes {
                chance: 0.5,
                seed: 3,
            },
            NoteTool::RandomizePitch { range: 5, seed: 3 },
        ] {
            let a = apply_note_tool(&notes, all(), 9, tool);
            let b = apply_note_tool(&notes, all(), 9, tool);
            assert_eq!(a, b, "{tool:?} is not deterministic");
            assert_ne!(a, notes, "{tool:?} changed nothing");
        }
        let a = apply_note_tool(
            &notes,
            all(),
            9,
            NoteTool::RandomizePitch { range: 5, seed: 3 },
        );
        let b = apply_note_tool(
            &notes,
            all(),
            9,
            NoteTool::RandomizePitch { range: 5, seed: 4 },
        );
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_pitch_stays_in_range_and_in_bounds() {
        let notes: Vec<Note> = (0..64)
            .map(|i| n(i * 10, if i % 2 == 0 { 1 } else { 126 }, 0.5))
            .collect();
        let out = apply_note_tool(
            &notes,
            all(),
            2,
            NoteTool::RandomizePitch { range: 4, seed: 1 },
        );
        assert_eq!(out.len(), notes.len());
        for x in &out {
            assert!(x.key <= 127);
            let orig = notes.iter().find(|o| o.tick == x.tick).unwrap();
            assert!(i32::from(x.key).abs_diff(i32::from(orig.key)) <= 4);
        }
    }

    #[test]
    fn pitch_tools_keep_one_note_per_tick_and_key() {
        // Three notes on one tick all sent to one key: only one can have it.
        let notes = [n(0, 60, 0.5), n(0, 62, 0.5), n(0, 64, 0.5), n(50, 60, 0.5)];
        let out = apply_note_tool(&notes, all(), 1, NoteTool::SetPitch(70));
        assert_eq!(out.len(), 4);
        let keys: BTreeSet<(i64, u8)> = out.iter().map(|x| (x.tick, x.key)).collect();
        assert_eq!(keys.len(), 4);
        assert!(
            out.windows(2)
                .all(|w| (w[0].tick, w[0].key) < (w[1].tick, w[1].key))
        );
    }

    #[test]
    fn alter_with_one_pitch_changes_nothing_and_zero_chance_too() {
        let same = [n(0, 60, 0.5), n(10, 60, 0.5)];
        let tool = NoteTool::AlterNotes {
            chance: 1.0,
            seed: 1,
        };
        assert_eq!(apply_note_tool(&same, all(), 1, tool), same.to_vec());
        let mixed = [n(0, 60, 0.5), n(10, 64, 0.5)];
        let tool = NoteTool::AlterNotes {
            chance: 0.0,
            seed: 1,
        };
        assert_eq!(apply_note_tool(&mixed, all(), 1, tool), mixed.to_vec());
    }

    #[test]
    fn alter_only_uses_pitches_of_the_edited_set() {
        let notes: Vec<Note> = (0..40)
            .map(|i| n(i * 10, [60, 64, 67][i as usize % 3], 0.5))
            .collect();
        let tool = NoteTool::AlterNotes {
            chance: 1.0,
            seed: 7,
        };
        let out = apply_note_tool(&notes, all(), 1, tool);
        assert!(out.iter().all(|x| [60, 64, 67].contains(&x.key)));
    }

    #[test]
    fn the_index_follows_every_note_through_a_pitch_change() {
        let notes: Vec<Note> = (0..24)
            .map(|i| n(i * 30, 40 + (i % 9) as u8, 0.5 + (i % 4) as f32 * 0.1))
            .collect();
        let tool = NoteTool::RandomizePitch { range: 9, seed: 2 };
        let (out, index) = apply_note_tool_indexed(&notes, all(), 4, tool);
        assert_eq!(index.len(), notes.len());
        for (i, o) in notes.iter().enumerate() {
            let moved = out[index[i]];
            assert_eq!((moved.tick, moved.velocity), (o.tick, o.velocity));
        }
        let (same, ident) = apply_note_tool_indexed(&notes, all(), 4, NoteTool::SetVelocity(1.0));
        assert_eq!(same.len(), ident.len());
        assert!(ident.iter().enumerate().all(|(i, j)| i == *j));
    }

    #[test]
    fn ranges_are_checked() {
        assert!(NoteTool::SetVelocity(1.5).validate().is_err());
        assert!(NoteTool::ScaleVelocity(9.0).validate().is_err());
        assert!(
            NoteTool::RandomizePitch { range: 60, seed: 0 }
                .validate()
                .is_err()
        );
        assert!(NoteTool::SetPitch(128).validate().is_err());
        assert!(
            NoteTool::RampVelocity {
                start: Some(-0.1),
                end: None
            }
            .validate()
            .is_err()
        );
        assert!(
            NoteTool::AlterNotes {
                chance: 0.5,
                seed: 0
            }
            .validate()
            .is_ok()
        );
    }
}
