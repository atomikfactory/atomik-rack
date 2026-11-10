//! The audio and MIDI setup panel: tones, meters, texts, pages, picks and what is drawn.

use super::*;
use crate::audio_setup_view::{Kind, SetupOutcome};
use crate::settings::HostChoice;
use atomik_audio_io::{DeviceInfo, HostInfo};
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton, Theme, UiEvent};

const VIEW: Size = Size::new(1000.0, 800.0);

fn device(name: &str, inputs: u16, outputs: u16) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        max_inputs: inputs,
        max_outputs: outputs,
        sample_rates: vec![48_000],
        buffer_range: None,
        is_default_output: false,
    }
}

fn lists() -> DeviceLists {
    DeviceLists {
        hosts: vec![HostInfo {
            // A fixed host, so the pages and goldens do not depend on the OS running the tests.
            kind: HostChoice::Wasapi.kind(),
            devices: vec![
                device("Speakers", 0, 2),
                device("Headset", 1, 2),
                device("Microphone", 2, 0),
            ],
        }],
        midi_ports: vec!["Keys".into(), "Pads".into()],
        midi_outputs: Vec::new(),
        errors: Vec::new(),
        input_note: None,
        stream_notes: Vec::new(),
    }
}

fn panel() -> AudioSetup {
    AudioSetup::new(AudioSettings::default(), lists())
}

fn running() -> AudioStatus {
    AudioStatus {
        host: "Test".into(),
        device: "Speakers".into(),
        sample_rate: 48_000,
        buffer_frames: Some(480),
        running: true,
        ..AudioStatus::default()
    }
}

fn click(s: &mut AudioSetup, kind: &Kind) -> SetupOutcome {
    let at = s
        .center_of(VIEW, kind)
        .unwrap_or_else(|| panic!("{kind:?} is on the page"));
    s.handle(
        &UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        VIEW,
    )
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn the_test_tone_is_a_one_second_chord_at_minus_18_dbfs() {
    let tone = chord_tone();
    assert_eq!(tone.len(), TONE_RATE as usize);
    let db = 20.0 * peak(&tone).log10();
    assert!((db - TONE_LEVEL_DB).abs() < 0.01, "{db} dBFS");
    assert!(tone[0].abs() < 1e-3, "it starts from silence");
    assert!(tone[tone.len() - 1].abs() < 1e-3, "it ends in silence");
    // A chord, not one pitch: the energy is not a single sine (its zero crossings are irregular).
    let rising: Vec<usize> = (1..tone.len())
        .filter(|&i| tone[i - 1] < 0.0 && tone[i] >= 0.0)
        .collect();
    let gaps: std::collections::BTreeSet<usize> = rising.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() > 2, "{gaps:?}");
}

#[test]
fn the_tone_of_a_key_follows_its_pitch_and_is_as_soft_as_the_chord() {
    let a4 = key_tone(69);
    assert_eq!(a4.len(), (KEY_SECONDS * TONE_RATE as f32) as usize);
    assert!((20.0 * peak(&a4).log10() - TONE_LEVEL_DB).abs() < 0.01);
    let crossings = |tone: &[f32]| {
        (1..tone.len())
            .filter(|&i| tone[i - 1] < 0.0 && tone[i] >= 0.0)
            .count()
    };
    let (a5, a4_count) = (crossings(&key_tone(81)), crossings(&a4));
    assert!(
        a5 > a4_count * 3 / 2,
        "an octave up has about twice the cycles ({a5} against {a4_count})"
    );
    // The top of the keyboard stays below the Nyquist frequency.
    assert!(peak(&key_tone(127)) > 0.0);
}

#[test]
fn a_meter_jumps_up_and_falls_back_to_nothing() {
    let mut m = Meter::default();
    assert_eq!(m.fraction(), 0.0);
    m.feed(0.5);
    assert_eq!(m.level(), 0.5);
    let loud = m.fraction();
    assert!(loud > 0.85 && loud < 1.0, "-6 dB is near the top: {loud}");
    m.feed(0.0);
    assert!(m.level() < 0.5 && m.level() > 0.0);
    for _ in 0..200 {
        m.feed(0.0);
    }
    assert_eq!(m.level(), 0.0);
    assert_eq!(m.fraction(), 0.0);
    m.feed(f32::NAN);
    assert_eq!(m.level(), 0.0, "NaN is silence");
}

#[test]
fn the_latency_is_the_reported_figure_or_an_estimate_from_the_buffer() {
    let mut s = running();
    assert!(
        latency_text(&s).contains("an estimate"),
        "{}",
        latency_text(&s)
    );
    assert!(latency_text(&s).contains("10.0 ms"), "480 frames at 48 kHz");
    s.output_latency_frames = 960;
    let text = latency_text(&s);
    assert!(
        text.contains("20.0 ms") && !text.contains("estimate"),
        "{text}"
    );
    s.running = false;
    assert!(latency_text(&s).contains("not measured"));
    s.running = true;
    s.output_latency_frames = 0;
    s.buffer_frames = None;
    assert!(latency_text(&s).contains("not measured yet"));
}

#[test]
fn a_failed_open_says_why_and_points_to_another_device() {
    let mut s = running();
    assert_eq!(output_problem(&s, &lists()), None);
    s.running = false;
    s.error = Some("device busy".into());
    let text = output_problem(&s, &lists()).unwrap();
    assert!(text.contains("device busy") && text.contains("another device"));
    s.error = None;
    assert!(
        output_problem(&s, &lists())
            .unwrap()
            .contains("Sound is off")
    );
    let none = DeviceLists {
        hosts: Vec::new(),
        ..lists()
    };
    assert!(
        output_problem(&s, &none)
            .unwrap()
            .contains("No sound output was found")
    );
}

#[test]
fn the_pages_go_forward_and_back_and_the_last_one_says_done() {
    let mut s = panel();
    assert_eq!(s.page(), Page::Output);
    assert!(
        s.center_of(VIEW, &Kind::Back).is_none(),
        "no Back on page 1"
    );
    assert!(s.center_of(VIEW, &Kind::Done).is_none());
    assert_eq!(click(&mut s, &Kind::Next), SetupOutcome::Open);
    assert_eq!(s.page(), Page::Input);
    assert_eq!(click(&mut s, &Kind::Next), SetupOutcome::Open);
    assert_eq!(s.page(), Page::Midi);
    assert!(s.is_last());
    assert!(s.center_of(VIEW, &Kind::Next).is_none());
    assert_eq!(click(&mut s, &Kind::Back), SetupOutcome::Open);
    assert_eq!(s.page(), Page::Input);
    click(&mut s, &Kind::Next);
    assert_eq!(click(&mut s, &Kind::Done), SetupOutcome::Done);
}

#[test]
fn skip_is_on_every_page_and_so_are_escape_and_enter() {
    for page in Page::ALL {
        let mut s = panel();
        for _ in 0..page.index() {
            s.next();
        }
        assert_eq!(click(&mut s, &Kind::Skip), SetupOutcome::Skip, "{page:?}");
        assert_eq!(s.handle(&key(Key::Escape), VIEW), SetupOutcome::Skip);
    }
    let mut s = panel();
    assert_eq!(s.handle(&key(Key::Enter), VIEW), SetupOutcome::Open);
    assert_eq!(s.page(), Page::Input);
    s.next();
    assert_eq!(s.handle(&key(Key::Enter), VIEW), SetupOutcome::Done);
}

#[test]
fn the_pages_list_what_the_audio_settings_list() {
    let mut s = panel();
    let labels = |s: &AudioSetup| -> Vec<String> {
        s.options().into_iter().map(|(label, _)| label).collect()
    };
    let output = labels(&s);
    assert!(output.contains(&"Host default".to_owned()));
    assert!(output.contains(&"Speakers".to_owned()) && output.contains(&"Headset".to_owned()));
    assert!(
        !output.contains(&"Microphone".to_owned()),
        "an input-only device is no output"
    );
    for b in ["64", "256", "2048"] {
        assert!(output.contains(&b.to_owned()), "buffer {b}");
    }
    assert!(!output.contains(&"Type a size".to_owned()));
    s.next();
    let input = labels(&s);
    assert!(input.contains(&"Same as the output device".to_owned()));
    assert!(input.contains(&"Microphone".to_owned()) && input.contains(&"Headset".to_owned()));
    assert!(!input.contains(&"Speakers".to_owned()));
    s.next();
    assert_eq!(labels(&s), ["Keys", "Pads"]);
}

#[test]
fn the_first_page_has_the_hint_the_latency_and_the_test_button() {
    let mut s = panel();
    s.update(&running(), None, (0, None));
    let lines = s.lines(VIEW);
    assert!(lines.contains(&"smaller = less delay, more CPU".to_owned()));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Output latency: about 10.0 ms"))
    );
    assert!(lines.contains(&"Test".to_owned()));
    assert!(lines.contains(&"Step 1 of 3: Sound output".to_owned()));
    assert_eq!(click(&mut s, &Kind::Test), SetupOutcome::Test);
    assert!(s.tested);
}

#[test]
fn picks_are_saved_in_the_audio_settings_and_round_trip() {
    let mut s = panel();
    let out = click(&mut s, &Kind::Pick(Choice::Buffer(512)));
    assert_eq!(out, SetupOutcome::Pick(Choice::Buffer(512)));
    // The click only asks; the shell makes the pick the value of the draft.
    assert_eq!(s.draft().buffer_frames, 256);
    s.choose(Choice::Buffer(512));
    s.choose(Choice::Device(Some("Headset".into())));
    s.next();
    s.choose(Choice::Input(Some("Microphone".into())));
    s.next();
    s.choose(Choice::MidiInput("Keys".into()));
    assert!(s.is_selected(&Choice::MidiInput("Keys".into())));
    assert!(!s.is_selected(&Choice::MidiInput("Pads".into())));
    let saved = AudioSettings::from_json(&s.draft().to_json()).unwrap();
    assert_eq!(saved.buffer_frames, 512);
    assert_eq!(saved.output_device.as_deref(), Some("Headset"));
    assert_eq!(saved.input_device.as_deref(), Some("Microphone"));
    assert_eq!(saved.enabled_midi_inputs().count(), 1);
    assert_eq!(saved.enabled_midi_inputs().next().unwrap().name, "Keys");
    // A second click on the same port switches it off again.
    s.choose(Choice::MidiInput("Keys".into()));
    assert_eq!(s.draft().enabled_midi_inputs().count(), 0);
    assert_eq!(
        s.opened_with().buffer_frames,
        256,
        "the start is remembered"
    );
}

#[test]
fn a_note_lights_the_lamp_and_asks_for_its_tone() {
    let mut s = panel();
    s.next();
    s.next();
    // Notes counted before the panel listened do not count.
    s.start_listening((5, Some(40)));
    let quiet = s.update(&running(), None, (5, Some(40)));
    assert_eq!(quiet.new_key, None);
    assert!(!s.lamp_lit());
    assert!(
        s.lines(VIEW)
            .contains(&"Play a key on your keyboard.".to_owned())
    );
    let heard = s.update(&running(), None, (6, Some(60)));
    assert_eq!(heard.new_key, Some(60));
    assert!(heard.changed && s.lamp_lit());
    assert!(
        s.lines(VIEW).contains(&"Note received: C3".to_owned()),
        "{:?}",
        s.lines(VIEW)
    );
    for _ in 0..LAMP_FRAMES {
        s.update(&running(), None, (6, Some(60)));
    }
    assert!(!s.lamp_lit(), "the lamp goes out");
    assert!(s.lines(VIEW).contains(&"Last note: C3".to_owned()));
}

#[test]
fn the_meters_follow_the_peaks_of_the_stream() {
    let mut s = panel();
    let update = s.update(&running(), Some((0.0, 0.1)), (0, None));
    assert!(update.changed);
    assert!(s.out_meter.fraction() > 0.5);
    assert_eq!(s.in_meter.level(), 0.0);
    let again = s.update(&running(), Some((0.0, 0.0)), (0, None));
    assert!(again.changed, "it is falling");
    for _ in 0..200 {
        s.update(&running(), Some((0.0, 0.0)), (0, None));
    }
    let still = s.update(&running(), Some((0.0, 0.0)), (0, None));
    assert!(
        !still.changed,
        "a silent, settled panel asks for no repaint"
    );
    // Without an engine there are no peaks at all.
    s.update(&running(), None, (0, None));
    assert_eq!(s.out_meter.level(), 0.0);
}

#[test]
fn the_input_page_offers_to_turn_the_input_on_only_when_the_project_has_none() {
    let mut s = panel();
    s.next();
    s.update(&running(), None, (0, None));
    assert!(s.input_unwanted);
    assert_eq!(click(&mut s, &Kind::TurnOnInput), SetupOutcome::TurnOnInput);
    let mut with_input = running();
    with_input.input_channels_wanted = 2;
    with_input.input_channels_open = 2;
    with_input.input_device = Some("Headset".into());
    s.update(&with_input, Some((0.2, 0.0)), (0, None));
    assert!(s.center_of(VIEW, &Kind::TurnOnInput).is_none());
    assert!(s.input_open);
    assert!(s.in_meter.fraction() > 0.5);
    assert!(
        s.lines(VIEW)
            .iter()
            .any(|l| l.contains("Input: 2 channels open (Headset)"))
    );
}

#[test]
fn the_midi_page_has_the_controller_wizard_button_and_says_when_there_is_no_port() {
    let mut s = panel();
    s.next();
    s.next();
    assert_eq!(
        click(&mut s, &Kind::SetUpController),
        SetupOutcome::SetUpController
    );
    assert!(s.lines(VIEW).contains(&"Set Up Controller...".to_owned()));
    let empty = AudioSetup::new(
        AudioSettings::default(),
        DeviceLists {
            midi_ports: Vec::new(),
            ..lists()
        },
    );
    let mut empty = empty;
    empty.next();
    empty.next();
    assert!(
        empty
            .lines(VIEW)
            .iter()
            .any(|l| l.starts_with("No MIDI input found"))
    );
}

#[test]
fn a_long_list_scrolls_with_the_wheel() {
    let mut many = lists();
    many.hosts[0].devices = (0..12)
        .map(|i| device(&format!("Card {i}"), 0, 2))
        .collect();
    let mut s = AudioSetup::new(AudioSettings::default(), many);
    let shown = |s: &AudioSetup| -> Vec<String> {
        s.widgets(VIEW)
            .into_iter()
            .filter(|w| matches!(w.kind, Kind::Pick(Choice::Device(_))))
            .map(|w| w.label)
            .collect()
    };
    let first = shown(&s);
    assert_eq!(first.len(), 5);
    assert_eq!(first[0], "Host default");
    let wheel = |dy: f64| UiEvent::Wheel {
        delta: Vec2::new(0.0, dy),
        pos: Point::new(500.0, 400.0),
        modifiers: Modifiers::NONE,
    };
    s.handle(&wheel(-3.0), VIEW);
    s.handle(&wheel(-3.0), VIEW);
    assert_eq!(shown(&s)[0], "Card 1");
    for _ in 0..40 {
        s.handle(&wheel(-1.0), VIEW);
    }
    assert_eq!(shown(&s).last().unwrap(), "Card 11", "it stops at the end");
    for _ in 0..40 {
        s.handle(&wheel(1.0), VIEW);
    }
    assert_eq!(shown(&s)[0], "Host default");
}

#[test]
fn a_small_window_shrinks_the_panel_and_keeps_every_button_inside_it() {
    let small = Size::new(500.0, 420.0);
    for page in Page::ALL {
        let mut s = panel();
        for _ in 0..page.index() {
            s.next();
        }
        let widgets = s.widgets(small);
        assert!(!widgets.is_empty());
        for w in widgets {
            if matches!(w.kind, Kind::Skip | Kind::Next | Kind::Done | Kind::Back) {
                assert!(
                    w.rect.x0 >= 0.0 && w.rect.x1 <= small.width && w.rect.y1 <= small.height,
                    "{:?} {:?} on {page:?}",
                    w.kind,
                    w.rect
                );
            }
        }
    }
}

fn check_golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
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
        "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
        path.display()
    );
}

/// The page `page` as the first start shows it: audio running, the tone just tested, a note just
/// played.
fn golden(name: &str, page: Page, theme: &Theme) {
    let mut s = panel();
    s.choose(Choice::Device(Some("Speakers".into())));
    s.choose(Choice::MidiInput("Keys".into()));
    for _ in 0..page.index() {
        s.next();
    }
    s.tested = true;
    s.start_listening((0, None));
    s.update(&running(), Some((0.0, 0.1)), (1, Some(64)));
    let mut list = DisplayList::new();
    s.paint(&mut list, theme, VIEW);
    check_golden(name, &list.to_json());
}

#[test]
fn golden_audio_setup_output_dark() {
    golden("audio_setup_output", Page::Output, &Theme::dark());
}

#[test]
fn golden_audio_setup_input_dark() {
    golden("audio_setup_input", Page::Input, &Theme::dark());
}

#[test]
fn golden_audio_setup_midi_dark() {
    golden("audio_setup_midi", Page::Midi, &Theme::dark());
}

#[test]
fn golden_audio_setup_output_light() {
    golden("audio_setup_output_light", Page::Output, &Theme::light());
}
