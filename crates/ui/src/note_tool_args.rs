//! The arguments of the note tool commands (`editor.note_tool`, `sequencer.note_tool`), their
//! labels for the menus and the parse into [`NoteTool`]. The piano roll applies a tool to its
//! selected notes and the arrangement to the notes of its selected clips; both send one
//! `ApplyNoteTool`.

use crate::seq_edit::MIN_VELOCITY;
use crate::zone_edit::note_name;
use atomik_model::NoteTool;

/// The menu groups, in menu order.
pub const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Set Velocity",
        &[
            ("velocity_set:16", "16"),
            ("velocity_set:32", "32"),
            ("velocity_set:48", "48"),
            ("velocity_set:64", "64"),
            ("velocity_set:80", "80"),
            ("velocity_set:96", "96"),
            ("velocity_set:112", "112"),
            ("velocity_set:127", "127"),
        ],
    ),
    (
        "Scale Velocity",
        &[
            ("velocity_scale:50", "50 %"),
            ("velocity_scale:75", "75 %"),
            ("velocity_scale:90", "90 %"),
            ("velocity_scale:110", "110 %"),
            ("velocity_scale:125", "125 %"),
            ("velocity_scale:150", "150 %"),
        ],
    ),
    (
        "Ramp Velocity",
        &[
            ("velocity_ramp:between", "First to Last Note"),
            ("velocity_ramp:fade_in", "Fade In"),
            ("velocity_ramp:fade_out", "Fade Out"),
        ],
    ),
    (
        "Randomize Velocity",
        &[
            ("velocity_random:10", "10 %"),
            ("velocity_random:25", "25 %"),
            ("velocity_random:50", "50 %"),
            ("velocity_random:100", "100 %"),
        ],
    ),
    (
        "Alter Notes",
        &[
            ("alter:10", "10 % Chance"),
            ("alter:25", "25 % Chance"),
            ("alter:50", "50 % Chance"),
            ("alter:100", "Every Note"),
        ],
    ),
    (
        "Randomize Notes",
        &[
            ("random_pitch:1", "1 Semitone"),
            ("random_pitch:2", "2 Semitones"),
            ("random_pitch:3", "3 Semitones"),
            ("random_pitch:5", "5 Semitones"),
            ("random_pitch:7", "7 Semitones"),
            ("random_pitch:12", "An Octave"),
        ],
    ),
    (
        "Set Pitch",
        &[
            ("pitch:60", "C4"),
            ("pitch:61", "C#4"),
            ("pitch:62", "D4"),
            ("pitch:63", "D#4"),
            ("pitch:64", "E4"),
            ("pitch:65", "F4"),
            ("pitch:66", "F#4"),
            ("pitch:67", "G4"),
            ("pitch:68", "G#4"),
            ("pitch:69", "A4"),
            ("pitch:70", "A#4"),
            ("pitch:71", "B4"),
        ],
    ),
];

/// Every argument, flat, in menu order: the choices of the two commands.
pub const NOTE_TOOL_ARGS: &[&str] = &[
    "velocity_set:16",
    "velocity_set:32",
    "velocity_set:48",
    "velocity_set:64",
    "velocity_set:80",
    "velocity_set:96",
    "velocity_set:112",
    "velocity_set:127",
    "velocity_scale:50",
    "velocity_scale:75",
    "velocity_scale:90",
    "velocity_scale:110",
    "velocity_scale:125",
    "velocity_scale:150",
    "velocity_ramp:between",
    "velocity_ramp:fade_in",
    "velocity_ramp:fade_out",
    "velocity_random:10",
    "velocity_random:25",
    "velocity_random:50",
    "velocity_random:100",
    "alter:10",
    "alter:25",
    "alter:50",
    "alter:100",
    "random_pitch:1",
    "random_pitch:2",
    "random_pitch:3",
    "random_pitch:5",
    "random_pitch:7",
    "random_pitch:12",
    "pitch:60",
    "pitch:61",
    "pitch:62",
    "pitch:63",
    "pitch:64",
    "pitch:65",
    "pitch:66",
    "pitch:67",
    "pitch:68",
    "pitch:69",
    "pitch:70",
    "pitch:71",
];

/// The values of `editor.legato`: how the notes meet. `overlap` and `gap` are a 1/16 note.
pub const LEGATO_ARGS: &[&str] = &["none", "overlap", "gap"];

/// The values of `editor.chop`: the grid the notes are cut at (`snap` is the roll's own step).
pub const CHOP_NOTE_ARGS: &[&str] = &[
    "snap",
    "quarter",
    "eighth",
    "sixteenth",
    "triplet_eighth",
    "triplet_sixteenth",
];

/// The overlap of a legato argument in ticks (`None` for an unknown one).
pub fn legato_overlap(arg: Option<&str>) -> Option<i64> {
    let sixteenth = crate::timeline::PPQ / 4;
    match arg {
        None | Some("none") => Some(0),
        Some("overlap") => Some(sixteenth / 4),
        Some("gap") => Some(-sixteenth / 4),
        Some(_) => None,
    }
}

/// The grid of a chop argument in ticks; `snap` is `step` (`None` for an unknown argument).
pub fn chop_grid(arg: Option<&str>, step: i64) -> Option<i64> {
    let ppq = crate::timeline::PPQ;
    match arg {
        None | Some("snap") => Some(step),
        Some("quarter") => Some(ppq),
        Some("eighth") => Some(ppq / 2),
        Some("sixteenth") => Some(ppq / 4),
        Some("triplet_eighth") => Some(ppq / 3),
        Some("triplet_sixteenth") => Some(ppq / 6),
        Some(_) => None,
    }
}

/// The tool an argument names, with `seed` for the random ones (`None` for an unknown or
/// out-of-range argument).
pub fn parse_note_tool(arg: &str, seed: u32) -> Option<NoteTool> {
    let (kind, value) = arg.split_once(':')?;
    let number = || value.parse::<u32>().ok();
    let tool = match kind {
        "velocity_set" => NoteTool::SetVelocity((number()? as f32 / 127.0).max(MIN_VELOCITY)),
        "velocity_scale" => NoteTool::ScaleVelocity(number()? as f32 / 100.0),
        "velocity_ramp" => match value {
            "between" => NoteTool::RampVelocity {
                start: None,
                end: None,
            },
            "fade_in" => NoteTool::RampVelocity {
                start: Some(0.1),
                end: None,
            },
            "fade_out" => NoteTool::RampVelocity {
                start: None,
                end: Some(0.1),
            },
            _ => return None,
        },
        "velocity_random" => NoteTool::RandomizeVelocity {
            amount: number()? as f32 / 100.0,
            seed,
        },
        "alter" => NoteTool::AlterNotes {
            chance: number()? as f32 / 100.0,
            seed,
        },
        "random_pitch" => NoteTool::RandomizePitch {
            range: u8::try_from(number()?).ok()?,
            seed,
        },
        "pitch" => NoteTool::SetPitch(u8::try_from(number()?).ok()?),
        _ => return None,
    };
    tool.validate().ok()?;
    Some(tool)
}

/// The label of a pitch entry, from the key naming of the editors.
pub fn pitch_label(key: u8) -> String {
    note_name(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_list_is_the_menu_groups_in_order() {
        let flat: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter().map(|(arg, _)| *arg))
            .collect();
        assert_eq!(flat, NOTE_TOOL_ARGS);
    }

    #[test]
    fn every_argument_parses_and_pitch_labels_match_the_key_names() {
        for arg in NOTE_TOOL_ARGS {
            assert!(parse_note_tool(arg, 1).is_some(), "{arg}");
        }
        for (arg, label) in GROUPS.last().unwrap().1 {
            let key: u8 = arg.trim_start_matches("pitch:").parse().unwrap();
            assert_eq!(pitch_label(key), *label);
        }
        assert_eq!(parse_note_tool("nope:1", 0), None);
        assert_eq!(parse_note_tool("pitch:200", 0), None);
        assert_eq!(parse_note_tool("alter:500", 0), None);
        assert_eq!(legato_overlap(Some("x")), None);
        assert_eq!(chop_grid(Some("x"), 5), None);
    }
}
