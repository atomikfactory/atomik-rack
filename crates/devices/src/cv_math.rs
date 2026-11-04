//! `atomik.cv-math`: arithmetic on two control voltages.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! a = a_in                       b = b_in when a cable is attached, the `b` knob otherwise
//! v = op(a, b)                   add, subtract (a - b), multiply, minimum, maximum, |a|
//! out = v * scale + offset       then limited to +-8, or to +-1 with `clip` on
//! ```
//!
//! * **Operations.** `add` is `a + b`, `subtract` is `a - b`, `multiply` is `a * b`, `minimum` and
//!   `maximum` pick the smaller or larger of the two, `absolute` is `|a|` (b is ignored).
//! * **b.** With a cable on `b_in` the second operand follows it sample by sample; without one it
//!   is the `b` knob, so one input and a constant is the common case (add an offset, multiply by a
//!   gain, take the larger of a CV and a floor).
//! * **Scale and offset** shape the result: `scale` runs -4..+4 (a negative scale inverts),
//!   `offset` -1..+1.
//! * **Limits.** Each input is limited to +-8 first (a non-finite sample counts as 0); the
//!   output is limited to +-8, or +-1 when `clip` is on.
//! * No latency and no state; allocation-free.

use crate::tool_support::{choice, cv_in, cv_out, finite, linear, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};

const P_OP: u32 = 0;
const P_B: u32 = 1;
const P_SCALE: u32 = 2;
const P_OFFSET: u32 = 3;
const P_CLIP: u32 = 4;

const A_IN: SocketId = SocketId(0);
const B_IN: SocketId = SocketId(1);
const OUT: SocketId = SocketId(2);

/// Input and output limit.
const LIMIT: f32 = 8.0;

static OPS: [&str; 6] = [
    "add", "subtract", "multiply", "minimum", "maximum", "absolute",
];

static PARAMS: [ParamDescriptor; 5] = [
    choice(P_OP, "op", "Operation", &OPS, 0.0),
    linear(P_B, "b", "B", -1.0, 1.0, 0.0, Unit::None),
    linear(P_SCALE, "scale", "Scale", -4.0, 4.0, 1.0, Unit::None),
    linear(P_OFFSET, "offset", "Offset", -1.0, 1.0, 0.0, Unit::None),
    toggle(P_CLIP, "clip", "Clip", 0.0),
];

static SOCKETS: [SocketDescriptor; 3] = [
    cv_in(0, "a_in", "A in"),
    cv_in(1, "b_in", "B in"),
    cv_out(2, "out", "Out"),
];

/// Static description of CV Math.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.cv-math",
    name: "CV Math",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`CvMath`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(CvMath::default()),
    }
}

/// The CV arithmetic device.
#[derive(Debug, Clone, Copy)]
pub struct CvMath {
    op: u8,
    b: f32,
    scale: f32,
    offset: f32,
    clip: bool,
}

impl Default for CvMath {
    fn default() -> Self {
        Self {
            op: 0,
            b: 0.0,
            scale: 1.0,
            offset: 0.0,
            clip: false,
        }
    }
}

#[inline]
fn limit(x: f32) -> f32 {
    finite(x).clamp(-LIMIT, LIMIT)
}

impl Device for CvMath {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_OP => self.op = v.clamp(0.0, (OPS.len() - 1) as f32).round() as u8,
            P_B => self.b = v.clamp(-1.0, 1.0),
            P_SCALE => self.scale = v.clamp(-4.0, 4.0),
            P_OFFSET => self.offset = v.clamp(-1.0, 1.0),
            P_CLIP => self.clip = v >= 0.5,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let a = *ctx.input(A_IN);
        let b_in = *ctx.input(B_IN);
        let b_cabled = ctx.is_connected(B_IN);
        let bound = if self.clip { 1.0 } else { LIMIT };
        let mut out = [0.0f32; BATCH];
        for n in 0..BATCH {
            let x = limit(a[n]);
            let y = if b_cabled { limit(b_in[n]) } else { self.b };
            let v = match self.op {
                0 => x + y,
                1 => x - y,
                2 => x * y,
                3 => x.min(y),
                4 => x.max(y),
                _ => x.abs(),
            };
            out[n] = (v * self.scale + self.offset).clamp(-bound, bound);
        }
        *ctx.output(OUT) = out;
    }
}

#[cfg(test)]
#[allow(
    clippy::needless_range_loop,
    clippy::float_cmp,
    clippy::type_complexity
)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};

    /// One batch with `a` and, when given, a cable of `b` (else `b_in` is left unconnected).
    fn run(h: &mut atomik_testkit::harness::DeviceHarness, a: f32, b: Option<f32>) -> f32 {
        *h.input_mut("a_in") = [a; BATCH];
        match b {
            Some(b) => *h.input_mut("b_in") = [b; BATCH],
            None => h.set_connected("b_in", false),
        }
        h.process();
        h.output("out")[BATCH - 1]
    }

    #[test]
    fn descriptor_lists_the_keys_and_ops_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &["op", "b", "scale", "offset", "clip"],
            &["a_in", "b_in", "out"],
        );
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    }

    #[test]
    fn each_operation_combines_a_and_the_cabled_b() {
        let mut h = harness(Box::new(CvMath::default()));
        let cases = [
            (0.0, 0.5, 0.25, 0.75),
            (1.0, 0.5, 0.25, 0.25),
            (2.0, 0.5, -0.5, -0.25),
            (3.0, 0.5, -0.5, -0.5),
            (4.0, 0.5, -0.5, 0.5),
            (5.0, -0.5, 0.75, 0.5),
        ];
        for (op, a, b, want) in cases {
            h.set_param("op", op);
            assert!((run(&mut h, a, Some(b)) - want).abs() < 1e-6, "op {op}");
        }
    }

    #[test]
    fn the_b_knob_stands_in_when_nothing_is_cabled() {
        let mut h = harness(Box::new(CvMath::default()));
        h.set_param("b", 0.25);
        assert!((run(&mut h, 0.5, None) - 0.75).abs() < 1e-6);
        h.set_param("op", 2.0);
        assert!((run(&mut h, 0.5, None) - 0.125).abs() < 1e-6);
        // A cable takes over from the knob.
        assert!((run(&mut h, 0.5, Some(0.5)) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn scale_and_offset_follow_the_operation() {
        let mut h = harness(Box::new(CvMath::default()));
        h.set_param("scale", -2.0);
        h.set_param("offset", 0.5);
        h.set_param("b", 0.0);
        // (0.25 + 0) * -2 + 0.5 = 0
        assert!(run(&mut h, 0.25, None).abs() < 1e-6);
        // Defaults leave a signal alone.
        let mut d = harness(Box::new(CvMath::default()));
        let x = run(&mut d, 0.3, None);
        assert_eq!(x, 0.3);
    }

    #[test]
    fn clip_limits_to_one_otherwise_to_eight() {
        let mut h = harness(Box::new(CvMath::default()));
        h.set_param("op", 2.0);
        h.set_param("scale", 4.0);
        assert_eq!(run(&mut h, 7.0, Some(7.0)), LIMIT);
        assert_eq!(run(&mut h, -7.0, Some(7.0)), -LIMIT);
        h.set_param("clip", 1.0);
        assert_eq!(run(&mut h, 7.0, Some(7.0)), 1.0);
        assert_eq!(run(&mut h, -7.0, Some(7.0)), -1.0);
    }

    #[test]
    fn non_finite_input_counts_as_zero_and_sweeps_stay_finite() {
        let mut h = harness(Box::new(CvMath::default()));
        *h.input_mut("a_in") = [f32::NAN; BATCH];
        *h.input_mut("b_in") = [f32::INFINITY; BATCH];
        h.process();
        assert!(h.output("out").iter().all(|v| *v == 0.0));
        *h.input_mut("a_in") = [0.25; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn it_follows_a_ramp_sample_by_sample() {
        let mut h = harness(Box::new(CvMath::default()));
        h.set_param("op", 2.0);
        let mut a = [0.0f32; BATCH];
        for (i, v) in a.iter_mut().enumerate() {
            *v = i as f32 / BATCH as f32;
        }
        *h.input_mut("a_in") = a;
        *h.input_mut("b_in") = a;
        h.process();
        for i in 0..BATCH {
            assert_eq!(h.output("out")[i], a[i] * a[i]);
        }
    }
}
