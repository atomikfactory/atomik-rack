//! The main views drawn in French: the rack, the arrangement, the transport bar, the piano roll,
//! the console, the command palette and the export panel. Each is a golden of its display list
//! under `tests/golden/fr/` (regenerate with `ATOMIK_UPDATE_GOLDEN=1`), and each says which of
//! its fixed texts have no French entry: none may, but the names of the devices and of the
//! things the test itself names. The language is set for the test's own thread, so no test
//! depends on the language of the machine; the English goldens of the same views are the ones
//! of their own tests and do not change.

mod support;

use atomik_ui::kurbo::Rect;
use atomik_ui::{
    CommandPaletteView, DisplayList, ExportScope, ExportValues, ExportView, Lang, PaletteEntry,
    Theme,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use support::console::*;
use support::song::*;
use support::*;

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/fr")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with ATOMIK_UPDATE_GOLDEN=1)"
    );
}

/// Texts the views draw that are names or values, not words of the program.
fn is_name_or_value(text: &str) -> bool {
    const NAMES: &[&str] = &[
        // Device and strip names the fixtures give.
        "Poly Synth",
        "Stereo Delay",
        "Line Mixer",
        "Bass",
        "Lead",
        "Lead Cutoff",
        "Lead Mute",
        "Kick",
        "Master",
        "Drums",
        "Pad",
        "Keys",
        "Key & Chord",
        "Key &",
        "KEY & CHORD",
        "Arpeggiator",
        "ARPEGGIATOR",
        // The plugins of the plugin fixture and one of their params.
        "Test Synth",
        "TEST SYNTH",
        "Test Fx",
        "TEST FX",
        "Filter Mode",
    ];
    // A shortcut, a level at minus infinity, a file name: values, not words.
    let shortcut = text.contains('+') && text.split('+').all(|k| !k.is_empty() && k.len() < 12);
    NAMES.contains(&text)
        || shortcut
        || text.ends_with("-inf")
        // A label cut to its room ends in a dot; a text made by a pattern starts with a plus.
        || (text.ends_with('.') && text.chars().count() <= 14)
        || text.starts_with('+')
        || text.ends_with(".wav")
        || text.chars().filter(|c| c.is_alphabetic()).count() <= 2
        || text.chars().any(|c| c.is_ascii_digit())
}

/// Paints in French through `paint`, checks the golden `name` and that every fixed word drawn
/// has an entry.
fn french(name: &str, paint: impl FnOnce() -> DisplayList) {
    let (list, misses) =
        atomik_ui::i18n::with_misses(|| atomik_ui::i18n::with_language(Lang::French, paint));
    check(name, &list.to_json());
    let missing: BTreeSet<&String> = misses.iter().filter(|m| !is_name_or_value(m)).collect();
    assert!(
        missing.is_empty(),
        "{name}: texts with no French entry:\n{}",
        missing
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The same view in English has the same words, so nothing the French drew is missing there.
    assert!(!list.texts().is_empty());
}

#[test]
fn the_rack_in_french() {
    let (doc, reg) = (reference_doc(), registry());
    french("rack_1x.json", || {
        let mut v = view(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_arrangement_in_french() {
    let (doc, reg) = (song_doc(), registry());
    french("arrangement_1x.json", || {
        let mut v = arrangement(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_transport_bar_in_french() {
    let (doc, reg) = (song_doc(), registry());
    french("transport_1x.json", || {
        let mut v = transport_bar(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_camera_button_in_french() {
    let (doc, reg) = (song_doc(), registry());
    for (name, on) in [("off", false), ("on", true)] {
        french(&format!("transport_camera_{name}_1x.json"), || {
            let mut v = atomik_ui::TransportBar::new(Theme::dark());
            v.set_bounds(Rect::new(0.0, 0.0, 1920.0, 44.0));
            v.sync(&doc, &reg);
            let state = atomik_ui::TransportState {
                camera: on,
                ..v.state()
            };
            v.set_state(state);
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &doc, &reg);
            dl
        });
    }
}

#[test]
fn the_piano_roll_in_french() {
    let (doc, reg) = (song_doc(), registry());
    french("piano_roll_1x.json", || {
        let mut v = piano_roll(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_console_in_french() {
    let (doc, reg) = (console_doc(), console_registry());
    french("console_1x.json", || {
        let mut v = console_view(&doc, &reg);
        v.set_frame_time(1.0);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

fn entry(id: &str, name: &str, path: &str, shortcut: Option<&str>) -> PaletteEntry {
    use atomik_ui::i18n::tr;
    PaletteEntry {
        key: id.to_owned(),
        id: id.to_owned(),
        name: tr(name).into_owned(),
        path: tr(path).into_owned(),
        shortcut: shortcut.map(str::to_owned),
        enabled: true,
    }
}

#[test]
fn the_command_palette_in_french() {
    french("command_palette_1x.json", || {
        let entries = vec![
            entry("editor.quantize", "Quantize", "Edit", Some("Ctrl+K")),
            entry("edit.undo", "Undo", "Edit", Some("Ctrl+Z")),
            entry("file.save", "Save", "File", Some("Ctrl+S")),
            entry("help.guided_tour", "Guided Tour", "Help", None),
            entry("view.toggle_launcher", "Launcher", "View", Some("F10")),
        ];
        let mut v = CommandPaletteView::new(Theme::dark(), entries, vec!["file.save".into()]);
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 600.0));
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl
    });
}

#[test]
fn the_export_panel_in_french() {
    for (name, scope) in [
        ("export_panel_song_1x.json", ExportScope::Song),
        ("export_panel_stems_1x.json", ExportScope::Stems),
    ] {
        french(name, || {
            let mut v = ExportView::new(Theme::dark());
            v.set_bounds(Rect::new(0.0, 0.0, 900.0, 600.0));
            v.open(scope, "my song.wav", 48_000, ExportValues::default());
            let mut dl = DisplayList::new();
            v.paint(&mut dl);
            dl
        });
    }
}

#[test]
fn the_audio_editor_vocal_mode_in_french() {
    use atomik_model::harmony::Key;
    use atomik_model::theory::ScaleKind;
    use atomik_ui::{AudioEditorView, AudioMode};
    use support::audio::*;
    let reg = registry();
    let mut doc = audio_doc();
    doc.song_mut().harmony.key = Some(Key::new(0, ScaleKind::Major));
    let clip = atomik_model::ClipId(10);
    french("audio_editor_vocal_1x.json", || {
        let mut v = AudioEditorView::new(Theme::dark());
        v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
        v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
        v.set_mode(AudioMode::Vocal);
        v.open(clip);
        v.sync(&doc, &reg);
        v.set_vocal_notes(clip, vocal_notes());
        v.set_vocal_selection(&[1]);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_detect_chords_panel_in_french() {
    use atomik_ui::{ChordsPreview, ChordsValues, ChordsView, PreviewChord};
    french("chords_panel_1x.json", || {
        let mut v = ChordsView::new(Theme::dark(), ChordsValues::default());
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 600.0));
        v.set_target("Piano.wav, bars 1 to 2".into());
        v.set_preview(Some(ChordsPreview {
            span: (0, 7680),
            chords: vec![
                PreviewChord {
                    tick: 0,
                    length: 3840,
                    symbol: "Am".into(),
                    confidence: 0.9,
                },
                PreviewChord {
                    tick: 3840,
                    length: 3840,
                    symbol: "E7".into(),
                    confidence: 0.3,
                },
            ],
            key: Some("A minor".into()),
        }));
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl
    });
}

#[test]
fn the_key_chord_and_arpeggiator_panels_following_the_song_in_french() {
    let mut doc = reference_doc();
    let mut kc = device("kc", "atomik.key-chord", "Key & Chord");
    kc.params.insert("chord.source".into(), 1.0);
    let mut arp = device("arp", "atomik.arpeggiator", "Arpeggiator");
    arp.params.insert("source".into(), 2.0);
    doc.devices = vec![kc, arp];
    doc.cables.clear();
    let reg = registry();
    french("key_chord_arp_song_1x.json", || {
        let mut v = view(&doc, &reg);
        v.set_song_harmony(Some("Am".into()), Some("Fmaj7".into()));
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    });
}

#[test]
fn the_learn_strip_of_a_plugin_window_in_french() {
    use atomik_ui::{PluginLearnMode, PluginLearnState, PluginLearnStatus, PluginLearnStrip};
    let states = [
        ("idle", None, PluginLearnStatus::Waiting),
        (
            "automation",
            Some(PluginLearnMode::Automation),
            PluginLearnStatus::LaneShown("Cutoff".into()),
        ),
        (
            "midi",
            Some(PluginLearnMode::Midi),
            PluginLearnStatus::MoveHardware("Cutoff".into()),
        ),
        (
            "mapped",
            Some(PluginLearnMode::Midi),
            PluginLearnStatus::Mapped {
                param: "Cutoff".into(),
                source: "CC 21 ch 1".into(),
            },
        ),
    ];
    for (name, mode, status) in states {
        french(&format!("plugin_learn_strip_{name}_1x.json"), || {
            let mut v = PluginLearnStrip::new(Theme::dark());
            v.set_bounds(Rect::new(0.0, 0.0, 720.0, atomik_ui::LEARN_STRIP_HEIGHT));
            v.set_state(PluginLearnState {
                device: key("lead"),
                plugin: "Test Synth".into(),
                mode,
                status,
            });
            let mut dl = DisplayList::new();
            v.paint(&mut dl);
            dl
        });
    }
}

/// The webcam controller's preview box and consent dialog in French.
#[test]
fn the_camera_views_in_french() {
    use atomik_ui::{
        CameraDialogState, CameraDialogView, CameraLamp, CameraPicture, CameraPreviewState,
        CameraPreviewView,
    };
    let picture = || CameraPicture {
        width: 2,
        height: 2,
        rgb: vec![10; 12],
    };
    let hand: Vec<(f32, f32)> = (0..21)
        .map(|i| (0.2 + 0.03 * i as f32, 0.9 - 0.035 * i as f32))
        .collect();
    let previews = [
        ("no_hand", CameraLamp::NoHand, false, None, None),
        ("hand", CameraLamp::Hand, true, None, None),
        (
            "pinch",
            CameraLamp::Pinch,
            true,
            Some(("Lead Cutoff", "2.4 kHz")),
            None,
        ),
        (
            "no_target",
            CameraLamp::Pinch,
            true,
            None,
            Some("Click a knob first"),
        ),
        (
            "pad",
            CameraLamp::Pinch,
            true,
            Some(("Lead Cutoff", "2.4 kHz")),
            None,
        ),
    ];
    for (name, lamp, with_hand, target, message) in previews {
        let (hand, picture) = (hand.clone(), picture());
        french(&format!("camera_preview_{name}_1x.json"), move || {
            let mut v = CameraPreviewView::new();
            v.set_origin(atomik_ui::kurbo::Point::new(660.0, 380.0));
            v.set_state(CameraPreviewState {
                lamp,
                picture: Some(picture),
                hands: match (with_hand, name) {
                    (_, "pad") => vec![hand.clone(), hand],
                    (true, _) => vec![hand],
                    (false, _) => Vec::new(),
                },
                target: target.map(|t| t.0.to_owned()),
                value: target.map(|t| t.1.to_owned()),
                message: message.map(str::to_owned),
                fps: 29.6,
                infer_ms: 21.4,
                controls: None,
                pad: (name == "pad").then_some((0.62, 0.4, 0.5)),
            });
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &Theme::dark());
            dl
        });
    }
    let dialogs = [
        (
            "consent",
            CameraDialogState::Consent {
                source: "storage.googleapis.com".into(),
                bytes: 7_818_763,
            },
        ),
        (
            "downloading",
            CameraDialogState::Downloading {
                received: 3_000_000,
                total: 7_818_763,
            },
        ),
        (
            "failed",
            CameraDialogState::Failed {
                message: "The server answered 500".into(),
            },
        ),
    ];
    for (name, state) in dialogs {
        french(&format!("camera_dialog_{name}_1x.json"), move || {
            let mut v = CameraDialogView::new((900.0, 600.0), state);
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &Theme::dark());
            dl
        });
    }
}

/// The Separate Stems dialog in each state, in French.
#[test]
fn the_stems_dialog_in_french() {
    use atomik_ui::{StemsDialogState, StemsDialogView, StemsPhase};
    let dialogs = [
        (
            "options",
            StemsDialogState::Options {
                clip: "Verse Guitar".into(),
                stems: [true, true, false, true],
                mute_original: true,
                mono: false,
            },
        ),
        (
            "options_mono",
            StemsDialogState::Options {
                clip: "Verse Guitar".into(),
                stems: [true, true, false, true],
                mute_original: true,
                mono: true,
            },
        ),
        (
            "working",
            StemsDialogState::Working {
                phase: StemsPhase::Separating,
                progress: 0.42,
            },
        ),
        (
            "failed",
            StemsDialogState::Failed {
                message: "cannot read the file".into(),
            },
        ),
    ];
    for (name, state) in dialogs {
        french(&format!("stems_dialog_{name}_1x.json"), move || {
            let mut v = StemsDialogView::new((900.0, 600.0), state);
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &Theme::dark());
            dl
        });
    }
}

/// The camera's MIDI controls in the preview box, and a MIDI Mappings list with camera sources,
/// in French.
#[test]
fn the_camera_midi_views_in_french() {
    use atomik_model::{
        CameraControl, CameraHand, DeviceKey, MapMode, MapOptions, MidiMapping, MidiSource,
        MidiTarget, RelativeKind,
    };
    use atomik_ui::{
        CameraBar, CameraControlsState, CameraLamp, CameraPicture, CameraPreviewState,
        CameraPreviewView, MidiMapRow, MidiMapsView,
    };
    for (name, learning) in [("midi", false), ("midi_learn", true)] {
        french(&format!("camera_preview_{name}_1x.json"), move || {
            let bar = |label, value, centred| CameraBar {
                label,
                value,
                centred,
            };
            let mut v = CameraPreviewView::new();
            v.set_origin(atomik_ui::kurbo::Point::new(660.0, 330.0));
            v.set_state(CameraPreviewState {
                lamp: CameraLamp::Pinch,
                picture: Some(CameraPicture {
                    width: 2,
                    height: 2,
                    rgb: vec![10; 12],
                }),
                hands: vec![
                    (0..21)
                        .map(|i| (0.2 + 0.03 * i as f32, 0.9 - 0.035 * i as f32))
                        .collect(),
                ],
                target: None,
                value: None,
                message: None,
                fps: 29.6,
                infer_ms: 21.4,
                controls: Some(CameraControlsState {
                    hand: "Right hand, channel 1",
                    bars: vec![
                        bar("Twist", 0.4, true),
                        bar("Height", 0.8, false),
                        bar("Left/right", 0.5, false),
                        bar("Closeness", 0.65, false),
                        bar("Pinch", 1.0, false),
                        bar("Palm", 0.0, false),
                    ],
                    learning,
                }),
                pad: None,
            });
            let mut dl = DisplayList::new();
            v.paint(&mut dl, &Theme::dark());
            dl
        });
    }
    french("midi_maps_camera_1x.json", || {
        let row = |id: u32, source: MidiSource, mode: MapMode, name: &str| MidiMapRow {
            mapping: MidiMapping {
                id,
                source,
                target: MidiTarget::Param {
                    device: DeviceKey("ch".into()),
                    param: "fader".into(),
                },
                options: MapOptions {
                    mode,
                    ..MapOptions::default()
                },
            },
            target: name.to_owned(),
            missing: false,
        };
        let mut v = MidiMapsView::new(Theme::dark());
        v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 400.0));
        v.set_rows(vec![
            row(
                1,
                MidiSource::camera(CameraHand::Right, CameraControl::Twist),
                MapMode::Relative(RelativeKind::TwosComplement),
                "Lead Cutoff",
            ),
            row(
                2,
                MidiSource::camera(CameraHand::Left, CameraControl::OpenPalm),
                MapMode::Toggle,
                "Lead Mute",
            ),
        ]);
        v.set_ports(vec!["Pads".into(), atomik_model::CAMERA_PORT.into()]);
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl
    });
}

#[test]
fn a_quarantined_plugin_device_in_french() {
    use atomik_ui::plugin_data::PluginDeviceStatus;
    use support::container::paint_rack;
    use support::plugin::plugin_rack;
    let d = plugin_rack();
    let reg = registry();
    french("plugin_panel_quarantined_1x.json", || {
        let mut v = plain_view(d.doc(), &reg);
        v.set_plugin_status(
            &key("synth"),
            PluginDeviceStatus {
                quarantined: Some("Crashed 3 times in a row: exited with code 3".into()),
                ..PluginDeviceStatus::default()
            },
        );
        paint_rack(&mut v, d.doc())
    });
}

#[test]
fn the_plugin_manager_with_its_process_column_and_a_quarantine_reason_in_french() {
    use atomik_device_sdk::PluginFormat;
    use atomik_ui::plugin_data::{PluginEntryStatus, PluginProcessChoice};
    use atomik_ui::{PluginManagerData, PluginManagerRow, PluginManagerView};
    let row = |id: &str, name: &str, status, process| PluginManagerRow {
        type_id: Some(id.to_owned()),
        name: name.to_owned(),
        vendor: "Vendor 1".to_owned(),
        format: PluginFormat::Clap,
        category: "Effect".to_owned(),
        status,
        process,
        path: format!("/p/{name}.clap"),
        scanned_secs: 1,
        scanned: "2026-11-01 10:00".to_owned(),
        favourite: false,
        tags: Vec::new(),
    };
    french("plugin_manager_1x.json", || {
        let mut v = PluginManagerView::new(
            Theme::dark(),
            PluginManagerData {
                rows: vec![
                    row(
                        "clap.a",
                        "Alpha 1",
                        PluginEntryStatus::Ok,
                        PluginProcessChoice::Default,
                    ),
                    row(
                        "clap.b",
                        "Beta 2",
                        PluginEntryStatus::Quarantined(
                            "Crashed 3 times in a row: exited with code 3".into(),
                        ),
                        PluginProcessChoice::Own,
                    ),
                    row(
                        "clap.c",
                        "Gamma 3",
                        PluginEntryStatus::Ok,
                        PluginProcessChoice::Shared,
                    ),
                ],
                scan: None,
                scanner_available: true,
                plugins_on: true,
            },
        );
        v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 700.0));
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl
    });
}
