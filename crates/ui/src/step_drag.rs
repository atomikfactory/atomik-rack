//! Drag and wheel stepping for value readouts (the tempo, the signature, the snap selector, the
//! quantize panel's rows).
//!
//! A press on the readout starts a [`StepDrag`]; every [`PX_PER_STEP`] logical pixels the pointer
//! moves up the value goes one step up, down one step down. The pixels not yet worth a step are
//! kept, so a change of step size mid-drag (Shift for the fine step) never jumps and a drag that
//! reverses at the end of a range answers at once. One wheel notch is one step
//! ([`wheel_steps`]). The values themselves (clamping, lists, decimals) belong to the caller.

/// Logical pixels of vertical pointer travel per step of a stepped value.
pub const PX_PER_STEP: f64 = 5.0;

/// Pointer travel that starts counting as a drag rather than a click, logical pixels.
pub const DRAG_SLOP: f64 = 3.0;

/// The vertical drag of one readout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepDrag {
    last_y: f64,
    /// Pixels travelled (up is positive) that have not made a step yet, in steps.
    rest: f64,
    start_y: f64,
    /// The pointer has been [`DRAG_SLOP`] or more from the press (it stays a drag).
    moved: bool,
}

impl StepDrag {
    /// A drag that starts with the pointer at `y`.
    pub fn new(y: f64) -> Self {
        Self {
            last_y: y,
            rest: 0.0,
            start_y: y,
            moved: false,
        }
    }

    /// The pointer is now at `y`: the whole steps to apply (positive = up) since the last call.
    /// `px_per_step` is the travel one step takes now (larger for the fine step).
    pub fn update(&mut self, y: f64, px_per_step: f64) -> i32 {
        let dy = self.last_y - y;
        self.last_y = y;
        self.moved |= (y - self.start_y).abs() >= DRAG_SLOP;
        self.rest += dy / px_per_step.max(0.5);
        let whole = self.rest.trunc();
        self.rest -= whole;
        whole as i32
    }

    /// True once the pointer has moved far enough to be a drag and not a click.
    pub fn moved(&self) -> bool {
        self.moved
    }
}

/// The steps of one wheel event: one notch is one step, up (positive `delta_y`) increases, a
/// touchpad's small deltas still make one step each. Zero makes none.
pub fn wheel_steps(delta_y: f64) -> i32 {
    if delta_y > 0.0 {
        1
    } else if delta_y < 0.0 {
        -1
    } else {
        0
    }
}

/// The index `at + steps` in a list of `len` entries, clamped to the ends (no wrap).
pub fn step_index(at: usize, steps: i32, len: usize) -> usize {
    let last = len.saturating_sub(1) as i64;
    (at as i64 + i64::from(steps)).clamp(0, last) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_takes_px_per_step_of_travel_up_or_down() {
        let mut d = StepDrag::new(100.0);
        assert_eq!(d.update(97.0, PX_PER_STEP), 0);
        assert_eq!(
            d.update(94.0, PX_PER_STEP),
            1,
            "6 px up: one step, 1 px kept"
        );
        assert_eq!(d.update(79.0, PX_PER_STEP), 3, "15 px more: three steps");
        assert_eq!(d.update(100.0, PX_PER_STEP), -4, "back down 21 px");
    }

    #[test]
    fn the_rest_is_kept_and_a_new_step_size_does_not_jump() {
        let mut d = StepDrag::new(0.0);
        assert_eq!(d.update(-4.0, PX_PER_STEP), 0);
        // Shift pressed: ten times the travel per step; the 4 px already made stay a fraction.
        assert_eq!(d.update(-8.0, PX_PER_STEP * 10.0), 0);
        assert_eq!(d.update(-50.0, PX_PER_STEP * 10.0), 1);
    }

    #[test]
    fn a_click_is_not_a_drag_until_the_slop() {
        let mut d = StepDrag::new(10.0);
        d.update(11.0, PX_PER_STEP);
        d.update(9.0, PX_PER_STEP);
        assert!(!d.moved());
        d.update(5.0, PX_PER_STEP);
        assert!(d.moved());
    }

    #[test]
    fn a_wheel_notch_is_one_step() {
        assert_eq!(wheel_steps(1.0), 1);
        assert_eq!(wheel_steps(3.0), 1);
        assert_eq!(wheel_steps(-0.2), -1);
        assert_eq!(wheel_steps(0.0), 0);
    }

    #[test]
    fn list_steps_clamp_at_both_ends() {
        assert_eq!(step_index(1, 1, 4), 2);
        assert_eq!(step_index(3, 5, 4), 3);
        assert_eq!(step_index(1, -9, 4), 0);
        assert_eq!(step_index(0, 1, 0), 0);
    }
}
