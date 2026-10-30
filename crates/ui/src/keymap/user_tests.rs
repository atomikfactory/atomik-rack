//! Tests of the user keymap (`keymap.json`) and the editor behind Settings.
//! Every test takes the OS as a parameter.

use super::*;

fn defs(profile: Profile) -> Vec<BindingDef> {
    Keymap::bundled_defs(profile).unwrap()
}

fn user(json: &str) -> UserKeymap {
    UserKeymap::from_json(json).unwrap().0
}

fn editor(os: Os, profile: Profile, json: &str) -> KeymapEditor {
    KeymapEditor::new(os, profile, user(json))
}

fn key(command: &str, context: Context) -> RowKey {
    RowKey {
        command: command.to_string(),
        arg: None,
        context,
    }
}

fn c(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

fn chords_of(e: &KeymapEditor, command: &str, context: Context) -> Vec<String> {
    e.keymap()
        .chords_of(command, None)
        .into_iter()
        .filter(|(ctx, _)| *ctx == context)
        .map(|(_, ch)| ch.to_string())
        .collect()
}

// ------------------------------------------------------------------ the file

#[test]
fn a_missing_or_empty_file_is_an_empty_diff() {
    let (u, notes) = UserKeymap::from_json("{}").unwrap();
    assert!(u.is_empty() && notes.is_empty());
    assert_eq!(u.profile, None);
    assert_eq!(
        u.effective_defs(&defs(Profile::Standard)),
        defs(Profile::Standard)
    );
}

#[test]
fn a_corrupt_file_is_an_error_not_a_panic() {
    assert!(UserKeymap::from_json("{ nope").is_err());
    assert!(UserKeymap::from_json("[1, 2]").is_err());
    assert!(UserKeymap::from_json("").is_err());
}

#[test]
fn loading_is_lenient_unknown_fields_ignored_unknown_ids_kept() {
    let (u, notes) = UserKeymap::from_json(
        r#"{
        "version": 7, "future": {"x": 1}, "profile": "legacy",
        "bindings": [
          {"command": "edit.duplicate", "context": "global", "chords": ["Cmd+Shift+D"], "extra": 1},
          {"command": "future.thing", "context": "editor", "chords": ["Cmd+J"]},
          {"command": "edit.copy", "context": "nowhere", "chords": ["Cmd+C"]},
          {"no_command": true},
          "junk"
        ],
        "removed": [{"command": "edit.copy", "chord": "Cmd+C"}, {"oops": 1}],
        "roles": {"alternate_tool": "Cmd+Alt", "future_role": "Shift"}
    }"#,
    )
    .unwrap();
    assert_eq!(u.profile, Some(Profile::Legacy));
    let ids: Vec<&str> = u.bindings.iter().map(|b| b.command.as_str()).collect();
    assert_eq!(
        ids,
        ["edit.duplicate", "future.thing"],
        "unknown ids are kept"
    );
    assert_eq!(u.removed.len(), 1);
    assert_eq!(u.roles.len(), 2, "an unknown role is kept too");
    assert_eq!(
        notes.len(),
        4,
        "{notes:?}: three bad bindings and one bad removal"
    );
}

#[test]
fn unknown_commands_are_kept_in_the_file_and_ignored_by_the_keymap() {
    let u = user(
        r#"{"bindings": [{"command": "future.thing", "context": "global", "chords": ["Cmd+Shift+D"]}]}"#,
    );
    for os in Os::ALL {
        let built = u.build(Profile::Standard, os);
        assert_eq!(built.report.unknown_commands, ["future.thing"]);
        assert!(built.keymap.chords_of("future.thing", None).is_empty());
        assert!(built.report.conflicts.is_empty());
    }
    let again = user(&u.to_json());
    assert_eq!(again, u, "saving keeps the unknown id");
}

#[test]
fn a_binding_replaces_the_profile_binding_of_the_same_command_and_context() {
    let u = user(
        r#"{"bindings": [{"command": "edit.duplicate", "context": "global", "chords": ["Cmd+Shift+D"]}]}"#,
    );
    for os in Os::ALL {
        let built = u.build(Profile::Standard, os);
        let got: Vec<String> = built
            .keymap
            .chords_of("edit.duplicate", None)
            .into_iter()
            .map(|(_, c)| c.to_string())
            .collect();
        assert_eq!(got, ["Cmd+Shift+D"], "{os:?}");
        assert!(built.report.conflicts.is_empty(), "{os:?}");
    }
}

#[test]
fn a_binding_for_an_unbound_command_adds_and_removed_takes_one_chord_away() {
    let u = user(
        r#"{
        "bindings": [{"command": "rack.auto_route", "context": "rack", "chords": ["Cmd+Alt+R"]}],
        "removed": [{"command": "edit.redo", "chord": "Cmd+Y"}]
    }"#,
    );
    let built = u.build(Profile::Standard, Os::Windows);
    assert_eq!(
        built.keymap.chords_of("rack.auto_route", None)[0]
            .1
            .to_string(),
        "Cmd+Alt+R"
    );
    let redo: Vec<String> = built
        .keymap
        .chords_of("edit.redo", None)
        .into_iter()
        .map(|(_, c)| c.to_string())
        .collect();
    assert_eq!(redo, ["Cmd+Shift+Z"]);
}

#[test]
fn removed_also_reaches_chords_that_come_from_an_os_override() {
    let u = user(r#"{"removed": [{"command": "view.toggle_console", "chord": "Cmd+Alt+Digit5"}]}"#);
    let mac = u.build(Profile::Standard, Os::Mac);
    let got: Vec<String> = mac
        .keymap
        .chords_of("view.toggle_console", None)
        .into_iter()
        .map(|(_, c)| c.to_string())
        .collect();
    assert_eq!(got, ["F5"]);
}

#[test]
fn saving_is_canonical_sorted_and_round_trips() {
    let a = user(
        r#"{"profile": "standard", "bindings": [
        {"command": "edit.paste", "context": "global", "chords": ["shift+cmd+v"]},
        {"command": "edit.copy", "context": "global", "chords": ["Cmd+Shift+C"]}],
        "roles": {"disable_snap": "Cmd+Alt", "alternate_tool": "Alt"}}"#,
    );
    let json = a.to_json();
    let b = UserKeymap::from_json(&json).unwrap().0;
    assert_eq!(b.to_json(), json, "a second save changes nothing");
    let copy = json.find("edit.copy").unwrap();
    let paste = json.find("edit.paste").unwrap();
    assert!(copy < paste, "bindings are sorted by command");
    assert!(
        json.contains("Cmd+Shift+V"),
        "chords are printed canonically: {json}"
    );
    assert!(json.find("alternate_tool").unwrap() < json.find("disable_snap").unwrap());
    assert!(json.ends_with("}\n"));
    assert!(json.contains("\"version\": 1"));
    assert!(
        !json.contains("time") && !json.contains("date"),
        "no timestamps"
    );
}

#[test]
fn an_empty_diff_saves_only_the_version() {
    let json = UserKeymap::default().to_json();
    assert_eq!(json, "{\n  \"version\": 1\n}\n");
}

// ------------------------------------------------------------------ conflicts, shadows, reserved

#[test]
fn a_crafted_file_reports_conflicts_shadows_bad_chords_and_reserved_chords() {
    let u = user(
        r#"{"bindings": [
        {"command": "edit.cut", "context": "global", "chords": ["Cmd+C"]},
        {"command": "tool.pencil", "context": "sequencer", "chords": ["Space"]},
        {"command": "edit.paste", "context": "global", "chords": ["Cmd+Hyper+V"]},
        {"command": "edit.undo", "context": "global", "chords": ["Alt+F4"]},
        {"command": "edit.select_all", "context": "global", "chords": ["Escape"]}
    ]}"#,
    );
    let built = u.build(Profile::Standard, Os::Windows);
    let r = &built.report;
    assert_eq!(r.conflicts.len(), 1, "{:?}", r.conflicts);
    assert_eq!(r.conflicts[0].chord.to_string(), "Cmd+C");
    assert!(
        r.shadows
            .iter()
            .any(|s| s.specific.0 == "tool.pencil" && s.global.0 == "transport.play_stop"),
        "{:?}",
        r.shadows
    );
    assert_eq!(
        r.bad_chords,
        [("edit.paste".to_string(), "Cmd+Hyper+V".to_string())]
    );
    let reserved: Vec<&str> = r.reserved.iter().map(|(_, c, _)| c.as_str()).collect();
    assert_eq!(reserved, ["Alt+F4", "Escape"]);
    assert!(!r.is_clean());
    let text = r.summary().unwrap();
    assert!(text.contains("Cmd+C"), "{text}");
    // The ignored chords are not in the keymap; the conflicting pair both are.
    assert!(
        built
            .keymap
            .chords_of("edit.undo", None)
            .iter()
            .all(|(_, c)| c.to_string() != "Alt+F4")
    );
}

#[test]
fn the_bundled_profiles_are_clean_on_every_os() {
    for profile in Profile::ALL {
        for os in Os::ALL {
            let built = UserKeymap::default().build(profile, os);
            assert!(
                built.report.is_clean(),
                "{profile:?} {os:?}: {:?}",
                built.report
            );
            assert!(built.report.summary().is_none());
        }
    }
}

#[test]
fn reserved_chords_are_the_os_keys_escape_and_plain_text_keys() {
    let r = |os, ctx, s: &str| reserved(os, ctx, &c(s));
    assert!(r(Os::Windows, Context::Global, "Alt+F4").is_some());
    assert!(r(Os::Linux, Context::Global, "Alt+F4").is_some());
    assert!(r(Os::Mac, Context::Global, "Alt+F4").is_none());
    for s in ["Cmd+Q", "Cmd+H", "Cmd+M"] {
        assert!(r(Os::Mac, Context::Global, s).is_some(), "{s}");
        assert!(r(Os::Windows, Context::Global, s).is_none(), "{s}");
    }
    assert!(r(Os::Windows, Context::Global, "Escape").is_some());
    assert!(r(Os::Mac, Context::Editor, "Shift+Escape").is_some());
    for s in ["Space", "Return", "Tab"] {
        assert!(r(Os::Windows, Context::Text, s).is_some(), "{s}");
        assert!(
            r(Os::Windows, Context::Global, s).is_none(),
            "{s} outside text"
        );
    }
    assert!(
        r(Os::Windows, Context::Text, "Cmd+Space").is_none(),
        "primary chords run in text"
    );
    assert!(
        r(Os::Windows, Context::Global, "Meta+K").is_some(),
        "the Windows key is the OS's"
    );
    assert!(r(Os::Windows, Context::Global, "Cmd+K").is_none());
}

// ------------------------------------------------------------------ the editor

#[test]
fn rebinding_changes_only_that_binding_and_the_diff_holds_only_it() {
    for os in Os::ALL {
        let mut e = editor(os, Profile::Standard, "{}");
        let k = key("edit.duplicate", Context::Global);
        let out = e.rebind(&k, Slot::Replace(0), c("Cmd+Shift+D"));
        assert_eq!(out, Rebind::Applied { notes: vec![] }, "{os:?}");
        assert_eq!(
            chords_of(&e, "edit.duplicate", Context::Global),
            ["Cmd+Shift+D"]
        );
        assert_eq!(e.user().bindings.len(), 1, "{os:?}");
        let rows = e.rows("duplicate");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, Source::User);
        // Restoring brings the default back and empties the diff.
        assert!(e.restore(&k).is_empty());
        assert_eq!(chords_of(&e, "edit.duplicate", Context::Global), ["Cmd+D"]);
        assert!(e.user().is_empty(), "{os:?}");
        assert_eq!(e.rows("duplicate")[0].source, Source::Default);
    }
}

#[test]
fn setting_a_binding_back_to_its_default_leaves_no_trace() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    e.rebind(&k, Slot::Replace(0), c("Cmd+Shift+D"));
    e.rebind(&k, Slot::Replace(0), c("Cmd+D"));
    assert!(e.user().is_empty());
}

#[test]
fn adding_a_second_chord_keeps_the_first() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    assert!(matches!(
        e.rebind(&k, Slot::Add, c("Cmd+Alt+D")),
        Rebind::Applied { .. }
    ));
    assert_eq!(
        chords_of(&e, "edit.duplicate", Context::Global),
        ["Cmd+D", "Cmd+Alt+D"]
    );
}

#[test]
fn a_chord_in_use_in_the_same_context_is_a_conflict_that_names_the_other_command() {
    for os in Os::ALL {
        let mut e = editor(os, Profile::Standard, "{}");
        let k = key("edit.duplicate", Context::Global);
        let out = e.rebind(&k, Slot::Replace(0), c("Cmd+C"));
        let Rebind::Conflict(clashes) = out else {
            panic!("{os:?}: {out:?}");
        };
        assert_eq!(clashes.len(), 1);
        assert_eq!(clashes[0].command, "edit.copy");
        assert_eq!(clashes[0].label, "Copy");
        assert!(e.user().is_empty(), "a conflict changes nothing yet");
    }
}

#[test]
fn unassign_other_takes_the_chord_from_the_other_command() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    e.resolve(&k, Slot::Replace(0), c("Cmd+C"), Resolve::UnassignOther);
    assert_eq!(chords_of(&e, "edit.duplicate", Context::Global), ["Cmd+C"]);
    assert!(chords_of(&e, "edit.copy", Context::Global).is_empty());
    assert!(find_conflicts(e.keymap()).is_empty());
    assert_eq!(
        e.rows("copy")
            .iter()
            .find(|r| r.key.command == "edit.copy")
            .unwrap()
            .source,
        Source::User
    );
}

#[test]
fn swap_gives_the_other_command_the_chord_the_slot_had() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    e.resolve(&k, Slot::Replace(0), c("Cmd+C"), Resolve::Swap);
    assert_eq!(chords_of(&e, "edit.duplicate", Context::Global), ["Cmd+C"]);
    assert_eq!(chords_of(&e, "edit.copy", Context::Global), ["Cmd+D"]);
    assert!(find_conflicts(e.keymap()).is_empty());
    // Restoring both brings the defaults back.
    e.restore(&k);
    e.restore(&key("edit.copy", Context::Global));
    assert!(e.user().is_empty());
}

#[test]
fn cancel_changes_nothing() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    e.resolve(&k, Slot::Replace(0), c("Cmd+C"), Resolve::Cancel);
    assert!(e.user().is_empty());
}

#[test]
fn shadowing_a_global_binding_from_a_context_is_a_note_not_a_conflict() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("tool.razor", Context::Sequencer);
    let out = e.rebind(&k, Slot::Replace(0), c("Space"));
    let Rebind::Applied { notes } = out else {
        panic!("{out:?}");
    };
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("Play"), "{notes:?}");
    assert!(find_conflicts(e.keymap()).is_empty());
}

#[test]
fn reserved_chords_are_refused_but_a_default_may_keep_its_own() {
    let mut e = editor(Os::Mac, Profile::Standard, "{}");
    let k = key("edit.duplicate", Context::Global);
    assert!(matches!(
        e.rebind(&k, Slot::Replace(0), c("Cmd+Q")),
        Rebind::Reserved(_)
    ));
    assert!(matches!(
        e.rebind(&k, Slot::Replace(0), c("Escape")),
        Rebind::Reserved(_)
    ));
    assert!(e.user().is_empty());
    // Quit owns Cmd+Q on macOS by default: re-setting it is allowed (and changes nothing).
    let q = key("file.quit", Context::Global);
    assert_eq!(
        e.rebind(&q, Slot::Replace(0), c("Cmd+Q")),
        Rebind::Unchanged
    );
}

#[test]
fn clearing_unbinds_and_restore_brings_it_back() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.undo", Context::Global);
    e.clear(&k, 0);
    assert!(chords_of(&e, "edit.undo", Context::Global).is_empty());
    let row = e.rows("undo").into_iter().find(|r| r.key == k).unwrap();
    assert!(row.chords.is_empty() && row.source == Source::User);
    let json = e.user().to_json();
    assert!(json.contains("edit.undo"), "{json}");
    e.restore(&k);
    assert_eq!(chords_of(&e, "edit.undo", Context::Global), ["Cmd+Z"]);
}

#[test]
fn clearing_one_of_two_chords_keeps_the_other() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("edit.redo", Context::Global);
    e.clear(&k, 0);
    assert_eq!(chords_of(&e, "edit.redo", Context::Global), ["Cmd+Shift+Z"]);
}

#[test]
fn restoring_all_empties_the_diff_but_keeps_the_chosen_profile() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    e.set_profile(Profile::Legacy);
    e.rebind(
        &key("edit.undo", Context::Global),
        Slot::Replace(0),
        c("Cmd+Alt+U"),
    );
    e.set_role(Role::AlternateTool, Mods::parse("Cmd+Shift").unwrap())
        .unwrap();
    e.restore_all();
    assert!(
        e.user().bindings.is_empty() && e.user().roles.is_empty() && e.user().removed.is_empty()
    );
    assert_eq!(e.profile(), Profile::Legacy);
}

#[test]
fn restoring_can_bring_a_conflict_back_and_says_so() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let dup = key("edit.duplicate", Context::Global);
    e.resolve(&dup, Slot::Replace(0), c("Cmd+C"), Resolve::UnassignOther);
    // Copy lost Cmd+C; restoring Copy while Duplicate still holds it is a conflict.
    let conflicts = e.restore(&key("edit.copy", Context::Global));
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].chord.to_string(), "Cmd+C");
}

#[test]
fn the_profile_choice_is_kept_and_changes_the_keymap() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    assert_eq!(e.profile(), Profile::Standard);
    e.set_profile(Profile::Legacy);
    assert_eq!(e.profile(), Profile::Legacy);
    assert_eq!(chords_of(&e, "edit.duplicate", Context::Rack), ["Cmd+D"]);
    let saved = e.user().to_json();
    assert!(saved.contains("\"profile\": \"legacy\""));
    let back = editor(Os::Windows, Profile::Standard, &saved);
    assert_eq!(
        back.profile(),
        Profile::Legacy,
        "persisted across a restart"
    );
    // A user who never chose follows the app default.
    assert_eq!(
        editor(Os::Windows, Profile::Legacy, "{}").profile(),
        Profile::Legacy
    );
}

#[test]
fn rows_list_every_command_by_group_with_args_one_row_each() {
    let e = editor(Os::Windows, Profile::Standard, "{}");
    let rows = e.rows("");
    for d in Registry::bundled().commands() {
        assert!(
            rows.iter().any(|r| r.key.command == d.id),
            "{} has a row",
            d.id
        );
    }
    let groups: Vec<Group> = rows.iter().map(|r| r.group).collect();
    let mut sorted = groups.clone();
    sorted.sort();
    assert_eq!(groups, sorted, "rows are grouped in listing order");
    let snaps = rows
        .iter()
        .filter(|r| r.key.command == "sequencer.set_snap")
        .count();
    assert_eq!(
        snaps,
        super::command::SNAP_ARGS.len(),
        "one row per argument value"
    );
    assert!(rows.iter().any(|r| r.key.command == "transport.go_locator"
        && r.key.arg.as_deref() == Some("left")
        && r.chords.iter().any(|c| c.to_string() == "Alt+Left")));
}

#[test]
fn search_matches_label_id_and_chord_text() {
    let e = editor(Os::Windows, Profile::Standard, "{}");
    let ids = |q: &str| -> Vec<String> { e.rows(q).into_iter().map(|r| r.key.command).collect() };
    assert!(ids("split").contains(&"sequencer.split_at_playhead".to_string()));
    assert!(ids("SPLIT").contains(&"sequencer.split_at_playhead".to_string()));
    assert!(ids("transport.rec").contains(&"transport.record".to_string()));
    assert!(ids("alt+x").contains(&"sequencer.split_at_playhead".to_string()));
    assert!(ids("zzzz-nothing").is_empty());
    assert!(ids("  ").len() > 80, "blank shows everything");
}

#[test]
fn the_row_chords_are_shown_per_os() {
    let win = editor(Os::Windows, Profile::Standard, "{}");
    let mac = editor(Os::Mac, Profile::Standard, "{}");
    let text = |e: &KeymapEditor| {
        e.rows("import audio")[0]
            .chords
            .iter()
            .map(|c| c.menu_text(e.os()))
            .collect::<Vec<_>>()
    };
    assert_eq!(text(&win), ["Ctrl+Shift+I"]);
    assert_eq!(text(&mac), ["Cmd+Opt+I"]);
}

#[test]
fn an_unbound_command_gets_a_binding_in_its_default_context() {
    let mut e = editor(Os::Linux, Profile::Standard, "{}");
    let row = e.rows("auto-route").remove(0);
    assert!(row.chords.is_empty());
    let out = e.rebind(&row.key, Slot::Add, c("Cmd+Alt+R"));
    assert!(matches!(out, Rebind::Applied { .. }));
    assert_eq!(
        chords_of(&e, "rack.auto_route", Context::Rack),
        ["Cmd+Alt+R"]
    );
}

#[test]
fn a_loaded_conflict_is_listed_for_the_ui() {
    let e = editor(
        Os::Windows,
        Profile::Standard,
        r#"{"bindings": [{"command": "edit.cut", "context": "global", "chords": ["Cmd+C"]}]}"#,
    );
    assert_eq!(e.report().conflicts.len(), 1);
    assert!(e.report().summary().unwrap().contains("Cmd+C"));
}

#[test]
fn roles_rebind_and_reject_a_clash_within_a_gesture_class() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    assert_eq!(
        e.roles()
            .binding(Role::AlternateTool)
            .unwrap()
            .mods
            .to_string(),
        "Alt"
    );
    e.set_role(Role::AlternateTool, Mods::parse("Shift+Alt").unwrap())
        .unwrap();
    assert_eq!(
        e.roles()
            .binding(Role::AlternateTool)
            .unwrap()
            .mods
            .to_string(),
        "Alt+Shift"
    );
    assert_eq!(e.user().roles.get("alternate_tool").unwrap(), "Alt+Shift");
    // Constrain is Shift on a drag of an item; copy_drag is Cmd on the same class.
    let err = e
        .set_role(Role::CopyDrag, Mods::parse("Shift").unwrap())
        .unwrap_err();
    assert!(err.contains("Constrain"), "{err}");
    assert_eq!(
        e.roles().binding(Role::CopyDrag).unwrap().mods.to_string(),
        "Cmd"
    );
    // Setting a role back to its default leaves no entry.
    e.set_role(Role::AlternateTool, Mods::parse("Alt").unwrap())
        .unwrap();
    assert!(e.user().roles.is_empty());
    assert!(e.set_role(Role::AlternateTool, Mods::default()).is_err());
}

#[test]
fn roles_in_the_file_apply_and_a_bad_one_is_reported() {
    let e = editor(
        Os::Mac,
        Profile::Standard,
        r#"{"roles": {"alternate_tool": "Ctrl+Shift", "constrain": "Banana", "future": "Alt"}}"#,
    );
    assert_eq!(
        e.roles()
            .binding(Role::AlternateTool)
            .unwrap()
            .mods
            .to_string(),
        "Ctrl+Shift"
    );
    assert_eq!(
        e.report().bad_roles,
        [("constrain".to_string(), "Banana".to_string())]
    );
    assert_eq!(
        e.roles().binding(Role::Constrain).unwrap().mods.to_string(),
        "Shift"
    );
}

#[test]
fn editing_is_deterministic_and_order_independent_in_the_saved_text() {
    let a = {
        let mut e = editor(Os::Windows, Profile::Standard, "{}");
        e.rebind(
            &key("edit.undo", Context::Global),
            Slot::Replace(0),
            c("Cmd+Alt+U"),
        );
        e.rebind(
            &key("edit.copy", Context::Global),
            Slot::Replace(0),
            c("Cmd+Alt+C"),
        );
        e.user().to_json()
    };
    let b = {
        let mut e = editor(Os::Windows, Profile::Standard, "{}");
        e.rebind(
            &key("edit.copy", Context::Global),
            Slot::Replace(0),
            c("Cmd+Alt+C"),
        );
        e.rebind(
            &key("edit.undo", Context::Global),
            Slot::Replace(0),
            c("Cmd+Alt+U"),
        );
        e.user().to_json()
    };
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------- additions

fn chord_texts(k: &Keymap, command: &str) -> Vec<String> {
    k.chords_of(command, None)
        .iter()
        .map(|(_, c)| c.to_string())
        .collect()
}

#[test]
fn a_user_binding_replaces_the_profile_os_overrides_too() {
    // Import Audio has a macOS override in the standard profile; a user entry holds the chord
    // list for every OS, so the override must not survive it.
    let json = r#"{"bindings":[{"command":"file.import_audio","context":"global","chords":["Cmd+Shift+K"]}]}"#;
    for os in Os::ALL {
        let built = user(json).build(Profile::Standard, os);
        assert_eq!(
            chord_texts(&built.keymap, "file.import_audio"),
            ["Cmd+Shift+K"],
            "{os:?}"
        );
        assert!(built.report.is_clean(), "{os:?}");
    }
    // The same through the editor (the Settings path).
    let mut e = editor(Os::Mac, Profile::Standard, "{}");
    let k = key("file.import_audio", Context::Global);
    assert!(matches!(
        e.rebind(&k, Slot::Replace(0), c("Cmd+Shift+K")),
        Rebind::Applied { .. }
    ));
    assert_eq!(
        chords_of(&e, "file.import_audio", Context::Global),
        ["Cmd+Shift+K"]
    );
}

#[test]
fn a_default_keeps_its_own_reserved_chord_in_a_loaded_file_but_no_other_command_gets_one() {
    // Quit is on Cmd+Q on macOS by default; adding a second chord keeps it. Another command
    // asking for Cmd+Q is refused and reported.
    let keeps = r#"{"bindings":[{"command":"file.quit","context":"global","chords":["Cmd+Q","Cmd+Shift+Q"]}]}"#;
    let built = user(keeps).build(Profile::Standard, Os::Mac);
    assert!(built.report.reserved.is_empty(), "{:?}", built.report);
    assert_eq!(
        chord_texts(&built.keymap, "file.quit"),
        ["Cmd+Q", "Cmd+Shift+Q"]
    );
    let steals = r#"{"bindings":[{"command":"file.save","context":"global","chords":["Cmd+Q"]}]}"#;
    let built = user(steals).build(Profile::Standard, Os::Mac);
    assert_eq!(built.report.reserved.len(), 1, "{:?}", built.report);
    assert_eq!(built.report.reserved[0].0, "file.save");
    assert_eq!(chord_texts(&built.keymap, "file.save"), ["Cmd+S"]);
}

#[test]
fn removals_are_saved_sorted_whatever_order_they_were_made_in() {
    let a = r#"{"removed":[{"command":"edit.undo","chord":"Cmd+Z"},{"command":"edit.copy","chord":"Cmd+C"},{"command":"edit.copy","chord":"Cmd+Insert"}]}"#;
    let b = r#"{"removed":[{"command":"edit.copy","chord":"Cmd+Insert"},{"command":"edit.undo","chord":"Cmd+Z"},{"command":"edit.copy","chord":"Cmd+C"}]}"#;
    assert_eq!(user(a).to_json(), user(b).to_json());
    let text = user(a).to_json();
    let copy = text.find("edit.copy").unwrap();
    let undo = text.find("edit.undo").unwrap();
    assert!(copy < undo, "sorted by command: {text}");
    let c1 = text.find("Cmd+C\"").unwrap();
    let c2 = text.find("Cmd+Insert").unwrap();
    assert!(c1 < c2, "then by chord: {text}");
}

#[test]
fn a_removed_chord_makes_the_row_a_user_row_and_restore_brings_it_back() {
    let json = r#"{"removed":[{"command":"edit.redo","chord":"Cmd+Y"}]}"#;
    let mut e = editor(Os::Windows, Profile::Legacy, json);
    let k = key("edit.redo", Context::Global);
    let row = e.rows("").into_iter().find(|r| r.key == k).unwrap();
    assert_eq!(row.source, Source::User);
    let before = chords_of(&e, "edit.redo", Context::Global);
    assert!(!before.contains(&"Ctrl+Y".to_string()), "{before:?}");
    e.restore(&k);
    assert!(e.user().removed.is_empty());
    let row = e.rows("").into_iter().find(|r| r.key == k).unwrap();
    assert_eq!(row.source, Source::Default);
    assert_eq!(
        chords_of(&e, "edit.redo", Context::Global).len(),
        before.len() + 1
    );
}

#[test]
fn a_global_binding_that_takes_a_chord_a_context_holds_is_noted() {
    // The sequencer keeps S; giving S to a global command leaves the sequencer's binding in
    // charge inside the sequencer: a note, not a conflict.
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    let k = key("file.save", Context::Global);
    let out = e.rebind(&k, Slot::Add, c("S"));
    let Rebind::Applied { notes } = out else {
        panic!("{out:?}");
    };
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(notes[0].contains("keeps S in the sequencer"), "{notes:?}");
    assert!(find_conflicts(e.keymap()).is_empty());
}

#[test]
fn a_role_goes_back_to_its_default_modifiers() {
    let mut e = editor(Os::Windows, Profile::Standard, "{}");
    e.set_role(Role::AlternateTool, Mods::parse("Shift+Alt").unwrap())
        .unwrap();
    assert!(e.user().roles.contains_key("alternate_tool"));
    e.reset_role(Role::AlternateTool);
    assert!(e.user().roles.is_empty());
    assert_eq!(
        e.roles()
            .binding(Role::AlternateTool)
            .unwrap()
            .mods
            .to_string(),
        "Alt"
    );
}

#[test]
fn the_old_profile_name_reference_loads_as_standard() {
    let (u, notes) = UserKeymap::from_json(r#"{"profile": "reference"}"#).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(u.profile, Some(Profile::Standard));
    let (u, _) = UserKeymap::from_json(r#"{"profile": "standard"}"#).unwrap();
    assert_eq!(u.profile, Some(Profile::Standard));
}
