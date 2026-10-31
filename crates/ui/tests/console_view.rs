//! ConsoleView behaviour through its public contract: control mapping, solo /
//! mute rules, console commands, folding, meters, readouts, pane actions; and the rack view
//! hiding console devices.

mod support;

use atomik_model::{Command, ConsoleCommand, RackDoc};
use atomik_ui::console_layout::SectionId;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::widget::KNOB_PIXELS_PER_RANGE;
use atomik_ui::{
    ChannelMeters, ConsoleButton, ConsoleView, DisplayList, Flip, Key, Modifiers, PointerButton,
    RackView, Theme, UiAction, UiEvent,
};
use support::console::*;
use support::*;

fn setup() -> (RackDoc, atomik_device_sdk::Registry, ConsoleView) {
    let (doc, reg) = (console_doc(), console_registry());
    let v = console_view(&doc, &reg);
    (doc, reg, v)
}

fn set_params(actions: &[UiAction]) -> Vec<(String, String, f32, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                key,
                param,
                value,
                merge,
            } => Some((key.0, param, value, merge)),
            _ => None,
        })
        .collect()
}

fn console_cmds(actions: &[UiAction]) -> Vec<ConsoleCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Console(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn click(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
) -> Vec<UiAction> {
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// ---------------------------------------------------------------- sync and layout

#[test]
fn strips_follow_console_order_and_show_source_names() {
    let (doc, _reg, v) = setup();
    assert_eq!(
        v.strip_keys(),
        [key("ch-bass"), key("ch-lead"), key("ch-keys")]
    );
    let a = v.strip_rect(&key("ch-bass")).unwrap();
    let b = v.strip_rect(&key("ch-lead")).unwrap();
    assert!(b.x0 > a.x1 - 1e-9, "left to right, no overlap");
    let mut dl = DisplayList::new();
    let mut v = v;
    v.paint(&mut dl, &doc, &console_registry());
    let texts = dl.texts();
    for t in [
        "Bass",
        "Lead",
        "Keys",
        "MASTER",
        "CH 1",
        "CH 3",
        "Stereo Delay",
        "FX RETURNS",
    ] {
        assert!(texts.contains(&t), "missing {t:?}");
    }
}

#[test]
fn hidden_params_are_never_shown() {
    let (_doc, _reg, v) = setup();
    assert!(
        v.control_center(&key("ch-bass"), "internal.solo_silence")
            .is_none()
    );
    assert!(v.control_center(&key("ch-bass"), "fader").is_some());
    let l = v.strip_layout().unwrap();
    assert!(l.controls.iter().all(|c| !c.param.starts_with("internal.")));
}

// ---------------------------------------------------------------- control mapping

#[test]
fn knob_drags_emit_merged_set_params_and_end_the_merge() {
    let (doc, reg, mut v) = setup();
    let p = v.control_center(&key("ch-lead"), "input.gain").unwrap();
    // The press only says which param is touched.
    assert!(
        v.handle(&down(p), &doc, &reg)
            .iter()
            .all(|a| matches!(a, UiAction::ParamTouched { .. }))
    );
    assert!(v.is_dragging());
    let a = v.handle(
        &mv(p - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 4.0)),
        &doc,
        &reg,
    );
    let b = v.handle(
        &mv(p - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 2.0)),
        &doc,
        &reg,
    );
    let (a, b) = (set_params(&a), set_params(&b));
    assert_eq!(a[0].0, "ch-lead");
    assert_eq!(a[0].1, "input.gain");
    // -24..24 dB, default 0: a quarter of the range is +12 dB, half is +24 dB.
    assert!(close(a[0].2, 12.0), "{:?}", a);
    assert!(close(b[0].2, 24.0), "{:?}", b);
    let merge = a[0].3.unwrap();
    assert_eq!(b[0].3, Some(merge));
    assert!(
        merge >= 3 << 40,
        "console merge ids never collide with other views"
    );
    let up_actions = v.handle(&up(p), &doc, &reg);
    assert_eq!(up_actions, [UiAction::EndMerge]);
    assert!(!v.is_dragging());
    // The next gesture gets a new merge id.
    v.handle(&down(p), &doc, &reg);
    let c = set_params(&v.handle(&mv(p - Vec2::new(0.0, 10.0)), &doc, &reg));
    assert_ne!(c[0].3, Some(merge));
}

#[test]
fn shift_drags_are_fine_and_escape_restores_the_start_value() {
    let (doc, reg, mut v) = setup();
    let p = v.control_center(&key("ch-lead"), "input.gain").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(
        &mv_with(p - Vec2::new(0.0, 100.0), Modifiers::SHIFT),
        &doc,
        &reg,
    );
    assert!(close(set_params(&a)[0].2, 2.4), "{a:?}");
    let esc = v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    let sp = set_params(&esc);
    assert!(close(sp[0].2, 0.0));
    assert!(sp[0].3.is_some(), "the restore joins the drag's undo step");
    assert_eq!(esc.last(), Some(&UiAction::EndMerge));
    assert!(!v.is_dragging());
}

#[test]
fn double_click_resets_to_the_default_and_wheel_steps() {
    let (doc, reg, mut v) = setup();
    // ch-bass fader is at 0.62; the default is 0.75 (unity).
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    let a = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(
        set_params(&a),
        [("ch-bass".to_string(), "fader".to_string(), 0.75, None)]
    );
    assert!(
        v.control_center(&key("ch-bass"), "eq.lf.gain").is_none(),
        "scrolled out of view"
    );
    assert!(v.scroll_to_control(&key("ch-bass"), "eq.lf.gain"));
    let q = v.control_center(&key("ch-bass"), "eq.lf.gain").unwrap();
    let w = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: q,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    // 4.5 dB + 2 % of a 30 dB range.
    assert!(close(set_params(&w)[0].2, 5.1), "{w:?}");
}

#[test]
fn faders_follow_the_pointer_along_their_travel() {
    let (doc, reg, mut v) = setup();
    let l = v.strip_layout().unwrap().clone();
    let fader = l.controls.iter().find(|c| c.param == "fader").unwrap();
    let travel = fader.rect.height() - 16.0;
    let p = v.control_center(&key("ch-lead"), "fader").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(&mv(p - Vec2::new(0.0, travel * 0.1)), &doc, &reg);
    assert!(close(set_params(&a)[0].2, 0.85), "{a:?}");
    v.handle(&up(p), &doc, &reg);
    // The master fader works the same way.
    let m = v.control_center(&key("master"), "fader").unwrap();
    v.handle(&down(m), &doc, &reg);
    let b = v.handle(&mv(m + Vec2::new(0.0, travel * 0.25)), &doc, &reg);
    assert_eq!(set_params(&b)[0].0, "master");
    assert!(close(set_params(&b)[0].2, 0.5), "{b:?}");
}

#[test]
fn fader_readouts_use_the_descriptor_mapping() {
    let (doc, reg, mut v) = setup();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    // ch-lead at unity (0.75), ch-keys at 0.9 = 60·log10(1.2) dB, master default unity.
    assert!(texts.contains(&"0.0 dB"), "{texts:?}");
    assert!(texts.contains(&"+4.8 dB"), "{texts:?}");
    // ch-bass at 0.62: 60·log10(0.62 / 0.75) = -4.96 dB.
    assert!(texts.contains(&"-5.0 dB"), "{texts:?}");
}

#[test]
fn send_bars_drag_both_ways_and_buttons_toggle() {
    let (doc, reg, mut v) = setup();
    assert!(v.scroll_to_control(&key("ch-bass"), "send1.level"));
    let p = v.control_center(&key("ch-bass"), "send1.level").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(&mv(p + Vec2::new(12.0, -12.0)), &doc, &reg);
    let sp = set_params(&a);
    assert_eq!(sp[0].1, "send1.level");
    // -12 dB on -60..6 is norm 48/66; +24 px of 120 px adds 0.2.
    let expected = -60.0 + 66.0 * (48.0 / 66.0 + 0.2);
    assert!(close(sp[0].2, expected), "{sp:?}");
    v.handle(&up(p), &doc, &reg);
    v.scroll_to_control(&key("ch-lead"), "eq.on");
    let b = v.control_center(&key("ch-lead"), "eq.on").unwrap();
    assert_eq!(
        set_params(&click(&mut v, &doc, &reg, b)),
        [("ch-lead".to_string(), "eq.on".to_string(), 1.0, None)]
    );
    v.scroll_to_control(&key("ch-keys"), "send2.pre");
    let pre = v.control_center(&key("ch-keys"), "send2.pre").unwrap();
    assert_eq!(set_params(&click(&mut v, &doc, &reg, pre))[0].2, 0.0);
}

#[test]
fn choice_controls_cycle_both_ways() {
    let (doc, reg, mut v) = setup();
    let p = v.control_center(&key("master"), "monitor.source").unwrap();
    assert_eq!(set_params(&click(&mut v, &doc, &reg, p))[0].2, 1.0);
    let back = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(set_params(&back)[0].2, 1.0, "wraps backwards");
}

// ---------------------------------------------------------------- solo and mute

#[test]
fn mute_is_disabled_while_the_channel_is_soloed() {
    let (doc, reg, mut v) = setup();
    assert!(!v.mute_enabled(&key("ch-keys")));
    assert!(v.mute_enabled(&key("ch-lead")));
    let m = v.control_center(&key("ch-keys"), "mute").unwrap();
    assert!(
        set_params(&click(&mut v, &doc, &reg, m)).is_empty(),
        "ignored under solo"
    );
    let w = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: m,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!(w.is_empty());
    // Other mutes and solos work.
    let lm = v.control_center(&key("ch-lead"), "mute").unwrap();
    assert_eq!(
        set_params(&click(&mut v, &doc, &reg, lm)),
        [("ch-lead".to_string(), "mute".to_string(), 0.0, None)]
    );
    let s = v.control_center(&key("ch-keys"), "solo").unwrap();
    assert_eq!(
        set_params(&click(&mut v, &doc, &reg, s)),
        [("ch-keys".to_string(), "solo".to_string(), 0.0, None)]
    );
    // Un-soloed, the mute responds again.
    let mut doc2 = doc.clone();
    let ch = doc2
        .devices
        .iter_mut()
        .find(|d| d.key == key("ch-keys"))
        .unwrap();
    ch.params.insert("solo".into(), 0.0);
    v.sync(&doc2, &reg);
    assert!(v.mute_enabled(&key("ch-keys")));
    assert_eq!(set_params(&click(&mut v, &doc2, &reg, m))[0].2, 0.0);
}

#[test]
fn solo_silences_the_other_channels_in_the_display() {
    let (doc, reg, mut v) = setup();
    assert!(v.solo_silenced(&key("ch-bass")));
    assert!(v.solo_silenced(&key("ch-lead")));
    assert!(!v.solo_silenced(&key("ch-keys")));
    let mut doc2 = doc.clone();
    let ch = doc2
        .devices
        .iter_mut()
        .find(|d| d.key == key("ch-keys"))
        .unwrap();
    ch.params.insert("solo".into(), 0.0);
    v.sync(&doc2, &reg);
    assert!(!v.solo_silenced(&key("ch-bass")));
}

#[test]
fn master_clears_solos_and_mutes() {
    let (doc, reg, mut v) = setup();
    let s = v.button_center(ConsoleButton::ClearSolos).unwrap();
    let m = v.button_center(ConsoleButton::ClearMutes).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, s)),
        [ConsoleCommand::ClearSolos]
    );
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, m)),
        [ConsoleCommand::ClearMutes]
    );
}

// ---------------------------------------------------------------- console commands

#[test]
fn add_channel_lists_instruments_without_a_channel() {
    let (doc, reg, mut v) = setup();
    let b = v.button_center(ConsoleButton::AddChannel).unwrap();
    assert!(click(&mut v, &doc, &reg, b).is_empty());
    assert_eq!(v.menu_items(), ["Pad"]);
    let row = v.menu_row_center(0).unwrap();
    let a = click(&mut v, &doc, &reg, row);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::AddChannel { source: key("pad") }]
    );
    assert!(v.menu_items().is_empty(), "the menu closes");
    // With every instrument on a channel, the menu says so and does nothing.
    let mut doc2 = doc.clone();
    doc2.console.channels.push(chan("ch-pad", Some("pad")));
    v.sync(&doc2, &reg);
    click(&mut v, &doc2, &reg, b);
    assert_eq!(v.menu_items().len(), 1);
    let row = v.menu_row_center(0).unwrap();
    assert!(click(&mut v, &doc2, &reg, row).is_empty());
    // Escape closes a menu.
    click(&mut v, &doc, &reg, b);
    v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(v.menu_items().is_empty());
}

#[test]
fn right_click_on_a_name_does_not_remove_the_channel() {
    let (doc, reg, mut v) = setup();
    let p = v.name_center(&key("ch-lead")).unwrap();
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
    // It opens the VCA menu, which only acts when an entry is picked.
    assert_eq!(v.menu_items()[0], "New VCA...");
}

#[test]
fn dragging_a_name_moves_the_channel() {
    let (doc, reg, mut v) = setup();
    let p = v.name_center(&key("ch-bass")).unwrap();
    let last = v.strip_rect(&key("ch-keys")).unwrap();
    v.handle(&down(p), &doc, &reg);
    v.handle(&mv(Point::new(last.x1 - 5.0, p.y)), &doc, &reg);
    let a = v.handle(&up(Point::new(last.x1 - 5.0, p.y)), &doc, &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveChannel {
            channel: key("ch-bass"),
            to: 2
        }]
    );
    // Back to the front.
    let q = v.name_center(&key("ch-keys")).unwrap();
    v.handle(&down(q), &doc, &reg);
    let first = v.strip_rect(&key("ch-bass")).unwrap();
    let a = v.handle(&up(Point::new(first.x0 + 5.0, q.y)), &doc, &reg);
    assert!(a.is_empty(), "no move without a drag");
    v.handle(&down(q), &doc, &reg);
    v.handle(&mv(Point::new(first.x0 + 5.0, q.y)), &doc, &reg);
    let a = v.handle(&up(Point::new(first.x0 + 5.0, q.y)), &doc, &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveChannel {
            channel: key("ch-keys"),
            to: 0
        }]
    );
    // A tiny wiggle is a click, not a move.
    v.handle(&down(p), &doc, &reg);
    v.handle(&mv(p + Vec2::new(2.0, 0.0)), &doc, &reg);
    assert!(
        v.handle(&up(p + Vec2::new(2.0, 0.0)), &doc, &reg)
            .is_empty()
    );
}

#[test]
fn return_pickers_offer_rack_effects() {
    let (doc, reg, mut v) = setup();
    let p = v.return_picker_center(1).unwrap();
    click(&mut v, &doc, &reg, p);
    assert_eq!(v.menu_items(), ["(none)", "Stereo Delay", "Echo"]);
    let row = v.menu_row_center(2).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetReturn {
            slot: 1,
            effect: Some(key("echo"))
        }]
    );
    let p0 = v.return_picker_center(0).unwrap();
    click(&mut v, &doc, &reg, p0);
    let row = v.menu_row_center(0).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetReturn {
            slot: 0,
            effect: None
        }]
    );
}

#[test]
fn the_pdc_button_toggles_compensation_and_readouts_show() {
    let (doc, reg, mut v) = setup();
    let b = v.button_center(ConsoleButton::Compensation).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, b)),
        [ConsoleCommand::SetCompensation(false)]
    );
    let mut doc2 = doc.clone();
    doc2.console.compensation = false;
    v.sync(&doc2, &reg);
    assert_eq!(
        console_cmds(&click(&mut v, &doc2, &reg, b)),
        [ConsoleCommand::SetCompensation(true)]
    );
    v.set_latency_readouts(&[
        (key("ch-bass"), 128),
        (key("ch-lead"), 480),
        (key("ch-keys"), 0),
    ]);
    v.set_pdc_warning(true);
    assert!(v.needs_repaint());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    // Per-strip latency sits by the name plate, only when non-zero.
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("smp") && !t.starts_with("LATENCY"))
    );
    assert!(!texts.contains(&"lat 0"));
    for t in [
        "lat 128",
        "lat 480",
        "LATENCY 480 smp  10.0 ms",
        "COMPENSATION LIMIT EXCEEDED",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
}

// ---------------------------------------------------------------- folding, pane, keys

#[test]
fn headers_fold_sections_on_every_strip() {
    let (doc, reg, mut v) = setup();
    let before = v.strip_layout().unwrap().upper_height;
    assert!(v.scroll_to_control(&key("ch-bass"), "eq.on"));
    let h = v.header_center(SectionId::Eq).unwrap();
    click(&mut v, &doc, &reg, h);
    assert!(v.folds().is_folded(SectionId::Eq));
    assert!(v.strip_layout().unwrap().upper_height < before);
    assert!(v.control_center(&key("ch-keys"), "eq.hf.gain").is_none());
    assert!(
        !v.scroll_to_control(&key("ch-bass"), "eq.on"),
        "folded controls are not shown"
    );
    let h = v.header_center(SectionId::Eq).unwrap();
    click(&mut v, &doc, &reg, h);
    assert!(!v.folds().is_folded(SectionId::Eq));
    // Fold all, then unfold all.
    let all = v.button_center(ConsoleButton::FoldAll).unwrap();
    click(&mut v, &doc, &reg, all);
    assert!(!v.folds().any_unfolded());
    assert!(v.control_center(&key("ch-bass"), "fader").is_none());
    click(&mut v, &doc, &reg, all);
    assert!(!v.folds().any_folded());
}

#[test]
fn pane_buttons_and_f5_emit_pane_actions() {
    let (doc, reg, mut v) = setup();
    let a = v.button_center(ConsoleButton::Arrange).unwrap();
    assert_eq!(click(&mut v, &doc, &reg, a), [UiAction::ToggleConsole]);
    let m = v.button_center(ConsoleButton::Maximise).unwrap();
    assert_eq!(click(&mut v, &doc, &reg, m), [UiAction::MaximiseConsole]);
    assert_eq!(
        v.handle(&key_event(Key::F(5), Modifiers::NONE), &doc, &reg),
        [UiAction::ToggleConsole]
    );
    assert_eq!(
        v.handle(&key_event(Key::F(5), Modifiers::SHIFT), &doc, &reg),
        [UiAction::MaximiseConsole]
    );
    assert_eq!(
        v.handle(&key_event(Key::Char('z'), Modifiers::CTRL), &doc, &reg),
        [],
        "undo is the registry command, not a key of the view"
    );
}

#[test]
fn upper_sections_scroll_and_strips_scroll_sideways() {
    let (doc, reg, mut v) = setup();
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 560.0, 420.0));
    v.sync(&doc, &reg);
    let p = v.header_center(SectionId::Input).unwrap();
    v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -2.0),
            pos: p,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!(v.scroll_y() > 0.0);
    // Over the pinned name plates the wheel scrolls sideways.
    let n = v.name_center(&key("ch-bass")).unwrap();
    v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: n,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!(v.scroll_x() > 0.0);
    // So does Shift+wheel over the upper sections (away from a control).
    v.set_scroll_x(0.0);
    v.set_scroll_y(0.0);
    let p = v.header_center(SectionId::Input).unwrap();
    v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: p,
            modifiers: Modifiers::SHIFT,
        },
        &doc,
        &reg,
    );
    assert!(v.scroll_x() > 0.0);
    v.set_scroll_x(1e9);
    let max = v.scroll_x();
    assert!(max > 0.0 && max < 1e9, "clamped");
}

// ---------------------------------------------------------------- meters

#[test]
fn meters_take_readings_and_ballistics_from_the_frame_time() {
    let (doc, reg, mut v) = setup();
    let m = ChannelMeters {
        out_peak_l: 0.5,
        out_peak_r: 1.0,
        out_rms_l: 0.25,
        out_rms_r: 0.5,
        gr: 0.5,
    };
    v.paint(&mut DisplayList::new(), &doc, &reg);
    v.set_meters(&[(key("ch-bass"), m), (key("nope"), m), (key("master"), m)]);
    v.set_frame_time(10.0);
    assert!(v.needs_repaint());
    let b = v.meter(&key("ch-bass")).unwrap();
    assert!(close(b.peak[0].level_db, -6.02));
    assert!(b.clip(1) && !b.clip(0));
    assert!(close(b.gr_db, 6.02));
    assert!(v.meter(&key("nope")).is_none());
    assert!(v.meter(&key("master")).is_some());
    v.set_meters(&[(key("ch-bass"), ChannelMeters::SILENT)]);
    v.set_frame_time(10.25);
    let b = v.meter(&key("ch-bass")).unwrap();
    assert!(close(b.peak[0].level_db, -12.02), "{}", b.peak[0].level_db);
    // Time going backwards is a zero step.
    v.set_frame_time(5.0);
    assert!(close(
        v.meter(&key("ch-bass")).unwrap().peak[0].level_db,
        -12.02
    ));
}

// ---------------------------------------------------------------- rack view

#[test]
fn the_rack_shows_console_devices_unless_hidden() {
    let (doc, reg) = (console_doc(), console_registry());
    let mut r = RackView::new(Theme::dark());
    let _ = r.handle(&UiEvent::Resize(VIEW), &doc, &reg);
    r.sync(&doc, &reg);
    // Shown by default.
    assert!(r.show_console_devices());
    assert_eq!(r.panel_revisions().len(), doc.devices.len() + 1);
    assert!(r.device_rect(&key("master")).is_some());
    r.set_show_console_devices(false);
    r.sync(&doc, &reg);
    let shown: Vec<String> = r.panel_revisions().into_iter().map(|(k, _)| k.0).collect();
    assert_eq!(
        shown,
        ["interface", "bass", "lead", "keys", "pad", "delay", "echo"]
    );
    assert!(r.device_rect(&key("ch-bass")).is_none());
    r.set_show_console_devices(true);
    r.sync(&doc, &reg);
    assert_eq!(r.panel_revisions().len(), doc.devices.len() + 1);
}

#[test]
fn moving_a_device_past_hidden_ones_uses_document_indices() {
    let (mut doc, reg) = (console_doc(), console_registry());
    // Hidden channel devices sit between the visible ones: bass, ch-bass, lead, ch-lead, ...
    doc.devices = vec![
        doc.devices[0].clone(),
        doc.devices[6].clone(),
        doc.devices[1].clone(),
        doc.devices[7].clone(),
        doc.devices[2].clone(),
        doc.devices[8].clone(),
        doc.devices[9].clone(),
    ];
    let mut r = RackView::new(Theme::dark());
    let _ = r.handle(&UiEvent::Resize(VIEW), &doc, &reg);
    r.sync(&doc, &reg);
    let keys_rect = r.device_rect(&key("keys")).unwrap();
    let p = r.plate_center(&key("bass")).unwrap();
    let target = r.rack_to_view(Point::new(0.0, keys_rect.y1 - 2.0));
    r.handle(&down(p), &doc, &reg);
    r.handle(&mv(Point::new(p.x, target.y)), &doc, &reg);
    let a = r.handle(&up(Point::new(p.x, target.y)), &doc, &reg);
    // After removing "bass", "keys" is at document index 3; dropping below it is index 4.
    assert_eq!(
        commands(&a),
        [Command::Workflow(
            atomik_model::WorkflowCommand::MoveDevicesToSlot {
                route: true,
                keys: vec![key("bass")],
                slot: atomik_model::RackSlot {
                    column: 0,
                    index: 4
                }
            }
        )]
    );
}

#[test]
fn rack_cables_to_summing_inputs_add_instead_of_replacing() {
    let (mut doc, reg) = (console_doc(), console_registry());
    doc.cables = vec![cable(("bass", "out_l"), ("master", "bus_l"))];
    let mut r = RackView::new(Theme::dark());
    let _ = r.handle(
        &UiEvent::Resize(atomik_ui::kurbo::Size::new(1000.0, 3000.0)),
        &doc,
        &reg,
    );
    r.set_show_console_devices(true);
    r.sync(&doc, &reg);
    r.set_flip(Flip::Back);
    let from = r.socket_center(&sref("lead", "out_l")).unwrap();
    let to = r.socket_center(&sref("master", "bus_l")).unwrap();
    r.handle(&down(from), &doc, &reg);
    r.handle(&mv(to), &doc, &reg);
    let a = r.handle(&up(to), &doc, &reg);
    // The summing input keeps the other cable, and the right pair comes with the left one.
    assert_eq!(
        commands(&a),
        [Command::Batch(vec![
            Command::Connect(cable(("lead", "out_l"), ("master", "bus_l"))),
            Command::Connect(cable(("lead", "out_r"), ("master", "bus_r"))),
        ])]
    );
}

#[test]
fn the_max_button_lights_when_the_app_says_the_console_is_maximised() {
    let (doc, reg, mut v) = setup();
    assert!(!v.maximised());
    let mut normal = DisplayList::new();
    v.paint(&mut normal, &doc, &reg);
    v.set_maximised(true);
    assert!(v.maximised());
    assert!(v.needs_repaint());
    let mut max = DisplayList::new();
    v.paint(&mut max, &doc, &reg);
    assert_ne!(
        normal.to_json(),
        max.to_json(),
        "the MAX button is drawn lit"
    );
    // The button still asks the app to toggle.
    let m = v.button_center(ConsoleButton::Maximise).unwrap();
    assert_eq!(click(&mut v, &doc, &reg, m), [UiAction::MaximiseConsole]);
}

#[test]
fn the_master_output_section_shows_live_peaks() {
    let (doc, reg, mut v) = setup();
    v.set_meters(&[(
        key("master"),
        ChannelMeters {
            out_peak_l: 0.5,
            out_peak_r: 0.25,
            out_rms_l: 0.1,
            out_rms_r: 0.1,
            gr: 0.5,
        },
    )]);
    v.set_frame_time(0.0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    for t in ["OUTPUT", "L -6.0", "R -12.0", "GR 6.0 dB"] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
}

// ---------------------------------------------------------------- keys as commands

fn run(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    id: &str,
) -> Vec<UiAction> {
    match v.command(id, None, doc, reg) {
        CommandOutcome::Handled(a) => a,
        other => panic!("{id}: {other:?}"),
    }
}

fn focus_fader(v: &mut ConsoleView, doc: &RackDoc, reg: &atomik_device_sdk::Registry) {
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    click(v, doc, reg, p);
}

#[test]
fn the_value_commands_step_the_focused_control_like_the_arrows_did() {
    let (doc, reg, mut v) = setup();
    focus_fader(&mut v, &doc, &reg);
    let mut get = |id: &str| set_params(&run(&mut v, &doc, &reg, id));
    let up = get("mixer.value_up");
    let down = get("mixer.value_down");
    let fine = get("mixer.value_up_fine");
    let page = get("mixer.value_page_up");
    let page_down = get("mixer.value_page_down");
    let page_fine = get("mixer.value_page_up_fine");
    let down_fine = get("mixer.value_down_fine");
    let page_down_fine = get("mixer.value_page_down_fine");
    for s in [
        &up,
        &down,
        &fine,
        &page,
        &page_down,
        &page_fine,
        &down_fine,
        &page_down_fine,
    ] {
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].0.as_str(), s[0].1.as_str()), ("ch-bass", "fader"));
    }
    let at = 0.62;
    assert!(up[0].2 > at && down[0].2 < at);
    assert!(fine[0].2 > at && fine[0].2 < up[0].2, "fine is smaller");
    assert!(
        page[0].2 > up[0].2 && page_down[0].2 < down[0].2,
        "a page is ten notches"
    );
    assert!(page_fine[0].2 > fine[0].2 && page_fine[0].2 < page[0].2);
    assert!(down_fine[0].2 < at && down_fine[0].2 > down[0].2);
    assert!(page_down_fine[0].2 < down_fine[0].2 && page_down_fine[0].2 > page_down[0].2);
}

#[test]
fn a_value_command_without_a_focused_control_is_taken_and_does_nothing() {
    let (doc, reg, mut v) = setup();
    assert!(run(&mut v, &doc, &reg, "mixer.value_up").is_empty());
}

#[test]
fn the_scroll_commands_move_the_strips_like_the_arrows_did() {
    let (doc, reg, mut v) = setup();
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 560.0, 420.0));
    v.sync(&doc, &reg);
    assert!(run(&mut v, &doc, &reg, "mixer.scroll_right").is_empty());
    let one = v.scroll_x();
    assert!(one > 0.0);
    run(&mut v, &doc, &reg, "mixer.scroll_end");
    let end = v.scroll_x();
    assert!(end >= one);
    run(&mut v, &doc, &reg, "mixer.scroll_left");
    assert!(v.scroll_x() < end);
    run(&mut v, &doc, &reg, "mixer.scroll_home");
    assert_eq!(v.scroll_x(), 0.0);
}

#[test]
fn raw_arrows_do_nothing_in_the_view_any_more() {
    let (doc, reg, mut v) = setup();
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 560.0, 420.0));
    v.sync(&doc, &reg);
    focus_fader(&mut v, &doc, &reg);
    for k in [
        Key::Up,
        Key::Down,
        Key::PageUp,
        Key::PageDown,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
    ] {
        for m in [Modifiers::NONE, Modifiers::SHIFT] {
            assert!(
                v.handle(&key_event(k.clone(), m), &doc, &reg).is_empty(),
                "{k:?}"
            );
        }
    }
    assert_eq!(v.scroll_x(), 0.0);
}

#[test]
fn the_view_declines_commands_that_are_not_its_own() {
    let (doc, reg, mut v) = setup();
    for id in [
        "edit.delete",
        "transport.play_stop",
        "mixer.add_channel",
        "mixer.nope",
    ] {
        assert!(
            matches!(v.command(id, None, &doc, &reg), CommandOutcome::Declined),
            "{id}"
        );
    }
}

// ---------------------------------------------------------------- text owner

fn typed(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    k: Key,
) -> Vec<UiAction> {
    v.handle(&key_event(k, Modifiers::NONE), doc, reg)
}

#[test]
fn the_console_owns_the_keyboard_only_while_a_name_is_edited() {
    let (doc, reg, mut v) = setup();
    assert!(!v.is_text_owner());
    assert!(
        !v.begin_rename(&key("no-such-channel")),
        "nothing to rename"
    );
    assert!(!v.is_text_owner());
    assert!(v.begin_rename(&key("ch-lead")));
    assert!(v.is_text_owner());
    assert!(v.rename_text().is_some());
    // Enter with the name unchanged commits nothing and ends the edit.
    assert!(typed(&mut v, &doc, &reg, Key::Enter).is_empty());
    assert!(!v.is_text_owner());
}

#[test]
fn typing_edits_the_name_and_enter_renames_the_device_the_strip_shows() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_rename(&key("ch-lead")));
    let start = v.rename_text().unwrap().chars().count();
    for _ in 0..start {
        typed(&mut v, &doc, &reg, Key::Backspace);
    }
    for k in [Key::Char('A'), Key::Char('b'), Key::Space, Key::Char('1')] {
        assert!(
            typed(&mut v, &doc, &reg, k).is_empty(),
            "typing emits nothing"
        );
    }
    assert_eq!(v.rename_text(), Some("Ab 1"));
    for k in [Key::Delete, Key::Up, Key::Left, Key::Home] {
        assert!(typed(&mut v, &doc, &reg, k).is_empty());
    }
    assert_eq!(
        v.rename_text(),
        Some("Ab 1"),
        "navigation keys are not text"
    );
    let out = typed(&mut v, &doc, &reg, Key::Enter);
    assert_eq!(
        commands(&out),
        [Command::Rename {
            // The strip shows the name of its source instrument.
            key: key("lead"),
            name: "Ab 1".to_string()
        }]
    );
    assert!(!v.is_text_owner());
}

#[test]
fn escape_cancels_the_edit_and_leaves_the_name_alone() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_rename(&key("ch-lead")));
    typed(&mut v, &doc, &reg, Key::Char('x'));
    assert!(typed(&mut v, &doc, &reg, Key::Escape).is_empty());
    assert!(!v.is_text_owner());
    assert_eq!(v.rename_text(), None);
}

#[test]
fn an_empty_name_is_not_committed() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_rename(&key("ch-lead")));
    while v.rename_text().is_some_and(|t| !t.is_empty()) {
        typed(&mut v, &doc, &reg, Key::Backspace);
    }
    assert!(typed(&mut v, &doc, &reg, Key::Enter).is_empty());
    assert!(!v.is_text_owner());
}

#[test]
fn a_command_chord_is_never_typed_into_the_name() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_rename(&key("ch-lead")));
    let before = v.rename_text().unwrap().to_string();
    for c in ['z', 'y', 'a', 's'] {
        assert!(
            v.handle(&key_event(Key::Char(c), Modifiers::CTRL), &doc, &reg)
                .is_empty()
        );
    }
    assert_eq!(v.rename_text(), Some(before.as_str()));
    assert!(v.is_text_owner());
}

#[test]
fn a_pointer_press_elsewhere_commits_the_edit_and_losing_the_focus_cancels_it() {
    let (doc, reg, mut v) = setup();
    assert!(v.begin_rename(&key("ch-lead")));
    typed(&mut v, &doc, &reg, Key::Char('q'));
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    assert!(
        commands(&a)
            .iter()
            .any(|c| matches!(c, Command::Rename { .. })),
        "a press elsewhere commits like Enter"
    );
    assert!(!v.is_text_owner());
    assert!(v.begin_rename(&key("ch-lead")));
    typed(&mut v, &doc, &reg, Key::Char('q'));
    v.set_focused(false);
    assert!(!v.is_text_owner());
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

#[test]
fn a_remapped_fine_adjust_role_makes_the_other_modifier_fine() {
    use atomik_ui::keymap::{Mods, Os, Role, Roles};
    let (doc, reg, mut v) = setup();
    v.set_roles(Roles::defaults(Os::Windows).with(Role::FineAdjust, Mods::parse("Alt").unwrap()));
    let p = v.control_center(&key("ch-lead"), "input.gain").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(&mv_with(p - Vec2::new(0.0, 100.0), alt()), &doc, &reg);
    assert!(close(set_params(&a)[0].2, 2.4), "Alt is fine now: {a:?}");
    v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    v.handle(&down(p), &doc, &reg);
    let b = v.handle(
        &mv_with(p - Vec2::new(0.0, 100.0), Modifiers::SHIFT),
        &doc,
        &reg,
    );
    assert!(
        set_params(&b)[0].2 > 2.4 + 1e-3,
        "Shift is coarse now: {b:?}"
    );
}

#[test]
fn a_click_with_the_reset_role_held_resets_a_control() {
    use atomik_ui::keymap::{Mods, Os, Role, Roles};
    let (doc, reg, mut v) = setup();
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    let click = |v: &mut ConsoleView| {
        v.handle(
            &UiEvent::PointerDown {
                pos: p,
                button: PointerButton::Primary,
                modifiers: ctrl,
                clicks: 1,
            },
            &doc,
            &reg,
        )
    };
    v.set_roles(Roles::defaults(Os::Windows));
    let a = click(&mut v);
    assert_eq!(
        set_params(&a),
        [("ch-bass".to_string(), "fader".to_string(), 0.75, None)]
    );
    v.handle(&up(p), &doc, &reg);
    v.set_roles(
        Roles::defaults(Os::Windows)
            .with(Role::EditAutomation, Mods::parse("Ctrl+Alt").unwrap())
            .with(Role::ResetParam, Mods::parse("Alt").unwrap()),
    );
    assert!(
        set_params(&click(&mut v)).is_empty(),
        "Ctrl no longer resets"
    );
}

// ---------------------------------------------------------------- selection and ganged edits

fn selected(v: &ConsoleView) -> Vec<String> {
    v.selected_strips().into_iter().map(|k| k.0).collect()
}

type Step = Vec<(String, String, f32, Option<u64>)>;

fn batches(actions: &[UiAction]) -> Vec<Step> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Batch(list) => Some(
                list.into_iter()
                    .filter_map(|c| match c {
                        Command::SetParam {
                            key,
                            param,
                            value,
                            merge,
                        } => Some((key.0, param, value, merge)),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn all_selected(v: &mut ConsoleView, doc: &RackDoc, reg: &atomik_device_sdk::Registry) {
    v.set_selected_strips(&[key("ch-bass"), key("ch-lead"), key("ch-keys")]);
    v.sync(doc, reg);
}

fn db_of(pos: f32) -> f32 {
    60.0 * (pos / 0.75).log10()
}

#[test]
fn left_and_right_select_the_adjacent_strips_and_shift_extends_the_range() {
    let (doc, reg, mut v) = setup();
    // With nothing selected Right starts at the first strip and Left at the last.
    run(&mut v, &doc, &reg, "mixer.select_right");
    assert_eq!(selected(&v), ["ch-bass"]);
    run(&mut v, &doc, &reg, "mixer.select_right");
    assert_eq!(selected(&v), ["ch-lead"]);
    run(&mut v, &doc, &reg, "mixer.extend_right");
    assert_eq!(selected(&v), ["ch-lead", "ch-keys"]);
    run(&mut v, &doc, &reg, "mixer.extend_left");
    assert_eq!(selected(&v), ["ch-lead"]);
    run(&mut v, &doc, &reg, "mixer.select_left");
    run(&mut v, &doc, &reg, "mixer.select_left");
    assert_eq!(selected(&v), ["ch-bass"], "it stops at the first strip");
    v.set_selected_strips(&[]);
    run(&mut v, &doc, &reg, "mixer.select_left");
    assert_eq!(
        v.selected_strips().last().map(|k| k.0.as_str()),
        Some("ch-keys")
    );
}

#[test]
fn the_selection_scrolls_into_view() {
    let (doc, reg, mut v) = setup();
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 560.0, 420.0));
    v.sync(&doc, &reg);
    run(&mut v, &doc, &reg, "mixer.scroll_end");
    assert!(v.scroll_x() > 0.0);
    assert!(
        v.strip_rect(&key("ch-bass")).unwrap().x1 <= 0.0,
        "scrolled out"
    );
    run(&mut v, &doc, &reg, "mixer.select_right");
    let r = v.strip_rect(&key("ch-bass")).unwrap();
    assert!(r.x0 >= -1e-6, "the first strip is back in view: {r:?}");
}

#[test]
fn select_all_selects_every_strip() {
    let (doc, reg, mut v) = setup();
    run(&mut v, &doc, &reg, "edit.select_all");
    assert_eq!(selected(&v), ["ch-bass", "ch-lead", "ch-keys"]);
}

#[test]
fn delete_removes_the_selected_channels() {
    let (doc, reg, mut v) = setup();
    assert!(matches!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Declined
    ));
    v.set_selected_strips(&[key("ch-bass"), key("ch-keys")]);
    let a = run(&mut v, &doc, &reg, "edit.delete");
    assert_eq!(
        console_cmds(&a),
        [
            ConsoleCommand::RemoveChannel {
                channel: key("ch-bass")
            },
            ConsoleCommand::RemoveChannel {
                channel: key("ch-keys")
            }
        ]
    );
}

#[test]
fn a_fader_drag_of_a_selected_strip_moves_every_selected_fader_by_the_same_db() {
    let (doc, reg, mut v) = setup();
    all_selected(&mut v, &doc, &reg);
    let l = v.strip_layout().unwrap().clone();
    let fader = l.controls.iter().find(|c| c.param == "fader").unwrap();
    let travel = fader.rect.height() - 16.0;
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(&mv(p + Vec2::new(0.0, travel * 0.1)), &doc, &reg);
    let b = v.handle(&mv(p + Vec2::new(0.0, travel * 0.15)), &doc, &reg);
    let (a, b) = (batches(&a), batches(&b));
    assert_eq!((a.len(), b.len()), (1, 1), "one batch per step");
    let (a, b) = (&a[0], &b[0]);
    assert_eq!(
        a.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["ch-bass", "ch-lead", "ch-keys"]
    );
    assert!(a.iter().all(|s| s.1 == "fader" && s.3.is_some()));
    assert!(
        a.iter().all(|s| s.3 == a[0].3 && s.3 == b[0].3),
        "one merge id"
    );
    let starts = [0.62, 0.75, 0.9];
    let moved = |step: &Step| -> Vec<f32> {
        step.iter()
            .zip(starts)
            .map(|(s, st)| db_of(s.2) - db_of(st))
            .collect()
    };
    let (m, n) = (moved(a), moved(b));
    assert!(m.iter().all(|d| (d - m[0]).abs() < 1e-3), "{m:?}");
    assert!(m[0] < -1.0, "dragging down lowers them: {m:?}");
    assert!(n.iter().all(|d| (d - n[0]).abs() < 1e-3 && *d < m[0]));
    let end = v.handle(&up(p), &doc, &reg);
    assert!(end.iter().any(|a| matches!(a, UiAction::EndMerge)));
}

#[test]
fn a_ganged_drag_stops_at_the_top_for_every_strip() {
    let (doc, reg, mut v) = setup();
    all_selected(&mut v, &doc, &reg);
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    v.handle(&down(p), &doc, &reg);
    let a = v.handle(&mv(p - Vec2::new(0.0, 400.0)), &doc, &reg);
    let a = &batches(&a)[0];
    // ch-keys started at 0.9: it reaches 1.0 and the others keep their offset to it.
    assert!(close(a[2].2, 1.0), "{a:?}");
    let offsets = [db_of(a[0].2) - db_of(0.62), db_of(a[1].2) - db_of(0.75)];
    assert!((offsets[0] - offsets[1]).abs() < 1e-3, "{offsets:?}");
}

#[test]
fn escape_puts_every_ganged_fader_back() {
    let (doc, reg, mut v) = setup();
    all_selected(&mut v, &doc, &reg);
    let p = v.control_center(&key("ch-lead"), "fader").unwrap();
    v.handle(&down(p), &doc, &reg);
    v.handle(&mv(p + Vec2::new(0.0, 20.0)), &doc, &reg);
    let a = v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    let b = &batches(&a)[0];
    assert_eq!(b.iter().map(|s| s.2).collect::<Vec<_>>(), [0.62, 0.75, 0.9]);
    assert!(a.iter().any(|a| matches!(a, UiAction::EndMerge)));
}

#[test]
fn a_mute_click_on_a_selected_strip_mutes_every_selected_strip() {
    let (doc, reg, mut v) = setup();
    all_selected(&mut v, &doc, &reg);
    v.scroll_to_control(&key("ch-lead"), "mute");
    let m = v.control_center(&key("ch-lead"), "mute").unwrap();
    let a = click(&mut v, &doc, &reg, m);
    let b = &batches(&a)[0];
    // ch-keys is soloed, and a soloed strip cannot be muted by a click.
    assert_eq!(
        b.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["ch-bass", "ch-lead"]
    );
    assert!(
        b.iter()
            .all(|s| s.1 == "mute" && s.2.to_bits() == b[0].2.to_bits() && s.3.is_none())
    );
    // A strip that is not selected is clicked alone.
    v.set_selected_strips(&[key("ch-bass"), key("ch-keys")]);
    v.sync(&doc, &reg);
    let a = click(&mut v, &doc, &reg, m);
    assert!(batches(&a).is_empty());
    assert_eq!(set_params(&a).len(), 1);
}

#[test]
fn a_double_click_on_a_ganged_fader_resets_every_selected_fader() {
    let (doc, reg, mut v) = setup();
    all_selected(&mut v, &doc, &reg);
    let p = v.control_center(&key("ch-bass"), "fader").unwrap();
    let mut a = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    a.extend(v.handle(&up(p), &doc, &reg));
    let b = &batches(&a)[0];
    assert!(b.iter().all(|s| close(s.2, 0.75)), "{b:?}");
}

#[test]
fn the_strip_menu_runs_the_channel_settings_commands_on_the_selection() {
    let (doc, reg, mut v) = setup();
    v.set_selected_strips(&[key("ch-bass")]);
    let p = v.name_center(&key("ch-lead")).unwrap();
    v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(selected(&v), ["ch-lead"], "the strip clicked is selected");
    let items = v.menu_items();
    for label in [
        "Copy settings",
        "Paste settings",
        "Reset channel",
        "Clear inserts",
    ] {
        assert!(items.iter().any(|i| i == label), "{items:?}");
    }
}
