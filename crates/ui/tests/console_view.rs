//! ConsoleView behaviour through its public contract: control mapping, solo /
//! mute rules, console commands, folding, meters, readouts, pane actions; and the rack view
//! hiding console devices.

mod support;

use atomik_model::{Command, ConsoleCommand, RackDoc};
use atomik_ui::console_layout::SectionId;
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
    assert!(v.handle(&down(p), &doc, &reg).is_empty());
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
        click(&mut v, &doc, &reg, m).is_empty(),
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
fn right_click_on_a_name_removes_the_channel() {
    let (doc, reg, mut v) = setup();
    let p = v.name_center(&key("ch-lead")).unwrap();
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RemoveChannel {
            channel: key("ch-lead")
        }]
    );
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
        [UiAction::Undo]
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
    assert_eq!(
        commands(&a),
        [Command::Connect(cable(
            ("lead", "out_l"),
            ("master", "bus_l")
        ))]
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
