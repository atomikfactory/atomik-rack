//! TransportBar behaviour (UI): buttons, readouts through the tempo map, tempo typing,
//! and the shared transport shortcuts.

mod support;

use atomik_model::{SongCommand, TempoEvent};
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
    doc.song.tempo = vec![
        TempoEvent {
            tick: 0,
            bpm: 120.0,
        },
        TempoEvent {
            tick: BAR,
            bpm: 60.0,
        },
    ];
    let mut v = transport_bar(&doc, &reg);
    v.set_state(TransportState {
        position: BAR + PPQ + PPQ / 2,
        ..TransportState::default()
    });
    assert_eq!(
        v.position_text(),
        ("2.2.07680".to_string(), "00:03.500".to_string())
    );
    doc.song.beats_per_bar = 3;
    v.sync(&doc, &reg);
    assert_eq!(v.position_text().0, "2.3.07680");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    for t in ["2.3.07680", "120.00", "3/4"] {
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
    doc.song.tempo.push(TempoEvent {
        tick: 8 * BAR,
        bpm: 90.0,
    });
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
            TempoEvent { tick: 0, bpm: 98.5 },
            TempoEvent {
                tick: 8 * BAR,
                bpm: 90.0
            }
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
