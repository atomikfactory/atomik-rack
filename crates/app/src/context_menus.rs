//! What the context menus list: per target, the registry commands
//! in order with their enabled flags. Labels and shortcut text are not written here: the menu
//! widget takes the label from the registry and the shortcut from the active keymap, so a key
//! and a menu item cannot differ.

use crate::commands::{ChopStep, FreezeMenu, ScaleStep};
use atomik_model::theory::{ChordKind, DiatonicSize, PITCH_NAMES, ScaleKind};
use atomik_ui::ContextTarget;
use atomik_ui::JackMenu;
use atomik_ui::JackOp;
use atomik_ui::arrangement::{AudioMenu, LaneMenu, OrgMenu};
use atomik_ui::chord_edit::{
    ARP_GATES, ARP_OCTAVES, ArpPattern, STRUM_AMOUNTS, Stamp, StrumDirection, strum_ticks,
};
use atomik_ui::context_menu::{Entry, MenuItemDef, Spec, SubmenuDef};
use atomik_ui::key_rows::Fold;
use atomik_ui::keymap::{ROOT_ARGS, SCALE_ARGS};
use atomik_ui::piano_roll::NoteTools;

/// What decides which items of a context menu can be chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuEnable {
    /// The pane has a selection Cut, Copy, Duplicate and Delete can work on.
    pub selection: bool,
    /// The clipboard holds something the pane can paste.
    pub paste: bool,
    /// Rack: the clipboard holds settings the primary device can take (Paste Settings).
    pub paste_settings: bool,
    /// Rack: the primary device has a song track (Go to Track).
    pub has_track: bool,
    /// Arrangement: two or more clips are selected (Join).
    pub join: bool,
    /// Arrangement: exactly one clip is selected (Rename Clip).
    pub clip_one: bool,
    /// Arrangement: a selected clip loops (Convert Loop).
    pub clip_looped: bool,
    /// Arrangement: exactly one clip is selected and it loops (Make Loop Instance Independent).
    pub loop_instance: bool,
    /// Arrangement: a selected clip is a note clip (Open in the editor).
    pub open_clip: bool,
    /// Arrangement: exactly one clip is selected and it is a note clip (Save Clip to Library).
    pub save_clip: bool,
    /// Arrangement: every selected clip is muted ("Unmute" instead of "Mute").
    pub clips_muted: bool,
    /// Piano roll: every selected note is muted ("Unmute Notes" instead of "Mute Notes").
    pub notes_muted: bool,
    /// Rack: the selection is the Master Section alone: it cannot be cut, copied, duplicated
    /// or deleted.
    pub master_only: bool,
    /// Rack: "Use as Send Effect" can work on the selection (the model would accept it).
    pub assign_send: bool,
    /// Rack: the primary device is a MIDI Out device (Choose MIDI Out Port works).
    pub midi_out: bool,
    /// Rack: Combine can work on the selection (the model would accept it).
    pub combine: bool,
    /// Rack: Uncombine can work on the selection (the model would accept it).
    pub uncombine: bool,
    /// Rack: the rack shows the inside of a container (Move Out of Container is listed).
    pub in_container: bool,
    /// Rack: the selected device of the open container can move out (the model would accept it).
    pub move_out: bool,
    /// Rack: how many columns the rack has (Remove Column needs two, Add Column room).
    pub columns: u16,
    /// Track header: the track has a note lane, so "Chase Notes" can be switched.
    pub track_can_chase: bool,
    /// Track header: the track chases notes (the item is checked).
    pub track_chase: bool,
    /// Track header: what the selected note lane allows (add, rename, delete, mute).
    pub lane: LaneMenu,
    /// Track header and folder row: what the selected tracks and folder allow.
    pub org: OrgMenu,
    /// Clip: what the selected audio clips allow.
    pub audio: AudioMenu,
    /// Track header and clip: what freezing, unfreezing and bouncing allow.
    pub freeze: FreezeMenu,
    /// Tempo point: "Ramp to Next" can be switched (the point is not the last).
    pub tempo_can_ramp: bool,
    /// Tempo point: it ramps to the next point (the item is checked).
    pub tempo_ramped: bool,
    /// Tempo point: it can be deleted (it is not the first).
    pub tempo_can_delete: bool,
    /// Meter change: it can be deleted (it is not the first).
    pub meter_can_delete: bool,
    /// Marker strip: clips are selected, so "Add Range Marker from Selection" works.
    pub marker_selection: bool,
    /// Piano roll: the scale, fold and chord state the check marks of the scale and chord
    /// submenus show.
    pub notes: NoteTools,
    /// Control menu: the control has a MIDI mapping (Forget MIDI works).
    pub control_mapped: bool,
    /// Control menu: the param can be assigned to a macro (nothing drives it and it can be).
    pub control_assign: bool,
    /// Control menu: the control has an automation lane to edit (Edit Automation works).
    pub control_lane: bool,
    /// Control menu: the lane of the control exists (Clear Automation works).
    pub control_has_lane: bool,
    /// Control menu: a macro drives the param (Remove from Macro works).
    pub control_driven: bool,
}

/// The macros of the control menu's Assign to Macro submenu: the command argument and the label.
const MACROS: [(&str, &str); 8] = [
    ("1", "Macro 1"),
    ("2", "Macro 2"),
    ("3", "Macro 3"),
    ("4", "Macro 4"),
    ("5", "Macro 5"),
    ("6", "Macro 6"),
    ("7", "Macro 7"),
    ("8", "Macro 8"),
];

/// The items of the colour part of a marker menu: the command argument and the label.
const MARKER_COLORS: [(&str, &str); 7] = [
    ("red", "Colour: Red"),
    ("orange", "Colour: Orange"),
    ("yellow", "Colour: Yellow"),
    ("green", "Colour: Green"),
    ("blue", "Colour: Blue"),
    ("purple", "Colour: Purple"),
    ("none", "Colour: None"),
];

fn marker_colors() -> impl Iterator<Item = Spec> {
    MARKER_COLORS
        .iter()
        .map(|(arg, label)| meter_item("sequencer.marker_color", arg, label))
}

/// The colours of the clip menu: the command argument and the label.
const CLIP_COLORS: [(&str, &str); 7] = [
    ("track", "Use Track Colour"),
    ("red", "Red"),
    ("orange", "Orange"),
    ("yellow", "Yellow"),
    ("green", "Green"),
    ("blue", "Blue"),
    ("purple", "Purple"),
];

/// The colours of the track and folder menus: the command argument and the label.
const TRACK_COLORS: [(&str, &str); 7] = [
    ("red", "Red"),
    ("orange", "Orange"),
    ("yellow", "Yellow"),
    ("green", "Green"),
    ("blue", "Blue"),
    ("purple", "Purple"),
    ("none", "None (the device's colour)"),
];

/// The height presets of the track and folder menus: the command argument and the label.
const TRACK_HEIGHTS: [(&str, &str); 5] = [
    ("small", "Small"),
    ("medium", "Medium"),
    ("large", "Large"),
    ("fit", "Fit the View"),
    ("minimise", "Minimise / Restore"),
];

/// The fade shapes of the audio clip menu: the command argument and the label.
const FADE_SHAPES: [(&str, &str); 4] = [
    ("linear", "Linear"),
    ("equal_power", "Equal Power"),
    ("exponential", "Exponential"),
    ("s_curve", "S-curve"),
];

/// The crossfade shapes of the audio clip menu: the command argument and the label.
const CROSSFADES: [(&str, &str); 5] = [
    ("off", "Off (hard cut)"),
    ("linear", "Linear"),
    ("equal_power", "Equal Power"),
    ("exponential", "Exponential"),
    ("s_curve", "S-curve"),
];

/// The organisation items shared by the track header menu and the folder menu: the two
/// submenus.
fn track_submenus() -> [Spec; 2] {
    [
        Spec::submenu(
            "Colour",
            TRACK_COLORS
                .iter()
                .map(|(arg, label)| meter_item("sequencer.track_color", arg, label))
                .collect(),
        ),
        Spec::submenu(
            "Height",
            TRACK_HEIGHTS
                .iter()
                .map(|(arg, label)| meter_item("sequencer.track_height", arg, label))
                .collect(),
        ),
    ]
}

/// The meters of the ruler menus: the command argument, the label of the insert item and the
/// label of the item that changes an existing meter.
const METERS: [(&str, &str, &str); 7] = [
    ("2/4", "Insert 2/4 Here", "Change to 2/4"),
    ("3/4", "Insert 3/4 Here", "Change to 3/4"),
    ("4/4", "Insert 4/4 Here", "Change to 4/4"),
    ("5/4", "Insert 5/4 Here", "Change to 5/4"),
    ("6/8", "Insert 6/8 Here", "Change to 6/8"),
    ("7/8", "Insert 7/8 Here", "Change to 7/8"),
    ("12/8", "Insert 12/8 Here", "Change to 12/8"),
];

fn meter_item(command: &'static str, arg: &'static str, label: &'static str) -> Spec {
    Spec::Item {
        command,
        arg: Some(arg),
        label: Some(label),
        enabled: true,
        checked: None,
    }
}

fn item(command: &'static str, on: bool) -> Spec {
    Spec::item(command).enabled(on)
}

/// The note tool submenus of `command` (`editor.note_tool` or `sequencer.note_tool`): a
/// Velocity submenu holding the four velocity tools, then Alter Notes, Randomize Notes and
/// Set Pitch. The labels and arguments are the ones of the ui crate.
fn note_tool_menu(command: &'static str, on: bool) -> Vec<Spec> {
    let group = |name: &'static str, items: &'static [(&'static str, &'static str)]| {
        Spec::submenu(
            name,
            items
                .iter()
                .map(|(arg, label)| meter_item(command, arg, label).enabled(on))
                .collect(),
        )
        .enabled(on)
    };
    let mut groups = atomik_ui::note_tool_args::GROUPS.iter();
    let velocity: Vec<Spec> = groups
        .by_ref()
        .take(4)
        .map(|(name, items)| group(name, items))
        .collect();
    let mut v = vec![Spec::submenu("Velocity", velocity).enabled(on)];
    v.extend(groups.map(|(name, items)| group(name, items)));
    v
}

/// The legato and chop entries of the Notes menu.
fn note_edit_items(on: bool) -> Vec<Spec> {
    let legato = [
        ("none", "Make Legato"),
        ("overlap", "Make Legato with Overlap"),
        ("gap", "Make Legato with Gap"),
    ];
    let chop = [
        ("snap", "At the Snap Step"),
        ("quarter", "1/4 Notes"),
        ("eighth", "1/8 Notes"),
        ("sixteenth", "1/16 Notes"),
        ("triplet_eighth", "1/8 Triplets"),
        ("triplet_sixteenth", "1/16 Triplets"),
    ];
    vec![
        Spec::submenu(
            "Legato",
            legato
                .iter()
                .map(|(arg, label)| meter_item("editor.legato", arg, label))
                .collect(),
        ),
        item("editor.join_notes", on),
        Spec::submenu(
            "Chop Notes",
            chop.iter()
                .map(|(arg, label)| meter_item("editor.chop", arg, label))
                .collect(),
        ),
    ]
}

/// A check item that runs `command` with `arg`.
fn choice(command: &'static str, arg: &'static str, label: &'static str, on: bool) -> Spec {
    Spec::Item {
        command,
        arg: Some(arg),
        label: Some(label),
        enabled: true,
        checked: Some(on),
    }
}

/// The twelve roots of a scale (`editor.set_scale_root`), the current one checked.
fn root_items(t: &NoteTools) -> Vec<Spec> {
    (0..12)
        .map(|pc| {
            choice(
                "editor.set_scale_root",
                ROOT_ARGS[pc],
                PITCH_NAMES[pc],
                t.root == pc as u8,
            )
        })
        .collect()
}

/// The scale kinds (`editor.set_scale`), the current one checked; "Chromatic" is no scale.
fn scale_items(t: &NoteTools) -> Vec<Spec> {
    let current = t.scale.map_or(ScaleKind::Chromatic, |s| s.kind);
    ScaleKind::ALL
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let label = if *kind == ScaleKind::Chromatic {
                "Chromatic (Off)"
            } else {
                kind.label()
            };
            choice("editor.set_scale", SCALE_ARGS[i], label, *kind == current)
        })
        .collect()
}

fn fold_items(t: &NoteTools) -> Vec<Spec> {
    let args = atomik_ui::keymap::FOLD_ARGS;
    Fold::ALL
        .iter()
        .enumerate()
        .map(|(i, f)| choice("editor.fold_set", args[i], f.label(), t.fold == *f))
        .collect()
}

/// The chords of the stamp menu (`editor.set_chord`), the current one checked: none, every
/// chord, then the diatonic chords and the inversions as submenus.
fn chord_items(t: &NoteTools) -> Vec<Spec> {
    let mut v = vec![choice(
        "editor.set_chord",
        Stamp::None.id(),
        "None",
        t.stamp == Stamp::None,
    )];
    v.extend(ChordKind::ALL.iter().map(|c| {
        let s = Stamp::Chord(*c);
        choice("editor.set_chord", s.id(), c.label(), t.stamp == s)
    }));
    v.push(Spec::Separator);
    let has_scale = t.scale.is_some();
    v.push(
        Spec::submenu(
            "Diatonic (from the Scale)",
            DiatonicSize::ALL
                .iter()
                .map(|size| {
                    let s = Stamp::Diatonic(*size);
                    choice("editor.set_chord", s.id(), size.label(), t.stamp == s)
                        .enabled(has_scale)
                })
                .collect(),
        )
        .enabled(has_scale),
    );
    v.push(Spec::submenu("Inversion", inversion_items(t)));
    v
}

fn inversion_items(t: &NoteTools) -> Vec<Spec> {
    const LABELS: [&str; 4] = [
        "Root Position",
        "1st Inversion",
        "2nd Inversion",
        "3rd Inversion",
    ];
    LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            choice(
                "editor.set_chord_inversion",
                atomik_ui::keymap::INVERSION_ARGS[i],
                label,
                usize::from(t.inversion) == i,
            )
        })
        .collect()
}

/// The options of arpeggiate and strum, as submenus.
fn arp_strum_options(t: &NoteTools) -> Vec<Spec> {
    const GATES: [&str; 4] = ["25%", "50%", "75%", "100%"];
    const OCTAVES: [&str; 3] = ["1 Octave", "2 Octaves", "3 Octaves"];
    const AMOUNTS: [&str; 4] = ["1/128 Note", "1/64 Note", "1/32 Note", "1/16 Note"];
    let pattern = ArpPattern::ALL
        .iter()
        .map(|p| {
            choice(
                "editor.set_arp_pattern",
                p.id(),
                p.label(),
                t.arp.pattern == *p,
            )
        })
        .collect();
    let gate = ARP_GATES
        .iter()
        .enumerate()
        .map(|(i, g)| {
            choice(
                "editor.set_arp_gate",
                atomik_ui::keymap::ARP_GATE_ARGS[i],
                GATES[i],
                t.arp.gate_percent == *g,
            )
        })
        .collect();
    let octaves = ARP_OCTAVES
        .iter()
        .enumerate()
        .map(|(i, o)| {
            choice(
                "editor.set_arp_octaves",
                atomik_ui::keymap::ARP_OCTAVE_ARGS[i],
                OCTAVES[i],
                t.arp.octaves == *o,
            )
        })
        .collect();
    let amount = STRUM_AMOUNTS
        .iter()
        .enumerate()
        .map(|(i, a)| {
            choice(
                "editor.set_strum_amount",
                atomik_ui::keymap::STRUM_AMOUNT_ARGS[i],
                AMOUNTS[i],
                t.strum.ticks == strum_ticks(*a),
            )
        })
        .collect();
    let direction = [StrumDirection::Up, StrumDirection::Down]
        .iter()
        .map(|d| {
            choice(
                "editor.set_strum_direction",
                d.id(),
                d.label(),
                t.strum.direction == *d,
            )
        })
        .collect();
    vec![
        Spec::submenu("Arpeggio Pattern", pattern),
        Spec::submenu("Arpeggio Gate", gate),
        Spec::submenu("Arpeggio Octaves", octaves),
        Spec::submenu("Strum Amount", amount),
        Spec::submenu("Strum Direction", direction),
    ]
}

/// The items of the menu opened on `target`.
pub fn specs(target: ContextTarget, e: &MenuEnable) -> Vec<Spec> {
    let edit = |label: Option<(&'static str, &'static str)>| {
        let on = e.selection && !e.master_only;
        let mut v = vec![
            item("edit.cut", on),
            item("edit.copy", on),
            item("edit.paste", e.paste),
            item("edit.duplicate", on),
            item("edit.delete", on),
        ];
        if let Some((dup, del)) = label {
            v[3] = v[3].clone().labelled(dup);
            v[4] = v[4].clone().labelled(del);
        }
        v
    };
    match target {
        ContextTarget::Device => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("create.instrument", true),
                item("create.effect", true),
                item("create.player", true),
                item("rack.auto_route", e.selection),
                item("rack.assign_send", e.assign_send),
                item("rack.disconnect", e.selection),
                item("rack.go_to_track", e.has_track),
                item("rack.save_to_library", e.selection && !e.master_only),
                Spec::Separator,
                item("rack.combine", e.combine),
                item("rack.uncombine", e.uncombine),
            ]);
            if e.in_container {
                v.push(item("rack.move_out", e.move_out));
            }
            v.extend([
                Spec::Separator,
                item("edit.copy_settings", e.selection && !e.master_only),
                item("edit.paste_settings", e.paste_settings),
                item("rack.reset_device", e.selection && !e.master_only),
                item("rack.save_patch", e.selection && !e.master_only),
                item("rack.browse_patch", e.selection && !e.master_only),
                item("rack.patch_prev", e.selection && !e.master_only),
                item("rack.patch_next", e.selection && !e.master_only),
                Spec::Separator,
                item("rack.midi_out_port", e.midi_out),
            ]);
            v
        }
        ContextTarget::Rack => vec![
            item("edit.paste", e.paste),
            Spec::Separator,
            item("create.instrument", true),
            item("create.effect", true),
            item("create.player", true),
            Spec::Separator,
            item(
                "rack.add_column",
                e.columns < atomik_model::MAX_RACK_COLUMNS,
            ),
            item("rack.remove_column", e.columns > 1),
            Spec::Separator,
            item("edit.select_all", true),
        ],
        ContextTarget::TrackHeader => {
            let mut v = edit(Some(("Duplicate Track", "Delete Track")));
            v.extend([
                Spec::Separator,
                Spec::Item {
                    command: "sequencer.toggle_chase",
                    arg: None,
                    label: None,
                    enabled: e.track_can_chase,
                    checked: Some(e.track_chase),
                },
                Spec::Separator,
                item("sequencer.add_note_lane", e.lane.can_add),
                item("sequencer.rename_note_lane", e.lane.can_rename),
                item("sequencer.delete_note_lane", e.lane.can_delete),
                Spec::Item {
                    command: "sequencer.toggle_lane_mute",
                    arg: None,
                    label: None,
                    enabled: e.lane.can_rename,
                    checked: Some(e.lane.muted),
                },
                Spec::Separator,
                item("sequencer.delete_automation_lane", e.lane.automation),
                Spec::Item {
                    command: "sequencer.mute_automation_lane",
                    arg: None,
                    label: None,
                    enabled: e.lane.automation,
                    checked: Some(e.lane.automation_muted),
                },
                item("sequencer.set_lane_static", e.lane.automation),
                item("sequencer.clear_lane_static", e.lane.automation_static),
                Spec::Separator,
                item("sequencer.freeze_track", e.freeze.can_freeze),
                item("sequencer.unfreeze_track", e.freeze.can_unfreeze),
                Spec::Separator,
                item("sequencer.group_tracks", e.org.can_group),
                item("sequencer.ungroup_tracks", e.org.in_folder),
                item("sequencer.move_track_up", e.org.can_move_up),
                item("sequencer.move_track_down", e.org.can_move_down),
                item("sequencer.rename_track", e.org.track_selected),
                item("sequencer.track_comment", e.org.track_selected),
                item("sequencer.delete_track_and_device", e.org.track_selected),
                Spec::Separator,
            ]);
            v.extend(track_submenus());
            v.push(meter_item(
                "sequencer.fold_lanes",
                "fold",
                "Fold Automation Lanes",
            ));
            v.push(meter_item(
                "sequencer.fold_lanes",
                "unfold",
                "Unfold Automation Lanes",
            ));
            v
        }
        ContextTarget::Folder => {
            let mut v = vec![
                item("sequencer.rename_folder", true),
                item("sequencer.toggle_folder", true).labelled(if e.org.collapsed {
                    "Expand Folder"
                } else {
                    "Collapse Folder"
                }),
                item("sequencer.ungroup_tracks", true),
                Spec::Separator,
                item("sequencer.move_track_up", e.org.can_move_up).labelled("Move Folder Up"),
                item("sequencer.move_track_down", e.org.can_move_down).labelled("Move Folder Down"),
                Spec::Separator,
            ];
            v.extend(track_submenus());
            v
        }
        ContextTarget::Clip => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("sequencer.open_clip", e.open_clip),
                item("sequencer.split_at_playhead", true),
                item("sequencer.join", e.join),
                item("sequencer.rename_clip", e.clip_one),
                Spec::submenu(
                    "Clip Colour",
                    CLIP_COLORS
                        .iter()
                        .map(|(arg, label)| {
                            meter_item("sequencer.clip_color", arg, label).enabled(e.selection)
                        })
                        .collect(),
                ),
                Spec::submenu(
                    "Convert Loop",
                    vec![
                        meter_item("sequencer.convert_loop", "single", "To a Single Clip")
                            .enabled(e.clip_looped),
                        meter_item("sequencer.convert_loop", "separate", "To Separate Clips")
                            .enabled(e.clip_looped),
                    ],
                ),
                item("sequencer.loop_instance", e.loop_instance),
                meter_item("sequencer.reverse_clips", "bars", "Reverse Clips").enabled(e.selection),
                Spec::submenu(
                    "Scale Tempo",
                    ScaleStep::ALL
                        .into_iter()
                        .map(|s| {
                            meter_item("sequencer.scale_tempo", s.arg(), s.label())
                                .enabled(e.selection)
                        })
                        .collect(),
                ),
                Spec::submenu(
                    "Chop Clips To",
                    ChopStep::ALL
                        .into_iter()
                        .map(|s| {
                            meter_item("sequencer.chop_clips", s.arg(), s.label())
                                .enabled(e.selection)
                        })
                        .collect(),
                ),
                Spec::submenu(
                    "Note Tools",
                    note_tool_menu("sequencer.note_tool", e.selection),
                )
                .enabled(e.selection),
                item("sequencer.save_clip_to_library", e.save_clip),
                Spec::Separator,
                item("sequencer.toggle_mute", e.selection).labelled(if e.clips_muted {
                    "Unmute"
                } else {
                    "Mute"
                }),
                item("edit.quantize_options", e.selection),
                item("sequencer.bounce_in_place", e.freeze.can_bounce),
                Spec::Separator,
                item("sequencer.audio_reverse", e.audio.can_edit),
                item("sequencer.audio_normalise", e.audio.can_edit),
                item("sequencer.audio_strip_silence", e.audio.can_edit),
                item("sequencer.audio_quantize", e.audio.can_warp),
                item("sequencer.audio_warp", e.audio.can_warp),
                item("sequencer.audio_warp_tempo", e.audio.can_warp),
                item("sequencer.audio_warp_panel", e.audio.can_edit),
                item("sequencer.audio_to_notes", e.audio.can_edit),
                meter_item("sequencer.audio_transpose", "up", "Transpose Up a Semitone")
                    .enabled(e.audio.can_warp),
                meter_item(
                    "sequencer.audio_transpose",
                    "down",
                    "Transpose Down a Semitone",
                )
                .enabled(e.audio.can_warp),
                Spec::submenu(
                    "Audio Fade Shape",
                    FADE_SHAPES
                        .iter()
                        .map(|(arg, label)| {
                            meter_item("sequencer.audio_fade_shape", arg, label)
                                .enabled(e.audio.can_edit)
                        })
                        .collect(),
                ),
                Spec::submenu(
                    "Audio Crossfade",
                    CROSSFADES
                        .iter()
                        .map(|(arg, label)| {
                            meter_item("sequencer.audio_crossfade", arg, label)
                                .enabled(e.audio.can_edit)
                        })
                        .collect(),
                ),
            ]);
            v
        }
        ContextTarget::Lane => vec![
            item("edit.paste", e.paste),
            Spec::Separator,
            item("edit.select_all", true),
        ],
        ContextTarget::TempoPoint => vec![
            Spec::Item {
                command: "sequencer.tempo_toggle_ramp",
                arg: None,
                label: None,
                enabled: e.tempo_can_ramp,
                checked: Some(e.tempo_ramped),
            },
            item("sequencer.tempo_set_value", true),
            Spec::Separator,
            item("sequencer.tempo_delete_point", e.tempo_can_delete),
        ],
        // The transport bar's overflow button: the controls that did not fit, as commands.
        ContextTarget::TransportMore => {
            let go = |arg: &'static str, label: &'static str| Spec::Item {
                command: "transport.go_locator",
                arg: Some(arg),
                label: Some(label),
                enabled: true,
                checked: None,
            };
            let quantize = |arg: &'static str, label: &'static str| Spec::Item {
                command: "transport.input_quantize",
                arg: Some(arg),
                label: Some(label),
                enabled: true,
                checked: None,
            };
            vec![
                item("transport.tap_tempo", true),
                item("transport.rewind_bar", true),
                item("transport.forward_bar", true),
                Spec::Separator,
                go("left", "Go to Left Locator"),
                go("right", "Go to Right Locator"),
                Spec::Separator,
                quantize("off", "Input Quantize: Off"),
                quantize("eighth", "Input Quantize: 1/8"),
                quantize("sixteenth", "Input Quantize: 1/16"),
            ]
        }
        ContextTarget::TempoLane => vec![item("sequencer.tempo_add_point", true)],
        ContextTarget::RulerMeter => {
            let mut v = vec![
                item("sequencer.locator_left_here", true),
                item("sequencer.locator_right_here", true),
                item("sequencer.marker_end_here", true),
                Spec::Separator,
            ];
            v.extend(
                METERS
                    .iter()
                    .map(|(arg, insert, _)| meter_item("sequencer.meter_insert", arg, insert)),
            );
            v
        }
        ContextTarget::MeterChange => {
            let mut v: Vec<Spec> = METERS
                .iter()
                .map(|(arg, _, set)| meter_item("sequencer.meter_set", arg, set))
                .collect();
            v.extend([
                Spec::Separator,
                item("sequencer.meter_delete", e.meter_can_delete),
            ]);
            v
        }
        ContextTarget::Control => {
            let mut macros: Vec<Spec> = MACROS
                .iter()
                .map(|(arg, label)| meter_item("macro.assign_control", arg, label))
                .collect();
            macros.push(Spec::Separator);
            macros.push(Spec::Item {
                command: "macro.assign_control_new",
                arg: Some("1"),
                label: None,
                enabled: true,
                checked: None,
            });
            vec![
                item("automation.edit_control", e.control_lane),
                item("automation.clear_control", e.control_has_lane),
                item("automation.static_control", e.control_lane),
                Spec::Separator,
                item("midi.learn_control", true),
                item("midi.forget_control", e.control_mapped),
                Spec::Separator,
                Spec::submenu("Assign to Macro", macros).enabled(e.control_assign),
                item("macro.remove_control", e.control_driven),
            ]
        }
        ContextTarget::Marker => {
            let mut v = vec![item("sequencer.marker_rename", true), Spec::Separator];
            v.extend(marker_colors());
            v.extend([Spec::Separator, item("sequencer.marker_delete", true)]);
            v
        }
        ContextTarget::MarkerRange => {
            let mut v = vec![
                item("sequencer.marker_loop_range", true),
                item("sequencer.marker_select_clips", true),
                Spec::Separator,
                item("sequencer.marker_rename", true),
                Spec::Separator,
            ];
            v.extend(marker_colors());
            v.extend([Spec::Separator, item("sequencer.marker_delete", true)]);
            v
        }
        ContextTarget::MarkerStrip => vec![
            item("sequencer.marker_add_here", true),
            item("transport.add_range_from_loop", true),
            item("sequencer.marker_range_from_selection", e.marker_selection),
        ],
        // The fields of the piano roll's scale bar open the same lists as the submenus.
        ContextTarget::PianoScaleRoot => root_items(&e.notes),
        ContextTarget::PianoScaleKind => scale_items(&e.notes),
        ContextTarget::PianoChord => chord_items(&e.notes),
        ContextTarget::Notes => {
            let t = &e.notes;
            let has_scale = t.scale.is_some();
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("editor.quantize", e.selection),
                item("edit.quantize_options", true),
                item("editor.toggle_note_mute", e.selection).labelled(if e.notes_muted {
                    "Unmute Notes"
                } else {
                    "Mute Notes"
                }),
                item("edit.select_all", true),
                item("editor.select_same_pitch", e.selection),
                Spec::Separator,
            ]);
            v.extend(note_edit_items(e.selection));
            v.extend(note_tool_menu("editor.note_tool", true));
            v.extend([
                Spec::Separator,
                Spec::submenu(
                    "Scale",
                    vec![
                        Spec::submenu("Root", root_items(t)),
                        Spec::submenu("Scale Type", scale_items(t)),
                        item("editor.detect_scale", true),
                        Spec::Separator,
                        Spec::Item {
                            command: "editor.scale_snap_toggle",
                            arg: None,
                            label: None,
                            enabled: true,
                            checked: Some(t.snap_scale),
                        },
                        Spec::submenu("Fold", fold_items(t)),
                        item("editor.conform_to_scale", e.selection && has_scale),
                    ],
                ),
                Spec::submenu("Chord Stamp", chord_items(t)),
                {
                    let mut chords = vec![
                        item("editor.chordify", e.selection && t.stamp.is_active(t.scale)),
                        item("editor.arpeggiate", e.selection),
                        item("editor.strum", e.selection),
                        Spec::Separator,
                    ];
                    chords.extend(arp_strum_options(t));
                    Spec::submenu("Chords", chords)
                },
            ]);
            v
        }
    }
}

/// The registry command every entry of the jack menu runs, with the entry as its argument.
const JACK_COMMAND: &str = "rack.jack";

/// The rows of the pop-up menu of a jack: where its cable can go (a submenu per device, in rack
/// order, an occupied input starred because choosing it replaces its cable), Disconnect, Scroll
/// to Connected Device, and New Mixer Channel on an audio output. Entries that cannot be chosen
/// are greyed. The labels are the document's own names, so the rows are built whole instead of
/// from registry commands.
pub fn jack_entries(menu: &JackMenu) -> Vec<Entry> {
    let item = |label: String, op: &JackOp, enabled: bool| {
        Entry::Item(MenuItemDef {
            command: JACK_COMMAND.to_string(),
            arg: Some(op.encode()),
            label,
            shortcut: None,
            enabled,
            checked: None,
        })
    };
    let submenu = |label: &str, entries: Vec<Entry>| {
        Entry::Submenu(SubmenuDef {
            label: label.to_string(),
            enabled: !entries.is_empty(),
            entries,
        })
    };
    let mut rows = vec![submenu(
        "Connect to",
        menu.groups
            .iter()
            .map(|g| {
                submenu(
                    &g.device,
                    g.jacks
                        .iter()
                        .map(|j| {
                            let label = if j.occupied {
                                format!("{} *", j.label)
                            } else {
                                j.label.clone()
                            };
                            item(label, &j.op, true)
                        })
                        .collect(),
                )
            })
            .collect(),
    )];
    rows.push(Entry::Separator);
    rows.push(item(
        "Disconnect".to_string(),
        &JackOp::Disconnect(menu.jack.clone()),
        !menu.peers.is_empty(),
    ));
    rows.push(match menu.peers.as_slice() {
        [one] => item(
            "Scroll to Connected Device".to_string(),
            &JackOp::Reveal(one.device.clone()),
            true,
        ),
        many => submenu(
            "Scroll to Connected Device",
            many.iter()
                .map(|p| item(p.name.clone(), &JackOp::Reveal(p.device.clone()), true))
                .collect(),
        ),
    });
    if menu.audio_output {
        rows.push(Entry::Separator);
        let op = menu
            .channel
            .clone()
            .unwrap_or_else(|| JackOp::Disconnect(menu.jack.clone()));
        rows.push(item(
            "New Mixer Channel".to_string(),
            &op,
            menu.channel.is_some(),
        ));
    }
    rows
}

/// The rows of the pop-up that chooses a MIDI Out device's port: "Default (settings)" first,
/// then one check item per output port the system lists (the chosen one checked), then the chosen
/// port itself when it is not plugged in (marked). Each runs `rack.midi_out_pick`; the argument
/// is the port name, empty for the default.
pub fn midi_out_entries(ports: &[String], current: Option<&str>) -> Vec<Entry> {
    let item = |label: String, arg: &str, on: bool| {
        Entry::Item(MenuItemDef {
            command: "rack.midi_out_pick".to_string(),
            arg: Some(arg.to_string()),
            label,
            shortcut: None,
            enabled: true,
            checked: Some(on),
        })
    };
    let mut rows = vec![item(
        "Default (port of the settings)".to_string(),
        "",
        current.is_none(),
    )];
    rows.extend(
        ports
            .iter()
            .map(|p| item(p.clone(), p, current == Some(p.as_str()))),
    );
    if let Some(missing) = current.filter(|c| !ports.iter().any(|p| p == c)) {
        rows.push(item(format!("{missing} (not found)"), missing, true));
    }
    rows
}

/// The rows of the pop-up on a patch name: one check item per patch (the current one checked)
/// that runs `rack.patch_pick` with the patch file as its argument.
pub fn patch_entries(items: &[(String, std::path::PathBuf)], current: Option<usize>) -> Vec<Entry> {
    items
        .iter()
        .enumerate()
        .map(|(i, (name, path))| {
            Entry::Item(MenuItemDef {
                command: "rack.patch_pick".to_string(),
                arg: Some(path.to_string_lossy().into_owned()),
                label: name.clone(),
                shortcut: None,
                enabled: true,
                checked: Some(current == Some(i)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::theory::Scale;

    fn jack(label: &str, from: &str, to: &str, occupied: bool) -> atomik_ui::JackChoice {
        let cable = atomik_model::Cable {
            from: sock("a", from),
            to: sock("b", to),
        };
        atomik_ui::JackChoice {
            label: label.into(),
            occupied,
            op: if occupied {
                JackOp::Replace(cable)
            } else {
                JackOp::Connect(cable)
            },
        }
    }

    fn sock(d: &str, s: &str) -> atomik_model::SocketRef {
        atomik_model::SocketRef {
            device: atomik_model::DeviceKey(d.into()),
            socket: s.into(),
        }
    }

    fn jack_menu(groups: Vec<atomik_ui::JackGroup>, peers: usize, channel: bool) -> JackMenu {
        JackMenu {
            jack: sock("a", "out_l"),
            groups,
            peers: (0..peers)
                .map(|i| atomik_ui::JackPeer {
                    device: atomik_model::DeviceKey(format!("p{i}")),
                    name: format!("Peer {i}"),
                })
                .collect(),
            audio_output: true,
            channel: channel.then(|| JackOp::NewChannel {
                device: atomik_model::DeviceKey("a".into()),
                out_l: "out_l".into(),
                out_r: "out_r".into(),
            }),
        }
    }

    fn row<'a>(rows: &'a [Entry], label: &str) -> &'a Entry {
        rows.iter()
            .find(|e| match e {
                Entry::Item(i) => i.label == label,
                Entry::Submenu(s) => s.label == label,
                Entry::Separator => false,
            })
            .unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn the_jack_menu_lists_devices_and_stars_the_occupied_inputs() {
        let groups = vec![atomik_ui::JackGroup {
            device: "Delay".into(),
            jacks: vec![
                jack("In L", "out_l", "in_l", false),
                jack("In R", "out_l", "in_r", true),
            ],
        }];
        let rows = jack_entries(&jack_menu(groups, 1, true));
        let Entry::Submenu(connect) = row(&rows, "Connect to") else {
            panic!("a submenu");
        };
        assert!(connect.enabled);
        let Entry::Submenu(delay) = row(&connect.entries, "Delay") else {
            panic!("a submenu per device");
        };
        let Entry::Item(free) = row(&delay.entries, "In L") else {
            panic!("an item");
        };
        let Entry::Item(taken) = row(&delay.entries, "In R *") else {
            panic!("the occupied jack is starred");
        };
        assert!(free.enabled && taken.enabled);
        assert_eq!(free.command, "rack.jack");
        assert!(matches!(
            JackOp::parse(taken.arg.as_deref().unwrap()),
            Some(JackOp::Replace(_))
        ));
        assert!(matches!(
            JackOp::parse(free.arg.as_deref().unwrap()),
            Some(JackOp::Connect(_))
        ));
    }

    #[test]
    fn the_jack_menu_greys_what_cannot_be_done() {
        // Nothing compatible, nothing cabled, a pair that is not free.
        let rows = jack_entries(&jack_menu(Vec::new(), 0, false));
        assert!(matches!(row(&rows, "Connect to"), Entry::Submenu(s) if !s.enabled));
        assert!(matches!(row(&rows, "Disconnect"), Entry::Item(i) if !i.enabled));
        assert!(
            matches!(row(&rows, "Scroll to Connected Device"), Entry::Submenu(s) if !s.enabled)
        );
        assert!(matches!(row(&rows, "New Mixer Channel"), Entry::Item(i) if !i.enabled));
        // The same jack, cabled and free: all of it is on.
        let rows = jack_entries(&jack_menu(Vec::new(), 1, true));
        assert!(matches!(row(&rows, "Disconnect"), Entry::Item(i) if i.enabled));
        assert!(matches!(row(&rows, "Scroll to Connected Device"), Entry::Item(i) if i.enabled));
        assert!(matches!(row(&rows, "New Mixer Channel"), Entry::Item(i) if i.enabled));
        // Two connected devices make a submenu of their names; an input has no channel row.
        let rows = jack_entries(&jack_menu(Vec::new(), 2, true));
        let Entry::Submenu(peers) = row(&rows, "Scroll to Connected Device") else {
            panic!("a submenu");
        };
        assert_eq!(peers.entries.len(), 2);
        let mut m = jack_menu(Vec::new(), 0, false);
        m.audio_output = false;
        assert!(
            !jack_entries(&m)
                .iter()
                .any(|e| matches!(e, Entry::Item(i) if i.label == "New Mixer Channel"))
        );
    }

    fn ids(target: ContextTarget, e: &MenuEnable) -> Vec<(&'static str, bool)> {
        specs(target, e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item {
                    command, enabled, ..
                } => Some((command, enabled)),
                Spec::Separator | Spec::Submenu { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_device_menu_lists_the_commands_of_fr_229_in_order() {
        let e = MenuEnable {
            selection: true,
            paste: true,
            has_track: true,
            ..MenuEnable::default()
        };
        let got: Vec<&str> = ids(ContextTarget::Device, &e)
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            got,
            [
                "edit.cut",
                "edit.copy",
                "edit.paste",
                "edit.duplicate",
                "edit.delete",
                "create.instrument",
                "create.effect",
                "create.player",
                "rack.auto_route",
                "rack.assign_send",
                "rack.disconnect",
                "rack.go_to_track",
                "rack.save_to_library",
                "rack.combine",
                "rack.uncombine",
                "edit.copy_settings",
                "edit.paste_settings",
                "rack.reset_device",
                "rack.save_patch",
                "rack.browse_patch",
                "rack.patch_prev",
                "rack.patch_next",
                "rack.midi_out_port",
            ]
        );
    }

    #[test]
    fn choosing_the_midi_out_port_needs_a_midi_out_device() {
        let enabled = |e: &MenuEnable| {
            ids(ContextTarget::Device, e)
                .into_iter()
                .find(|(c, _)| *c == "rack.midi_out_port")
                .unwrap()
                .1
        };
        let some = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        assert!(!enabled(&some));
        assert!(enabled(&MenuEnable {
            midi_out: true,
            ..some
        }));
    }

    #[test]
    fn the_midi_out_pop_up_lists_the_default_the_ports_and_a_missing_choice() {
        let ports = vec!["Example Synth".to_string(), "Example Hub 1".to_string()];
        let rows = |current: Option<&str>| -> Vec<(String, String, Option<bool>)> {
            midi_out_entries(&ports, current)
                .into_iter()
                .map(|e| match e {
                    Entry::Item(i) => {
                        assert_eq!(i.command, "rack.midi_out_pick");
                        (i.label, i.arg.unwrap(), i.checked)
                    }
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        let none = rows(None);
        assert_eq!(none.len(), 3);
        assert_eq!(
            none[0],
            (
                "Default (port of the settings)".into(),
                String::new(),
                Some(true)
            )
        );
        assert_eq!(
            none[1],
            ("Example Synth".into(), "Example Synth".into(), Some(false))
        );
        let chosen = rows(Some("Example Hub 1"));
        assert_eq!(chosen[0].2, Some(false));
        assert_eq!(chosen[2].2, Some(true));
        let missing = rows(Some("Old Synth"));
        assert_eq!(missing.len(), 4);
        assert_eq!(
            missing[3],
            (
                "Old Synth (not found)".into(),
                "Old Synth".into(),
                Some(true)
            )
        );
    }

    #[test]
    fn save_to_library_needs_a_selection_that_is_not_the_master_alone() {
        let on = |e: &MenuEnable| {
            ids(ContextTarget::Device, e)
                .into_iter()
                .find(|(c, _)| *c == "rack.save_to_library")
                .unwrap()
                .1
        };
        assert!(!on(&MenuEnable::default()), "nothing selected");
        let some = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        assert!(on(&some));
        let master = MenuEnable {
            master_only: true,
            ..some
        };
        assert!(!on(&master), "the Master Section cannot be saved");
    }

    #[test]
    fn enabling_follows_the_selection_the_clipboard_and_the_device() {
        let none = MenuEnable::default();
        let list = ids(ContextTarget::Device, &none);
        let on = |id: &str| list.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(!on("edit.cut") && !on("edit.copy") && !on("edit.paste"));
        assert!(!on("rack.disconnect") && !on("rack.go_to_track"));
        assert!(on("create.instrument") && on("create.effect"));
        assert!(!on("rack.auto_route"), "nothing selected, nothing to route");
        let some = MenuEnable {
            selection: true,
            paste: true,
            has_track: true,
            ..MenuEnable::default()
        };
        let list = ids(ContextTarget::Device, &some);
        let on = |id: &str| list.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(
            on("edit.cut")
                && on("edit.paste")
                && on("rack.disconnect")
                && on("rack.auto_route")
                && on("rack.go_to_track")
        );
    }

    #[test]
    fn the_rack_background_menu_offers_paste_create_columns_and_select_all() {
        let got: Vec<&str> = ids(ContextTarget::Rack, &MenuEnable::default())
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            got,
            [
                "edit.paste",
                "create.instrument",
                "create.effect",
                "create.player",
                "rack.add_column",
                "rack.remove_column",
                "edit.select_all"
            ]
        );
    }

    #[test]
    fn remove_column_needs_two_columns_and_add_column_needs_room() {
        let on = |columns: u16, id: &str| {
            ids(
                ContextTarget::Rack,
                &MenuEnable {
                    columns,
                    ..Default::default()
                },
            )
            .into_iter()
            .find(|(c, _)| *c == id)
            .map(|(_, on)| on)
            .unwrap()
        };
        assert!(!on(1, "rack.remove_column") && on(1, "rack.add_column"));
        assert!(on(2, "rack.remove_column"));
        assert!(!on(atomik_model::MAX_RACK_COLUMNS, "rack.add_column"));
    }

    #[test]
    fn the_track_header_menu_says_track() {
        let e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let labels: Vec<Option<&str>> = specs(ContextTarget::TrackHeader, &e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item { label, .. } => Some(label),
                Spec::Separator | Spec::Submenu { .. } => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                None,
                None,
                None,
                Some("Duplicate Track"),
                Some("Delete Track"),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("Fold Automation Lanes"),
                Some("Unfold Automation Lanes")
            ]
        );
    }

    #[test]
    fn the_track_header_menu_has_a_checked_chase_item_for_note_tracks_only() {
        let chase = |e: &MenuEnable| {
            specs(ContextTarget::TrackHeader, e)
                .into_iter()
                .find_map(|s| match s {
                    Spec::Item {
                        command: "sequencer.toggle_chase",
                        enabled,
                        checked,
                        ..
                    } => Some((enabled, checked)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(chase(&MenuEnable::default()), (false, Some(false)));
        let note_track = MenuEnable {
            track_can_chase: true,
            track_chase: true,
            ..MenuEnable::default()
        };
        assert_eq!(chase(&note_track), (true, Some(true)));
    }

    #[test]
    fn the_track_header_menu_lists_the_lane_items_after_chase_notes() {
        let items = |e: &MenuEnable| -> Vec<(&'static str, bool, Option<bool>)> {
            specs(ContextTarget::TrackHeader, e)
                .into_iter()
                .filter_map(|s| match s {
                    Spec::Item {
                        command,
                        enabled,
                        checked,
                        ..
                    } if command.ends_with("_note_lane")
                        || command == "sequencer.toggle_lane_mute"
                        || command == "sequencer.toggle_chase" =>
                    {
                        Some((command, enabled, checked))
                    }
                    _ => None,
                })
                .collect()
        };
        let none = items(&MenuEnable::default());
        let ids: Vec<_> = none.iter().map(|i| i.0).collect();
        assert_eq!(
            ids,
            [
                "sequencer.toggle_chase",
                "sequencer.add_note_lane",
                "sequencer.rename_note_lane",
                "sequencer.delete_note_lane",
                "sequencer.toggle_lane_mute"
            ]
        );
        assert!(none[1..].iter().all(|i| !i.1), "nothing selected: all off");
        let several = MenuEnable {
            lane: LaneMenu {
                can_add: true,
                can_rename: true,
                can_delete: true,
                muted: true,
                ..LaneMenu::default()
            },
            ..MenuEnable::default()
        };
        let list = items(&several);
        assert!(list[1..].iter().all(|i| i.1));
        assert_eq!(list[4].2, Some(true), "Mute Lane is checked when muted");
        let one = MenuEnable {
            lane: LaneMenu {
                can_add: true,
                can_rename: true,
                ..LaneMenu::default()
            },
            ..MenuEnable::default()
        };
        assert!(!items(&one)[3].1, "Delete needs a second lane");
    }

    #[test]
    fn the_clip_menu_enables_join_and_open_by_selection() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let list = ids(ContextTarget::Clip, &e);
        let on = |l: &[(&str, bool)], id: &str| l.iter().find(|(c, _)| *c == id).unwrap().1;
        assert!(!on(&list, "sequencer.join") && !on(&list, "sequencer.open_clip"));
        assert!(on(&list, "sequencer.split_at_playhead"));
        e.join = true;
        e.open_clip = true;
        let list = ids(ContextTarget::Clip, &e);
        assert!(on(&list, "sequencer.join") && on(&list, "sequencer.open_clip"));
    }

    #[test]
    fn the_clip_menu_enables_save_to_library_for_one_note_clip() {
        let on = |e: &MenuEnable| {
            ids(ContextTarget::Clip, e)
                .into_iter()
                .find(|(c, _)| *c == "sequencer.save_clip_to_library")
                .unwrap()
                .1
        };
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        assert!(!on(&e), "not exactly one note clip");
        e.save_clip = true;
        assert!(on(&e));
    }

    fn labels(target: ContextTarget, e: &MenuEnable) -> Vec<(&'static str, Option<&'static str>)> {
        specs(target, e)
            .into_iter()
            .filter_map(|s| match s {
                Spec::Item { command, label, .. } => Some((command, label)),
                Spec::Separator | Spec::Submenu { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_clip_menu_labels_the_mute_item_by_the_toggle_result() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let find = |e: &MenuEnable| {
            labels(ContextTarget::Clip, e)
                .into_iter()
                .find(|(c, _)| *c == "sequencer.toggle_mute")
                .unwrap()
                .1
        };
        assert_eq!(find(&e), Some("Mute"));
        e.clips_muted = true;
        assert_eq!(find(&e), Some("Unmute"));
        assert!(
            ids(ContextTarget::Clip, &MenuEnable::default())
                .contains(&("sequencer.toggle_mute", false)),
            "disabled without a selection"
        );
    }

    #[test]
    fn the_notes_menu_labels_the_mute_item_by_the_toggle_result() {
        let mut e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let find = |e: &MenuEnable| {
            labels(ContextTarget::Notes, e)
                .into_iter()
                .find(|(c, _)| *c == "editor.toggle_note_mute")
                .unwrap()
                .1
        };
        assert_eq!(find(&e), Some("Mute Notes"));
        e.notes_muted = true;
        assert_eq!(find(&e), Some("Unmute Notes"));
    }

    /// What `walk` reports for an item: command, argument, enabled, checked.
    type Walked = (&'static str, Option<&'static str>, bool, Option<bool>);

    /// Every item of `specs`, submenus included.
    fn walk(specs: &[Spec], out: &mut Vec<Walked>) {
        for s in specs {
            match s {
                Spec::Item {
                    command,
                    arg,
                    enabled,
                    checked,
                    ..
                } => out.push((*command, *arg, *enabled, *checked)),
                Spec::Submenu { items, .. } => walk(items, out),
                Spec::Separator => {}
            }
        }
    }

    fn walked(target: ContextTarget, e: &MenuEnable) -> Vec<Walked> {
        let mut all = Vec::new();
        walk(&specs(target, e), &mut all);
        all
    }

    fn tools(scale: Option<Scale>) -> NoteTools {
        NoteTools {
            scale,
            root: scale.map_or(0, |s| s.root),
            ..NoteTools::default()
        }
    }

    fn submenu_labels(specs: &[Spec]) -> Vec<&'static str> {
        specs
            .iter()
            .filter_map(|s| match s {
                Spec::Submenu { label, .. } => Some(*label),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_notes_menu_has_the_scale_chord_stamp_and_chords_submenus() {
        let e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        assert_eq!(
            submenu_labels(&specs(ContextTarget::Notes, &e)),
            [
                "Legato",
                "Chop Notes",
                "Velocity",
                "Alter Notes",
                "Randomize Notes",
                "Set Pitch",
                "Scale",
                "Chord Stamp",
                "Chords"
            ]
        );
    }

    #[test]
    fn the_notes_menu_offers_every_note_tool_argument_once() {
        let e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let all = walked(ContextTarget::Notes, &e);
        let args = |cmd: &str| -> Vec<String> {
            all.iter()
                .filter(|(c, ..)| *c == cmd)
                .filter_map(|(_, a, ..)| a.map(str::to_owned))
                .collect()
        };
        assert_eq!(
            args("editor.note_tool"),
            atomik_ui::note_tool_args::NOTE_TOOL_ARGS
        );
        assert_eq!(
            args("editor.legato"),
            atomik_ui::note_tool_args::LEGATO_ARGS
        );
        assert_eq!(
            args("editor.chop"),
            atomik_ui::note_tool_args::CHOP_NOTE_ARGS
        );
        // The clip menu of the arrangement has the same tools for whole clips.
        let clip = walked(ContextTarget::Clip, &e);
        let clip_args: Vec<&str> = clip
            .iter()
            .filter(|(c, ..)| *c == "sequencer.note_tool")
            .filter_map(|(_, a, ..)| *a)
            .collect();
        assert_eq!(clip_args, atomik_ui::note_tool_args::NOTE_TOOL_ARGS);
    }

    #[test]
    fn every_scale_and_chord_item_is_a_registry_command_with_an_argument_it_accepts() {
        let registry = atomik_ui::keymap::Registry::bundled();
        let t = NoteTools {
            snap_scale: true,
            fold: Fold::Scale,
            stamp: Stamp::Diatonic(DiatonicSize::Seventh),
            inversion: 1,
            ..tools(Some(Scale::new(2, ScaleKind::Dorian)))
        };
        let e = MenuEnable {
            selection: true,
            notes: t,
            ..MenuEnable::default()
        };
        for target in [
            ContextTarget::Notes,
            ContextTarget::PianoScaleRoot,
            ContextTarget::PianoScaleKind,
            ContextTarget::PianoChord,
        ] {
            let all = walked(target, &e);
            assert!(!all.is_empty(), "{target:?}");
            for (command, arg, _, _) in all {
                let def = registry
                    .get(command)
                    .unwrap_or_else(|| panic!("{command} is not a command"));
                match (arg, def.args) {
                    (Some(a), Some(kind)) => {
                        assert!(kind.accepts(a), "{command} does not take {a:?}");
                    }
                    (None, None) => {}
                    (a, k) => panic!("{command}: argument {a:?} against {k:?}"),
                }
            }
        }
    }

    #[test]
    fn the_notes_menu_offers_every_scale_chord_and_option() {
        let e = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        let all = walked(ContextTarget::Notes, &e);
        let args = |command: &str| -> Vec<&str> {
            all.iter()
                .filter(|(c, ..)| *c == command)
                .filter_map(|(_, a, ..)| *a)
                .collect()
        };
        assert_eq!(args("editor.set_scale_root").len(), 12);
        assert_eq!(args("editor.set_scale"), atomik_ui::keymap::SCALE_ARGS);
        assert_eq!(
            args("editor.set_chord"),
            atomik_ui::keymap::CHORD_ARGS,
            "none, 21 chords and the diatonic chords"
        );
        assert_eq!(args("editor.set_chord_inversion").len(), 4);
        assert_eq!(args("editor.fold_set"), atomik_ui::keymap::FOLD_ARGS);
        assert_eq!(args("editor.set_arp_pattern").len(), 6);
        assert_eq!(args("editor.set_arp_gate").len(), 4);
        assert_eq!(args("editor.set_arp_octaves").len(), 3);
        assert_eq!(args("editor.set_strum_amount").len(), 4);
        assert_eq!(args("editor.set_strum_direction").len(), 2);
        for c in [
            "editor.detect_scale",
            "editor.scale_snap_toggle",
            "editor.conform_to_scale",
            "editor.chordify",
            "editor.arpeggiate",
            "editor.strum",
        ] {
            assert!(all.iter().any(|(id, ..)| *id == c), "{c}");
        }
    }

    #[test]
    fn conform_needs_a_scale_and_chordify_an_active_stamp_and_both_a_selection() {
        let enabled = |e: &MenuEnable, command: &str| {
            walked(ContextTarget::Notes, e)
                .iter()
                .find(|(c, ..)| *c == command)
                .unwrap()
                .2
        };
        let none = MenuEnable {
            selection: true,
            ..MenuEnable::default()
        };
        assert!(!enabled(&none, "editor.conform_to_scale"));
        assert!(!enabled(&none, "editor.chordify"));
        assert!(enabled(&none, "editor.arpeggiate") && enabled(&none, "editor.strum"));
        let scaled = MenuEnable {
            selection: true,
            notes: NoteTools {
                stamp: Stamp::Chord(ChordKind::Minor),
                ..tools(Some(Scale::new(0, ScaleKind::Major)))
            },
            ..MenuEnable::default()
        };
        assert!(enabled(&scaled, "editor.conform_to_scale"));
        assert!(enabled(&scaled, "editor.chordify"));
        let no_selection = MenuEnable {
            notes: scaled.notes,
            ..MenuEnable::default()
        };
        for c in [
            "editor.conform_to_scale",
            "editor.chordify",
            "editor.arpeggiate",
            "editor.strum",
        ] {
            assert!(!enabled(&no_selection, c), "{c}");
        }
        // A diatonic stamp without a scale builds nothing.
        let diatonic = MenuEnable {
            selection: true,
            notes: NoteTools {
                stamp: Stamp::Diatonic(DiatonicSize::Triad),
                ..NoteTools::default()
            },
            ..MenuEnable::default()
        };
        assert!(!enabled(&diatonic, "editor.chordify"));
    }

    #[test]
    fn the_checks_show_the_current_scale_root_fold_and_stamp() {
        let t = NoteTools {
            stamp: Stamp::Chord(ChordKind::MajorSeventh),
            fold: Fold::Used,
            snap_scale: true,
            ..tools(Some(Scale::new(7, ScaleKind::Blues)))
        };
        let e = MenuEnable {
            notes: t,
            ..MenuEnable::default()
        };
        let checked = |target, command: &str| -> Vec<&str> {
            walked(target, &e)
                .iter()
                .filter(|(c, _, _, ch)| *c == command && *ch == Some(true))
                .filter_map(|(_, a, ..)| *a)
                .collect()
        };
        assert_eq!(
            checked(ContextTarget::PianoScaleRoot, "editor.set_scale_root"),
            ["g"]
        );
        assert_eq!(
            checked(ContextTarget::PianoScaleKind, "editor.set_scale"),
            ["blues"]
        );
        assert_eq!(
            checked(ContextTarget::PianoChord, "editor.set_chord"),
            ["maj7"]
        );
        assert_eq!(checked(ContextTarget::Notes, "editor.fold_set"), ["used"]);
        let snap = walked(ContextTarget::Notes, &e)
            .into_iter()
            .find(|(c, ..)| *c == "editor.scale_snap_toggle")
            .unwrap();
        assert_eq!(snap.3, Some(true));
        // No scale: Chromatic is the checked kind.
        let on: Vec<_> = walked(ContextTarget::PianoScaleKind, &MenuEnable::default())
            .into_iter()
            .filter(|(_, _, _, c)| *c == Some(true))
            .collect();
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].1, Some("chromatic"));
    }

    #[test]
    fn the_organisation_items_follow_the_selection_state() {
        let state = |e: &MenuEnable, target| -> Vec<(&'static str, bool, Option<&'static str>)> {
            specs(target, e)
                .into_iter()
                .filter_map(|s| match s {
                    Spec::Item {
                        command,
                        enabled,
                        label,
                        ..
                    } => Some((command, enabled, label)),
                    _ => None,
                })
                .collect()
        };
        let on = |list: &[(&'static str, bool, Option<&'static str>)], id: &str| {
            list.iter().find(|i| i.0 == id).map(|i| i.1)
        };
        let none = state(&MenuEnable::default(), ContextTarget::TrackHeader);
        assert_eq!(on(&none, "sequencer.group_tracks"), Some(false));
        assert_eq!(on(&none, "sequencer.track_comment"), Some(false));
        assert_eq!(on(&none, "sequencer.rename_track"), Some(false));
        let track = MenuEnable {
            org: OrgMenu {
                can_group: true,
                track_selected: true,
                can_move_up: true,
                ..OrgMenu::default()
            },
            ..MenuEnable::default()
        };
        let list = state(&track, ContextTarget::TrackHeader);
        assert_eq!(on(&list, "sequencer.group_tracks"), Some(true));
        assert_eq!(on(&list, "sequencer.move_track_up"), Some(true));
        assert_eq!(on(&list, "sequencer.move_track_down"), Some(false));
        assert_eq!(on(&list, "sequencer.ungroup_tracks"), Some(false));
        // The colours and heights are submenus whose items carry the argument.
        let walked: Vec<_> = {
            fn walk(items: &[Spec], out: &mut Vec<(&'static str, Option<&'static str>)>) {
                for s in items {
                    match s {
                        Spec::Item { command, arg, .. } => out.push((command, *arg)),
                        Spec::Submenu { items, .. } => walk(items, out),
                        Spec::Separator => {}
                    }
                }
            }
            let mut out = Vec::new();
            walk(&specs(ContextTarget::TrackHeader, &track), &mut out);
            out
        };
        let args = |id: &str| -> Vec<&str> {
            walked
                .iter()
                .filter(|(c, _)| *c == id)
                .filter_map(|(_, a)| *a)
                .collect()
        };
        assert_eq!(
            args("sequencer.track_color"),
            ["red", "orange", "yellow", "green", "blue", "purple", "none"]
        );
        assert_eq!(
            args("sequencer.track_height"),
            ["small", "medium", "large", "fit", "minimise"]
        );
        assert_eq!(args("sequencer.fold_lanes"), ["fold", "unfold"]);
        // The folder row has its own list, and the toggle names what it will do.
        let folder = MenuEnable {
            org: OrgMenu {
                in_folder: true,
                collapsed: true,
                folder_selected: true,
                can_move_down: true,
                ..OrgMenu::default()
            },
            ..MenuEnable::default()
        };
        let list = state(&folder, ContextTarget::Folder);
        let ids: Vec<&str> = list.iter().map(|i| i.0).collect();
        assert_eq!(
            ids,
            [
                "sequencer.rename_folder",
                "sequencer.toggle_folder",
                "sequencer.ungroup_tracks",
                "sequencer.move_track_up",
                "sequencer.move_track_down",
            ]
        );
        assert_eq!(list[1].2, Some("Expand Folder"));
        assert_eq!(list[3].2, Some("Move Folder Up"));
        assert_eq!(on(&list, "sequencer.move_track_up"), Some(false));
        assert_eq!(on(&list, "sequencer.move_track_down"), Some(true));
    }
}
