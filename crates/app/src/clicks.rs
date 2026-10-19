//! Click counting (the UI crate takes click counts from the app): a press of the same button
//! within [`DOUBLE_CLICK_TIME`] and [`DOUBLE_CLICK_DISTANCE`] of the previous one continues the
//! sequence (2 = double-click, 3 = triple…).

use atomik_ui::PointerButton;
use atomik_ui::kurbo::Point;
use std::time::Duration;

/// Longest gap between presses of one multi-click.
pub const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(400);
/// Largest pointer travel (logical pixels) between presses of one multi-click.
pub const DOUBLE_CLICK_DISTANCE: f64 = 4.0;

/// Counts consecutive clicks. Times are offsets from any fixed epoch (the app uses its start).
#[derive(Debug, Clone, Default)]
pub struct ClickCounter {
    last: Option<(Duration, Point, PointerButton, u8)>,
}

impl ClickCounter {
    /// No previous click.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a press and returns its click count (1, 2, 3…; saturates at 255).
    pub fn press(&mut self, at: Duration, pos: Point, button: PointerButton) -> u8 {
        let count = match self.last {
            Some((t, p, b, n))
                if b == button
                    && at.saturating_sub(t) <= DOUBLE_CLICK_TIME
                    && p.distance(pos) <= DOUBLE_CLICK_DISTANCE =>
            {
                n.saturating_add(1)
            }
            _ => 1,
        };
        self.last = Some((at, pos, button, count));
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    const L: PointerButton = PointerButton::Primary;

    #[test]
    fn a_single_press_counts_one() {
        let mut c = ClickCounter::new();
        assert_eq!(c.press(ms(1000), Point::new(10.0, 10.0), L), 1);
    }

    #[test]
    fn a_quick_second_press_nearby_is_a_double_click_and_a_third_a_triple() {
        let mut c = ClickCounter::new();
        c.press(ms(1000), Point::new(10.0, 10.0), L);
        assert_eq!(c.press(ms(1300), Point::new(12.0, 13.0), L), 2);
        assert_eq!(c.press(ms(1650), Point::new(12.0, 13.0), L), 3);
    }

    #[test]
    fn the_time_limit_is_inclusive_at_400_ms() {
        let mut c = ClickCounter::new();
        c.press(ms(0), Point::ZERO, L);
        assert_eq!(c.press(ms(400), Point::ZERO, L), 2);
        assert_eq!(c.press(ms(801), Point::ZERO, L), 1);
    }

    #[test]
    fn moving_more_than_4_px_starts_a_new_sequence() {
        let mut c = ClickCounter::new();
        c.press(ms(0), Point::ZERO, L);
        assert_eq!(c.press(ms(100), Point::new(4.0, 0.0), L), 2);
        assert_eq!(c.press(ms(200), Point::new(8.1, 0.0), L), 1);
    }

    #[test]
    fn another_button_starts_a_new_sequence() {
        let mut c = ClickCounter::new();
        c.press(ms(0), Point::ZERO, L);
        assert_eq!(c.press(ms(100), Point::ZERO, PointerButton::Secondary), 1);
        assert_eq!(c.press(ms(200), Point::ZERO, L), 1);
    }

    #[test]
    fn the_count_saturates() {
        let mut c = ClickCounter::new();
        let mut last = 0;
        for i in 0..300 {
            last = c.press(ms(i * 10), Point::ZERO, L);
        }
        assert_eq!(last, 255);
    }
}
