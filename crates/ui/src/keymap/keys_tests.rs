use super::*;

fn km(profile: Profile, os: Os) -> Keymap {
    Keymap::bundled(profile, os)
}

fn first(r: Resolution) -> Resolved {
    match r {
        Resolution::Bound(mut list) => list.remove(0),
        other => panic!("not bound: {other:?}"),
    }
}

fn chord(text: &str, os: Os) -> PhysChord {
    Chord::parse(text).unwrap().physical(os)
}

fn stack(c: &[Context]) -> ContextStack {
    ContextStack::new(c)
}

#[test]
fn the_keys_context_plays_notes_and_keeps_the_global_keys_it_does_not_use() {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = km(profile, os);
            let st = stack(&[Context::Keys]);
            for (c, cmd, arg) in [
                ("A", "keys.note", Some("c")),
                ("W", "keys.note", Some("c_sharp")),
                ("L", "keys.note", Some("d_up")),
                (";", "keys.note", Some("e_up")),
                (":", "keys.note", Some("e_up")),
                ("Z", "keys.octave_down", None),
                ("X", "keys.octave_up", None),
                ("1", "keys.velocity", Some("1")),
                ("0", "keys.velocity", Some("10")),
            ] {
                let r = first(k.resolve(&chord(c, os), &st));
                assert_eq!((r.command.as_str(), r.arg.as_deref()), (cmd, arg), "{c}");
            }
            // Outside the context the letters keep their commands.
            assert_eq!(
                first(k.resolve(&chord("Tab", os), &stack(&[Context::Rack]))).command,
                "view.toggle_flip"
            );
            assert_eq!(
                first(k.resolve(&chord("Cmd+S", os), &st)).command,
                "file.save"
            );
        }
    }
}

#[test]
fn piano_keys_open_on_f4_in_the_standard_profile_and_f9_in_legacy() {
    for os in Os::ALL {
        for (profile, key) in [(Profile::Standard, "F4"), (Profile::Legacy, "F9")] {
            let k = km(profile, os);
            assert_eq!(
                first(k.resolve(&chord(key, os), &stack(&[Context::Keys]))).command,
                "view.toggle_piano_keys"
            );
        }
    }
}

#[test]
fn the_key_commands_have_their_own_group_and_every_argument_is_bound() {
    let r = Registry::bundled();
    for d in r.commands().iter().filter(|d| d.id.starts_with("keys.")) {
        assert_eq!(
            (d.group, d.context),
            (Group::Keys, Context::Keys),
            "{}",
            d.id
        );
    }
    assert!(r.validate("keys.note", Some("f_up")));
    assert!(r.validate("keys.velocity", Some("10")));
    assert!(!r.validate("keys.note", Some("g_up")));
    for profile in Profile::ALL {
        let k = km(profile, Os::Windows);
        for arg in KEYS_NOTE_ARGS {
            assert!(!k.chords_of("keys.note", Some(arg)).is_empty(), "{arg}");
        }
        assert!(find_conflicts(&k).is_empty());
    }
}
