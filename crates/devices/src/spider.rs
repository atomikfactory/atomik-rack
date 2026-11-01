//! `atomik.spider-audio` and `atomik.spider-cv`: cable mergers and splitters.
//!
//! * **Spider Audio** has four stereo merge inputs `m1_l/r` .. `m4_l/r`, each a summing input (any
//!   number of cables, summed in source order like a console bus), and one stereo merge output
//!   `merge_out_l/r` carrying the sum of the four pairs added in the order 1, 2, 3, 4. Beside it
//!   a stereo split input `split_in_l/r` is copied to four stereo outputs `split1_out_l/r` ..
//!   `split4_out_l/r`.
//! * **Spider CV** has four CV merge inputs `in1` .. `in4` (summing inputs), each with a
//!   `trim1` .. `trim4` amount (-1..+1, default +1, so a negative trim inverts and a small one
//!   attenuates), added in that order to `sum_out`; and two split inputs `split1_in`, `split2_in`
//!   with a `split1_level`, `split2_level` amount (-1..+1, default +1), each giving a normal
//!   output `splitN_out` (`in * level`) and an inverted one `splitN_inv_out` (`-in * level`).
//!
//! Neither has latency or state; both are `DeviceKind::Utility` (no bypass). Non-finite input
//! samples count as 0. With a single cable on one merge input and the rest unconnected, the
//! merged output is that cable bit for bit (adding 0.0 changes no bits).

use crate::tool_support::{cv_in, cv_out, finite, sock, trim};
use atomik_core::{BATCH, ParamDescriptor, ParamId};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

const fn audio_sum(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Audio, Direction::SummingInput)
}

const fn audio_i(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Audio, Direction::Input)
}

const fn audio_o(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Audio, Direction::Output)
}

// ---------------------------------------------------------------------------------- audio

/// Merge input sockets `m{k}_l`/`m{k}_r` for `k` = 1..=4 start at id 0, two per pair.
const A_MERGE_OUT: u16 = 8;
const A_SPLIT_IN: u16 = 10;
const A_SPLIT_OUT: u16 = 12;

static AUDIO_SOCKETS: [SocketDescriptor; 20] = [
    audio_sum(0, "m1_l", "Merge 1 L"),
    audio_sum(1, "m1_r", "Merge 1 R"),
    audio_sum(2, "m2_l", "Merge 2 L"),
    audio_sum(3, "m2_r", "Merge 2 R"),
    audio_sum(4, "m3_l", "Merge 3 L"),
    audio_sum(5, "m3_r", "Merge 3 R"),
    audio_sum(6, "m4_l", "Merge 4 L"),
    audio_sum(7, "m4_r", "Merge 4 R"),
    audio_o(8, "merge_out_l", "Merged L"),
    audio_o(9, "merge_out_r", "Merged R"),
    audio_i(10, "split_in_l", "Split in L"),
    audio_i(11, "split_in_r", "Split in R"),
    audio_o(12, "split1_out_l", "Split 1 L"),
    audio_o(13, "split1_out_r", "Split 1 R"),
    audio_o(14, "split2_out_l", "Split 2 L"),
    audio_o(15, "split2_out_r", "Split 2 R"),
    audio_o(16, "split3_out_l", "Split 3 L"),
    audio_o(17, "split3_out_r", "Split 3 R"),
    audio_o(18, "split4_out_l", "Split 4 L"),
    audio_o(19, "split4_out_r", "Split 4 R"),
];

/// Static description of Spider Audio.
pub static AUDIO_DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.spider-audio",
    name: "Spider Audio",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &AUDIO_SOCKETS,
};

/// Factory registering [`SpiderAudio`].
pub fn audio_factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &AUDIO_DESCRIPTOR,
        create: || Box::new(SpiderAudio),
    }
}

/// The audio merger/splitter.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpiderAudio;

impl Device for SpiderAudio {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &AUDIO_DESCRIPTOR
    }

    fn prepare(&mut self, _info: &atomik_device_sdk::PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, _id: ParamId, _plain: f32) {}

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for side in 0..2u16 {
            let mut sum = [0.0f32; BATCH];
            for pair in 0..4u16 {
                let x = *ctx.input(SocketId(pair * 2 + side));
                for n in 0..BATCH {
                    sum[n] += finite(x[n]);
                }
            }
            *ctx.output(SocketId(A_MERGE_OUT + side)) = sum;
            let mut src = *ctx.input(SocketId(A_SPLIT_IN + side));
            for s in &mut src {
                *s = finite(*s);
            }
            for copy in 0..4u16 {
                *ctx.output(SocketId(A_SPLIT_OUT + copy * 2 + side)) = src;
            }
        }
    }
}

// ------------------------------------------------------------------------------------ CV

const C_SUM_OUT: u16 = 4;
const C_SPLIT_IN: u16 = 5;

static CV_PARAMS: [ParamDescriptor; 6] = [
    trim(0, "trim1", "Trim 1"),
    trim(1, "trim2", "Trim 2"),
    trim(2, "trim3", "Trim 3"),
    trim(3, "trim4", "Trim 4"),
    trim(4, "split1_level", "Split 1 level"),
    trim(5, "split2_level", "Split 2 level"),
];

static CV_SOCKETS: [SocketDescriptor; 11] = [
    sock(0, "in1", "In 1", SocketKind::Cv, Direction::SummingInput),
    sock(1, "in2", "In 2", SocketKind::Cv, Direction::SummingInput),
    sock(2, "in3", "In 3", SocketKind::Cv, Direction::SummingInput),
    sock(3, "in4", "In 4", SocketKind::Cv, Direction::SummingInput),
    cv_out(4, "sum_out", "Sum"),
    cv_in(5, "split1_in", "Split 1 in"),
    cv_in(6, "split2_in", "Split 2 in"),
    cv_out(7, "split1_out", "Split 1"),
    cv_out(8, "split1_inv_out", "Split 1 inverted"),
    cv_out(9, "split2_out", "Split 2"),
    cv_out(10, "split2_inv_out", "Split 2 inverted"),
];

/// Static description of Spider CV.
pub static CV_DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.spider-cv",
    name: "Spider CV",
    kind: DeviceKind::Utility,
    version: 1,
    params: &CV_PARAMS,
    sockets: &CV_SOCKETS,
};

/// Factory registering [`SpiderCv`].
pub fn cv_factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &CV_DESCRIPTOR,
        create: || Box::new(SpiderCv::default()),
    }
}

/// The CV merger/attenuator/inverter.
#[derive(Debug, Clone, Copy)]
pub struct SpiderCv {
    levels: [f32; 6],
}

impl Default for SpiderCv {
    fn default() -> Self {
        Self { levels: [1.0; 6] }
    }
}

impl Device for SpiderCv {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &CV_DESCRIPTOR
    }

    fn prepare(&mut self, _info: &atomik_device_sdk::PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: ParamId, plain: f32) {
        if let Some(l) = self.levels.get_mut(id.0 as usize) {
            *l = finite(plain).clamp(-1.0, 1.0);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let mut sum = [0.0f32; BATCH];
        for k in 0..4u16 {
            let x = *ctx.input(SocketId(k));
            let t = self.levels[usize::from(k)];
            for n in 0..BATCH {
                sum[n] += finite(x[n]) * t;
            }
        }
        *ctx.output(SocketId(C_SUM_OUT)) = sum;
        for k in 0..2u16 {
            let x = *ctx.input(SocketId(C_SPLIT_IN + k));
            let t = self.levels[4 + usize::from(k)];
            let (mut plain, mut inv) = ([0.0f32; BATCH], [0.0f32; BATCH]);
            for n in 0..BATCH {
                let v = finite(x[n]) * t;
                plain[n] = v;
                inv[n] = -v;
            }
            *ctx.output(SocketId(7 + k * 2)) = plain;
            *ctx.output(SocketId(8 + k * 2)) = inv;
        }
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};

    #[test]
    fn descriptors_validate_and_use_summing_merge_inputs() {
        assert_eq!(AUDIO_DESCRIPTOR.validate(), Ok(()));
        assert_eq!(CV_DESCRIPTOR.validate(), Ok(()));
        for k in 0..8 {
            assert_eq!(AUDIO_SOCKETS[k].direction, Direction::SummingInput);
        }
        for k in 0..4 {
            assert_eq!(CV_SOCKETS[k].direction, Direction::SummingInput);
        }
        assert_eq!(AUDIO_DESCRIPTOR.kind, DeviceKind::Utility);
        assert_eq!(CV_DESCRIPTOR.kind, DeviceKind::Utility);
        assert_keys(
            &CV_DESCRIPTOR,
            &[
                "trim1",
                "trim2",
                "trim3",
                "trim4",
                "split1_level",
                "split2_level",
            ],
            &[
                "in1",
                "in2",
                "in3",
                "in4",
                "sum_out",
                "split1_in",
                "split2_in",
                "split1_out",
                "split1_inv_out",
                "split2_out",
                "split2_inv_out",
            ],
        );
    }

    #[test]
    fn audio_merge_sums_the_four_pairs_and_split_copies() {
        let mut h = harness(Box::new(SpiderAudio));
        *h.input_mut("m1_l") = [0.1; BATCH];
        *h.input_mut("m2_l") = [0.2; BATCH];
        *h.input_mut("m4_l") = [0.4; BATCH];
        *h.input_mut("m3_r") = [-0.3; BATCH];
        *h.input_mut("split_in_l") = [0.7; BATCH];
        *h.input_mut("split_in_r") = [-0.6; BATCH];
        h.process();
        assert_eq!(h.output("merge_out_l")[5], 0.1 + 0.2 + 0.0 + 0.4);
        assert_eq!(h.output("merge_out_r")[5], -0.3);
        for k in 1..=4 {
            assert_eq!(h.output(&format!("split{k}_out_l"))[9], 0.7);
            assert_eq!(h.output(&format!("split{k}_out_r"))[9], -0.6);
        }
    }

    #[test]
    fn a_single_merge_cable_comes_through_bit_for_bit() {
        let mut h = harness(Box::new(SpiderAudio));
        let mut x = [0.0f32; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = (i as f32 * 0.37).sin() * 0.9;
        }
        *h.input_mut("m3_l") = x;
        h.process();
        for i in 0..BATCH {
            assert_eq!(h.output("merge_out_l")[i].to_bits(), x[i].to_bits());
        }
    }

    #[test]
    fn cv_merge_trims_sum_and_split_inverts() {
        let mut h = harness(Box::new(SpiderCv::default()));
        h.set_param("trim2", -0.5);
        h.set_param("trim3", 0.0);
        *h.input_mut("in1") = [0.5; BATCH];
        *h.input_mut("in2") = [0.5; BATCH];
        *h.input_mut("in3") = [0.9; BATCH];
        *h.input_mut("in4") = [0.25; BATCH];
        *h.input_mut("split1_in") = [0.4; BATCH];
        *h.input_mut("split2_in") = [0.8; BATCH];
        h.set_param("split2_level", 0.5);
        h.process();
        assert!((h.output("sum_out")[3] - (0.5 - 0.25 + 0.0 + 0.25)).abs() < 1e-6);
        assert_eq!(h.output("split1_out")[0], 0.4);
        assert_eq!(h.output("split1_inv_out")[0], -0.4);
        assert!((h.output("split2_out")[0] - 0.4).abs() < 1e-6);
        assert!((h.output("split2_inv_out")[0] + 0.4).abs() < 1e-6);
    }

    #[test]
    fn non_finite_input_counts_as_zero_and_limits_stay_finite() {
        let mut h = harness(Box::new(SpiderCv::default()));
        *h.input_mut("in1") = [f32::NAN; BATCH];
        *h.input_mut("split1_in") = [f32::INFINITY; BATCH];
        sweep_params_min_max(&mut h, &CV_DESCRIPTOR);
        assert!(h.output("sum_out").iter().all(|v| *v == 0.0));
        let mut a = harness(Box::new(SpiderAudio));
        *a.input_mut("m1_l") = [f32::NAN; BATCH];
        a.process();
        assert!(a.output("merge_out_l").iter().all(|v| *v == 0.0));
    }
}
