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

// ---------------------------------------------------------------- drag and wheel on the readouts

use atomik_ui::kurbo::Point;

fn wheel(pos: Point, dy: f64, modifiers: Modifiers) -> UiEvent {
    UiEvent::Wheel {
        delta: Vec2::new(0.0, dy),
        pos,
        modifiers,
    }
}

/// The tempo maps and merge ids of the drag commands among `actions`.
fn drag_tempos(actions: &[UiAction]) -> Vec<(f64, u64)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragTempoMap { map, merge } => Some((map[0].bpm, merge)),
            _ => None,
        })
        .collect()
}

fn drag_meters(actions: &[UiAction]) -> Vec<((u8, u8), u64)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::DragMeterMap { map, merge } => Some(((map[0].num, map[0].den), merge)),
            _ => None,
        })
        .collect()
}

fn ends(actions: &[UiAction]) -> usize {
    actions.iter().filter(|a| **a == UiAction::EndMerge).count()
}

/// Presses at `from`, moves to each `ys` (same x), releases at the last.
fn drag_y(
    v: &mut atomik_ui::TransportBar,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    from: Point,
    ys: &[f64],
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut ev = vec![down(from)];
    ev.extend(
        ys.iter()
            .map(|y| mv_with(Point::new(from.x, *y), modifiers)),
    );
    ev.push(up(Point::new(from.x, *ys.last().unwrap_or(&from.y))));
    run(|e| v.handle(e, doc, reg), &ev)
}

#[test]
fn dragging_the_tempo_up_steps_whole_bpm_live_in_one_merge_group() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let p = v.tempo_center();
    let a = drag_y(
        &mut v,
        &doc,
        &reg,
        p,
        &[p.y - 5.0, p.y - 10.0, p.y - 20.0],
        Modifiers::NONE,
    );
    let d = drag_tempos(&a);
    assert_eq!(
        d.iter().map(|x| x.0).collect::<Vec<_>>(),
        [121.0, 122.0, 124.0]
    );
    assert!(d.iter().all(|x| x.1 == d[0].1), "one merge id: {d:?}");
    assert_eq!(ends(&a), 1, "released once");
    assert!(tempo_maps(&a).is_empty());
}

#[test]
fn dragging_the_tempo_down_lowers_it_and_each_drag_has_its_own_group() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let p = v.tempo_center();
    let a = drag_y(&mut v, &doc, &reg, p, &[p.y + 10.0], Modifiers::NONE);
    assert_eq!(drag_tempos(&a).last().map(|x| x.0), Some(118.0));
    let b = drag_y(&mut v, &doc, &reg, p, &[p.y + 5.0], Modifiers::NONE);
    assert_ne!(drag_tempos(&a)[0].1, drag_tempos(&b)[0].1);
}

#[test]
fn the_tempo_decimals_step_by_hundredths_and_shift_steps_the_whole_part_by_tenths() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let c = v.tempo_center();
    let dec = v.tempo_decimals_point();
    let a = drag_y(&mut v, &doc, &reg, dec, &[dec.y - 15.0], Modifiers::NONE);
    assert_eq!(drag_tempos(&a).last().map(|x| x.0), Some(120.03));
    let a = drag_y(&mut v, &doc, &reg, c, &[c.y - 10.0], Modifiers::SHIFT);
    assert_eq!(drag_tempos(&a).last().map(|x| x.0), Some(120.2));
}

#[test]
fn a_tempo_drag_clamps_to_the_range_and_a_click_does_nothing() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let p = v.tempo_center();
    let a = drag_y(&mut v, &doc, &reg, p, &[p.y - 10000.0], Modifiers::NONE);
    assert_eq!(
        drag_tempos(&a).last().map(|x| x.0),
        Some(atomik_model::TEMPO_MAX)
    );
    let a = drag_y(&mut v, &doc, &reg, p, &[p.y], Modifiers::NONE);
    assert!(a.is_empty(), "a click without movement: {a:?}");
    assert!(!v.is_editing_tempo());
}

#[test]
fn the_bar_drags_while_a_readout_is_held_and_not_after() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let p = v.tempo_center();
    assert!(!v.is_dragging());
    v.handle(&down(p), &doc, &reg);
    assert!(v.is_dragging());
    v.handle(&up(p), &doc, &reg);
    assert!(!v.is_dragging());
}

#[test]
fn the_tempo_wheel_steps_the_field_under_the_pointer() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let c = v.tempo_center();
    let whole = v.handle(&wheel(c, 1.0, Modifiers::NONE), &doc, &reg);
    assert_eq!(tempo_maps(&whole)[0][0].bpm, 121.0);
    let down_one = v.handle(&wheel(c, -1.0, Modifiers::NONE), &doc, &reg);
    assert_eq!(tempo_maps(&down_one)[0][0].bpm, 119.0);
    let fine = v.handle(&wheel(c, 1.0, Modifiers::SHIFT), &doc, &reg);
    assert_eq!(tempo_maps(&fine)[0][0].bpm, 120.1);
    let dec = v.handle(
        &wheel(v.tempo_decimals_point(), 1.0, Modifiers::NONE),
        &doc,
        &reg,
    );
    assert_eq!(tempo_maps(&dec)[0][0].bpm, 120.01);
}

#[test]
fn dragging_the_signature_changes_the_field_under_the_pointer() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let num = v.meter_num_point();
    let den = v.meter_den_point();
    let a = drag_y(
        &mut v,
        &doc,
        &reg,
        num,
        &[num.y - 5.0, num.y - 10.0],
        Modifiers::NONE,
    );
    let d = drag_meters(&a);
    assert_eq!(d.iter().map(|x| x.0).collect::<Vec<_>>(), [(5, 4), (6, 4)]);
    assert!(d.iter().all(|x| x.1 == d[0].1));
    assert_eq!(ends(&a), 1);
    // The beat unit steps through 2, 4, 8, 16 and stops at the ends.
    let a = drag_y(
        &mut v,
        &doc,
        &reg,
        den,
        &[den.y - 5.0, den.y - 10.0, den.y - 15.0],
        Modifiers::NONE,
    );
    assert_eq!(
        drag_meters(&a).iter().map(|x| x.0).collect::<Vec<_>>(),
        [(4, 8), (4, 16)]
    );
    let a = drag_y(&mut v, &doc, &reg, den, &[den.y + 50.0], Modifiers::NONE);
    assert_eq!(drag_meters(&a).last().map(|x| x.0), Some((4, 2)));
    let a = drag_y(&mut v, &doc, &reg, num, &[num.y + 500.0], Modifiers::NONE);
    assert_eq!(drag_meters(&a).last().map(|x| x.0), Some((1, 4)));
}

#[test]
fn the_signature_wheel_steps_the_field_under_the_pointer() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let a = v.handle(
        &wheel(v.meter_num_point(), 1.0, Modifiers::NONE),
        &doc,
        &reg,
    );
    assert_eq!(
        meter_maps(&a)[0][0],
        MeterEvent {
            bar: 1,
            num: 5,
            den: 4
        }
    );
    let a = v.handle(
        &wheel(v.meter_den_point(), -1.0, Modifiers::NONE),
        &doc,
        &reg,
    );
    assert_eq!(
        meter_maps(&a)[0][0],
        MeterEvent {
            bar: 1,
            num: 4,
            den: 2
        }
    );
}

#[test]
fn a_double_click_still_types_after_the_drag_support() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let c = v.meter_center();
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(c, Modifiers::NONE, 2), up(c)],
    );
    assert!(a.is_empty());
    assert!(v.is_editing_meter());
}

#[test]
fn the_decimals_zone_is_as_wide_as_the_painted_decimals() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    // The readout is 92 wide with 8 px of padding; the text is laid out at 13 px.
    let right = v.tempo_center().x + 46.0 - 8.0;
    let w = atomik_ui::paint::estimate_width(".00", 13.0);
    let y = v.tempo_center().y;
    let at = |x: f64| Point::new(x, y);
    let step = |v: &mut atomik_ui::TransportBar, x: f64| {
        let a = v.handle(&wheel(at(x), 1.0, Modifiers::NONE), &doc, &reg);
        tempo_maps(&a)[0][0].bpm
    };
    assert_eq!(step(&mut v, right - w + 0.5), 120.01);
    assert_eq!(step(&mut v, right - w - 0.5), 121.0);
    assert!((v.tempo_decimals_point().x - (right - w / 2.0)).abs() < 1e-9);
}

#[test]
fn the_count_in_shows_its_beat_in_place_of_the_position() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let shown = |v: &mut atomik_ui::TransportBar| -> Vec<String> {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.texts().iter().map(ToString::to_string).collect()
    };
    let idle = shown(&mut v);
    assert!(idle.contains(&"POS".to_string()), "{idle:?}");
    assert!(!idle.iter().any(|t| t.starts_with("PRE")), "{idle:?}");
    v.set_state(TransportState {
        pre_count: 2,
        ..TransportState::default()
    });
    assert!(shown(&mut v).contains(&"PRE 2".to_string()));
    v.set_state(TransportState {
        pre_count: 2,
        count_in: 3,
        ..TransportState::default()
    });
    let counting = shown(&mut v);
    assert!(counting.contains(&"COUNT".to_string()), "{counting:?}");
    assert!(counting.contains(&"3".to_string()), "{counting:?}");
    assert!(!counting.contains(&"POS".to_string()), "{counting:?}");
}

#[test]
fn the_auto_lanes_switch_sits_next_to_record_and_toggles() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    let rec = v.button_center(TransportButton::Record);
    let auto = v.button_center(TransportButton::AutoLanes);
    assert!(auto.x > rec.x && auto.x - rec.x < 40.0, "{rec:?} {auto:?}");
    let a = run(|e| v.handle(e, &doc, &reg), &[down(auto), up(auto)]);
    assert_eq!(
        a,
        vec![UiAction::Transport(TransportAction::ToggleAutoLanes)]
    );
    // Lit while on.
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let off = dl.to_json();
    v.set_state(TransportState {
        auto_lanes: true,
        ..TransportState::default()
    });
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert_ne!(off, dl.to_json());
}

#[test]
fn the_override_lamp_shows_only_while_a_lane_is_held_and_a_click_clears_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = transport_bar(&doc, &reg);
    assert!(v.override_center().is_none());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!dl.texts().contains(&"OVERRIDE"));
    v.set_state(TransportState {
        overrides: 2,
        ..TransportState::default()
    });
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"OVERRIDE"));
    let p = v.override_center().unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(a, vec![UiAction::Transport(TransportAction::ClearOverride)]);
    assert!(v.help_at(p).is_some());
}
