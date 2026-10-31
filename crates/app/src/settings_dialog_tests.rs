//! Settings dialog tests (headless).

use super::*;
use crate::settings::{MidiInputRole, MidiInputSetting};
use atomik_audio_io::{DeviceInfo, HostKind};
use atomik_ui::{DisplayList, Modifiers};

const VIEW: Size = Size::new(1000.0, 800.0);

fn device(name: &str, outputs: u16) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        max_inputs: 2,
        max_outputs: outputs,
        sample_rates: vec![48_000],
        buffer_range: None,
        is_default_output: false,
    }
}

fn lists() -> DeviceLists {
    DeviceLists {
        hosts: vec![
            HostInfo {
                kind: HostKind::Wasapi,
                devices: vec![
                    device("Speakers (Example USB Audio)", 2),
                    device("Virtual Cable Input", 8),
                    device("Microphone", 0),
                ],
            },
            HostInfo {
                kind: HostKind::Asio,
                devices: vec![device("Example USB ASIO", 2)],
            },
        ],
        midi_ports: vec!["Example Keyboard".into()],
        midi_outputs: vec!["Example Surface".into()],
        errors: vec![],
        input_note: None,
        stream_notes: vec![],
    }
}

fn current() -> AudioSettings {
    AudioSettings {
        host: HostChoice::Wasapi,
        output_device: Some("Virtual Cable Input".into()),
        buffer_frames: 256,
        midi_inputs: Vec::new(),
        workers: 2,
        worker_linger_spins: 0,
        metronome_gain: 0.5,
        ..AudioSettings::default()
    }
}

fn click(d: &mut SettingsDialog, p: Point) -> DialogOutcome {
    let down = UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let out = d.handle(&down, VIEW);
    if out != DialogOutcome::Open {
        return out;
    }
    d.handle(
        &UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
        VIEW,
    )
}

fn option(d: &SettingsDialog, label: &str) -> Point {
    d.layout(VIEW)
        .options
        .iter()
        .find(|(_, l, _)| l == label)
        .unwrap_or_else(|| panic!("no option {label:?}"))
        .0
        .center()
}

fn pick(d: &mut SettingsDialog, label: &str) -> DialogOutcome {
    let p = option(d, label);
    click(d, p)
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn labels(d: &SettingsDialog, title: &str) -> Vec<String> {
    d.sections()
        .into_iter()
        .find(|s| s.title == title)
        .unwrap()
        .options
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

#[test]
fn the_sections_cover_every_setting() {
    let d = SettingsDialog::new(current(), lists());
    let titles: Vec<_> = d.sections().iter().map(|s| s.title).collect();
    assert_eq!(
        titles,
        [
            "Audio host",
            "Output device",
            "Input device",
            "Buffer size (frames)",
            "MIDI inputs",
            "Worker threads",
            "Worker linger (spins)",
            "Record offset (frames)",
            "Pre-count (bars)",
            "If the audio device is lost",
        ]
    );
    assert_eq!(labels(&d, "Pre-count (bars)"), ["Off", "1", "2", "3", "4"]);
    assert_eq!(labels(&d, "Audio host"), ["WASAPI", "ASIO"]);
    assert_eq!(
        labels(&d, "Output device"),
        [
            "Host default",
            "Speakers (Example USB Audio)",
            "Virtual Cable Input",
        ],
        "input-only devices are not offered"
    );
    assert_eq!(
        labels(&d, "Input device"),
        [
            "Same as the output device",
            "Speakers (Example USB Audio)",
            "Virtual Cable Input",
            "Microphone",
        ],
        "every device of the host with inputs, after the default"
    );
    assert_eq!(
        labels(&d, "Record offset (frames)"),
        ["-480", "-240", "-96", "0", "96", "240", "480"]
    );
    assert_eq!(
        labels(&d, "Buffer size (frames)"),
        ["64", "128", "256", "512", "1024", "2048"]
    );
    assert_eq!(labels(&d, "MIDI inputs"), ["Example Keyboard"]);
    assert_eq!(
        labels(&d, "Worker threads"),
        ["0", "1", "2", "3", "4", "6", "8"]
    );
    assert_eq!(
        labels(&d, "Worker linger (spins)"),
        ["Engine default", "20000", "100000", "500000"]
    );
}

#[test]
fn the_current_values_are_selected() {
    let d = SettingsDialog::new(current(), lists());
    assert!(d.is_selected(&Choice::Host(HostChoice::Wasapi)));
    assert!(!d.is_selected(&Choice::Host(HostChoice::Asio)));
    assert!(d.is_selected(&Choice::Device(Some("Virtual Cable Input".into()))));
    assert!(d.is_selected(&Choice::Buffer(256)));
    assert!(!d.is_selected(&Choice::MidiInput("Example Keyboard".into())));
    assert!(d.draft().midi_inputs.is_empty());
    assert!(d.is_selected(&Choice::Workers(2)));
    assert!(d.is_selected(&Choice::Linger(0)));
    assert!(d.is_selected(&Choice::Input(None)));
    assert!(d.is_selected(&Choice::RecordOffset(0)));
}

#[test]
fn several_midi_inputs_can_be_switched_on_and_off() {
    let mut two = lists();
    two.midi_ports.push("Example Pads".into());
    let mut d = SettingsDialog::new(current(), two);
    assert_eq!(
        labels(&d, "MIDI inputs"),
        ["Example Keyboard", "Example Pads"],
        "there is no None entry: every port is a switch"
    );
    pick(&mut d, "Example Keyboard");
    pick(&mut d, "Example Pads");
    let names = |d: &SettingsDialog| -> Vec<String> {
        d.draft()
            .enabled_midi_inputs()
            .map(|i| i.name.clone())
            .collect()
    };
    assert_eq!(names(&d), ["Example Keyboard", "Example Pads"]);
    assert!(d.is_selected(&Choice::MidiInput("Example Pads".into())));
    // A second click switches it off; the entry stays so it can be switched on again.
    pick(&mut d, "Example Keyboard");
    assert_eq!(names(&d), ["Example Pads"]);
    assert_eq!(d.draft().midi_inputs.len(), 2);
}

#[test]
fn an_enabled_input_can_be_made_mapping_only() {
    let mut d = SettingsDialog::new(current(), lists());
    let titles = |d: &SettingsDialog| -> Vec<&'static str> {
        d.sections().iter().map(|s| s.title).collect()
    };
    assert!(!titles(&d).contains(&"MIDI inputs for mappings only"));
    pick(&mut d, "Example Keyboard");
    assert!(titles(&d).contains(&"MIDI inputs for mappings only"));
    assert_eq!(
        labels(&d, "MIDI inputs for mappings only"),
        ["Example Keyboard"]
    );
    // The same label is in both sections; the one of the mapping section is the second.
    let layout = d.layout(VIEW);
    let (rect, _, _) = layout
        .options
        .iter()
        .find(|(_, _, c)| matches!(c, Choice::MidiMappingOnly(_)))
        .unwrap();
    click(&mut d, rect.center());
    assert_eq!(d.draft().midi_inputs[0].role, MidiInputRole::Mapping);
    let layout = d.layout(VIEW);
    let (rect, _, _) = layout
        .options
        .iter()
        .find(|(_, _, c)| matches!(c, Choice::MidiMappingOnly(_)))
        .unwrap();
    click(&mut d, rect.center());
    assert_eq!(d.draft().midi_inputs[0].role, MidiInputRole::Keyboard);
}

#[test]
fn listing_the_ports_again_keeps_the_draft_and_the_stream_notes() {
    let mut first = lists();
    first.stream_notes = vec!["Buffer 256 frames".into()];
    let mut d = SettingsDialog::new(current(), first);
    pick(&mut d, "512");
    let mut again = lists();
    again.midi_ports.push("Example Pads".into());
    d.set_lists(again);
    assert_eq!(d.draft().buffer_frames, 512);
    assert_eq!(
        labels(&d, "MIDI inputs"),
        ["Example Keyboard", "Example Pads"]
    );
    assert_eq!(d.lists().stream_notes, ["Buffer 256 frames"]);
}
#[test]
fn missing_saved_values_are_still_offered_and_marked() {
    let d = SettingsDialog::new(
        AudioSettings {
            host: HostChoice::Alsa,
            output_device: Some("Gone".into()),
            buffer_frames: 300,
            midi_inputs: vec![MidiInputSetting::enabled("Old Keys")],
            workers: 5,
            worker_linger_spins: 7,
            metronome_gain: 0.5,
            ..AudioSettings::default()
        },
        lists(),
    );
    assert!(labels(&d, "Audio host").contains(&"ALSA (not available)".to_string()));
    assert!(labels(&d, "Output device").contains(&"Gone (not found)".to_string()));
    assert!(labels(&d, "Buffer size (frames)").contains(&"300".to_string()));
    assert!(labels(&d, "MIDI inputs").contains(&"Old Keys (not found)".to_string()));
    assert!(labels(&d, "Worker threads").contains(&"5".to_string()));
    assert!(labels(&d, "Worker linger (spins)").contains(&"7".to_string()));
}

#[test]
fn choosing_options_edits_the_draft_and_apply_returns_it() {
    let mut d = SettingsDialog::new(current(), lists());
    assert_eq!(pick(&mut d, "512"), DialogOutcome::Open);
    assert_eq!(pick(&mut d, "Example Keyboard"), DialogOutcome::Open);
    assert_eq!(pick(&mut d, "4"), DialogOutcome::Open);
    assert_eq!(pick(&mut d, "Host default"), DialogOutcome::Open);
    let apply = d.layout(VIEW).apply.center();
    let expected = AudioSettings {
        buffer_frames: 512,
        midi_inputs: vec![MidiInputSetting::enabled("Example Keyboard")],
        workers: 4,
        output_device: None,
        ..current()
    };
    assert_eq!(click(&mut d, apply), DialogOutcome::Apply(expected));
}

#[test]
fn switching_host_lists_its_devices_and_drops_a_foreign_device() {
    let mut d = SettingsDialog::new(current(), lists());
    pick(&mut d, "ASIO");
    assert_eq!(d.draft().host, HostChoice::Asio);
    assert_eq!(d.draft().output_device, None);
    assert_eq!(
        labels(&d, "Output device"),
        ["Host default", "Example USB ASIO"]
    );
    pick(&mut d, "Example USB ASIO");
    assert_eq!(d.draft().output_device.as_deref(), Some("Example USB ASIO"));
}

#[test]
fn escape_and_cancel_discard_enter_applies() {
    let mut d = SettingsDialog::new(current(), lists());
    pick(&mut d, "1024");
    assert_eq!(d.handle(&key(Key::Escape), VIEW), DialogOutcome::Cancel);
    let mut d = SettingsDialog::new(current(), lists());
    let cancel = d.layout(VIEW).cancel.center();
    assert_eq!(click(&mut d, cancel), DialogOutcome::Cancel);
    let mut d = SettingsDialog::new(current(), lists());
    pick(&mut d, "1024");
    assert_eq!(
        d.handle(&key(Key::Enter), VIEW),
        DialogOutcome::Apply(AudioSettings {
            buffer_frames: 1024,
            ..current()
        })
    );
}

#[test]
fn clicks_outside_the_panel_change_nothing() {
    let mut d = SettingsDialog::new(current(), lists());
    let panel = d.layout(VIEW).panel;
    assert_eq!(
        click(&mut d, Point::new(panel.x0 - 5.0, 5.0)),
        DialogOutcome::Open
    );
    assert_eq!(d.draft(), &current());
}

#[test]
fn a_tall_dialog_scrolls_within_bounds() {
    let mut many = lists();
    many.hosts[0].devices = (0..60).map(|i| device(&format!("Out {i}"), 2)).collect();
    let mut d = SettingsDialog::new(current(), many);
    let l = d.layout(VIEW);
    assert!(l.content_height > l.content.height());
    assert!(l.panel.y1 <= VIEW.height);
    let first = l.options[0].0;
    let wheel = |dy| UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
        pos: l.content.center(),
        modifiers: Modifiers::NONE,
    };
    assert_eq!(d.handle(&wheel(-3.0), VIEW), DialogOutcome::Open);
    let scrolled = d.layout(VIEW).options[0].0;
    assert!(scrolled.y0 < first.y0, "wheel down scrolls the content up");
    d.handle(&wheel(1000.0), VIEW);
    assert_eq!(d.layout(VIEW).options[0].0, first, "clamped at the top");
    d.handle(&wheel(-1000.0), VIEW);
    let l = d.layout(VIEW);
    let last = l.options.last().unwrap().0;
    assert!(last.y1 <= l.content.y1 + 0.5, "clamped at the bottom");
}

#[test]
fn options_and_buttons_do_not_overlap() {
    let d = SettingsDialog::new(current(), lists());
    // Tall enough that nothing scrolls: the dialog scrolls its sections in a shorter window.
    let l = d.layout(Size::new(1000.0, 1200.0));
    for (i, (a, ..)) in l.options.iter().enumerate() {
        assert!(l.panel.contains(a.center()));
        for (b, ..) in &l.options[i + 1..] {
            assert!(a.intersect(*b).area() <= 0.0, "{a:?} overlaps {b:?}");
        }
    }
    assert!(l.apply.intersect(l.cancel).area() <= 0.0);
}

#[test]
fn painting_shows_titles_options_buttons_and_notes() {
    let mut l = lists();
    l.errors.push("MIDI ports: backend down".into());
    let d = SettingsDialog::new(current(), l);
    let mut list = DisplayList::new();
    d.paint(&mut list, &Theme::dark(), VIEW);
    let texts = list.texts();
    for t in [
        "Audio settings",
        "Audio host",
        "WASAPI",
        "Apply",
        "Cancel",
        "Example Keyboard",
    ] {
        assert!(texts.contains(&t), "missing {t}");
    }
    assert!(texts.iter().any(|t| t.contains("backend down")));
    assert!(texts.iter().any(|t| t.contains("selected device")));
}

#[test]
fn the_input_device_and_the_record_offset_are_chosen_like_the_other_options() {
    let mut d = SettingsDialog::new(current(), lists());
    pick(&mut d, "Microphone");
    assert_eq!(d.draft().input_device.as_deref(), Some("Microphone"));
    assert!(d.is_selected(&Choice::Input(Some("Microphone".into()))));
    pick(&mut d, "Same as the output device");
    assert_eq!(d.draft().input_device, None);
    // The offsets are at the bottom of the scrolling list: choose without a click.
    d.choose(Choice::RecordOffset(-96));
    assert_eq!(d.draft().record_offset_frames, -96);
    let out = d.handle(&key(Key::Enter), VIEW);
    let DialogOutcome::Apply(s) = out else {
        panic!("apply");
    };
    assert_eq!(s.record_offset_frames, -96);
}

#[test]
fn a_saved_input_device_that_is_gone_and_an_unusual_offset_stay_selectable() {
    let d = SettingsDialog::new(
        AudioSettings {
            input_device: Some("Old Interface".into()),
            record_offset_frames: 133,
            ..current()
        },
        lists(),
    );
    assert!(labels(&d, "Input device").contains(&"Old Interface (not found)".to_string()));
    assert!(labels(&d, "Record offset (frames)").contains(&"133".to_string()));
    assert!(d.is_selected(&Choice::RecordOffset(133)));
}

#[test]
fn changing_the_host_drops_an_input_device_the_new_host_does_not_have() {
    let mut d = SettingsDialog::new(
        AudioSettings {
            host: HostChoice::Wasapi,
            input_device: Some("Microphone".into()),
            ..current()
        },
        lists(),
    );
    pick(&mut d, "ASIO");
    assert_eq!(d.draft().input_device, None);
}

#[test]
fn the_dialog_shows_how_many_input_channels_are_open() {
    let mut l = lists();
    l.input_note = Some("Input: 1 of 2 channels open (Example USB ASIO)".into());
    let d = SettingsDialog::new(current(), l);
    let notes: Vec<String> = d.layout(VIEW).notes.into_iter().map(|(_, n)| n).collect();
    assert!(
        notes.iter().any(|n| n.contains("1 of 2 channels open")),
        "{notes:?}"
    );
    // Without a note nothing about inputs is shown.
    let d = SettingsDialog::new(current(), lists());
    assert!(
        !d.layout(VIEW)
            .notes
            .iter()
            .any(|(_, n)| n.starts_with("Input:"))
    );
}

#[test]
fn the_pre_count_is_a_chip_row_that_edits_the_draft() {
    let mut d = SettingsDialog::new(current(), lists());
    assert!(d.is_selected(&Choice::PreCount(0)));
    // At the bottom of the scrolling list: choose without a click.
    d.choose(Choice::PreCount(2));
    assert!(d.is_selected(&Choice::PreCount(2)));
    assert!(!d.is_selected(&Choice::PreCount(0)));
    let apply = d.layout(VIEW).apply.center();
    let DialogOutcome::Apply(s) = click(&mut d, apply) else {
        panic!("apply");
    };
    assert_eq!(s.pre_count_bars, 2);
}

#[test]
fn the_device_loss_choice_is_edited_and_the_stream_notes_are_listed() {
    let mut d = SettingsDialog::new(current(), lists());
    assert_eq!(
        labels(&d, "If the audio device is lost"),
        ["Offer a restart", "Switch to the default devices"]
    );
    assert!(d.is_selected(&Choice::LossReset(false)));
    d.choose(Choice::LossReset(true));
    assert!(d.draft().device_loss.reset_to_default);
    assert!(d.is_selected(&Choice::LossReset(true)));
    assert!(!d.is_selected(&Choice::LossReset(false)));
    let mut with_notes = lists();
    with_notes.stream_notes = vec![
        "Buffer 256 frames (5.3 ms).".into(),
        "Warning: drift".into(),
    ];
    let notes = SettingsDialog::new(current(), with_notes).notes();
    assert!(notes.contains(&"Buffer 256 frames (5.3 ms).".to_string()));
    assert!(notes.contains(&"Warning: drift".to_string()));
}
