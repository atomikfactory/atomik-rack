//! Conflict detection over a [`Keymap`]: two commands on one chord in one context
//! are a conflict (an error); a context-specific binding over a `Global` one is a shadow (a
//! note, the specific one wins in its context).

use super::{Chord, Context, Keymap, PhysChord};
use std::collections::BTreeMap;

/// Two or more different commands on one chord in one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The context both bindings are in.
    pub context: Context,
    /// The chord as authored in the first binding.
    pub chord: Chord,
    /// The commands (id and argument), in file order.
    pub commands: Vec<(String, Option<String>)>,
}

/// A binding in a specific context over a different command on the same chord in `Global`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shadow {
    /// The specific context.
    pub context: Context,
    /// The chord as authored in the specific binding.
    pub chord: Chord,
    /// The command that wins in `context`.
    pub specific: (String, Option<String>),
    /// The `Global` command it shadows there.
    pub global: (String, Option<String>),
}

type Entry = (String, Option<String>);

fn entries(keymap: &Keymap) -> BTreeMap<(Context, PhysChord), (Chord, Vec<Entry>)> {
    let mut map: BTreeMap<(Context, PhysChord), (Chord, Vec<Entry>)> = BTreeMap::new();
    for b in keymap.bindings() {
        for c in &b.chords {
            let slot = map
                .entry((b.context, c.physical(keymap.os())))
                .or_insert_with(|| (*c, Vec::new()));
            let e = (b.command.clone(), b.arg.clone());
            if !slot.1.contains(&e) {
                slot.1.push(e);
            }
        }
    }
    map
}

/// The conflicts of a keymap, ordered by context and chord.
pub fn find_conflicts(keymap: &Keymap) -> Vec<Conflict> {
    entries(keymap)
        .into_iter()
        .filter(|(_, (_, cmds))| cmds.len() > 1)
        .map(|((context, _), (chord, commands))| Conflict {
            context,
            chord,
            commands,
        })
        .collect()
}

/// The shadows of a keymap, ordered by context and chord.
pub fn find_shadows(keymap: &Keymap) -> Vec<Shadow> {
    let map = entries(keymap);
    let mut out = Vec::new();
    for ((context, phys), (chord, cmds)) in &map {
        if *context == Context::Global {
            continue;
        }
        let Some((_, globals)) = map.get(&(Context::Global, *phys)) else {
            continue;
        };
        for specific in cmds {
            for global in globals {
                if specific != global {
                    out.push(Shadow {
                        context: *context,
                        chord: *chord,
                        specific: specific.clone(),
                        global: global.clone(),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{BindingDef, Os};

    fn def(command: &str, ctx: Context, chords: &[&str]) -> BindingDef {
        BindingDef {
            command: command.to_string(),
            arg: None,
            context: ctx,
            chords: chords.iter().map(|c| (*c).to_string()).collect(),
            os: None,
        }
    }

    fn map(defs: &[BindingDef]) -> Keymap {
        Keymap::from_defs(Os::Windows, defs).unwrap()
    }

    #[test]
    fn the_same_chord_for_two_commands_in_one_context_is_a_conflict() {
        let k = map(&[
            def("edit.copy", Context::Global, &["Cmd+C"]),
            def("create.effect", Context::Global, &["Cmd+C", "Cmd+E"]),
            def("edit.cut", Context::Global, &["Cmd+X"]),
        ]);
        let c = find_conflicts(&k);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].context, Context::Global);
        assert_eq!(c[0].chord.to_string(), "Cmd+C");
        assert_eq!(
            c[0].commands,
            [
                ("edit.copy".to_string(), None),
                ("create.effect".to_string(), None)
            ]
        );
    }

    #[test]
    fn cmd_and_ctrl_collide_on_windows_but_not_on_macos() {
        let defs = [
            def("edit.copy", Context::Global, &["Cmd+C"]),
            def("edit.cut", Context::Global, &["Ctrl+C"]),
        ];
        assert_eq!(
            find_conflicts(&Keymap::from_defs(Os::Windows, &defs).unwrap()).len(),
            1
        );
        assert_eq!(
            find_conflicts(&Keymap::from_defs(Os::Linux, &defs).unwrap()).len(),
            1
        );
        assert!(find_conflicts(&Keymap::from_defs(Os::Mac, &defs).unwrap()).is_empty());
    }

    #[test]
    fn the_same_command_twice_on_a_chord_is_not_a_conflict() {
        let k = map(&[
            def("edit.copy", Context::Global, &["Cmd+C"]),
            def("edit.copy", Context::Global, &["Cmd+C"]),
        ]);
        assert!(find_conflicts(&k).is_empty());
    }

    #[test]
    fn a_specific_context_over_global_is_a_shadow_not_a_conflict() {
        let k = map(&[
            def("transport.go_start", Context::Global, &["Return"]),
            def("sequencer.open_clip", Context::Sequencer, &["Return"]),
            def("tool.razor", Context::Sequencer, &["R"]),
        ]);
        assert!(find_conflicts(&k).is_empty());
        let s = find_shadows(&k);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].context, Context::Sequencer);
        assert_eq!(s[0].specific.0, "sequencer.open_clip");
        assert_eq!(s[0].global.0, "transport.go_start");
    }

    #[test]
    fn different_contexts_do_not_conflict() {
        let k = map(&[
            def("rack.select_next", Context::Rack, &["Down"]),
            def("sequencer.select_next_track", Context::Sequencer, &["Down"]),
        ]);
        assert!(find_conflicts(&k).is_empty());
        assert!(find_shadows(&k).is_empty());
    }
}

#[cfg(test)]
mod qa_tests {
    use super::*;
    use crate::keymap::{BindingDef, Os};

    #[test]
    fn the_same_command_in_global_and_a_context_is_not_a_shadow() {
        let def = |ctx| BindingDef {
            command: "edit.undo".to_string(),
            arg: None,
            context: ctx,
            chords: vec!["Cmd+Z".to_string()],
            os: None,
        };
        let k =
            Keymap::from_defs(Os::Windows, &[def(Context::Global), def(Context::Editor)]).unwrap();
        assert!(find_shadows(&k).is_empty());
        assert!(find_conflicts(&k).is_empty());
    }
}
