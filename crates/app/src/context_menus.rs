//! What the context menus list: per target, the registry commands
//! in order with their enabled flags. Labels and shortcut text are not written here: the menu
//! widget takes the label from the registry and the shortcut from the active keymap, so a key
//! and a menu item cannot differ.

use atomik_model::theory::{ChordKind, DiatonicSize, PITCH_NAMES, ScaleKind};
use atomik_ui::ContextTarget;
use atomik_ui::chord_edit::{
    ARP_GATES, ARP_OCTAVES, ArpPattern, STRUM_AMOUNTS, Stamp, StrumDirection, strum_ticks,
};
use atomik_ui::context_menu::Spec;
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
    /// Rack: the primary device has a song track (Go to Track).
    pub has_track: bool,
    /// Arrangement: two or more clips are selected (Join).
    pub join: bool,
    /// Arrangement: a selected clip is a note clip (Open in the editor).
    pub open_clip: bool,
    /// Arrangement: every selected clip is muted ("Unmute" instead of "Mute").
    pub clips_muted: bool,
    /// Piano roll: every selected note is muted ("Unmute Notes" instead of "Mute Notes").
    pub notes_muted: bool,
    /// Rack: the selection is the Master Section alone: it cannot be cut, copied, duplicated
    /// or deleted.
    pub master_only: bool,
    /// Rack: "Use as Send Effect" can work on the selection (the model would accept it).
    pub assign_send: bool,
    /// Rack: how many columns the rack has (Remove Column needs two, Add Column room).
    pub columns: u16,
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
}

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
                item("rack.auto_route", e.selection),
                item("rack.assign_send", e.assign_send),
                item("rack.disconnect", e.selection),
                item("rack.go_to_track", e.has_track),
            ]);
            v
        }
        ContextTarget::Rack => vec![
            item("edit.paste", e.paste),
            Spec::Separator,
            item("create.instrument", true),
            item("create.effect", true),
            Spec::Separator,
            item(
                "rack.add_column",
                e.columns < atomik_model::MAX_RACK_COLUMNS,
            ),
            item("rack.remove_column", e.columns > 1),
            Spec::Separator,
            item("edit.select_all", true),
        ],
        ContextTarget::TrackHeader => edit(Some(("Duplicate Track", "Delete Track"))),
        ContextTarget::Clip => {
            let mut v = edit(None);
            v.extend([
                Spec::Separator,
                item("sequencer.open_clip", e.open_clip),
                item("sequencer.split_at_playhead", true),
                item("sequencer.join", e.join),
                Spec::Separator,
                item("sequencer.toggle_mute", e.selection).labelled(if e.clips_muted {
                    "Unmute"
                } else {
                    "Mute"
                }),
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
        ContextTarget::TempoLane => vec![item("sequencer.tempo_add_point", true)],
        ContextTarget::RulerMeter => METERS
            .iter()
            .map(|(arg, insert, _)| meter_item("sequencer.meter_insert", arg, insert))
            .collect(),
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
                item("editor.toggle_note_mute", e.selection).labelled(if e.notes_muted {
                    "Unmute Notes"
                } else {
                    "Mute Notes"
                }),
                item("edit.select_all", true),
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

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::theory::Scale;

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
                "rack.auto_route",
                "rack.assign_send",
                "rack.disconnect",
                "rack.go_to_track",
            ]
        );
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
                Some("Delete Track")
            ]
        );
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
            ["Scale", "Chord Stamp", "Chords"]
        );
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
}
