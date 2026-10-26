use super::*;

const MIN: (f64, f64) = (320.0, 200.0);
const DEFAULT: (f64, f64) = (1280.0, 720.0);

fn mon(name: &str, x: i32, y: i32, w: u32, h: u32, scale: f64) -> MonitorInfo {
    MonitorInfo {
        name: name.to_owned(),
        x,
        y,
        width: w,
        height: h,
        units_per_logical: scale,
    }
}

fn geom(x: i32, y: i32, w: f64, h: f64) -> WindowGeom {
    WindowGeom {
        x,
        y,
        width: w,
        height: h,
        maximised: false,
        monitor: None,
    }
}

/// Two monitors side by side at 100 %: the primary at the origin, the second to its right.
fn two() -> Vec<MonitorInfo> {
    vec![
        mon("A", 0, 0, 1920, 1080, 1.0),
        mon("B", 1920, 0, 1920, 1080, 1.0),
    ]
}

#[test]
fn place_saved_monitor_present_keeps_position() {
    let g = WindowGeom {
        monitor: Some("B".into()),
        ..geom(2100, 120, 800.0, 600.0)
    };
    let p = place(Some(&g), DEFAULT, MIN, &two(), 0);
    assert_eq!(p.pos, Some((2100, 120)));
    assert_eq!(p.size, (800.0, 600.0));
    assert!(!p.maximised);
}

#[test]
fn place_monitor_gone_moves_to_primary_centred() {
    let g = WindowGeom {
        monitor: Some("unplugged".into()),
        ..geom(5000, 100, 800.0, 600.0)
    };
    let p = place(Some(&g), DEFAULT, MIN, &two(), 0);
    assert_eq!(p.pos, Some(((1920 - 800) / 2, (1080 - 600) / 2)));
    assert_eq!(p.size, (800.0, 600.0));
}

#[test]
fn place_monitor_gone_but_window_overlaps_other_keeps_position() {
    let g = WindowGeom {
        monitor: Some("unplugged".into()),
        ..geom(2000, 50, 800.0, 600.0)
    };
    assert_eq!(
        place(Some(&g), DEFAULT, MIN, &two(), 0).pos,
        Some((2000, 50))
    );
}

#[test]
fn place_partly_offscreen_keeps_when_title_strip_visible() {
    // Hanging off the right edge; 400 units of the title strip still show.
    let g = geom(1520, 500, 800.0, 600.0);
    let one = vec![mon("A", 0, 0, 1920, 1080, 1.0)];
    assert_eq!(
        place(Some(&g), DEFAULT, MIN, &one, 0).pos,
        Some((1520, 500))
    );
}

#[test]
fn place_title_strip_hidden_centres_on_primary() {
    // Only 20 units of width show: not enough to grab the title bar.
    let g = geom(1900, 100, 800.0, 600.0);
    let one = vec![mon("A", 0, 0, 1920, 1080, 1.0)];
    assert_eq!(place(Some(&g), DEFAULT, MIN, &one, 0).pos, Some((560, 240)));
    // Title strip above the monitor.
    let g = geom(100, -300, 800.0, 600.0);
    assert_eq!(place(Some(&g), DEFAULT, MIN, &one, 0).pos, Some((560, 240)));
}

#[test]
fn place_larger_than_monitor_shrinks_to_fit() {
    let g = geom(0, 0, 4000.0, 3000.0);
    let one = vec![mon("A", 0, 0, 1920, 1080, 1.0)];
    let p = place(Some(&g), DEFAULT, MIN, &one, 0);
    assert_eq!(p.size, (1920.0 - 32.0, 1080.0 - 32.0));
}

#[test]
fn place_shrink_uses_logical_size_on_a_scaled_monitor() {
    // 3840x2160 physical at 200 % is 1920x1080 logical.
    let g = geom(0, 0, 4000.0, 3000.0);
    let one = vec![mon("A", 0, 0, 3840, 2160, 2.0)];
    let p = place(Some(&g), DEFAULT, MIN, &one, 0);
    assert_eq!(p.size, (1888.0, 1048.0));
}

#[test]
fn place_never_smaller_than_min_size() {
    let g = geom(10, 10, 10.0, 10.0);
    assert_eq!(place(Some(&g), DEFAULT, MIN, &two(), 0).size, MIN);
    // A tiny monitor does not push the window below the minimum either.
    let tiny = vec![mon("T", 0, 0, 300, 200, 1.0)];
    assert_eq!(place(None, DEFAULT, MIN, &tiny, 0).size, MIN);
}

#[test]
fn place_no_monitors_leaves_position_to_os() {
    let g = geom(100, 100, 800.0, 600.0);
    let p = place(Some(&g), DEFAULT, MIN, &[], 0);
    assert_eq!(p.pos, None);
    assert_eq!(p.size, (800.0, 600.0));
    assert_eq!(place(None, DEFAULT, MIN, &[], 0).size, DEFAULT);
}

#[test]
fn place_zero_sized_monitors_are_ignored() {
    let ms = vec![mon("Z", 0, 0, 0, 0, 1.0), mon("A", 100, 50, 1000, 800, 1.0)];
    let p = place(None, (400.0, 300.0), MIN, &ms, 0);
    // Index 0 is degenerate: the first usable monitor stands in for the primary.
    assert_eq!(p.pos, Some((100 + 300, 50 + 250)));
    assert_eq!(place(None, DEFAULT, MIN, &[ms[0].clone()], 0).pos, None);
}

#[test]
fn place_no_saved_geometry_centres_default_on_primary() {
    let p = place(None, (800.0, 600.0), MIN, &two(), 1);
    assert_eq!(p.pos, Some((1920 + 560, 240)));
    assert_eq!(p.size, (800.0, 600.0));
}

#[test]
fn place_negative_coordinates_left_of_primary_are_kept() {
    let ms = vec![
        mon("Primary", 0, 0, 1920, 1080, 1.0),
        mon("Left", -1920, 0, 1920, 1080, 1.0),
    ];
    let g = WindowGeom {
        monitor: Some("Left".into()),
        ..geom(-1500, 200, 800.0, 600.0)
    };
    assert_eq!(
        place(Some(&g), DEFAULT, MIN, &ms, 0).pos,
        Some((-1500, 200))
    );
    // Gone: centred on the primary, not on a negative origin.
    assert_eq!(
        place(Some(&g), DEFAULT, MIN, &ms[..1], 0).pos,
        Some((560, 240))
    );
}

#[test]
fn place_maximised_kept() {
    let g = WindowGeom {
        maximised: true,
        ..geom(100, 100, 800.0, 600.0)
    };
    let p = place(Some(&g), DEFAULT, MIN, &two(), 0);
    assert!(p.maximised);
    assert_eq!(p.pos, Some((100, 100)));
    assert_eq!(p.size, (800.0, 600.0));
}

#[test]
fn place_maximised_with_monitor_gone_restores_on_primary() {
    let g = WindowGeom {
        maximised: true,
        monitor: Some("gone".into()),
        ..geom(9000, 9000, 800.0, 600.0)
    };
    let p = place(Some(&g), DEFAULT, MIN, &two(), 0);
    assert!(p.maximised);
    assert_eq!(p.pos, Some((560, 240)));
}

#[test]
fn place_not_finite_saved_size_falls_back_to_default() {
    let g = geom(10, 10, f64::NAN, -5.0);
    assert_eq!(
        place(Some(&g), (800.0, 600.0), MIN, &two(), 0).size,
        (800.0, 600.0)
    );
}

#[test]
fn place_primary_index_out_of_range_uses_first_monitor() {
    let p = place(None, (800.0, 600.0), MIN, &two(), 9);
    assert_eq!(p.pos, Some((560, 240)));
}

fn rect(
    kind: WindowKind,
    (x, y): (f64, f64),
    (width, height): (f64, f64),
    scale: f64,
    space: DesktopSpace,
) -> WindowRect {
    WindowRect {
        kind,
        x,
        y,
        width,
        height,
        scale,
        space,
    }
}

#[test]
fn window_under_front_window_wins() {
    let rects = [
        rect(
            WindowKind::Main,
            (0.0, 0.0),
            (1000.0, 800.0),
            1.0,
            DesktopSpace::Physical,
        ),
        rect(
            WindowKind::Rack,
            (500.0, 100.0),
            (1000.0, 600.0),
            1.0,
            DesktopSpace::Physical,
        ),
    ];
    let mut z = ZOrder::default();
    z.focus(WindowKind::Main);
    z.focus(WindowKind::Rack);
    let hit = window_under(&rects, (600.0, 200.0), &z).unwrap();
    assert_eq!(hit.kind, WindowKind::Rack);
    assert_eq!(hit.logical, (100.0, 100.0));
    z.focus(WindowKind::Main);
    assert_eq!(
        window_under(&rects, (600.0, 200.0), &z).unwrap().kind,
        WindowKind::Main
    );
}

#[test]
fn window_under_outside_all_is_none() {
    let rects = [rect(
        WindowKind::Main,
        (0.0, 0.0),
        (100.0, 100.0),
        1.0,
        DesktopSpace::Physical,
    )];
    let z = ZOrder::default();
    assert_eq!(window_under(&rects, (100.0, 50.0), &z), None);
    assert_eq!(window_under(&rects, (-0.5, 50.0), &z), None);
    assert!(window_under(&rects, (99.9, 99.9), &z).is_some());
}

#[test]
fn window_under_skips_zero_sized_and_unordered_windows_sit_behind() {
    let r = |k, w| rect(k, (0.0, 0.0), (w, w), 1.0, DesktopSpace::Physical);
    let rects = [
        r(WindowKind::Main, 100.0),
        r(WindowKind::Rack, 0.0),
        r(WindowKind::Console, 100.0),
    ];
    let mut z = ZOrder::default();
    z.focus(WindowKind::Rack);
    // The focused Rack has no area, so it is skipped; Main and Console are unordered and
    // the first rect wins.
    assert_eq!(
        window_under(&rects, (5.0, 5.0), &z).unwrap().kind,
        WindowKind::Main
    );
    z.focus(WindowKind::Console);
    assert_eq!(
        window_under(&rects, (5.0, 5.0), &z).unwrap().kind,
        WindowKind::Console
    );
}

#[test]
fn window_under_returns_logical_position_for_scaled_physical_window() {
    // A 150 % window at physical x = 1920: a point 300 physical px in is 200 logical.
    let rects = [rect(
        WindowKind::Rack,
        (1920.0, 0.0),
        (1500.0, 900.0),
        1.5,
        DesktopSpace::Physical,
    )];
    let hit = window_under(&rects, (2220.0, 150.0), &ZOrder::default()).unwrap();
    assert_eq!(hit.logical, (200.0, 100.0));
}

#[test]
fn to_desktop_mixed_scales_physical_maps_between_windows() {
    // Main at 100 % at the origin, Rack at 200 % starting at physical x = 1920. A pointer at
    // local physical (2000, 40) in Main is at desktop (2000, 40), 80 px inside the Rack.
    let main = to_desktop(DesktopSpace::Physical, (0.0, 0.0), 1.0, (2000.0, 40.0));
    assert_eq!(main, (2000.0, 40.0));
    let rects = [rect(
        WindowKind::Rack,
        (1920.0, 0.0),
        (1600.0, 1200.0),
        2.0,
        DesktopSpace::Physical,
    )];
    let hit = window_under(&rects, main, &ZOrder::default()).unwrap();
    assert_eq!(hit.logical, (40.0, 20.0));
}

#[test]
fn to_desktop_points_space_uses_each_window_scale() {
    // macOS: the origin is in points and the pointer arrives in physical pixels of the window
    // it started in, so each window divides by its own scale.
    let a = to_desktop(DesktopSpace::Points, (100.0, 50.0), 2.0, (400.0, 200.0));
    assert_eq!(a, (300.0, 150.0));
    let b = to_desktop(DesktopSpace::Points, (100.0, 50.0), 1.0, (400.0, 200.0));
    assert_eq!(b, (500.0, 250.0));
    let rects = [rect(
        WindowKind::Console,
        (200.0, 100.0),
        (800.0, 600.0),
        2.0,
        DesktopSpace::Points,
    )];
    let hit = window_under(&rects, a, &ZOrder::default()).unwrap();
    assert_eq!(hit.logical, (100.0, 50.0));
}

#[test]
fn to_desktop_zero_scale_is_treated_as_one() {
    assert_eq!(
        to_desktop(DesktopSpace::Points, (0.0, 0.0), 0.0, (10.0, 20.0)),
        (10.0, 20.0)
    );
}

#[test]
fn reconcile_creates_missing_and_closes_extra() {
    use WindowKind::*;
    let r = reconcile(&[Main, Console], &[Main, Rack]);
    assert_eq!(r.create, vec![Rack]);
    assert_eq!(r.close, vec![Console]);
    assert_eq!(reconcile(&[Main], &[Main]), Reconcile::default());
}

#[test]
fn reconcile_never_closes_main_and_ignores_duplicates() {
    use WindowKind::*;
    let r = reconcile(&[Main, Rack, Rack], &[Console, Console]);
    assert_eq!(r.create, vec![Console]);
    assert_eq!(r.close, vec![Rack]);
}

#[test]
fn zorder_focus_moves_window_to_front() {
    use WindowKind::*;
    let mut z = ZOrder::default();
    z.focus(Main);
    z.focus(Rack);
    z.focus(Console);
    assert_eq!(z.front_to_back(), &[Console, Rack, Main]);
    z.focus(Main);
    z.focus(Main);
    assert_eq!(z.front_to_back(), &[Main, Console, Rack]);
    z.remove(Console);
    assert_eq!(z.front_to_back(), &[Main, Rack]);
}

#[test]
fn ui_state_without_windows_key_defaults_attached() {
    let s: crate::ui_state::UiState = serde_json::from_str(r#"{ "rack_split": 0.4 }"#).unwrap();
    assert_eq!(s.windows, WindowsState::default());
    assert!(!s.windows.rack.detached);
    assert!(!s.windows.console.detached);
    assert_eq!(s.windows.main, None);
}

#[test]
fn ui_state_windows_round_trip() {
    let mut s = crate::ui_state::UiState::default();
    s.windows.rack = DetachState {
        detached: true,
        geom: Some(WindowGeom {
            monitor: Some("DISPLAY2".into()),
            ..geom(-1800, 40, 1200.5, 700.0)
        }),
    };
    s.windows.main = Some(WindowGeom {
        maximised: true,
        ..geom(0, 0, 1280.0, 860.0)
    });
    let text = serde_json::to_string(&s).unwrap();
    let back: crate::ui_state::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(back, s);
}

#[test]
fn window_kind_serialises_lowercase_and_orders_main_first() {
    assert_eq!(
        serde_json::to_string(&WindowKind::Main).unwrap(),
        "\"main\""
    );
    assert_eq!(
        serde_json::to_string(&WindowKind::Rack).unwrap(),
        "\"rack\""
    );
    assert_eq!(
        serde_json::to_string(&WindowKind::Console).unwrap(),
        "\"console\""
    );
    assert!(WindowKind::Main < WindowKind::Rack && WindowKind::Rack < WindowKind::Console);
}

#[test]
fn windows_state_accessors_address_each_kind() {
    let mut w = WindowsState::default();
    w.set_detached(WindowKind::Console, true);
    w.set_geom(WindowKind::Rack, geom(1, 2, 3.0, 4.0));
    w.set_geom(WindowKind::Main, geom(5, 6, 7.0, 8.0));
    assert!(w.detached(WindowKind::Console));
    assert!(!w.detached(WindowKind::Rack));
    // The main window is never detached.
    w.set_detached(WindowKind::Main, true);
    assert!(!w.detached(WindowKind::Main));
    assert_eq!(w.geom(WindowKind::Rack).unwrap().x, 1);
    assert_eq!(w.geom(WindowKind::Main).unwrap().x, 5);
    assert_eq!(w.geom(WindowKind::Console), None);
    assert_eq!(w.wanted(), vec![WindowKind::Main, WindowKind::Console]);
}

#[test]
fn ui_state_sanitized_drops_unusable_window_geometry() {
    let mut s = crate::ui_state::UiState::default();
    s.windows.main = Some(geom(0, 0, f64::NAN, 100.0));
    s.windows.rack.geom = Some(geom(0, 0, 100.0, 0.0));
    s.windows.console.geom = Some(geom(0, 0, 100.0, 100.0));
    let s = s.sanitized();
    assert_eq!(s.windows.main, None);
    assert_eq!(s.windows.rack.geom, None);
    assert!(s.windows.console.geom.is_some());
}

fn three() -> Vec<MonitorInfo> {
    vec![
        mon("A", 0, 0, 1920, 1080, 1.0),
        mon("B", 1920, 0, 1920, 1080, 1.0),
        mon("C", 3840, 0, 1920, 1080, 1.0),
    ]
}

/// An open app window of `kind` at `(x, y)` on the desktop, 800x600.
fn open(kind: WindowKind, x: f64, y: f64) -> WindowRect {
    rect(kind, (x, y), (800.0, 600.0), 1.0, DesktopSpace::Physical)
}

#[test]
fn place_detached_three_monitors_rack_and_console_take_the_two_free_ones() {
    let monitors = three();
    let mut occupied = vec![open(WindowKind::Main, 100.0, 100.0)];
    let rack = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(rack.pos, Some((1920 + 320, 180)));
    assert!(rack.maximised);
    occupied.push(open(WindowKind::Rack, 2000.0, 100.0));
    let console = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(console.pos, Some((3840 + 320, 180)));
    assert!(console.maximised);
}

#[test]
fn place_detached_two_monitors_rack_goes_to_the_second_and_console_cascades() {
    let monitors = two();
    let mut occupied = vec![open(WindowKind::Main, 100.0, 100.0)];
    let rack = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(rack.pos, Some((1920 + 320, 180)));
    assert!(rack.maximised);
    occupied.push(open(WindowKind::Rack, 2000.0, 100.0));
    let console = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(console.pos, Some((132, 132)));
    assert_eq!(console.size, DEFAULT);
    assert!(!console.maximised);
}

#[test]
fn place_detached_one_monitor_cascades_from_the_main_window() {
    let monitors = vec![mon("A", 0, 0, 1920, 1080, 1.0)];
    let occupied = [open(WindowKind::Main, 100.0, 100.0)];
    let p = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(p.pos, Some((132, 132)));
    assert_eq!(p.size, DEFAULT);
    assert!(!p.maximised);
}

#[test]
fn place_detached_cascade_is_clamped_onto_the_monitor() {
    let monitors = vec![mon("A", 0, 0, 1920, 1080, 1.0)];
    let occupied = [open(WindowKind::Main, 900.0, 500.0)];
    let p = place_detached(None, DEFAULT, MIN, &monitors, 0, &occupied);
    assert_eq!(p.pos, Some((640, 360)));
}

#[test]
fn place_detached_main_on_the_right_monitor_wraps_to_the_left() {
    let monitors = two();
    let occupied = [open(WindowKind::Main, 2000.0, 100.0)];
    let p = place_detached(None, DEFAULT, MIN, &monitors, 1, &occupied);
    assert_eq!(p.pos, Some((320, 180)));
    assert!(p.maximised);
}

#[test]
fn place_detached_main_on_the_middle_monitor_rack_goes_right_then_console_left() {
    let monitors = three();
    let mut occupied = vec![open(WindowKind::Main, 2000.0, 100.0)];
    let rack = place_detached(None, DEFAULT, MIN, &monitors, 1, &occupied);
    assert_eq!(rack.pos, Some((3840 + 320, 180)));
    occupied.push(open(WindowKind::Rack, 3900.0, 100.0));
    let console = place_detached(None, DEFAULT, MIN, &monitors, 1, &occupied);
    assert_eq!(console.pos, Some((320, 180)));
}

#[test]
fn place_detached_monitors_are_ordered_left_to_right_whatever_the_list_order() {
    let monitors = vec![
        mon("C", 3840, 0, 1920, 1080, 1.0),
        mon("A", 0, 0, 1920, 1080, 1.0),
        mon("B", 1920, 0, 1920, 1080, 1.0),
    ];
    let occupied = [open(WindowKind::Main, 100.0, 100.0)];
    let p = place_detached(None, DEFAULT, MIN, &monitors, 1, &occupied);
    assert_eq!(p.pos, Some((1920 + 320, 180)));
}

#[test]
fn place_detached_saved_geometry_wins_over_a_free_monitor() {
    let g = WindowGeom {
        monitor: Some("A".into()),
        ..geom(300, 200, 800.0, 600.0)
    };
    let occupied = [open(WindowKind::Main, 100.0, 100.0)];
    let p = place_detached(Some(&g), DEFAULT, MIN, &three(), 0, &occupied);
    assert_eq!(p.pos, Some((300, 200)));
    assert_eq!(p.size, (800.0, 600.0));
    assert!(!p.maximised);
}

#[test]
fn place_detached_unusable_saved_geometry_counts_as_none() {
    let g = geom(300, 200, 0.0, 0.0);
    let occupied = [open(WindowKind::Main, 100.0, 100.0)];
    let p = place_detached(Some(&g), DEFAULT, MIN, &two(), 0, &occupied);
    assert!(p.maximised);
    assert_eq!(p.pos, Some((1920 + 320, 180)));
}

#[test]
fn place_detached_no_monitors_leaves_position_to_the_os() {
    let p = place_detached(None, DEFAULT, MIN, &[], 0, &[]);
    assert_eq!(p.pos, None);
}
