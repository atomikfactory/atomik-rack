//! AC-W2 and AC-W6: the golden listings of the bundled keymaps (profile x OS), and the legacy
//! profile against the transport keys the views map themselves. Regenerate the listings with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test keymap_goldens`.

use atomik_ui::keymap::{
    Context, ContextStack, Keymap, Os, PhysChord, Profile, Resolution, find_conflicts,
};
use atomik_ui::{Key, Modifiers, TransportAction, transport_shortcut};
use std::path::PathBuf;

fn golden_path(os: Os, profile: Profile) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/keymaps")
        .join(format!("{}-{}.txt", os.name(), profile.name()))
}

#[test]
fn bundled_keymap_listings_match_the_golden_files() {
    let update = std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1");
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = Keymap::bundled(profile, os);
            assert!(find_conflicts(&k).is_empty(), "{os:?} {profile:?}");
            let actual = k.listing();
            let path = golden_path(os, profile);
            if update {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &actual).unwrap();
                continue;
            }
            let expected = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| {
                    panic!(
                        "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                        path.display()
                    )
                })
                .replace("\r\n", "\n");
            assert_eq!(
                expected,
                actual,
                "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
                path.display()
            );
        }
    }
}

#[test]
fn the_listing_is_sorted_and_stable() {
    let k = Keymap::bundled(Profile::Standard, Os::Windows);
    let a = k.listing();
    assert_eq!(a, k.listing());
    let names: Vec<&str> = a.lines().map(|l| l.split(" | ").next().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

/// The transport keys of the legacy profile equal `transport_shortcut` (which the sequencer
/// views still call themselves) for every key and for Shift, in the contexts where they work.
#[test]
fn legacy_transport_keys_equal_the_view_transport_shortcut() {
    let k = Keymap::bundled(Profile::Legacy, Os::Windows);
    let keys = [
        Key::Space,
        Key::Enter,
        Key::Char('l'),
        Key::Char('L'),
        Key::Char('r'),
        Key::Char('R'),
        Key::Char('q'),
        Key::Tab,
        Key::Escape,
        Key::Up,
        Key::F(5),
        Key::Delete,
    ];
    for ctx in [Context::Rack, Context::Sequencer] {
        let stack = ContextStack::new(&[ctx]);
        for key in &keys {
            for shift in [false, true] {
                let mods = Modifiers {
                    shift,
                    ..Modifiers::NONE
                };
                let want = transport_shortcut(key, mods);
                let got = match k.resolve(&PhysChord::from_event(key, mods).unwrap(), &stack) {
                    Resolution::Bound(list) => list.iter().find_map(|r| match r.command.as_str() {
                        "transport.play_stop" => Some(TransportAction::TogglePlay),
                        "transport.go_start" => Some(TransportAction::ReturnToZero),
                        "transport.loop" => Some(TransportAction::ToggleLoop),
                        "transport.record" => Some(TransportAction::ToggleRecord),
                        _ => None,
                    }),
                    _ => None,
                };
                assert_eq!(got, want, "{ctx:?} {key:?} shift={shift}");
            }
        }
    }
}

/// The legacy transport keys over the whole key by modifier grid on every OS, in the three
/// contexts the old views handled them in (QA-1, AC-W6): without a command or Alt modifier the
/// keymap says exactly what the views' `transport_shortcut` says; with one, only Ctrl+Space
/// (Record, the old menu shortcut) runs a transport command.
#[test]
fn the_legacy_transport_keys_equal_the_view_function_over_the_modifier_grid_on_every_os() {
    let keys = [
        Key::Space,
        Key::Enter,
        Key::Tab,
        Key::Escape,
        Key::Backspace,
        Key::Delete,
        Key::Up,
        Key::Left,
        Key::F(5),
        Key::Char('l'),
        Key::Char('L'),
        Key::Char('r'),
        Key::Char('R'),
        Key::Char('q'),
        Key::Char('x'),
        Key::Char('1'),
    ];
    let transport = |list: &[atomik_ui::keymap::Resolved]| {
        list.iter().find_map(|r| match r.command.as_str() {
            "transport.play_stop" => Some(TransportAction::TogglePlay),
            "transport.go_start" => Some(TransportAction::ReturnToZero),
            "transport.loop" => Some(TransportAction::ToggleLoop),
            "transport.record" => Some(TransportAction::ToggleRecord),
            _ => None,
        })
    };
    for os in Os::ALL {
        let k = Keymap::bundled(Profile::Legacy, os);
        for stack in [
            ContextStack::new(&[Context::Rack]),
            ContextStack::new(&[Context::Sequencer]),
            ContextStack::new(&[Context::Editor, Context::Sequencer]),
        ] {
            for key in &keys {
                for bits in 0..16u8 {
                    let mods = Modifiers {
                        shift: bits & 1 != 0,
                        alt: bits & 2 != 0,
                        ctrl: bits & 4 != 0,
                        meta: bits & 8 != 0,
                    };
                    let got = match k.resolve(&PhysChord::from_event(key, mods).unwrap(), &stack) {
                        Resolution::Bound(list) => transport(&list),
                        _ => None,
                    };
                    let what = format!("{os:?} {:?} {key:?} {mods:?}", stack.contexts());
                    if !mods.command() && !mods.alt {
                        assert_eq!(got, transport_shortcut(key, mods), "{what}");
                    } else if got.is_some() {
                        let primary = if os == Os::Mac {
                            mods.meta && !mods.ctrl
                        } else {
                            mods.ctrl && !mods.meta
                        };
                        assert!(
                            got == Some(TransportAction::ToggleRecord)
                                && *key == Key::Space
                                && primary
                                && !mods.alt,
                            "{what}: {got:?}"
                        );
                    }
                }
            }
        }
    }
}
