//! `atomik.headphone-correction`: the last device on the master bus while mixing on headphones.
//! It flattens the response of the user's own headphones so the mix is heard against a neutral
//! reference. Bypass it (or remove it) before the export: it is a monitoring correction, not part
//! of the mix.
//!
//! # Data
//!
//! The device has no built-in headphone data. The user imports a file of their own and the
//! project keeps what was read (see [`atomik_device_sdk::headphone`]), so a project does not
//! depend on the file path:
//!
//! * a **frequency response** (CSV or text, `frequency, dB`, optionally a left and a right
//!   column): with `Data is` = *Measured response* the correction is `target - response`
//!   (the response levelled to 0 dB at 1 kHz); with *Correction curve* the file is the
//!   correction itself;
//! * a **parametric EQ text** (`Preamp:` and `Filter n: ON PK Fc .. Gain .. Q ..` lines): already
//!   a correction; `Data is` and `Target` do not apply to it.
//!
//! The data reaches the device through [`Device::swap_player_data`] as
//! [`atomik_device_sdk::PlayerData::Headphone`]. Without data the device is a bit-exact
//! passthrough.
//!
//! # Signal flow and DSP
//!
//! ```text
//! in L/R -> zero-latency partitioned convolution with a minimum-phase FIR -> output gain -> out
//! ```
//!
//! The correction curve ([`curve`]: grid, smoothing to 1/12 octave, `amount`, `simulate`, a limit
//! of +-20 dB, the held edges below 20 Hz and above 20 kHz) becomes a 4096-tap minimum-phase FIR
//! by the cepstrum method, for either file format, scaled by the preamp. The convolution is the
//! `atomik_dsp::Convolver` of the convolution reverb: a 256-sample head in the time domain and
//! the rest in 256-sample partitions, so the device adds **no latency**. A minimum-phase FIR has
//! the least possible phase shift for its magnitude and no pre-ringing.
//!
//! * **Rebuilds** (new data, `amount`, `target`, `Data is`, `simulate`, `auto gain`, a sample-rate
//!   change) are computed on the audio thread in small steps, one per batch (a chunk of the grid,
//!   one FFT, a few partitions), never in one go. The old filter keeps sounding; when the new one
//!   is complete the convolver crossfades to it over 256 samples, so a change never clicks. The
//!   steps are counted in batches, so a bounce is bit-identical at any host block size. While a
//!   knob is dragged a rebuild finishes before the next one starts.
//! * **First filter.** A filter that is built while no filter sounds is faded in over 10 ms (the
//!   convolver starts from silence); removing the data fades out the same way, after which the
//!   device is the exact passthrough again.
//! * **Amount 0**, or a curve that is flat, builds a unit impulse: the output is the input to the
//!   rounding of the convolver (and, with `auto gain`, no preamp).
//! * **Preamp.** With `auto gain` (default) the preamp is minus the largest boost of the curve, so
//!   the correction never exceeds 0 dB; the applied value is published (meter `preamp`). With it
//!   off the preamp is the parametric file's own `Preamp` (scaled by `amount`) and 0 for a
//!   response. `output` is added on top (smoothed over 30 ms).
//! * **Simulate** applies the inverse of the correction: the headphone's own deviation from the
//!   target. Heard through speakers or other headphones it gives a rough idea of how the mix
//!   would sound on the headphones. It is an approximation: a magnitude-only inverse of a
//!   smoothed curve, limited to +-20 dB; it does not model distortion, fit or the ear.
//! * **Stereo.** Both channels use the same curve unless the response file has a third column
//!   (left, right): then each side has its own.
//! * Non-finite input counts as 0. Deterministic and allocation-free outside `prepare`.
//!
//! # Meters
//!
//! [`METERS`] values: `loaded` (1 when data is installed), `preamp` (the applied preamp in dB),
//! `boost` (the largest boost of the curve in dB, before the preamp) and `busy` (1 while a filter
//! is being built).

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, choice, finite, linear, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::headphone::{HeadphoneBody, HeadphoneData};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PlayerDataRef, PrepareInfo,
    ProcessContext, SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, Convolver, exp2_det};

pub mod curve;
#[cfg(test)]
mod tests;

use curve::{DataKind, Design, Settings, TAPS, Target};

const P_AMOUNT: u32 = 0;
const P_TARGET: u32 = 1;
const P_FILE_KIND: u32 = 2;
const P_SIMULATE: u32 = 3;
const P_AUTO_GAIN: u32 = 4;
const P_OUTPUT: u32 = 5;

/// The `target` choices.
static TARGETS: [&str; 2] = ["Flat", "Studio"];
/// The `file_kind` choices (what a response file is).
static FILE_KINDS: [&str; 2] = ["Measured response", "Correction curve"];

/// The number of published meter values.
pub const METERS: usize = 4;
/// Indices into the meter values.
pub mod meter {
    /// 1 when headphone data is installed, else 0.
    pub const LOADED: usize = 0;
    /// The preamp the filter applies, dB (0 or less with auto gain).
    pub const PREAMP: usize = 1;
    /// The largest boost of the correction before the preamp, dB.
    pub const BOOST: usize = 2;
    /// 1 while a filter is being designed or loaded.
    pub const BUSY: usize = 3;
}

/// Partitions transformed per channel and batch while a filter loads.
const BUILD_BUDGET: usize = 8;
/// Batches to wait after a filter is installed before the next one may start (the crossfade of
/// the convolver is a block of 256 samples after the next block boundary).
const SETTLE_BATCHES: u32 = 8;
/// The fade from and to the dry signal when the first filter comes or the data goes, seconds.
const ENGAGE_SECONDS: f32 = 0.01;

static PARAMS: [ParamDescriptor; 7] = [
    linear(P_AMOUNT, "amount", "Amount", 0.0, 2.0, 1.0, Unit::Percent),
    choice(P_TARGET, "target", "Target", &TARGETS, 0.0),
    choice(P_FILE_KIND, "file_kind", "Data is", &FILE_KINDS, 0.0),
    toggle(P_SIMULATE, "simulate", "Simulate", 0.0),
    toggle(P_AUTO_GAIN, "auto_gain", "Auto gain", 1.0),
    linear(P_OUTPUT, "output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    bypass_param(6),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

static SOCKETS: [SocketDescriptor; 4] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
];

/// Static description of the headphone correction device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.headphone-correction",
    name: "Headphone Correction",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`HeadphoneCorrection`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(HeadphoneCorrection::new())),
    }
}

/// What a built (or wanted) filter was made from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Built {
    /// The generation of the installed data.
    id: u64,
    settings: Settings,
}

/// Where the filter build is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Designing the FIR of channel `0` or `1`.
    Design(usize),
    /// Waiting for the convolvers to be free to take the new FIR.
    Load,
    /// The convolvers are transforming the partitions.
    Build,
}

/// Whether `data` can be designed from (the lengths agree), so a malformed value never panics.
fn usable(data: &HeadphoneData) -> bool {
    match &data.body {
        HeadphoneBody::Response(r) => {
            r.freq.len() >= 2
                && r.left.len() == r.freq.len()
                && r.right.as_ref().is_none_or(|v| v.len() == r.freq.len())
        }
        HeadphoneBody::Filters(_) => true,
    }
}

/// Headphone data to hand to the device, or `None` for a value that is not headphone data.
fn headphone_of(data: &Option<PlayerDataRef>) -> Option<&HeadphoneData> {
    data.as_deref()?.headphone().filter(|d| usable(d))
}

/// The headphone correction device.
#[derive(Debug, Clone)]
pub struct HeadphoneCorrection {
    sample_rate: f32,
    amount: f32,
    target: u8,
    kind: u8,
    simulate: bool,
    auto_gain: bool,
    output_db: f32,
    data: Option<PlayerDataRef>,
    data_id: u64,
    design: Design,
    conv: [Convolver; 2],
    ir: [Vec<f32>; 2],
    phase: Phase,
    /// What the design that is running (or loading) is made from.
    job: Built,
    /// The preamp and boost of the finished design.
    job_levels: (f32, f32),
    /// What the installed filter was made from.
    active: Option<Built>,
    levels: (f32, f32),
    settle: u32,
    /// The convolvers carry the signal (or are fading in).
    engaged: bool,
    /// The convolvers are in the cleared state (nothing has been fed since).
    rested: bool,
    /// The weight of the filtered signal, 0..=1.
    wet: f32,
    out: BlockSmoother,
    fresh: bool,
    meters: [f32; METERS],
}

impl Default for HeadphoneCorrection {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadphoneCorrection {
    /// A device without data; allocates everything it needs.
    pub fn new() -> Self {
        let settings = Settings::default();
        Self {
            sample_rate: 48_000.0,
            amount: 1.0,
            target: 0,
            kind: 0,
            simulate: false,
            auto_gain: true,
            output_db: 0.0,
            data: None,
            data_id: 0,
            design: Design::new(),
            conv: [Convolver::new(TAPS), Convolver::new(TAPS)],
            ir: [vec![0.0; TAPS], vec![0.0; TAPS]],
            phase: Phase::Idle,
            job: Built { id: 0, settings },
            job_levels: (0.0, 0.0),
            active: None,
            levels: (0.0, 0.0),
            settle: 0,
            engaged: false,
            rested: true,
            wet: 0.0,
            out: BlockSmoother::new(1.0),
            fresh: true,
            meters: [0.0; METERS],
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            kind: if self.kind == 0 {
                DataKind::Response
            } else {
                DataKind::Correction
            },
            target: if self.target == 0 {
                Target::Flat
            } else {
                Target::Studio
            },
            amount: self.amount,
            simulate: self.simulate,
            auto_gain: self.auto_gain,
        }
    }

    /// One step of the filter build (at most one FFT or a few partitions).
    fn advance(&mut self) {
        let data = headphone_of(&self.data);
        let want = data.map(|_| Built {
            id: self.data_id,
            settings: self.settings(),
        });
        let sr = self.sample_rate;
        match self.phase {
            Phase::Idle => {
                if self.settle > 0 {
                    self.settle -= 1;
                    return;
                }
                match (want, data) {
                    (Some(w), Some(d)) if self.active != Some(w) => {
                        self.job = w;
                        self.design.begin(d, 0, &w.settings, sr);
                        self.phase = Phase::Design(0);
                    }
                    (None, _) if self.active.is_some() => {
                        // The data is gone: fade to the dry signal, then the convolvers rest.
                        self.active = None;
                        self.levels = (0.0, 0.0);
                        self.engaged = false;
                    }
                    _ => {}
                }
            }
            Phase::Design(ch) => {
                let Some(d) = data.filter(|_| want.is_some_and(|w| w.id == self.job.id)) else {
                    self.design.cancel();
                    self.phase = Phase::Idle;
                    return;
                };
                if !self.design.step(d, &mut self.ir[ch]) {
                    return;
                }
                let separate = matches!(
                    &d.body,
                    HeadphoneBody::Response(r) if r.right.is_some()
                );
                if ch == 0 {
                    let c = self.design.curve();
                    self.job_levels = (c.preamp_db, c.max_boost_db);
                    if separate {
                        self.design.begin(d, 1, &self.job.settings, sr);
                        self.phase = Phase::Design(1);
                    } else {
                        let (a, b) = self.ir.split_at_mut(1);
                        b[0].copy_from_slice(&a[0]);
                        self.phase = Phase::Load;
                    }
                } else {
                    self.phase = Phase::Load;
                }
            }
            Phase::Load => {
                if self.conv[0].fading() || self.conv[1].fading() {
                    return;
                }
                for (c, ir) in self.conv.iter_mut().zip(&self.ir) {
                    c.begin_build(ir, 1.0);
                }
                self.phase = Phase::Build;
            }
            Phase::Build => {
                for (c, ir) in self.conv.iter_mut().zip(&self.ir) {
                    c.build_step(ir, BUILD_BUDGET);
                }
                if !self.conv[0].building() && !self.conv[1].building() {
                    self.active = Some(self.job);
                    self.levels = self.job_levels;
                    self.engaged = self.data.is_some();
                    self.settle = SETTLE_BATCHES;
                    self.phase = Phase::Idle;
                }
            }
        }
    }
}

impl Device for HeadphoneCorrection {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.out.set_time(30.0, self.sample_rate, BATCH);
        // A filter is designed for one sample rate: build it again.
        self.design.cancel();
        self.phase = Phase::Idle;
        self.active = None;
        self.levels = (0.0, 0.0);
        self.engaged = false;
        self.settle = 0;
        self.reset();
        self.wet = 0.0;
    }

    fn reset(&mut self) {
        for c in &mut self.conv {
            c.reset();
        }
        self.rested = true;
        self.wet = if self.engaged { 1.0 } else { 0.0 };
        self.fresh = true;
        self.meters = [0.0; METERS];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_AMOUNT => self.amount = v.clamp(0.0, 2.0),
            P_TARGET => self.target = v.round().clamp(0.0, 1.0) as u8,
            P_FILE_KIND => self.kind = v.round().clamp(0.0, 1.0) as u8,
            P_SIMULATE => self.simulate = v >= 0.5,
            P_AUTO_GAIN => self.auto_gain = v >= 0.5,
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            _ => {}
        }
    }

    fn swap_player_data(&mut self, data: Option<PlayerDataRef>) -> Option<PlayerDataRef> {
        // Anything but headphone data goes straight back to the caller.
        if data.as_deref().is_some_and(|d| d.headphone().is_none()) {
            return data;
        }
        self.data_id += 1;
        std::mem::replace(&mut self.data, data)
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let inputs = [*ctx.input(IN_L), *ctx.input(IN_R)];
        self.advance();

        let gain_target = if self.output_db == 0.0 {
            1.0
        } else {
            exp2_det(self.output_db * (std::f32::consts::LOG2_10 / 20.0))
        };
        if self.fresh {
            self.out.set_immediate(gain_target);
            self.fresh = false;
        }
        let gain = self.out.advance(gain_target, BATCH);

        let step = 1.0 / (ENGAGE_SECONDS * self.sample_rate);
        let mut outs = [[0.0f32; BATCH]; 2];
        for n in 0..BATCH {
            if self.engaged {
                self.wet = (self.wet + step).min(1.0);
            } else {
                self.wet = (self.wet - step).max(0.0);
            }
            let ticking = self.engaged || self.wet > 0.0;
            self.rested &= !ticking;
            let g = gain.at(n);
            for ch in 0..2 {
                let x = finite(inputs[ch][n]);
                let y = if ticking {
                    let w = self.conv[ch].tick(x);
                    if self.wet >= 1.0 {
                        w
                    } else {
                        x + (w - x) * self.wet
                    }
                } else {
                    x
                };
                // `y * 1.0` is exact: the passthrough is bit for bit at 0 dB.
                outs[ch][n] = y * g;
            }
        }
        if !self.engaged && self.wet <= 0.0 && self.active.is_none() && !self.rested {
            // Nothing is filtering any more: the convolvers go back to the cleared state, so the
            // next filter installs without a crossfade.
            for c in &mut self.conv {
                c.reset();
            }
            self.rested = true;
        }
        self.meters = [
            if headphone_of(&self.data).is_some() {
                1.0
            } else {
                0.0
            },
            self.levels.0,
            self.levels.1,
            if self.phase == Phase::Idle { 0.0 } else { 1.0 },
        ];
        *ctx.output(OUT_L) = outs[0];
        *ctx.output(OUT_R) = outs[1];
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }
}
