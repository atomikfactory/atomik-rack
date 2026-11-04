//! `atomik.slew-limiter`: limits how fast a control voltage may rise and fall.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! linear       y moves toward x by at most 1 / time per sample   (a full 0 -> 1 swing takes `time`)
//! exponential  y += (x - y) * (1 - exp(-1 / time))                (`time` is the time constant)
//! ```
//!
//! * **Separate rise and fall.** `rise` is the time while the input is above the output (the
//!   output goes up), `fall` the time while it is below (the output goes down). Both run from 0.1
//!   ms to 10 s.
//! * **Linear** limits the slope: a jump of 1 takes the set time, a jump of 0.5 half of it, and the
//!   output reaches the input exactly (it never overshoots). **Exponential** is a one-pole
//!   smoother with separate rise and fall time constants: it covers 63 % of a jump in the set time
//!   and never quite arrives (it snaps when the gap is below 1e-9).
//! * The input is limited to +-8 (a non-finite sample counts as 0). The output starts at 0, and
//!   `reset` returns it there. The exponential coefficients come from the deterministic `2^x`.
//! * No latency; allocation-free.

use crate::tool_support::{choice, cv_in, cv_out, expo, finite, time_coef};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};

const P_RISE: u32 = 0;
const P_FALL: u32 = 1;
const P_SHAPE: u32 = 2;

const CV_IN: SocketId = SocketId(0);
const CV_OUT: SocketId = SocketId(1);

/// Input limit.
const LIMIT: f32 = 8.0;
/// An exponential output closer to its target than this snaps to it.
const SNAP: f64 = 1e-9;

static SHAPES: [&str; 2] = ["linear", "exponential"];

static PARAMS: [ParamDescriptor; 3] = [
    expo(P_RISE, "rise", "Rise", 0.1, 10_000.0, 100.0, Unit::Ms),
    expo(P_FALL, "fall", "Fall", 0.1, 10_000.0, 100.0, Unit::Ms),
    choice(P_SHAPE, "shape", "Shape", &SHAPES, 0.0),
];

static SOCKETS: [SocketDescriptor; 2] = [cv_in(0, "cv_in", "CV in"), cv_out(1, "cv_out", "CV out")];

/// Static description of Slew Limiter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.slew-limiter",
    name: "Slew Limiter",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`SlewLimiter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(SlewLimiter::new()),
    }
}

/// The slew limiter.
#[derive(Debug, Clone, Copy)]
pub struct SlewLimiter {
    rise_ms: f32,
    fall_ms: f32,
    exponential: bool,
    sample_rate: f32,
    /// Per-sample step of the linear shape, up and down.
    rise_step: f64,
    fall_step: f64,
    /// One-pole coefficients of the exponential shape.
    rise_coef: f64,
    fall_coef: f64,
    /// The output, in `f64` so a long time still makes progress at a small step.
    y: f64,
}

impl SlewLimiter {
    fn new() -> Self {
        let mut s = Self {
            rise_ms: 100.0,
            fall_ms: 100.0,
            exponential: false,
            sample_rate: 48_000.0,
            rise_step: 0.0,
            fall_step: 0.0,
            rise_coef: 0.0,
            fall_coef: 0.0,
            y: 0.0,
        };
        s.derive();
        s
    }

    fn derive(&mut self) {
        let step = |ms: f32| 1.0 / (f64::from(ms) * 0.001 * f64::from(self.sample_rate));
        self.rise_step = step(self.rise_ms);
        self.fall_step = step(self.fall_ms);
        self.rise_coef = time_coef(self.rise_ms, self.sample_rate);
        self.fall_coef = time_coef(self.fall_ms, self.sample_rate);
    }
}

impl Device for SlewLimiter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.y = 0.0;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_RISE => {
                self.rise_ms = v.clamp(0.1, 10_000.0);
                self.derive();
            }
            P_FALL => {
                self.fall_ms = v.clamp(0.1, 10_000.0);
                self.derive();
            }
            P_SHAPE => self.exponential = v >= 0.5,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let x = *ctx.input(CV_IN);
        let mut out = [0.0f32; BATCH];
        for n in 0..BATCH {
            let target = f64::from(finite(x[n]).clamp(-LIMIT, LIMIT));
            let rising = target > self.y;
            if self.exponential {
                let coef = if rising {
                    self.rise_coef
                } else {
                    self.fall_coef
                };
                self.y += (target - self.y) * coef;
                if (target - self.y).abs() < SNAP {
                    self.y = target;
                }
            } else if rising {
                self.y = (self.y + self.rise_step).min(target);
            } else {
                self.y = (self.y - self.fall_step).max(target);
            }
            out[n] = self.y as f32;
        }
        *ctx.output(CV_OUT) = out;
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::needless_range_loop)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;

    /// Feeds a constant `x` for `samples` samples and returns the output (whole batches).
    fn hold(h: &mut DeviceHarness, x: f32, samples: usize) -> Vec<f32> {
        *h.input_mut("cv_in") = [x; BATCH];
        h.render(samples.div_ceil(BATCH), &["cv_out"]).remove(0)
    }

    #[test]
    fn descriptor_lists_the_keys_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &["rise", "fall", "shape"],
            &["cv_in", "cv_out"],
        );
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    }

    #[test]
    fn linear_rises_and_falls_at_their_own_speed_and_arrives_exactly() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("rise", 10.0); // 480 samples for a swing of 1
        h.set_param("fall", 40.0); // 1920 samples
        let up = hold(&mut h, 1.0, 960);
        let rise_step = 1.0 / 480.0;
        assert!((up[239] - 240.0 * rise_step).abs() < 1e-5);
        assert_eq!(up[480], 1.0);
        assert_eq!(*up.last().unwrap(), 1.0);
        // Monotonic, never past the target.
        assert!(up.windows(2).all(|w| w[1] >= w[0] && w[1] <= 1.0));
        let down = hold(&mut h, 0.0, 3840);
        assert!((down[959] - (1.0 - 960.0 / 1920.0)).abs() < 1e-4);
        assert_eq!(*down.last().unwrap(), 0.0);
        // A half swing takes half the time.
        let half = hold(&mut h, 0.5, 960);
        assert_eq!(half[240], 0.5);
    }

    #[test]
    fn a_swing_is_limited_by_the_slope_whatever_its_size() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("rise", 10.0);
        let y = hold(&mut h, -1.0, 4800);
        assert_eq!(
            *y.last().unwrap(),
            -1.0,
            "falls at the (default 100 ms) fall time"
        );
        let y = hold(&mut h, 3.0, 4800);
        // A jump of 4 at 1/480 per sample takes 1920 samples.
        assert!((y[959] - (-1.0 + 960.0 / 480.0)).abs() < 1e-4);
        assert_eq!(y[1920], 3.0);
        assert!(y[1900] < 3.0);
    }

    #[test]
    fn exponential_follows_a_one_pole_with_separate_time_constants() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("shape", 1.0);
        h.set_param("rise", 10.0);
        h.set_param("fall", 100.0);
        let up = hold(&mut h, 1.0, 4800);
        let tau = 480usize; // 10 ms
        let want = 1.0 - (-1.0f64).exp();
        assert!((f64::from(up[tau - 1]) - want).abs() < 0.01);
        assert!(up.windows(2).all(|w| w[1] >= w[0]));
        assert!(up[4799] > 0.999);
        let down = hold(&mut h, 0.0, 9600);
        let tau = 4800usize;
        let want = (-1.0f64).exp();
        assert!((f64::from(down[tau - 1]) - want * f64::from(up[4799])).abs() < 0.01);
    }

    #[test]
    fn exponential_snaps_to_the_target_in_the_end() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("shape", 1.0);
        h.set_param("rise", 1.0);
        let y = hold(&mut h, 0.7, 48_000);
        assert_eq!(*y.last().unwrap(), 0.7);
    }

    #[test]
    fn a_fast_setting_follows_a_step_within_a_few_samples() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("rise", 0.1);
        h.set_param("fall", 0.1);
        let y = hold(&mut h, 1.0, BATCH);
        assert_eq!(y[5], 1.0); // 0.1 ms is 4.8 samples
        assert!(y[0] < 0.3);
    }

    #[test]
    fn reset_returns_the_output_to_zero() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        h.set_param("rise", 0.1);
        hold(&mut h, 1.0, 480);
        h.device_mut().reset();
        *h.input_mut("cv_in") = [0.0; BATCH];
        h.process();
        assert_eq!(h.output("cv_out")[0], 0.0);
    }

    #[test]
    fn non_finite_input_counts_as_zero_and_sweeps_stay_finite() {
        let mut h = harness(Box::new(SlewLimiter::new()));
        let y = hold(&mut h, f32::NAN, BATCH);
        assert!(y.iter().all(|v| *v == 0.0));
        *h.input_mut("cv_in") = [100.0; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
        assert!(h.output("cv_out").iter().all(|v| v.abs() <= LIMIT));
    }
}
