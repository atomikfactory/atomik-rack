//! Keymap, router and key-script tests. The OS is always a parameter.

use super::script::KeyScript;
use super::*;
use crate::event::NumpadKey;

fn km(profile: Profile, os: Os) -> Keymap {
    Keymap::bundled(profile, os)
}

fn chord(text: &str, os: Os) -> PhysChord {
    Chord::parse(text).unwrap().physical(os)
}

fn first(res: Resolution) -> Resolved {
    match res {
        Resolution::Bound(list) => list.into_iter().next().expect("non-empty"),
        other => panic!("not bound: {other:?}"),
    }
}

fn ids(res: Resolution) -> Vec<String> {
    match res {
        Resolution::Bound(list) => list.into_iter().map(|r| r.command).collect(),
        other => panic!("not bound: {other:?}"),
    }
}

fn stack(contexts: &[Context]) -> ContextStack {
    ContextStack::new(contexts)
}

#[test]
fn every_profile_builds_for_every_os_with_known_commands_and_valid_args() {
    let registry = Registry::bundled();
    for profile in Profile::ALL {
        for os in Os::ALL {
            let k = Keymap::try_bundled(profile, os).unwrap();
            assert!(k.bindings().len() > 30, "{profile:?} {os:?}");
            for b in k.bindings() {
                assert!(
                    registry.validate(&b.command, b.arg.as_deref()),
                    "{profile:?}: {} {:?}",
                    b.command,
                    b.arg
                );
                assert!(
                    !b.chords.is_empty() || b.command.is_empty(),
                    "{}",
                    b.command
                );
            }
        }
    }
}

#[test]
fn bundled_profiles_have_zero_conflicts_on_every_os() {
    for profile in Profile::ALL {
        for os in Os::ALL {
            let conflicts = find_conflicts(&km(profile, os));
            assert!(conflicts.is_empty(), "{profile:?} {os:?}: {conflicts:?}");
        }
    }
}

#[test]
fn reference_shadows_are_the_expected_notes() {
    let shadows = find_shadows(&km(Profile::Standard, Os::Windows));
    let text: Vec<String> = shadows
        .iter()
        .map(|s| {
            format!(
                "{} {} {} over {}",
                s.context.name(),
                s.chord,
                s.specific.0,
                s.global.0
            )
        })
        .collect();
    assert_eq!(
        text,
        [
            "sequencer Return sequencer.open_clip over transport.go_start",
            "sequencer Cmd+E view.toggle_editor over create.effect",
            "browser Return browser.create over transport.go_start",
            "files F files.favorite over sequencer.follow",
            "files Return files.open over transport.go_start",
            "files Space files.toggle_preview over transport.play_stop",
            "files Backspace files.parent over edit.delete",
            "files Cmd+A files.select_all over edit.select_all",
            "files Cmd+F files.search over view.toggle_browser",
            "keys F keys.note over sequencer.follow",
            "keys K keys.note over view.cables_cycle",
            "keys L keys.note over transport.loop",
        ]
    );
    assert!(find_shadows(&km(Profile::Legacy, Os::Windows)).is_empty());
}

#[test]
fn legacy_golden_table_of_todays_keys() {
    // (chord, contexts, expected first command, expected arg)
    let rows: &[(&str, &[Context], &str)] = &[
        ("Cmd+N", &[Context::Global], "file.new"),
        ("Cmd+O", &[Context::Global], "file.open"),
        ("Cmd+S", &[Context::Global], "file.save"),
        ("Cmd+Shift+S", &[Context::Global], "file.save_as"),
        ("Cmd+Q", &[Context::Global], "file.quit"),
        ("Cmd+I", &[Context::Global], "file.import_audio"),
        ("Cmd+Z", &[Context::Global], "edit.undo"),
        ("Cmd+Y", &[Context::Global], "edit.redo"),
        ("Cmd+Shift+Z", &[Context::Global], "edit.redo"),
        ("Cmd+=", &[Context::Global], "view.zoom_in"),
        ("Cmd++", &[Context::Global], "view.zoom_in"),
        ("Cmd+-", &[Context::Global], "view.zoom_out"),
        ("Cmd+0", &[Context::Global], "view.zoom_reset"),
        ("Cmd+B", &[Context::Global], "view.toggle_browser"),
        ("Cmd+,", &[Context::Global], "audio.settings"),
        ("Cmd+Space", &[Context::Global], "transport.record"),
        ("Tab", &[Context::Global], "view.toggle_flip"),
        ("F5", &[Context::Global], "view.toggle_console"),
        ("Shift+F5", &[Context::Global], "view.maximise_console"),
        ("Space", &[Context::Rack], "transport.play_stop"),
        ("Return", &[Context::Rack], "transport.go_start"),
        ("L", &[Context::Rack], "transport.loop"),
        ("R", &[Context::Rack], "transport.record"),
        ("Space", &[Context::Sequencer], "transport.play_stop"),
        (
            "R",
            &[Context::Editor, Context::Sequencer],
            "transport.record",
        ),
        (
            "Cmd+E",
            &[Context::Sequencer],
            "sequencer.split_at_playhead",
        ),
        ("Cmd+D", &[Context::Sequencer], "edit.duplicate"),
        ("M", &[Context::Sequencer], "sequencer.toggle_mute"),
        ("M", &[Context::Editor], "editor.toggle_note_mute"),
        ("Cmd+D", &[Context::Rack], "edit.duplicate"),
        ("Cmd+A", &[Context::Editor], "edit.select_all"),
        ("Q", &[Context::Editor], "editor.quantize"),
        ("Up", &[Context::Editor], "editor.transpose_up"),
        ("Shift+Down", &[Context::Editor], "editor.octave_down"),
        ("Left", &[Context::Editor], "editor.nudge_left"),
        ("Shift+Left", &[Context::Editor], "editor.nudge_left_beat"),
        ("Shift+Right", &[Context::Editor], "editor.nudge_right_beat"),
        ("Alt+Left", &[Context::Editor], "editor.nudge_left_tick"),
        ("Alt+Right", &[Context::Editor], "editor.nudge_right_tick"),
        ("Delete", &[Context::Rack], "edit.delete"),
        ("Backspace", &[Context::Sequencer], "edit.delete"),
    ];
    for os in Os::ALL {
        let k = km(Profile::Legacy, os);
        for (c, ctxs, want) in rows {
            let got = first(k.resolve(&chord(c, os), &stack(ctxs)));
            assert_eq!(got.command, *want, "{os:?} {c} in {ctxs:?}");
        }
    }
}

#[test]
fn legacy_does_not_know_the_new_keys() {
    let k = km(Profile::Legacy, Os::Windows);
    let none = |c: &str, ctxs: &[Context]| {
        assert_eq!(
            k.resolve(&chord(c, Os::Windows), &stack(ctxs)),
            Resolution::Unbound,
            "{c}"
        );
    };
    none("F8", &[Context::Global]);
    none("Cmd+K", &[Context::Editor]);
    none("Cmd+K", &[Context::Sequencer]);
    none("Alt+X", &[Context::Sequencer]);
    none("Cmd+T", &[Context::Global]);
    none("Numpad0", &[Context::Global]);
    none("Cmd+Return", &[Context::Global]);
    none("C", &[Context::Rack]);
    none("S", &[Context::Sequencer]);
}

#[test]
fn reference_default_table_of_the_plan() {
    let os = Os::Windows;
    let k = km(Profile::Standard, os);
    let rows: &[(&str, &[Context], &str, Option<&str>)] = &[
        ("Cmd+Shift+I", &[Context::Global], "file.import_audio", None),
        ("Cmd+I", &[Context::Global], "create.instrument", None),
        ("Insert", &[Context::Global], "create.instrument", None),
        ("Cmd+E", &[Context::Global], "create.effect", None),
        ("Cmd+T", &[Context::Global], "create.audio_track", None),
        (
            "Cmd+Shift+M",
            &[Context::Global],
            "create.mixer_channel",
            None,
        ),
        ("F3", &[Context::Global], "view.toggle_browser", None),
        ("Cmd+F", &[Context::Global], "view.toggle_browser", None),
        ("Cmd+B", &[Context::Global], "view.toggle_browser", None),
        (
            "Cmd+Shift+F",
            &[Context::Global],
            "view.focus_browser_search",
            None,
        ),
        ("F5", &[Context::Global], "view.toggle_console", None),
        ("F6", &[Context::Global], "view.toggle_rack", None),
        ("F7", &[Context::Global], "view.sequencer_only", None),
        ("F8", &[Context::Global], "view.toggle_editor", None),
        ("Shift+Tab", &[Context::Global], "view.toggle_editor", None),
        ("K", &[Context::Global], "view.cables_cycle", None),
        ("Space", &[Context::Global], "transport.play_stop", None),
        ("Cmd+Return", &[Context::Global], "transport.record", None),
        ("Cmd+Space", &[Context::Global], "transport.record", None),
        ("NumpadMul", &[Context::Global], "transport.record", None),
        ("NumpadEnter", &[Context::Global], "transport.play", None),
        ("Shift+Return", &[Context::Global], "transport.stop", None),
        ("Numpad0", &[Context::Global], "transport.stop", None),
        (
            "NumpadDecimal",
            &[Context::Global],
            "transport.go_start",
            None,
        ),
        ("Numpad4", &[Context::Global], "transport.rewind_bar", None),
        ("Numpad5", &[Context::Global], "transport.forward_bar", None),
        ("Numpad7", &[Context::Global], "transport.bar_back", None),
        ("Numpad8", &[Context::Global], "transport.bar_forward", None),
        (
            "Numpad1",
            &[Context::Global],
            "transport.go_locator",
            Some("left"),
        ),
        (
            "Alt+Right",
            &[Context::Global],
            "transport.go_locator",
            Some("right"),
        ),
        ("L", &[Context::Global], "transport.loop", None),
        ("NumpadDiv", &[Context::Global], "transport.loop", None),
        ("C", &[Context::Global], "transport.metronome", None),
        ("Numpad9", &[Context::Global], "transport.metronome", None),
        ("NumpadAdd", &[Context::Global], "transport.tempo_up", None),
        (
            "NumpadSub",
            &[Context::Global],
            "transport.tempo_down",
            None,
        ),
        ("S", &[Context::Sequencer], "sequencer.snap_toggle", None),
        ("F", &[Context::Global], "sequencer.follow", None),
        ("H", &[Context::Sequencer], "view.zoom_in", None),
        ("G", &[Context::Editor], "view.zoom_out", None),
        ("Z", &[Context::Sequencer], "sequencer.zoom_selection", None),
        ("Q", &[Context::Sequencer], "tool.select", None),
        ("W", &[Context::Editor], "tool.pencil", None),
        ("E", &[Context::Sequencer], "tool.eraser", None),
        ("R", &[Context::Sequencer], "tool.razor", None),
        (
            "Alt+X",
            &[Context::Sequencer],
            "sequencer.split_at_playhead",
            None,
        ),
        ("Cmd+J", &[Context::Sequencer], "sequencer.join", None),
        (
            "Cmd+Shift+T",
            &[Context::Sequencer],
            "sequencer.group_tracks",
            None,
        ),
        (
            "Cmd+Alt+Up",
            &[Context::Sequencer],
            "sequencer.move_track_up",
            None,
        ),
        (
            "Cmd+Alt+Down",
            &[Context::Sequencer],
            "sequencer.move_track_down",
            None,
        ),
        ("M", &[Context::Sequencer], "sequencer.toggle_mute", None),
        ("M", &[Context::Editor], "editor.toggle_note_mute", None),
        ("Return", &[Context::Sequencer], "sequencer.open_clip", None),
        (
            "Cmd+L",
            &[Context::Sequencer],
            "transport.loop_to_selection",
            None,
        ),
        ("P", &[Context::Sequencer], "transport.loop_and_play", None),
        ("Cmd+K", &[Context::Editor], "editor.quantize", None),
        ("Cmd+K", &[Context::Sequencer], "editor.quantize", None),
        ("Cmd+Up", &[Context::Editor], "editor.transpose_up", None),
        ("Cmd+Shift+Up", &[Context::Editor], "editor.octave_up", None),
        ("Cmd+Left", &[Context::Editor], "editor.nudge_left", None),
        ("Left", &[Context::Editor], "editor.select_prev_note", None),
        ("Right", &[Context::Editor], "editor.select_next_note", None),
        (
            "Shift+Left",
            &[Context::Editor],
            "editor.extend_prev_note",
            None,
        ),
        (
            "Shift+Right",
            &[Context::Editor],
            "editor.extend_next_note",
            None,
        ),
        (
            "Cmd+Shift+Left",
            &[Context::Editor],
            "editor.nudge_left_beat",
            None,
        ),
        (
            "Cmd+Shift+Right",
            &[Context::Editor],
            "editor.nudge_right_beat",
            None,
        ),
        (
            "Cmd+Alt+Left",
            &[Context::Editor],
            "editor.nudge_left_tick",
            None,
        ),
        (
            "Cmd+Alt+Right",
            &[Context::Editor],
            "editor.nudge_right_tick",
            None,
        ),
        ("Down", &[Context::Rack], "rack.select_next", None),
        ("Shift+Up", &[Context::Rack], "rack.extend_prev", None),
        ("Cmd+0", &[Context::Rack], "view.zoom_reset", None),
        ("Cmd+A", &[Context::Rack], "edit.select_all", None),
        ("Delete", &[Context::Editor], "edit.delete", None),
    ];
    for (c, ctxs, want, arg) in rows {
        let got = first(k.resolve(&chord(c, os), &stack(ctxs)));
        assert_eq!(
            (got.command.as_str(), got.arg.as_deref()),
            (*want, *arg),
            "{c}"
        );
    }
}

#[test]
fn the_mac_set_swaps_cmd_for_command_and_adds_the_secondaries() {
    let os = Os::Mac;
    let k = km(Profile::Standard, os);
    let g = stack(&[]);
    let r = |c: &str| k.resolve(&chord(c, os), &g);
    assert_eq!(first(r("Cmd+Alt+I")).command, "file.import_audio");
    assert_eq!(
        r("Cmd+Shift+I"),
        Resolution::Unbound,
        "Cmd+Shift+I is not import on macOS"
    );
    assert_eq!(first(r("Cmd+I")).command, "create.instrument");
    assert_eq!(
        r("Insert"),
        Resolution::Unbound,
        "Insert is not bound on macOS"
    );
    assert_eq!(first(r("Cmd+Alt+Digit3")).command, "view.toggle_browser");
    assert_eq!(first(r("Cmd+Alt+Digit5")).command, "view.toggle_console");
    assert_eq!(first(r("Cmd+Alt+Digit6")).command, "view.toggle_rack");
    assert_eq!(first(r("Cmd+Alt+Digit7")).command, "view.sequencer_only");
    assert_eq!(first(r("Cmd+Alt+Digit8")).command, "view.toggle_editor");
    assert_eq!(first(r("Cmd+Q")).command, "file.quit");
    // On Windows the Mac secondaries do not exist.
    let win = km(Profile::Standard, Os::Windows);
    assert_eq!(
        win.resolve(&chord("Cmd+Alt+Digit3", Os::Windows), &g),
        Resolution::Unbound
    );
}

#[test]
fn events_resolve_strictly_per_os() {
    let save_ctrl = (Key::Char('s'), Modifiers::CTRL);
    let save_meta = (
        Key::Char('s'),
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        },
    );
    let g = stack(&[]);
    for os in [Os::Windows, Os::Linux] {
        let k = km(Profile::Standard, os);
        assert_eq!(
            first(k.resolve_event(&save_ctrl.0, save_ctrl.1, &g)).command,
            "file.save"
        );
        assert_eq!(
            k.resolve_event(&save_meta.0, save_meta.1, &g),
            Resolution::Unbound,
            "Super is not Cmd on {os:?}"
        );
    }
    let mac = km(Profile::Standard, Os::Mac);
    assert_eq!(
        first(mac.resolve_event(&save_meta.0, save_meta.1, &g)).command,
        "file.save"
    );
    assert_eq!(
        mac.resolve_event(&save_ctrl.0, save_ctrl.1, &g),
        Resolution::Unbound,
        "Control is not Command on macOS"
    );
}

#[test]
fn most_specific_context_first_then_global_and_decline_falls_through() {
    let k = km(Profile::Standard, Os::Windows);
    let enter = chord("Return", Os::Windows);
    assert_eq!(
        ids(k.resolve(&enter, &stack(&[Context::Sequencer]))),
        ["sequencer.open_clip", "transport.go_start"],
        "Enter opens the clip, and returns to zero when the command declines"
    );
    assert_eq!(
        ids(k.resolve(&enter, &stack(&[Context::Rack]))),
        ["transport.go_start"]
    );
    assert!(
        Registry::bundled()
            .get("sequencer.open_clip")
            .unwrap()
            .can_decline
    );
    assert_eq!(
        ids(k.resolve(&enter, &stack(&[Context::Browser, Context::Rack]))),
        ["browser.create", "transport.go_start"]
    );
}

#[test]
fn text_swallows_plain_keys_but_primary_chords_and_function_keys_run() {
    let k = km(Profile::Standard, Os::Windows);
    let t = stack(&[Context::Text, Context::Rack]);
    for c in [
        "S", "Space", "Return", "Up", "Z", "1", "Shift+L", "Tab", "Numpad4", "Delete",
    ] {
        assert_eq!(
            k.resolve(&chord(c, Os::Windows), &t),
            Resolution::Text,
            "{c}"
        );
    }
    assert_eq!(
        first(k.resolve(&chord("Cmd+S", Os::Windows), &t)).command,
        "file.save"
    );
    assert_eq!(
        first(k.resolve(&chord("Cmd+Z", Os::Windows), &t)).command,
        "edit.undo"
    );
    assert_eq!(
        first(k.resolve(&chord("F5", Os::Windows), &t)).command,
        "view.toggle_console"
    );
    // On macOS the primary modifier is Command.
    let mac = km(Profile::Standard, Os::Mac);
    assert_eq!(
        first(mac.resolve(&chord("Cmd+S", Os::Mac), &t)).command,
        "file.save"
    );
    assert_eq!(
        mac.resolve(&Chord::parse("Ctrl+S").unwrap().physical(Os::Mac), &t),
        Resolution::Text
    );
}

#[test]
fn numpad_keys_are_distinct_from_the_main_row() {
    let k = km(Profile::Standard, Os::Windows);
    let g = stack(&[]);
    let numpad = k.resolve_event(&Key::Numpad(NumpadKey::Digit(0)), Modifiers::NONE, &g);
    assert_eq!(first(numpad).command, "transport.stop");
    assert_eq!(
        k.resolve_event(&Key::Char('0'), Modifiers::NONE, &g),
        Resolution::Unbound
    );
    assert_eq!(
        first(k.resolve_event(&Key::Numpad(NumpadKey::Decimal), Modifiers::NONE, &g)).command,
        "transport.go_start"
    );
    assert_eq!(
        first(k.resolve_event(&Key::Insert, Modifiers::NONE, &g)).command,
        "create.instrument"
    );
    assert_eq!(
        first(k.resolve_event(&Key::F(8), Modifiers::NONE, &g)).command,
        "view.toggle_editor"
    );
    assert_eq!(
        k.resolve_event(&Key::Other, Modifiers::NONE, &g),
        Resolution::Unbound
    );
}

#[test]
fn shortcut_text_is_derived_from_the_keymap_and_the_os() {
    let win = km(Profile::Standard, Os::Windows);
    let mac = km(Profile::Standard, Os::Mac);
    assert_eq!(
        win.shortcut_text("file.save_as", None).as_deref(),
        Some("Ctrl+Shift+S")
    );
    assert_eq!(
        mac.shortcut_text("file.save_as", None).as_deref(),
        Some("Cmd+Shift+S")
    );
    assert_eq!(
        mac.shortcut_text("file.import_audio", None).as_deref(),
        Some("Cmd+Opt+I")
    );
    assert_eq!(
        win.shortcut_text("file.import_audio", None).as_deref(),
        Some("Ctrl+Shift+I")
    );
    assert_eq!(
        win.shortcut_text("transport.go_locator", Some("left"))
            .as_deref(),
        Some("Numpad1")
    );
    assert_eq!(win.shortcut_text("transport.go_locator", None), None);
    assert_eq!(win.shortcut_text("no.such", None), None);
    let legacy = km(Profile::Legacy, Os::Windows);
    assert_eq!(
        legacy.shortcut_text("edit.redo", None).as_deref(),
        Some("Ctrl+Y")
    );
    assert_eq!(
        legacy.shortcut_text("transport.record", None).as_deref(),
        Some("Ctrl+Space")
    );
    assert_eq!(
        legacy.shortcut_text("transport.go_start", None).as_deref(),
        Some("Enter")
    );
    assert_eq!(
        legacy
            .shortcut_text("view.maximise_console", None)
            .as_deref(),
        Some("Shift+F5")
    );
}

#[test]
fn a_user_defs_can_rebind_and_unbind_per_os() {
    let defs = vec![
        BindingDef {
            command: "edit.duplicate".into(),
            arg: None,
            context: Context::Global,
            chords: vec!["Cmd+Shift+D".into()],
            os: Some(BTreeMap::from([("mac".to_string(), Vec::new())])),
        },
        BindingDef {
            command: "future.command".into(),
            arg: None,
            context: Context::Global,
            chords: vec!["Cmd+Alt+Q".into()],
            os: None,
        },
    ];
    let win = Keymap::from_defs(Os::Windows, &defs).unwrap();
    assert_eq!(
        first(win.resolve(&chord("Cmd+Shift+D", Os::Windows), &stack(&[]))).command,
        "edit.duplicate"
    );
    assert_eq!(
        first(win.resolve(&chord("Cmd+Alt+Q", Os::Windows), &stack(&[]))).command,
        "future.command",
        "unknown ids are kept"
    );
    let mac = Keymap::from_defs(Os::Mac, &defs).unwrap();
    assert_eq!(
        mac.resolve(&chord("Cmd+Shift+D", Os::Mac), &stack(&[])),
        Resolution::Unbound,
        "an empty os list unbinds there"
    );
    let bad = vec![BindingDef {
        command: "x.y".into(),
        arg: None,
        context: Context::Global,
        chords: vec!["Hyper+Q".into()],
        os: None,
    }];
    assert!(matches!(
        Keymap::from_defs(Os::Windows, &bad),
        Err(KeymapError::Chord { .. })
    ));
}

#[test]
fn defs_round_trip_through_json_without_empty_fields() {
    let defs = Keymap::bundled_defs(Profile::Standard).unwrap();
    let text = serde_json::to_string(&defs).unwrap();
    let back: Vec<BindingDef> = serde_json::from_str(&text).unwrap();
    assert_eq!(defs, back);
    assert!(!text.contains("\"arg\":null"));
}

// --- the router: repeat, release, text ---

#[test]
fn only_repeatable_commands_see_key_repeat() {
    let mut r = Router::new(km(Profile::Standard, Os::Windows));
    let g = stack(&[]);
    let (undo, mods) = (Key::Char('z'), Modifiers::CTRL);
    assert!(matches!(r.route(&undo, mods, true, &g), Route::Commands(_)));
    assert!(
        matches!(r.route(&undo, mods, true, &g), Route::Commands(_)),
        "undo repeats"
    );
    let save = Key::Char('s');
    assert!(matches!(r.route(&save, mods, true, &g), Route::Commands(_)));
    assert_eq!(r.route(&save, mods, true, &g), Route::RepeatIgnored);
    assert_eq!(r.route(&save, mods, false, &g), Route::Release);
    assert!(
        matches!(r.route(&save, mods, true, &g), Route::Commands(_)),
        "after the release a press is a press again"
    );
}

#[test]
fn release_after_shift_came_up_still_clears_the_key() {
    let mut r = Router::new(km(Profile::Standard, Os::Windows));
    let g = stack(&[]);
    let ctrl_shift = Modifiers {
        shift: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    // Save As: the key types 'S'; Shift is let go first, so the key up arrives as 's'.
    assert!(matches!(
        r.route(&Key::Char('S'), ctrl_shift, true, &g),
        Route::Commands(_)
    ));
    r.route(&Key::Char('s'), Modifiers::CTRL, false, &g);
    assert!(
        matches!(
            r.route(&Key::Char('S'), ctrl_shift, true, &g),
            Route::Commands(_)
        ),
        "the press after that release is a press, not a repeat"
    );
}

#[test]
fn clearing_held_keys_makes_the_next_press_a_press() {
    let mut r = Router::new(km(Profile::Standard, Os::Windows));
    let g = stack(&[]);
    let space = Key::Space;
    assert!(matches!(
        r.route(&space, Modifiers::NONE, true, &g),
        Route::Commands(_)
    ));
    assert_eq!(
        r.route(&space, Modifiers::NONE, true, &g),
        Route::RepeatIgnored
    );
    r.clear_held();
    assert!(matches!(
        r.route(&space, Modifiers::NONE, true, &g),
        Route::Commands(_)
    ));
}

#[test]
fn releases_are_delivered_for_every_key_even_unbound_and_modifiers() {
    let mut r = Router::new(km(Profile::Standard, Os::Windows));
    let g = stack(&[]);
    for key in [
        Key::Other,
        Key::Char('q'),
        Key::F(12),
        Key::Insert,
        Key::Numpad(NumpadKey::Add),
    ] {
        assert_eq!(r.route(&key, Modifiers::NONE, false, &g), Route::Release);
    }
}

#[test]
fn the_router_routes_text_and_unbound() {
    let mut r = Router::new(km(Profile::Standard, Os::Windows));
    let text = stack(&[Context::Text]);
    assert_eq!(
        r.route(&Key::Char('s'), Modifiers::NONE, true, &text),
        Route::Text
    );
    assert_eq!(
        r.route(&Key::Char('j'), Modifiers::NONE, true, &stack(&[])),
        Route::Unbound
    );
    r.set_keymap(km(Profile::Legacy, Os::Windows));
    assert_eq!(r.keymap().os(), Os::Windows);
}

// --- key scripts ---

#[test]
fn script_enter_opens_the_clip_in_the_sequencer_and_returns_to_zero_elsewhere() {
    KeyScript::run(
        "
        ctx sequencer
        down Return
        expect commands sequencer.open_clip,transport.go_start
        up Return
        ctx rack
        down Return
        expect commands transport.go_start
        ",
    )
    .unwrap();
}

#[test]
fn script_text_swallows_letters_but_cmd_chords_run() {
    KeyScript::run(
        "
        ctx text browser rack
        down S
        expect text
        up S
        down Space
        expect text
        up Space
        down Cmd+S
        expect command file.save
        up Cmd+S
        down F5
        expect command view.toggle_console
        ",
    )
    .unwrap();
}

#[test]
fn script_repeat_only_for_repeatable_commands() {
    KeyScript::run(
        "
        down Cmd+Z
        expect command edit.undo
        down Cmd+Z
        expect command edit.undo
        down Cmd+S
        down Cmd+S
        expect repeat-ignored
        up Cmd+S
        down Cmd+S
        expect command file.save
        ",
    )
    .unwrap();
}

#[test]
fn script_strict_ctrl_versus_cmd_per_os() {
    KeyScript::run(
        "
        os mac
        down Cmd+S
        expect command file.save
        up Cmd+S
        down Ctrl+S
        expect unbound
        up Ctrl+S
        os win
        down Cmd+S
        expect command file.save
        up Cmd+S
        down Meta+S
        expect unbound
        ",
    )
    .unwrap();
}

#[test]
fn script_profiles_switch_the_keys() {
    KeyScript::run(
        "
        profile legacy
        ctx editor sequencer
        down Q
        expect command editor.quantize
        up Q
        down R
        expect command transport.record
        up R
        profile standard
        down Q
        expect command tool.select
        up Q
        down R
        expect command tool.razor
        ",
    )
    .unwrap();
}

/// The default table holds on Linux as on Windows, and on macOS
/// with the logical `Cmd` meaning Command and the listed exceptions only.
#[test]
fn reference_keys_hold_on_linux_and_mac_with_only_the_listed_exceptions() {
    let rows: &[(&str, &[Context], &str)] = &[
        ("Cmd+I", &[Context::Global], "create.instrument"),
        ("Cmd+E", &[Context::Global], "create.effect"),
        ("Cmd+T", &[Context::Global], "create.audio_track"),
        ("Cmd+Shift+M", &[Context::Global], "create.mixer_channel"),
        ("Cmd+Return", &[Context::Global], "transport.record"),
        ("Cmd+Space", &[Context::Global], "transport.record"),
        ("Space", &[Context::Global], "transport.play_stop"),
        ("Shift+Return", &[Context::Global], "transport.stop"),
        ("Numpad0", &[Context::Global], "transport.stop"),
        ("NumpadEnter", &[Context::Global], "transport.play"),
        ("F3", &[Context::Global], "view.toggle_browser"),
        ("F6", &[Context::Global], "view.toggle_rack"),
        ("F7", &[Context::Global], "view.sequencer_only"),
        ("F8", &[Context::Global], "view.toggle_editor"),
        ("Q", &[Context::Sequencer], "tool.select"),
        ("W", &[Context::Editor], "tool.pencil"),
        ("E", &[Context::Editor], "tool.eraser"),
        ("R", &[Context::Sequencer], "tool.razor"),
        (
            "Alt+X",
            &[Context::Sequencer],
            "sequencer.split_at_playhead",
        ),
        ("Cmd+K", &[Context::Editor], "editor.quantize"),
        ("Cmd+K", &[Context::Sequencer], "editor.quantize"),
        ("Cmd+Up", &[Context::Editor], "editor.transpose_up"),
        (
            "Cmd+Shift+T",
            &[Context::Sequencer],
            "sequencer.group_tracks",
        ),
        (
            "Cmd+Alt+Up",
            &[Context::Sequencer],
            "sequencer.move_track_up",
        ),
        ("Cmd+Shift+Down", &[Context::Editor], "editor.octave_down"),
        ("Return", &[Context::Sequencer], "sequencer.open_clip"),
        ("Z", &[Context::Sequencer], "sequencer.zoom_selection"),
        ("S", &[Context::Editor], "sequencer.snap_toggle"),
        ("F", &[Context::Global], "sequencer.follow"),
    ];
    for os in [Os::Linux, Os::Mac] {
        let k = km(Profile::Standard, os);
        for (c, ctxs, want) in rows {
            let got = first(k.resolve(&chord(c, os), &stack(ctxs)));
            assert_eq!(got.command, *want, "{os:?} {c}");
        }
    }
    // The exceptions: import audio and the Insert alias.
    let linux = km(Profile::Standard, Os::Linux);
    assert_eq!(
        first(linux.resolve(&chord("Cmd+Shift+I", Os::Linux), &stack(&[]))).command,
        "file.import_audio"
    );
    assert_eq!(
        first(linux.resolve(&chord("Insert", Os::Linux), &stack(&[]))).command,
        "create.instrument"
    );
    // The same chords on Windows and Linux for every binding.
    let win = km(Profile::Standard, Os::Windows);
    assert_eq!(win.listing(), linux.listing().replace("Linux", "Windows"));
}

/// The standard profile keeps every legacy key except the listed moves;
/// the moved ones are exactly the listed ones, on every OS.
#[test]
fn every_legacy_key_is_unchanged_in_standard_or_a_listed_move() {
    for os in Os::ALL {
        let legacy = km(Profile::Legacy, os);
        let reference = km(Profile::Standard, os);
        let mut moved: Vec<String> = Vec::new();
        for b in legacy.bindings() {
            for c in &b.chords {
                let st = stack(&[b.context]);
                let want = (b.command.clone(), b.arg.clone());
                let same = match reference.resolve(&c.physical(os), &st) {
                    Resolution::Bound(list) => list
                        .iter()
                        .any(|r| (r.command.clone(), r.arg.clone()) == want),
                    _ => false,
                };
                if !same {
                    moved.push(format!("{:?} {} {}", b.context, c, b.command));
                }
            }
        }
        moved.sort();
        let mut allowed: Vec<String> = MOVED.iter().map(|s| s.to_string()).collect();
        allowed.sort();
        assert_eq!(moved, allowed, "{os:?}");
    }
}

/// What move or drop in the standard profile (context, legacy chord,
/// command it ran): the editor keys that became Cmd chords, record on R, split on Cmd+E,
/// import on Cmd+I, and the Shift variants the legacy `shortcut()` accepted by accident
/// (Shift is a real modifier now: Shift+Return stops, Shift+Space is not play).
const MOVED: &[&str] = &[
    "Editor Alt+Left editor.nudge_left_tick",
    "Editor Alt+Right editor.nudge_right_tick",
    "Editor Down editor.transpose_down",
    "Editor Left editor.nudge_left",
    "Editor Q editor.quantize",
    "Editor Right editor.nudge_right",
    "Editor Shift+Down editor.octave_down",
    "Editor Shift+Left editor.nudge_left_beat",
    "Editor Shift+Q editor.quantize",
    "Editor Shift+Right editor.nudge_right_beat",
    "Editor Shift+Up editor.octave_up",
    "Editor Up editor.transpose_up",
    "Global Cmd+I file.import_audio",
    "Global Cmd+Shift+Y edit.redo",
    "Global F4 view.toggle_file_browser",
    "Global F9 view.toggle_piano_keys",
    "Rack R transport.record",
    "Rack Shift+L transport.loop",
    "Rack Shift+R transport.record",
    "Rack Shift+Return transport.go_start",
    "Rack Shift+Space transport.play_stop",
    "Sequencer Cmd+E sequencer.split_at_playhead",
    "Sequencer R transport.record",
    "Sequencer Shift+L transport.loop",
    "Sequencer Shift+R transport.record",
    "Sequencer Shift+Return transport.go_start",
    "Sequencer Shift+Space transport.play_stop",
];

/// On most keyboards Ctrl+= zooms in as Ctrl+Shift+= (the `+` character with
/// Shift held); the legacy profile took it, so the standard profile must too.
#[test]
fn zoom_in_works_with_the_plus_character_typed_with_shift_in_both_profiles() {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = km(profile, os);
            for c in ["Cmd+=", "Cmd+Shift+=", "Cmd++", "Cmd+Shift++"] {
                assert_eq!(
                    first(k.resolve(&chord(c, os), &stack(&[Context::Rack]))).command,
                    "view.zoom_in",
                    "{os:?} {profile:?} {c}"
                );
            }
        }
    }
}

// ------------------------------------------------------------------ the file browser

#[test]
fn the_file_browser_toggle_is_f9_in_standard_and_f4_in_legacy_in_every_context() {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = km(profile, os);
            let toggle = if profile == Profile::Standard {
                "F9"
            } else {
                "F4"
            };
            for ctxs in [
                &[][..],
                &[Context::Files][..],
                &[Context::Text, Context::Files][..],
                &[Context::Rack][..],
                &[Context::Sequencer][..],
            ] {
                assert_eq!(
                    first(k.resolve(&chord(toggle, os), &stack(ctxs))).command,
                    "view.toggle_file_browser",
                    "{os:?} {profile:?} {ctxs:?}"
                );
            }
            assert_eq!(
                k.shortcut_text("view.toggle_file_browser", None).as_deref(),
                Some(toggle)
            );
        }
    }
    // Function keys are media keys on a Mac keyboard: the same alternative as F3 and F5.
    let mac = km(Profile::Standard, Os::Mac);
    assert_eq!(
        first(mac.resolve(&chord("Cmd+Alt+Digit9", Os::Mac), &stack(&[]))).command,
        "view.toggle_file_browser"
    );
}

#[test]
fn the_files_context_takes_the_panels_keys_before_the_global_ones() {
    let rows: &[(&str, &str)] = &[
        ("Up", "files.up"),
        ("Down", "files.down"),
        ("Left", "files.left"),
        ("Right", "files.right"),
        ("PageUp", "files.page_up"),
        ("PageDown", "files.page_down"),
        ("Home", "files.home"),
        ("End", "files.end"),
        ("Return", "files.open"),
        ("Escape", "files.stop"),
        ("Space", "files.toggle_preview"),
        ("Backspace", "files.parent"),
        ("Alt+Up", "files.parent"),
        ("F", "files.favorite"),
        ("F2", "files.rename"),
        ("Cmd+F", "files.search"),
        ("Cmd+A", "files.select_all"),
    ];
    let globals: &[(&str, &str)] = &[
        ("Cmd+S", "file.save"),
        ("Cmd+Z", "edit.undo"),
        ("F9", "view.toggle_file_browser"),
        ("F5", "view.toggle_console"),
        ("F3", "view.toggle_browser"),
        ("Cmd+Shift+Z", "edit.redo"),
        ("Delete", "edit.delete"),
        ("Cmd+Return", "transport.record"),
    ];
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = km(profile, os);
            let st = stack(&[Context::Files]);
            for (c, want) in rows {
                assert_eq!(
                    first(k.resolve(&chord(c, os), &st)).command,
                    *want,
                    "{os:?} {profile:?} {c}"
                );
            }
            for (c, want) in globals {
                if profile == Profile::Legacy && matches!(*c, "F3" | "F9" | "Delete" | "Cmd+Return")
                {
                    continue;
                }
                assert_eq!(
                    first(k.resolve(&chord(c, os), &st)).command,
                    *want,
                    "{os:?} {profile:?} {c}"
                );
            }
        }
    }
}

#[test]
fn files_commands_are_in_their_own_group_and_context() {
    let r = Registry::bundled();
    let files: Vec<&CommandDef> = r
        .commands()
        .iter()
        .filter(|d| d.id.starts_with("files."))
        .collect();
    assert_eq!(files.len(), 16);
    for d in files {
        assert_eq!(d.group, Group::Files, "{}", d.id);
        assert_eq!(d.context, Context::Files, "{}", d.id);
        assert!(!d.menu, "{}", d.id);
    }
    let toggle = r.get("view.toggle_file_browser").unwrap();
    assert_eq!(
        (toggle.group, toggle.context),
        (Group::View, Context::Global)
    );
    assert!(toggle.menu);
    assert_eq!(toggle.label, "File Browser");
    // Held arrows and page keys repeat while browsing; the one-shot keys do not.
    for id in ["files.up", "files.down", "files.page_up", "files.page_down"] {
        assert!(r.get(id).unwrap().repeatable, "{id}");
    }
    assert!(!r.get("files.toggle_preview").unwrap().repeatable);
}

#[test]
fn while_a_panel_field_has_the_text_only_chords_and_esc_run() {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let k = km(profile, os);
            let t = stack(&[Context::Text, Context::Files]);
            for c in [
                "Space",
                "Up",
                "Down",
                "F",
                "Backspace",
                "Return",
                "Home",
                "Alt+Up",
            ] {
                assert_eq!(
                    k.resolve(&chord(c, os), &t),
                    Resolution::Text,
                    "{os:?} {profile:?} {c}"
                );
            }
            // Esc reaches the field first as a command of the panel, then the rest.
            assert_eq!(
                first(k.resolve(&chord("Escape", os), &t)).command,
                "files.stop"
            );
            assert_eq!(
                first(k.resolve(&chord("Cmd+A", os), &t)).command,
                "files.select_all"
            );
            assert_eq!(
                first(k.resolve(&chord("Cmd+F", os), &t)).command,
                "files.search"
            );
            assert_eq!(
                first(k.resolve(&chord("Cmd+S", os), &t)).command,
                "file.save"
            );
        }
    }
}

#[test]
fn script_the_panel_keys_win_over_play_and_delete() {
    KeyScript::run(
        "os win\nprofile standard\nctx files\n\
         down Space\nexpect command files.toggle_preview\nup Space\n\
         down Backspace\nexpect command files.parent\n\
         down Cmd+S\nexpect command file.save\nup Cmd+S\n\
         down F9\nexpect command view.toggle_file_browser\nup F9\n\
         ctx text files\n\
         up Space\ndown Space\nexpect text\n\
         down Cmd+S\nexpect command file.save\n",
    )
    .unwrap();
}

#[test]
fn default_keymap_detach_chords_no_conflict() {
    for os in Os::ALL {
        let k = km(Profile::Standard, os);
        let rack = first(k.resolve(&chord("Cmd+Shift+F6", os), &stack(&[Context::Global])));
        assert_eq!(rack.command, "view.detach_rack", "{os:?}");
        let mixer = first(k.resolve(&chord("Cmd+Shift+F5", os), &stack(&[Context::Global])));
        assert_eq!(mixer.command, "view.detach_console", "{os:?}");
        // The plain F5 and F6 keep their commands.
        let f6 = first(k.resolve(&chord("F6", os), &stack(&[Context::Global])));
        assert_eq!(f6.command, "view.toggle_rack");
        assert!(find_conflicts(&k).is_empty(), "{os:?}");
        for text in ["Cmd+Shift+F5", "Cmd+Shift+F6"] {
            let c = Chord::parse(text).unwrap();
            assert_eq!(reserved(os, Context::Global, &c), None, "{text} on {os:?}");
        }
    }
}

#[test]
fn the_standard_profile_binds_the_daily_keys_of_the_classic_layout() {
    let rows: &[(&str, &[Context], &str, Option<&str>)] = &[
        (
            "X",
            &[Context::Sequencer],
            "sequencer.audio_crossfade",
            Some("toggle"),
        ),
        (
            "Alt+M",
            &[Context::Sequencer],
            "sequencer.track_height",
            Some("fit"),
        ),
        ("Cmd+E", &[Context::Sequencer], "view.toggle_editor", None),
        ("Cmd+E", &[Context::Global], "create.effect", None),
        (
            "Escape",
            &[Context::Sequencer],
            "sequencer.close_clip",
            None,
        ),
        (
            "Shift+Z",
            &[Context::Sequencer],
            "sequencer.zoom_selection",
            None,
        ),
        (
            "Alt+Z",
            &[Context::Sequencer],
            "sequencer.zoom_tracks_reset",
            None,
        ),
        ("Cmd+Alt+=", &[Context::Global], "view.zoom_in", None),
        ("Cmd+Alt+-", &[Context::Global], "view.zoom_out", None),
        (
            "Alt+Shift+Left",
            &[Context::Global],
            "transport.loop_move",
            Some("bar:-1"),
        ),
        (
            "Alt+Shift+Right",
            &[Context::Global],
            "transport.loop_move",
            Some("bar:+1"),
        ),
        (
            "Alt+Shift+Down",
            &[Context::Global],
            "transport.loop_move",
            Some("length:-1"),
        ),
        (
            "Alt+Shift+Up",
            &[Context::Global],
            "transport.loop_move",
            Some("length:+1"),
        ),
        (
            "Shift+!",
            &[Context::Global],
            "transport.all_notes_off",
            None,
        ),
        ("F", &[Context::Global], "sequencer.follow", None),
    ];
    for os in Os::ALL {
        let k = km(Profile::Standard, os);
        for (c, ctxs, want, arg) in rows {
            let got = first(k.resolve(&chord(c, os), &stack(ctxs)));
            assert_eq!(got.command, *want, "{os:?} {c}");
            assert_eq!(got.arg.as_deref(), *arg, "{os:?} {c}");
        }
    }
    // The legacy profile keeps its keys.
    let legacy = km(Profile::Legacy, Os::Windows);
    assert_eq!(
        legacy.resolve(&chord("X", Os::Windows), &stack(&[Context::Sequencer])),
        Resolution::Unbound
    );
}
