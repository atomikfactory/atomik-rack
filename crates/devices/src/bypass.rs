//! [`Bypassable`]: the one-click bypass shared by every built-in effect.
//!
//! Every `DeviceKind::Effect` built-in appends the host-style `bypass` param (key `"bypass"`,
//! a 0/1 toggle, default 0, built by [`bypass_param`]) as the last entry of its param table and
//! registers `Bypassable::new(effect)` in its factory. The wrapper owns the effect and forwards
//! every [`Device`] method to it; only `set_param` of the bypass id and the main stereo outputs
//! are its own.
//!
//! * **Dry path.** The main inputs (`in_l`, `in_r`) are written into a ring every sample and read
//!   back `latency_frames()` samples late, so a bypassed latent effect stays time-aligned with
//!   the plugin-delay compensation that was computed for it. The latency is read after each
//!   batch's `process` (the value the engine will see); a device that changes its latency at run
//!   time moves the dry tap at that batch boundary. The ring holds 100 ms at the prepared sample
//!   rate (rounded up to a power of two), far above any built-in latency; a larger latency is
//!   clamped to the ring.
//! * **The effect keeps running.** `process` always runs the inner effect, so its state and tail
//!   continue and its other outputs (CV outs, meters) are untouched; only `out_l`/`out_r` are
//!   replaced by the blend.
//! * **Crossfade.** A per-sample linear ramp of 5 ms (`1 / round(sr * 0.005)` per sample, the
//!   same as plugin devices) between wet and the delayed dry: `wet * (1 - m) + dry * m`. At
//!   `m == 0` the wet output is left bit-for-bit as the effect wrote it, at `m == 1` it is the
//!   delayed input bit-for-bit.
//! * **Initial state.** The first batch after `prepare` or `reset` snaps to the current target
//!   instead of fading, so a project that loads bypassed starts bypassed.
//! * Deterministic and allocation-free outside `prepare`.

use atomik_core::{BATCH, Batch, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, PlayerDataRef, PrepareInfo, ProcessContext, RunMode, SampleMapRef,
    SocketId,
};

/// Length of the bypass crossfade in seconds.
const FADE_SECONDS: f64 = 0.005;
/// Dry ring length in seconds (rounded up to a power of two frames).
const RING_SECONDS: f64 = 0.1;
/// Smallest ring in frames.
const MIN_RING: usize = 8192;

/// The shared `bypass` param descriptor, with `id` the index it is appended at.
pub(crate) const fn bypass_param(id: u32) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key: "bypass",
        name: "Bypass",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    }
}

fn fade_step_for(sample_rate: f64) -> f32 {
    let frames = (sample_rate * FADE_SECONDS).round().max(1.0);
    (1.0 / frames) as f32
}

fn mix(wet: f32, dry: f32, m: f32) -> f32 {
    if m <= 0.0 {
        wet
    } else if m >= 1.0 {
        dry
    } else {
        wet * (1.0 - m) + dry * m
    }
}

/// An effect with a bypass (see the module docs).
pub struct Bypassable<D: Device> {
    inner: D,
    bypass_id: ParamId,
    in_l: SocketId,
    in_r: SocketId,
    out_l: SocketId,
    out_r: SocketId,
    bypassed: bool,
    /// 0 = wet, 1 = dry.
    mix: f32,
    fade_step: f32,
    /// First batch after prepare/reset: snap to the target instead of fading.
    snap: bool,
    ring: [Vec<f32>; 2],
    mask: usize,
    write: usize,
}

impl<D: Device> Bypassable<D> {
    /// Wraps `inner`.
    ///
    /// # Panics
    /// If its descriptor lacks the `bypass` param or one of the `in_l`, `in_r`, `out_l`, `out_r`
    /// sockets, or is a multi-stage device (a programming error caught by the registry test).
    pub fn new(inner: D) -> Self {
        let d = inner.descriptor();
        let socket = |key: &str| {
            d.socket_by_key(key)
                .unwrap_or_else(|| panic!("{}: no `{key}` socket", d.type_id))
                .id
        };
        let bypass_id = d
            .param_by_key("bypass")
            .unwrap_or_else(|| panic!("{}: no `bypass` param", d.type_id))
            .id;
        assert!(d.stages.is_empty(), "{}: multi-stage device", d.type_id);
        let (in_l, in_r, out_l, out_r) = (
            socket("in_l"),
            socket("in_r"),
            socket("out_l"),
            socket("out_r"),
        );
        Self {
            inner,
            bypass_id,
            in_l,
            in_r,
            out_l,
            out_r,
            bypassed: false,
            mix: 0.0,
            fade_step: fade_step_for(48_000.0),
            snap: true,
            ring: [Vec::new(), Vec::new()],
            mask: 0,
            write: 0,
        }
    }

    fn target(&self) -> f32 {
        if self.bypassed { 1.0 } else { 0.0 }
    }
}

impl<D: Device> Device for Bypassable<D> {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        self.inner.descriptor()
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.inner.prepare(info);
        let sr = info.sample_rate.hz();
        self.fade_step = fade_step_for(sr);
        let len = ((sr * RING_SECONDS).ceil() as usize)
            .max(MIN_RING)
            .next_power_of_two();
        self.ring = [vec![0.0; len], vec![0.0; len]];
        self.mask = len - 1;
        self.write = 0;
        self.snap = true;
    }

    fn reset(&mut self) {
        self.inner.reset();
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.write = 0;
        self.snap = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        if id == self.bypass_id {
            if plain.is_finite() {
                self.bypassed = plain >= 0.5;
            }
        } else {
            self.inner.set_param(id, plain);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let dry_in: [Batch; 2] = [*ctx.input(self.in_l), *ctx.input(self.in_r)];
        self.inner.process(ctx);
        let target = self.target();
        if self.snap {
            self.mix = target;
            self.snap = false;
        }
        if self.ring[0].is_empty() {
            return;
        }
        let latency = (self.inner.latency_frames() as usize).min(self.mask);
        let (mut left, mut right) = (*ctx.output(self.out_l), *ctx.output(self.out_r));
        for i in 0..BATCH {
            let w = self.write;
            self.ring[0][w & self.mask] = dry_in[0][i];
            self.ring[1][w & self.mask] = dry_in[1][i];
            let r = w.wrapping_sub(latency) & self.mask;
            self.write = w.wrapping_add(1);
            if (target >= 1.0 && self.mix >= 1.0) || (target <= 0.0 && self.mix <= 0.0) {
                if self.mix >= 1.0 {
                    left[i] = self.ring[0][r];
                    right[i] = self.ring[1][r];
                }
                continue;
            }
            if self.mix < target {
                self.mix = (self.mix + self.fade_step).min(target);
            } else {
                self.mix = (self.mix - self.fade_step).max(target);
            }
            left[i] = mix(left[i], self.ring[0][r], self.mix);
            right[i] = mix(right[i], self.ring[1][r], self.mix);
        }
        *ctx.output(self.out_l) = left;
        *ctx.output(self.out_r) = right;
    }

    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        if stage == 0 {
            self.process(ctx);
        }
    }

    fn swap_samples(&mut self, map: Option<SampleMapRef>) -> Option<SampleMapRef> {
        self.inner.swap_samples(map)
    }

    fn swap_player_data(&mut self, data: Option<PlayerDataRef>) -> Option<PlayerDataRef> {
        self.inner.swap_player_data(data)
    }

    fn has_meter_values(&self) -> bool {
        self.inner.has_meter_values()
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        self.inner.meter_values(out)
    }

    fn scope_tap(&self) -> Option<std::sync::Arc<atomik_device_sdk::ScopeTap>> {
        self.inner.scope_tap()
    }

    fn latency_frames(&self) -> u32 {
        self.inner.latency_frames()
    }

    fn pinned_to_audio_thread(&self) -> bool {
        self.inner.pinned_to_audio_thread()
    }

    fn set_run_mode(&mut self, mode: RunMode) {
        self.inner.set_run_mode(mode);
    }
}

#[cfg(test)]
mod tests;
