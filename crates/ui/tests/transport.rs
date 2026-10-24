//! TransportBar behaviour (UI): buttons, readouts through the tempo map, tempo and meter
//! typing, and the shared transport shortcuts.

mod support;

use atomik_model::{MeterEvent, SongCommand, TempoEvent};
use atomik_ui::kurbo::Vec2;
use atomik_ui::{
    DisplayList, Key, Modifiers, TransportAction, TransportButton, TransportState, UiAction,
    UiEvent, transport_shortcut,
};
use support::song::*;
use support::*;

#[test]
fn buttons_emit_transport_actions() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    for (b, expected) in [
        (TransportButton::ReturnToZero, TransportAction::ReturnToZero),
        (TransportButton::Stop, TransportAction::Stop),
        (TransportButton::Play, TransportAction::Play),
        (TransportButton::Record, TransportAction::ToggleRecord),
        (TransportButton::Loop, TransportAction::ToggleLoop),
        (TransportButton::Metronome, TransportAction::ToggleMetronome),
    ] {
        let p = v.button_center(b);
        let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
        assert_eq!(a, vec![UiAction::Transport(expected)], "{b:?}");
    }
}

#[test]
fn position_readouts_follow_the_tempo_map_and_meter() {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.tempo = vec![TempoEvent::step(0, 120.0), TempoEvent::step(BAR, 60.0)];
    let mut v = transport_bar(&doc, &reg);
    v.set_state(TransportState {
        position: BAR + PPQ + PPQ / 2,
        ..TransportState::default()
    });
    assert_eq!(
        v.position_text(),
        ("2.2.07680".to_string(), "00:03.500".to_string())
    );
    doc.song.meter[0].num = 3;
    v.sync(&doc, &reg);
    assert_eq!(v.position_text().0, "2.3.07680");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    // The tempo shown is the one at the playhead (the second event: 60 BPM).
    for t in ["2.3.07680", "60.00", "3/4"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
}

fn tempo_maps(actions: &[UiAction]) -> Vec<Vec<TempoEvent>> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetTempoMap(m) => Some(m),
            _ => None,
        })
        .collect()
}

#[test]
fn double_click_types_a_tempo_and_keeps_later_events() {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.tempo.push(TempoEvent::step(8 * BAR, 90.0));
    let mut v = transport_bar(&doc, &reg);
    let t = v.tempo_center();
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(t, Modifiers::NONE, 2), up(t)],
    );
    assert!(a.is_empty());
    assert!(v.is_editing_tempo());
    assert_eq!(v.tempo_text(), Some("120"));
    let typed: Vec<UiEvent> = "98.5"
        .chars()
        .map(|c| press(Key::Char(c), Modifiers::NONE))
        .collect();
    let a = run(|e| v.handle(e, &doc, &reg), &typed);
    assert!(
        a.is_empty(),
        "typing is local; Space/L/R do not leak to the transport"
    );
    assert_eq!(v.tempo_text(), Some("98.5"));
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    assert_eq!(
        tempo_maps(&a),
        vec![vec![
            TempoEvent::step(0, 98.5),
            TempoEvent::step(8 * BAR, 90.0)
        ]]
    );
    assert!(!v.is_editing_tempo());
}

#[test]
fn tempo_entry_clamps_cancels_and_ignores_no_ops() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let t = v.tempo_center();
    let edit = |v: &mut atomik_ui::TransportBar, text: &str, end: Key| {
        let mut ev = vec![down_mods(t, Modifiers::NONE, 2), up(t)];
        ev.extend(text.chars().map(|c| press(Key::Char(c), Modifiers::NONE)));
        ev.push(press(end, Modifiers::NONE));
        run(|e| v.handle(e, &doc, &reg), &ev)
    };
    let a = edit(&mut v, "5000", Key::Enter);
    assert_eq!(tempo_maps(&a)[0][0].bpm, 999.0);
    let a = edit(&mut v, "88", Key::Escape);
    assert!(a.is_empty() && !v.is_editing_tempo());
    let a = edit(&mut v, "120", Key::Enter);
    assert!(a.is_empty(), "same tempo: no command");
    let a = edit(&mut v, "..", Key::Enter);
    assert!(a.is_empty(), "unparsable: no command");
    // The wheel nudges the tempo by 1 BPM (Shift: 0.1).
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: t,
            modifiers: Modifiers::SHIFT,
        },
        &doc,
        &reg,
    );
    assert!((tempo_maps(&a)[0][0].bpm - 120.1).abs() < 1e-9);
}

fn meter_maps(actions: &[UiAction]) -> Vec<Vec<MeterEvent>> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(atomik_model::Command::Song(SongCommand::SetMeterMap(m))) => {
                Some(m.clone())
            }
            _ => None,
        })
        .collect()
}

fn type_meter(
    v: &mut atomik_ui::TransportBar,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    text: &str,
    end: Key,
) -> Vec<UiAction> {
    let c = v.meter_center();
    let mut ev = vec![down_mods(c, Modifiers::NONE, 2), up(c)];
    ev.extend(text.chars().map(|ch| press(Key::Char(ch), Modifiers::NONE)));
    ev.push(press(end, Modifiers::NONE));
    run(|e| v.handle(e, doc, reg), &ev)
}

#[test]
fn double_click_types_a_signature_and_keeps_later_changes() {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.meter.push(MeterEvent {
        bar: 5,
        num: 7,
        den: 8,
    });
    let mut v = transport_bar(&doc, &reg);
    let c = v.meter_center();
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(c, Modifiers::NONE, 2), up(c)],
    );
    assert!(a.is_empty() && v.is_editing_meter() && !v.is_editing_tempo());
    assert_eq!(v.meter_text(), Some("4/4"));
    let typed: Vec<UiEvent> = "3/4"
        .chars()
        .map(|c| press(Key::Char(c), Modifiers::NONE))
        .collect();
    let a = run(|e| v.handle(e, &doc, &reg), &typed);
    assert!(a.is_empty(), "typing is local");
    assert_eq!(v.meter_text(), Some("3/4"));
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    assert_eq!(
        meter_maps(&a),
        vec![vec![
            MeterEvent {
                bar: 1,
                num: 3,
                den: 4
            },
            MeterEvent {
                bar: 5,
                num: 7,
                den: 8
            }
        ]]
    );
    assert!(!v.is_editing_meter());
}

#[test]
fn the_signature_typed_goes_to_the_change_at_the_playhead() {
    let (mut doc, reg) = (song_doc(), registry());
    doc.song.meter.push(MeterEvent {
        bar: 5,
        num: 7,
        den: 8,
    });
    let mut v = transport_bar(&doc, &reg);
    v.set_state(TransportState {
        position: 5 * BAR,
        ..TransportState::default()
    });
    let a = type_meter(&mut v, &doc, &reg, "5/16", Key::Enter);
    let maps = meter_maps(&a);
    assert_eq!(maps.len(), 1);
    assert_eq!((maps[0][0].num, maps[0][0].den), (4, 4));
    assert_eq!(
        maps[0][1],
        MeterEvent {
            bar: 5,
            num: 5,
            den: 16
        }
    );
}

#[test]
fn signature_entry_rejects_bad_values_cancels_and_ignores_no_ops() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    for bad in ["0/4", "33/4", "4/3", "4/", "/4", "//", "4/32"] {
        let a = type_meter(&mut v, &doc, &reg, bad, Key::Enter);
        assert!(a.is_empty(), "{bad:?}");
        assert!(!v.is_editing_meter());
    }
    let a = type_meter(&mut v, &doc, &reg, "6/8", Key::Escape);
    assert!(a.is_empty() && !v.is_editing_meter());
    let a = type_meter(&mut v, &doc, &reg, "4/4", Key::Enter);
    assert!(a.is_empty(), "same signature: no command");
    // A bare number keeps the beat unit.
    let a = type_meter(&mut v, &doc, &reg, "5", Key::Enter);
    assert_eq!(meter_maps(&a)[0][0].num, 5);
    assert_eq!(meter_maps(&a)[0][0].den, 4);
    // Letters and Space are not typed and do not leak to the transport.
    let a = type_meter(&mut v, &doc, &reg, "3x/4 ", Key::Enter);
    assert_eq!(meter_maps(&a)[0][0].num, 3);
}

#[test]
fn the_signature_being_typed_is_painted() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let c = v.meter_center();
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(c, Modifiers::NONE, 2),
            up(c),
            press(Key::Char('6'), Modifiers::NONE),
        ],
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"6_"), "{:?}", dl.texts());
}

#[test]
fn shortcuts() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let a = v.handle(&press(Key::Space, Modifiers::NONE), &doc, &reg);
    assert_eq!(a, vec![UiAction::Transport(TransportAction::TogglePlay)]);
    assert_eq!(
        transport_shortcut(&Key::Enter, Modifiers::NONE),
        Some(TransportAction::ReturnToZero)
    );
    assert_eq!(
        transport_shortcut(&Key::Char('L'), Modifiers::SHIFT),
        Some(TransportAction::ToggleLoop)
    );
    assert_eq!(
        transport_shortcut(&Key::Char('r'), Modifiers::NONE),
        Some(TransportAction::ToggleRecord)
    );
    assert_eq!(transport_shortcut(&Key::Char('r'), Modifiers::CTRL), None);
    assert_eq!(transport_shortcut(&Key::Char('x'), Modifiers::NONE), None);
}
