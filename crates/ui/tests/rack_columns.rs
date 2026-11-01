//! The multi-column rack: layout, the 2-D move drag, dragging devices out of the browser,
//! auto-scroll while dragging and the column commands of the rack menu.

mod support;

use atomik_device_sdk::{RACK_UNIT, RACK_WIDTH, Registry};
use atomik_model::{Command, CreateOptions, CreateSource, RackDoc, RackSlot, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::{Point, Size, Vec2};
use atomik_ui::{
    COLUMN_GAP, ContextTarget, DisplayList, Flip, Key, Modifiers, PointerButton, RackView,
    UiAction, UiEvent,
};
use support::*;

const WIDE: Size = Size::new(3200.0, 2400.0);

/// Synth and delay in column 0, the mixer alone in column 1.
fn two_columns() -> (RackDoc, Registry, RackView) {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.columns = 2;
    doc.devices[2].column = 1;
    let mut v = RackView::new(atomik_ui::Theme::dark());
    // Ctrl-click toggles the selection (the Windows table; macOS uses Command).
    v.set_roles(Roles::defaults(Os::Windows));
    let _ = v.handle(&UiEvent::Resize(WIDE), &doc, &reg);
    v.sync(&doc, &reg);
    (doc, reg, v)
}

fn col_x(c: u16) -> f64 {
    f64::from(c) * (f64::from(RACK_WIDTH) + COLUMN_GAP)
}

/// A view point inside column `c` at rack height `y`.
fn in_column(v: &RackView, c: u16, y: f64) -> Point {
    v.rack_to_view(Point::new(col_x(c) + 300.0, y))
}

fn drag(v: &mut RackView, doc: &RackDoc, reg: &Registry, from: Point, to: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(from), doc, reg);
    out.extend(v.handle(&mv(to), doc, reg));
    out.extend(v.handle(&up(to), doc, reg));
    out
}

fn esc() -> UiEvent {
    UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn row_of(v: &RackView, reg: &Registry, type_id: &str) -> Point {
    let i = v
        .browser_rows(reg)
        .iter()
        .position(|t| *t == type_id)
        .expect("row");
    v.browser_row_center(i).expect("browser open")
}

// ---------------------------------------------------------------- layout

#[test]
fn columns_sit_side_by_side_and_the_interface_tops_column_zero_only() {
    let (_, _, v) = two_columns();
    let iface = v.device_rect(&key("interface")).unwrap();
    assert!(iface.x0.abs() < 1e-9 && iface.y0.abs() < 1e-9);
    let synth = v.device_rect(&key("synth")).unwrap();
    assert!((synth.y0 - iface.y1).abs() < 1e-9);
    let delay = v.device_rect(&key("delay")).unwrap();
    assert!((delay.y0 - synth.y1).abs() < 1e-9 && delay.x0.abs() < 1e-9);
    let mixer = v.device_rect(&key("mixer")).unwrap();
    assert!((mixer.x0 - col_x(1)).abs() < 1e-9, "{mixer:?}");
    assert!(mixer.y0.abs() < 1e-9, "column 1 starts at the top");
    assert!((mixer.width() - f64::from(RACK_WIDTH)).abs() < 1e-9);
    // The content is as tall as the tallest column and as wide as both.
    assert!((v.content_height() - delay.y1).abs() < 1e-9);
    assert!((v.content_width() - (col_x(1) + f64::from(RACK_WIDTH))).abs() < 1e-9);
}

#[test]
fn one_column_keeps_the_old_geometry() {
    let (doc, reg) = (reference_doc(), registry());
    let v = view(&doc, &reg);
    assert!((v.content_width() - f64::from(RACK_WIDTH)).abs() < 1e-9);
    let synth = v.device_rect(&key("synth")).unwrap();
    assert!(synth.x0.abs() < 1e-9);
}

#[test]
fn an_empty_column_still_takes_room() {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.columns = 3;
    let mut v = view(&doc, &reg);
    let _ = v.handle(&UiEvent::Resize(WIDE), &doc, &reg);
    assert!((v.content_width() - (col_x(2) + f64::from(RACK_WIDTH))).abs() < 1e-9);
}

/// The rack x at the left and right edge of the view.
fn visible_x(v: &RackView, width: f64) -> (f64, f64) {
    (
        v.view_to_rack(Point::new(0.0, 0.0)).x,
        v.view_to_rack(Point::new(width, 0.0)).x,
    )
}

#[test]
fn horizontal_scroll_covers_every_column() {
    let (doc, reg, mut v) = two_columns();
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 1200.0)), &doc, &reg);
    v.scroll_by(Vec2::new(1e9, 0.0));
    let (_, right) = visible_x(&v, 1000.0);
    assert!(
        right >= col_x(1) + f64::from(RACK_WIDTH) - 1e-6,
        "{right} must reach the end of column 1"
    );
    v.scroll_by(Vec2::new(-1e9, 0.0));
    assert!(visible_x(&v, 1000.0).0 <= 0.0);
    // Shift + wheel scrolls sideways.
    let before = visible_x(&v, 1000.0).0;
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: Point::new(500.0, 500.0),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        },
        &doc,
        &reg,
    );
    assert!(visible_x(&v, 1000.0).0 > before);
}

#[test]
fn painting_two_columns_draws_both_and_stays_deterministic() {
    let (doc, reg, mut v) = two_columns();
    let mut a = DisplayList::new();
    v.paint(&mut a, &doc, &reg);
    let mut b = DisplayList::new();
    v.paint(&mut b, &doc, &reg);
    assert_eq!(a.ops().len(), b.ops().len());
    assert!(!a.ops().is_empty());
}

// ---------------------------------------------------------------- hit testing

#[test]
fn a_device_in_the_second_column_selects_and_edits() {
    let (doc, reg, mut v) = two_columns();
    let p = v.plate_center(&key("mixer")).unwrap();
    assert!(p.x > v.rack_to_view(Point::new(col_x(1), 0.0)).x);
    let a = v.handle(&down(p), &doc, &reg);
    let _ = v.handle(&up(p), &doc, &reg);
    assert!(
        a.iter()
            .any(|x| matches!(x, UiAction::Select(k) if *k == key("mixer")))
    );
    assert_eq!(v.selected(), Some(&key("mixer")));
    // A control of the mixer answers the pointer.
    let c = v.control_center(&key("mixer"), "ch1.level").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(Point::new(c.x, c.y - 20.0)), &doc, &reg);
    assert!(
        commands(&a)
            .iter()
            .any(|c| matches!(c, Command::SetParam { key: k, .. } if *k == key("mixer")))
    );
    let _ = v.handle(&up(c), &doc, &reg);
}

#[test]
fn cables_reach_across_columns() {
    let (doc, reg, mut v) = two_columns();
    v.set_flip(Flip::Back);
    let delay_out = v.socket_center(&sref("delay", "out_l")).unwrap();
    let mixer_in = v.socket_center(&sref("mixer", "ch1_in_l")).unwrap();
    assert!(
        mixer_in.x > delay_out.x + 300.0,
        "{delay_out:?} {mixer_in:?}"
    );
    let c = cable(("delay", "out_l"), ("mixer", "ch1_in_l"));
    let mid = v.cable_midpoint(&c).unwrap();
    assert!(mid.x > delay_out.x.min(mixer_in.x) - 1.0 && mid.x < delay_out.x.max(mixer_in.x) + 1.0);
    // Painting the back with cables between columns works.
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!dl.ops().is_empty());
}

// ---------------------------------------------------------------- the move drag

#[test]
fn dragging_a_device_into_another_column_moves_it_to_the_slot_under_the_pointer() {
    let (doc, reg, mut v) = two_columns();
    let from = v.plate_center(&key("synth")).unwrap();
    // Below the mixer: the end of column 1.
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y1 + 40.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("synth")],
            slot: RackSlot {
                column: 1,
                index: 1
            }
        })]
    );
}

#[test]
fn the_slot_index_counts_the_devices_above_the_pointer_in_that_column() {
    let (doc, reg, mut v) = two_columns();
    let from = v.plate_center(&key("synth")).unwrap();
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y0 + 4.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("synth")],
            slot: RackSlot {
                column: 1,
                index: 0
            }
        })]
    );
}

#[test]
fn a_reorder_inside_one_column_is_a_slot_move_and_a_no_op_sends_nothing() {
    let (doc, reg, mut v) = two_columns();
    let synth = v.device_rect(&key("synth")).unwrap();
    let delay = v.device_rect(&key("delay")).unwrap();
    let from = v.plate_center(&key("delay")).unwrap();
    let to = in_column(&v, 0, synth.y0 + 4.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("delay")],
            slot: RackSlot {
                column: 0,
                index: 0
            }
        })]
    );
    // Dropped back where it is.
    let (doc, reg, mut v) = two_columns();
    let from = v.plate_center(&key("delay")).unwrap();
    let to = in_column(&v, 0, delay.y1 + 200.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn a_group_moves_between_columns_in_one_command() {
    let (doc, reg, mut v) = two_columns();
    let click = |v: &mut RackView, k: &str, m: Modifiers| {
        let p = v.plate_center(&key(k)).unwrap();
        let _ = v.handle(
            &UiEvent::PointerDown {
                pos: p,
                button: PointerButton::Primary,
                modifiers: m,
                clicks: 1,
            },
            &doc,
            &reg,
        );
        let _ = v.handle(&up(p), &doc, &reg);
    };
    click(&mut v, "synth", Modifiers::NONE);
    click(&mut v, "delay", Modifiers::CTRL);
    let from = v.plate_center(&key("delay")).unwrap();
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y1 + 10.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("synth"), key("delay")],
            slot: RackSlot {
                column: 1,
                index: 1
            }
        })]
    );
}

#[test]
fn dropping_right_of_the_last_column_creates_a_column() {
    let (doc, reg, mut v) = two_columns();
    let from = v.plate_center(&key("synth")).unwrap();
    let zone = v.rack_to_view(Point::new(col_x(2) + 40.0, 100.0));
    let a = drag(&mut v, &doc, &reg, from, zone);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("synth")],
            slot: RackSlot {
                column: 2,
                index: 0
            }
        })]
    );
}

#[test]
fn the_new_column_zone_exists_only_while_dragging() {
    let (doc, reg, mut v) = two_columns();
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 1200.0)), &doc, &reg);
    let reach = |v: &RackView| visible_x(v, 1000.0).1;
    let from = v.plate_center(&key("synth")).unwrap();
    v.scroll_by(Vec2::new(1e9, 0.0));
    let still = reach(&v);
    v.scroll_by(Vec2::new(-1e9, 0.0));
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(Point::new(from.x + 30.0, from.y + 30.0)), &doc, &reg);
    v.scroll_by(Vec2::new(1e9, 0.0));
    assert!(
        reach(&v) > still + 100.0,
        "the zone widens the scroll range"
    );
    let _ = v.handle(&esc(), &doc, &reg);
    v.scroll_by(Vec2::new(1e9, 0.0));
    assert!((reach(&v) - still).abs() < 1e-6);
}

#[test]
fn the_master_stays_locked_in_column_zero() {
    let reg = registry();
    let mut doc = support::starter::two_instruments(&reg).doc().clone();
    doc.columns = 2;
    let mut v = RackView::new(atomik_ui::Theme::dark());
    let _ = v.handle(&UiEvent::Resize(WIDE), &doc, &reg);
    v.sync(&doc, &reg);
    // The master cannot be dragged to another column.
    let master = v.plate_center(&key("master")).unwrap();
    let to = in_column(&v, 1, 50.0);
    let a = drag(&mut v, &doc, &reg, master, to);
    assert!(commands(&a).is_empty(), "{a:?}");
    // Nothing goes above it in column 0.
    let bass = v.plate_center(&key("bass")).unwrap();
    let top = v.device_rect(&key("master")).unwrap().y0 + 2.0;
    let to = in_column(&v, 0, top);
    let a = drag(&mut v, &doc, &reg, bass, to);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            route: true,
            keys: vec![key("bass")],
            slot: RackSlot {
                column: 0,
                index: 1
            }
        })]
    );
}

#[test]
fn a_press_without_travel_is_not_a_move() {
    let (doc, reg, mut v) = two_columns();
    let p = v.plate_center(&key("synth")).unwrap();
    let a = drag(&mut v, &doc, &reg, p, Point::new(p.x + 1.0, p.y + 1.0));
    assert!(
        commands(&a)
            .iter()
            .all(|c| !matches!(c, Command::Workflow(_)))
    );
}

#[test]
fn a_sideways_drag_alone_starts_a_move() {
    let (doc, reg, mut v) = two_columns();
    let from = v.plate_center(&key("delay")).unwrap();
    let to = Point::new(in_column(&v, 1, 0.0).x, from.y);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Workflow(WorkflowCommand::MoveDevicesToSlot { slot, .. })] if slot.column == 1
        ),
        "{a:?}"
    );
}

// ---------------------------------------------------------------- the browser drag

fn open_browser(v: &mut RackView) {
    v.set_browser_visible(true);
}

#[test]
fn dragging_an_effect_row_into_a_column_creates_it_at_the_slot() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y1 + 20.0);
    let a = drag(&mut v, &doc, &reg, row, to);
    let options = CreateOptions {
        route: true,
        skip_track: false,
        disarm: Vec::new(),
    };
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(WorkflowCommand::CreateEffectAt {
            slot: RackSlot {
                column: 1,
                index: 1
            },
            source: CreateSource::Type("atomik.stereo-delay".into()),
            key: None,
            options,
        })]
    );
}

#[test]
fn dragging_an_instrument_row_creates_an_instrument() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.poly-synth");
    let to = in_column(&v, 0, 4.0);
    let a = drag(&mut v, &doc, &reg, row, to);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Workflow(WorkflowCommand::CreateInstrumentAt { slot, source, .. })]
                if *slot == (RackSlot { column: 0, index: 0 })
                    && *source == CreateSource::Type("atomik.poly-synth".into())
        ),
        "{a:?}"
    );
}

#[test]
fn dragging_another_device_row_adds_it_plainly() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.line-mixer");
    let to = in_column(&v, 1, 4.0);
    let a = drag(&mut v, &doc, &reg, row, to);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::AddDeviceAt { slot, type_id, .. }]
                if *slot == (RackSlot { column: 1, index: 0 }) && type_id == "atomik.line-mixer"
        ),
        "{a:?}"
    );
}

#[test]
fn dragging_a_player_row_adds_it_at_the_slot_without_cabling_it() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.arpeggiator");
    let to = in_column(&v, 1, 4.0);
    let a = drag(&mut v, &doc, &reg, row, to);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::AddDeviceAt { slot, type_id, .. }]
                if *slot == (RackSlot { column: 1, index: 0 }) && type_id == "atomik.arpeggiator"
        ),
        "{a:?}"
    );
}

#[test]
fn shift_inverts_the_route_option_of_a_dropped_creation() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    let to = in_column(&v, 0, 4.0);
    let _ = v.handle(&down(row), &doc, &reg);
    let _ = v.handle(&mv(to), &doc, &reg);
    let a = v.handle(
        &UiEvent::PointerUp {
            pos: to,
            button: PointerButton::Primary,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        },
        &doc,
        &reg,
    );
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Workflow(WorkflowCommand::CreateEffectAt { options, .. })] if !options.route
        ),
        "{a:?}"
    );
}

#[test]
fn dropping_a_row_in_the_new_column_zone_creates_the_column() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.poly-synth");
    let zone = v.rack_to_view(Point::new(col_x(2) + 30.0, 80.0));
    let a = drag(&mut v, &doc, &reg, row, zone);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Workflow(WorkflowCommand::CreateInstrumentAt { slot, .. })]
                if slot.column == 2
        ),
        "{a:?}"
    );
}

#[test]
fn releasing_over_the_browser_or_pressing_escape_cancels_the_browser_drag() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    // Released over the browser itself.
    let other = v.browser_row_center(0).unwrap();
    let a = drag(&mut v, &doc, &reg, row, Point::new(other.x, other.y + 60.0));
    assert!(commands(&a).is_empty(), "{a:?}");
    // Escape while dragging, even with the browser focused.
    let _ = v.handle(&down(row), &doc, &reg);
    let _ = v.handle(&mv(in_column(&v, 0, 100.0)), &doc, &reg);
    let _ = v.handle(&esc(), &doc, &reg);
    let a = v.handle(&up(in_column(&v, 0, 100.0)), &doc, &reg);
    assert!(commands(&a).is_empty(), "{a:?}");
    // Outside the window.
    let _ = v.handle(&down(row), &doc, &reg);
    let _ = v.handle(&mv(Point::new(-50.0, -50.0)), &doc, &reg);
    let a = v.handle(&up(Point::new(-50.0, -50.0)), &doc, &reg);
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn a_press_on_a_row_without_travel_only_highlights_it() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    let a = drag(&mut v, &doc, &reg, row, Point::new(row.x + 1.0, row.y));
    assert!(commands(&a).is_empty(), "{a:?}");
}

#[test]
fn double_click_on_a_row_still_adds_below_the_selection() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.line-mixer");
    let a = v.handle(&down_with(row, PointerButton::Primary, 2), &doc, &reg);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::AddDevice { index: 3, .. }]
        ),
        "{a:?}"
    );
}

#[test]
fn the_ghost_label_and_drop_line_are_painted_during_a_browser_drag() {
    let (doc, reg, mut v) = two_columns();
    open_browser(&mut v);
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    let mut plain = DisplayList::new();
    v.paint(&mut plain, &doc, &reg);
    let _ = v.handle(&down(row), &doc, &reg);
    let _ = v.handle(&mv(in_column(&v, 1, 300.0)), &doc, &reg);
    let mut dragging = DisplayList::new();
    v.paint(&mut dragging, &doc, &reg);
    assert!(dragging.ops().len() > plain.ops().len());
    let _ = v.handle(&esc(), &doc, &reg);
}

// ---------------------------------------------------------------- auto-scroll

#[test]
fn dragging_near_the_edge_scrolls_deterministically() {
    let (doc, reg, mut v) = two_columns();
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 400.0)), &doc, &reg);
    v.scroll_by(Vec2::new(-1e9, -1e9));
    let from = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    // Near the right edge, vertically in the middle.
    let _ = v.handle(&mv(Point::new(995.0, 200.0)), &doc, &reg);
    assert!(v.is_auto_scrolling());
    assert!(!v.tick(10.0), "the first tick only sets the clock");
    let before = v.view_to_rack(Point::ZERO);
    assert!(v.tick(10.1));
    let after = v.view_to_rack(Point::ZERO);
    assert!(after.x > before.x + 1.0, "{before:?} {after:?}");
    assert!(
        (after.y - before.y).abs() < 1e-9,
        "no vertical motion mid-height"
    );
    // The same two ticks give the same result.
    let (doc2, reg2, mut v2) = two_columns();
    let _ = v2.handle(&UiEvent::Resize(Size::new(1000.0, 400.0)), &doc2, &reg2);
    v2.scroll_by(Vec2::new(-1e9, -1e9));
    let from2 = v2.plate_center(&key("synth")).unwrap();
    let _ = v2.handle(&down(from2), &doc2, &reg2);
    let _ = v2.handle(&mv(Point::new(995.0, 200.0)), &doc2, &reg2);
    v2.tick(10.0);
    v2.tick(10.1);
    assert!((v2.view_to_rack(Point::ZERO).x - after.x).abs() < 1e-9);
    // Bottom edge scrolls down.
    let _ = v.handle(&mv(Point::new(500.0, 398.0)), &doc, &reg);
    let y0 = v.view_to_rack(Point::ZERO).y;
    v.tick(10.2);
    assert!(v.view_to_rack(Point::ZERO).y > y0);
    // A long gap is clamped, so a stall does not throw the rack across.
    let x0 = v.view_to_rack(Point::ZERO).x;
    v.tick(100.0);
    assert!(v.view_to_rack(Point::ZERO).x - x0 < 400.0);
    let _ = v.handle(&esc(), &doc, &reg);
    assert!(!v.is_auto_scrolling());
    assert!(!v.tick(101.0));
}

#[test]
fn the_middle_of_the_view_does_not_scroll_and_nor_does_a_plain_hover() {
    let (doc, reg, mut v) = two_columns();
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 400.0)), &doc, &reg);
    let _ = v.handle(&mv(Point::new(995.0, 200.0)), &doc, &reg);
    assert!(!v.is_auto_scrolling(), "no drag, no auto-scroll");
    let from = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(Point::new(500.0, 200.0)), &doc, &reg);
    assert!(!v.is_auto_scrolling());
}

#[test]
fn a_browser_drag_over_the_browser_does_not_scroll_left() {
    let (doc, reg, mut v) = two_columns();
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 600.0)), &doc, &reg);
    open_browser(&mut v);
    v.scroll_by(Vec2::new(300.0, 0.0));
    let row = row_of(&v, &reg, "atomik.stereo-delay");
    let _ = v.handle(&down(row), &doc, &reg);
    let _ = v.handle(&mv(Point::new(row.x + 5.0, 300.0)), &doc, &reg);
    assert!(!v.is_auto_scrolling());
}

// ---------------------------------------------------------------- the rack menu

#[test]
fn right_click_on_empty_rack_asks_for_the_rack_menu_and_remembers_the_column() {
    let (doc, reg, mut v) = two_columns();
    let at = in_column(&v, 1, 800.0);
    let a = v.handle(&down_with(at, PointerButton::Secondary, 1), &doc, &reg);
    assert!(a.contains(&UiAction::ContextMenu {
        target: ContextTarget::Rack,
        at
    }));
    let out = v.command("rack.remove_column", None, &doc, &reg);
    assert_eq!(
        out,
        CommandOutcome::Handled(vec![UiAction::Command(Command::RemoveRackColumn {
            column: 1
        })])
    );
}

#[test]
fn add_column_appends_and_remove_column_needs_two() {
    let (doc, reg, mut v) = two_columns();
    let out = v.command("rack.add_column", None, &doc, &reg);
    assert_eq!(
        out,
        CommandOutcome::Handled(vec![UiAction::Command(Command::AddRackColumn)])
    );
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert!(matches!(
        v.command("rack.remove_column", None, &doc, &reg),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn the_unit_is_still_a_rack_unit() {
    let (_, _, v) = two_columns();
    let r = v.device_rect(&key("interface")).unwrap();
    assert!((r.height() - f64::from(RACK_UNIT)).abs() < 1e-9);
}

// ---------------------------------------------------------------- plugins and the zone

#[test]
fn a_plugin_that_is_neither_instrument_nor_effect_drops_into_a_slot_and_the_new_column() {
    use atomik_model::PluginCommand;
    use atomik_ui::browser::BrowserRow;
    let (doc, reg, mut v) = two_columns();
    v.set_plugin_catalog(support::plugin::catalog());
    open_browser(&mut v);
    let i = v
        .browser_list(&reg)
        .iter()
        .position(|r| *r == BrowserRow::Plugin(2))
        .expect("the note effect");
    let row = v.browser_row_center(i).unwrap();
    // Into the new column: one command that adds the plugin at slot (2, 0), column included.
    let zone = v.rack_to_view(Point::new(col_x(2) + 30.0, 80.0));
    let a = drag(&mut v, &doc, &reg, row, zone);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Plugin(PluginCommand::AddAt { slot, plugin, .. })]
                if *slot == RackSlot { column: 2, index: 0 } && *plugin == support::plugin::arp_ref()
        ),
        "{a:?}"
    );
    // Into the top of column 1: the slot, not a flat index that would join column 0.
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let to = in_column(&v, 1, mixer.y0 + 4.0);
    let a = drag(&mut v, &doc, &reg, row, to);
    assert!(
        matches!(
            commands(&a).as_slice(),
            [Command::Plugin(PluginCommand::AddAt { slot, .. })]
                if *slot == RackSlot { column: 1, index: 0 }
        ),
        "{a:?}"
    );
}

#[test]
fn the_new_column_zone_is_on_screen_while_a_device_is_dragged_whatever_the_window() {
    let (doc, reg) = (reference_doc(), registry());
    for navigator in [false, true] {
        for width in [820.0, 900.0, 1000.0, 1100.0, 1400.0, 1800.0] {
            let mut v = RackView::new(atomik_ui::Theme::dark());
            v.set_show_console_devices(false);
            v.set_navigator_visible(navigator);
            let size = Size::new(width + if navigator { 136.0 } else { 0.0 }, 1200.0);
            let _ = v.handle(&UiEvent::Resize(size), &doc, &reg);
            v.sync(&doc, &reg);
            let area_right = v.navigator_rect().map_or(size.width, |r| r.x0);
            let from = v.plate_center(&key("synth")).unwrap();
            let _ = v.handle(&down(from), &doc, &reg);
            let _ = v.handle(&mv(Point::new(from.x + 20.0, from.y + 20.0)), &doc, &reg);
            // Drop point in the middle of the zone, shown fully inside the rack area.
            let zone_l = v.rack_to_view(Point::new(col_x(1), 80.0));
            let zone_r = v.rack_to_view(Point::new(col_x(1) + 200.0, 80.0));
            assert!(
                zone_l.x >= 0.0 && zone_r.x <= area_right + 1e-6,
                "{width} wide, navigator {navigator}: zone {zone_l:?}..{zone_r:?} of {area_right}"
            );
            let to = Point::new((zone_l.x + zone_r.x) / 2.0, zone_l.y);
            let _ = v.handle(&mv(to), &doc, &reg);
            let a = v.handle(&up(to), &doc, &reg);
            assert_eq!(
                commands(&a),
                vec![Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
                    route: true,
                    keys: vec![key("synth")],
                    slot: RackSlot {
                        column: 1,
                        index: 0
                    }
                })],
                "{width} wide, navigator {navigator}"
            );
        }
    }
}

#[test]
fn a_rack_that_fits_does_not_move_when_a_drag_starts_if_the_zone_fits_beside_it() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let _ = v.handle(&UiEvent::Resize(Size::new(1800.0, 1200.0)), &doc, &reg);
    let from = v.plate_center(&key("synth")).unwrap();
    let rest = v.rack_to_view(Point::ZERO);
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(Point::new(from.x + 20.0, from.y + 20.0)), &doc, &reg);
    assert!((v.rack_to_view(Point::ZERO) - rest).hypot() < 1e-9);
    let _ = v.handle(&esc(), &doc, &reg);
}
