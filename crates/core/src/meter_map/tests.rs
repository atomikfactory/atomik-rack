use super::*;

fn meter(events: &[(u32, u8, u8)]) -> MeterTable {
    let events: Vec<MeterEvent> = events
        .iter()
        .map(|&(bar, num, den)| MeterEvent { bar, num, den })
        .collect();
    MeterTable::new(&events).unwrap()
}

#[test]
fn compile_meter_rejects_bad_maps() {
    let ev = |bar, num, den| MeterEvent { bar, num, den };
    assert_eq!(compile_meter(&[]), Err(MapError::MeterStart));
    assert_eq!(compile_meter(&[ev(2, 4, 4)]), Err(MapError::MeterStart));
    assert_eq!(
        compile_meter(&[ev(1, 4, 4), ev(1, 3, 4)]),
        Err(MapError::MeterOrder)
    );
    assert_eq!(
        compile_meter(&[ev(1, 4, 3)]),
        Err(MapError::MeterRange(4, 3))
    );
    assert_eq!(
        compile_meter(&[ev(1, 33, 4)]),
        Err(MapError::MeterRange(33, 4))
    );
    assert_eq!(
        compile_meter(&[ev(1, 0, 4)]),
        Err(MapError::MeterRange(0, 4))
    );
}

#[test]
fn bars_and_ticks_convert_through_meter_changes() {
    // 4/4 for bars 1-2, 3/4 for bars 3-4, 6/8 from bar 5.
    let t = meter(&[(1, 4, 4), (3, 3, 4), (5, 6, 8)]);
    let m = t.map();
    let (b44, b34, b68) = (4 * PPQ, 3 * PPQ, 3 * PPQ);
    assert_eq!(m.bar_start(1), 0);
    assert_eq!(m.bar_start(2), b44);
    assert_eq!(m.bar_start(3), 2 * b44);
    assert_eq!(m.bar_start(4), 2 * b44 + b34);
    assert_eq!(m.bar_start(5), 2 * b44 + 2 * b34);
    assert_eq!(m.bar_start(7), 2 * b44 + 2 * b34 + 2 * b68);
    assert_eq!(m.bar_start(0), 0);
    assert_eq!(m.meter_at(0), (4, 4));
    assert_eq!(m.meter_at(2 * b44 - 1), (4, 4));
    assert_eq!(m.meter_at(2 * b44), (3, 4));
    assert_eq!(m.meter_at(2 * b44 + 2 * b34), (6, 8));
    assert_eq!(m.meter_at(-9), (4, 4));
    assert_eq!(m.beat_ticks_at(2 * b44 + 2 * b34), PPQ / 2);
    assert_eq!(m.bar_ticks_at(2 * b44), b34);
    for bar in 1..12u32 {
        let start = m.bar_start(bar);
        assert_eq!(m.bar_at(start), bar, "bar {bar}");
        assert_eq!(m.bar_at(start + 1), bar);
        assert_eq!(m.bar_at(m.bar_start(bar + 1) - 1), bar);
    }
    assert_eq!(m.bar_at(-1), 1);
}

#[test]
fn bbt_converts_with_a_changing_meter() {
    let t = meter(&[(1, 4, 4), (3, 3, 4), (5, 6, 8)]);
    let m = t.map();
    assert_eq!(m.bbt(0), (1, 1, 0));
    assert_eq!(m.bbt(PPQ + 5), (1, 2, 5));
    assert_eq!(m.bbt(2 * 4 * PPQ), (3, 1, 0));
    assert_eq!(m.bbt(2 * 4 * PPQ + 2 * PPQ), (3, 3, 0));
    assert_eq!(m.bbt(m.bar_start(5) + PPQ / 2 * 4 + 3), (5, 5, 3));
    assert_eq!(m.bbt(-3), (1, 1, 0));
    for tick in (0..30 * PPQ).step_by(1_237) {
        assert_eq!(m.from_bbt(m.bbt(tick)), tick, "tick {tick}");
    }
    assert_eq!(m.from_bbt((0, 0, 7)), 7);
}

#[test]
fn bar_lines_are_found_in_both_directions() {
    let t = meter(&[(1, 4, 4), (3, 3, 4)]);
    let m = t.map();
    let (b44, b34) = (4 * PPQ, 3 * PPQ);
    assert_eq!(m.next_bar_line(0), b44);
    assert_eq!(m.next_bar_line(b44 - 1), b44);
    assert_eq!(m.next_bar_line(b44), 2 * b44);
    assert_eq!(m.next_bar_line(2 * b44), 2 * b44 + b34);
    assert_eq!(m.prev_bar_line(0), 0);
    assert_eq!(m.prev_bar_line(5), 0);
    assert_eq!(m.prev_bar_line(b44), 0);
    assert_eq!(m.prev_bar_line(b44 + 1), b44);
    assert_eq!(m.prev_bar_line(2 * b44), b44);
    assert_eq!(m.prev_bar_line(2 * b44 + b34 + 1), 2 * b44 + b34);
}

#[test]
fn beats_walk_across_a_meter_change() {
    let t = meter(&[(1, 3, 4), (2, 6, 8)]);
    let m = t.map();
    let mut beats = Vec::new();
    let mut b = m.beat_at_or_before(0);
    while b.0 < 3 * PPQ + 3 * PPQ {
        beats.push(b);
        b = m.beat_after(b.0);
    }
    let h = PPQ / 2;
    assert_eq!(
        beats,
        vec![
            (0, true),
            (PPQ, false),
            (2 * PPQ, false),
            (3 * PPQ, true),
            (3 * PPQ + h, false),
            (3 * PPQ + 2 * h, false),
            (3 * PPQ + 3 * h, false),
            (3 * PPQ + 4 * h, false),
            (3 * PPQ + 5 * h, false),
        ]
    );
    assert_eq!(m.beat_at_or_before(PPQ + 100), (PPQ, false));
    assert_eq!(m.beat_at_or_before(-5), (0, true));
    // A 4/4 map is a click per quarter with the accent every fourth.
    let four = MeterTable::constant(4, 4);
    assert_eq!(four.map().beat_at_or_before(4 * PPQ + 1), (4 * PPQ, true));
    assert_eq!(four.map().beat_after(4 * PPQ), (5 * PPQ, false));
}
