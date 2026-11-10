use super::*;

fn spec(role: Role, source: SourceKind, mode: ControlMode) -> ControlSpec {
    ControlSpec {
        role,
        source,
        channel: 0,
        mode,
        feedback: false,
    }
}

fn sample() -> ControllerProfile {
    ControllerProfile {
        name: "Desk Controller".to_owned(),
        input: "USB MIDI Controller".to_owned(),
        output: Some("USB MIDI Controller".to_owned()),
        controls: vec![
            ControlSpec {
                feedback: true,
                ..spec(Role::Knob(1), SourceKind::Cc(21), ControlMode::Absolute)
            },
            spec(
                Role::Knob(2),
                SourceKind::Cc(22),
                ControlMode::Relative(RelativeKind::BinaryOffset),
            ),
            spec(Role::Knob(3), SourceKind::Nrpn(300), ControlMode::Absolute),
            spec(Role::Fader(1), SourceKind::PitchBend, ControlMode::Absolute),
            ControlSpec {
                channel: 1,
                ..spec(Role::Fader(2), SourceKind::Cc14(3), ControlMode::Absolute)
            },
            spec(Role::Play, SourceKind::Note(94), ControlMode::Button),
            spec(Role::Loop, SourceKind::Cc(115), ControlMode::Button),
        ],
        options: ProfileOptions::default(),
    }
}

/// A scratch folder that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-profile-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_profile_round_trips_through_its_file_text() {
    let p = sample();
    let text = p.to_json();
    assert_eq!(ControllerProfile::from_json(&text).unwrap(), p);
    // The channel is written from 1, the feedback only when set.
    assert!(text.contains("\"channel\": 2"), "{text}");
    assert_eq!(text.matches("\"feedback\"").count(), 1);
}

#[test]
fn the_documented_example_reads() {
    let text = r#"{ "format": 1, "name": "Desk", "input": "In", "controls": [
        { "role": "knob", "index": 1, "message": "cc", "number": 21, "channel": 1,
          "mode": "absolute", "feedback": true },
        { "role": "fader", "index": 1, "message": "pitchbend", "channel": 1, "mode": "absolute" },
        { "role": "play", "message": "note", "number": 94, "channel": 1, "mode": "button" } ] }"#;
    let p = ControllerProfile::from_json(text).unwrap();
    assert_eq!(p.controls.len(), 3);
    assert!(p.output.is_none());
    assert!(p.control(Role::Knob(1)).unwrap().feedback);
    assert_eq!(p.control(Role::Play).unwrap().source, SourceKind::Note(94));
}

#[test]
fn damaged_and_newer_files_are_refused_with_a_reason() {
    let ok = sample().to_json();
    let bad = |from: &str, to: &str| ControllerProfile::from_json(&ok.replace(from, to));
    assert!(matches!(
        bad("\"format\": 1", "\"format\": 2"),
        Err(ProfileError::Newer(2))
    ));
    assert!(bad("\"cc\"", "\"sysex\"").is_err());
    assert!(bad("\"absolute\"", "\"sideways\"").is_err());
    assert!(bad("\"knob\"", "\"dial\"").is_err());
    assert!(bad("\"number\": 21", "\"number\": 200").is_err());
    assert!(bad("\"channel\": 1,", "\"channel\": 17,").is_err());
    assert!(ControllerProfile::from_json("not json").is_err());
    assert!(ControllerProfile::from_json("{}").is_err());
}

#[test]
fn contradictions_are_refused() {
    let mut p = sample();
    p.controls.push(spec(
        Role::Knob(1),
        SourceKind::Cc(50),
        ControlMode::Absolute,
    ));
    assert!(p.validate().unwrap_err().to_string().contains("twice"));
    let mut p = sample();
    p.controls.push(spec(
        Role::Knob(4),
        SourceKind::Cc(21),
        ControlMode::Absolute,
    ));
    assert!(
        p.validate()
            .unwrap_err()
            .to_string()
            .contains("same message")
    );
    let mut p = sample();
    p.controls
        .push(spec(Role::Stop, SourceKind::Cc(116), ControlMode::Absolute));
    assert!(p.validate().is_err(), "a key must be a button");
    let mut p = sample();
    p.controls.push(spec(
        Role::Knob(5),
        SourceKind::Note(1),
        ControlMode::Button,
    ));
    assert!(p.validate().is_err(), "a knob cannot be a button");
    let mut p = sample();
    p.name = "!!".to_owned();
    assert!(p.validate().is_err());
}

#[test]
fn the_store_saves_lists_loads_and_deletes() {
    let dir = Scratch::new("store");
    let p = sample();
    let path = save(&dir.0, &p).unwrap();
    assert!(path.ends_with("desk-controller.json"));
    assert_eq!(load(&dir.0, "desk-controller").unwrap(), p);
    std::fs::write(dir.0.join("broken.json"), "{").unwrap();
    let listed = list(&dir.0);
    assert_eq!(listed.len(), 2);
    assert!(listed[0].1.is_err() && listed[1].1.is_ok());
    delete(&dir.0, "desk-controller").unwrap();
    delete(&dir.0, "desk-controller").unwrap();
    assert!(load(&dir.0, "desk-controller").is_err());
}

#[test]
fn a_controller_is_found_again_by_its_port_name() {
    let dir = Scratch::new("port");
    save(&dir.0, &sample()).unwrap();
    assert!(for_port(&dir.0, "USB MIDI Controller").is_some());
    // Plugged in elsewhere, the system adds an index to the port name.
    assert!(for_port(&dir.0, "USB MIDI Controller 2").is_some());
    assert!(for_port(&dir.0, "Another Port").is_none());
    // Two profiles that both fit: none is guessed.
    let mut other = sample();
    other.name = "Second".to_owned();
    other.input = "USB MIDI Controller 2 B".to_owned();
    save(&dir.0, &other).unwrap();
    assert!(for_port(&dir.0, "USB MIDI Controller 2").is_none());
    assert_eq!(
        for_port(&dir.0, "USB MIDI Controller").unwrap().name,
        "Desk Controller"
    );
}

#[test]
fn export_and_import_share_a_profile_and_never_overwrite() {
    let dir = Scratch::new("share");
    let theirs = Scratch::new("theirs");
    let p = sample();
    let shared = theirs.0.join("shared.json");
    export(&p, &shared).unwrap();
    let first = import(&dir.0, &shared).unwrap();
    assert_eq!(first, p);
    let second = import(&dir.0, &shared).unwrap();
    assert_eq!(second.name, "Desk Controller 2");
    assert_eq!(second.controls, p.controls);
    assert_eq!(list(&dir.0).len(), 2);
    // A file that is not a profile imports nothing.
    std::fs::write(theirs.0.join("bad.json"), "[]").unwrap();
    assert!(import(&dir.0, &theirs.0.join("bad.json")).is_err());
    assert_eq!(list(&dir.0).len(), 2);
}

#[test]
fn driver_ids_and_slugs() {
    assert_eq!(slug("Desk  Controller!"), "desk-controller");
    assert_eq!(slug("--"), "");
    assert_eq!(sample().driver_id(), "profile:desk-controller");
    assert_eq!(slug_of_driver("profile:abc"), Some("abc"));
    assert_eq!(slug_of_driver("mcu"), None);
    assert_eq!(slug_of_driver("profile:"), None);
    assert_eq!(
        profile_dir(Some(Path::new("/x/y/settings.json"))),
        Some(PathBuf::from("/x/y/controllers"))
    );
    assert_eq!(profile_dir(None), None);
}

#[test]
fn a_profile_without_the_new_fields_reads_as_the_defaults() {
    // The file of an older build: no page keys, no options.
    let text = sample().to_json();
    assert!(!text.contains("pickup") && !text.contains("keep_page"));
    let back = ControllerProfile::from_json(&text).unwrap();
    assert_eq!(back.options, ProfileOptions::default());
    assert!(back.options.pickup && !back.options.keep_page);
}

#[test]
fn the_options_and_the_page_keys_round_trip() {
    let mut p = sample();
    p.options = ProfileOptions {
        pickup: false,
        keep_page: true,
    };
    p.controls.push(spec(
        Role::PageDown,
        SourceKind::Note(96),
        ControlMode::Button,
    ));
    p.controls.push(spec(
        Role::PageUp,
        SourceKind::Note(97),
        ControlMode::Button,
    ));
    let text = p.to_json();
    assert!(text.contains("\"page_down\"") && text.contains("\"page_up\""));
    assert!(text.contains("\"pickup\": false") && text.contains("\"keep_page\": true"));
    assert_eq!(ControllerProfile::from_json(&text).unwrap(), p);
    // A page key is a button like the others.
    let mut bad = sample();
    bad.controls.push(spec(
        Role::PageUp,
        SourceKind::Cc(30),
        ControlMode::Absolute,
    ));
    assert!(bad.validate().is_err());
}
