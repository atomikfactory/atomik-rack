//! `atomik.channel`: one console channel strip.
//!
//! ## Signal flow
//! `in` → input gain/phase → HP/LP filters → dynamics (gate, then compressor)
//! → 4-band EQ → insert send/return → latency adjust → width → fader → pan → `out`.
//! * `route.dyn_post_eq` swaps dynamics and EQ.
//! * `route.insert_pre` moves the insert point to just after the filters (before dynamics/EQ).
//! * `route.filters_to_sc` takes the HP/LP filters out of the audio path and puts them in the
//!   dynamics side-chain instead.
//! * Mono input: if only one of `in_l`/`in_r` is connected, it feeds both sides (as the line
//!   mixer does).
//!
//! ## Processing stages
//! * Stage 0 (`in_*`, `key_in` → `insert_send_*`): latency-free input → gain/phase → filters
//!   → dynamics/EQ (unless `route.insert_pre`).
//! * Stage 1 (`insert_return_*` → `send*`, `out_*`, `gr_cv_out`): insert return (pass-through
//!   when unconnected or bypassed) → latency adjust → dynamics/EQ (with `route.insert_pre`, using
//!   the `key_in` copy taken in stage 0) → width → fader → pan.
//! * The signal between the stages lives in preallocated buffers. [`Device::process`] runs
//!   both stages in order (single-call hosts and tests).
//!
//! ## Design choices
//! * **First batch:** after `prepare`/`reset` the first stage 0 snaps every smoother to its
//!   target, so params set before playback start apply from sample 0 (no ramp from defaults).
//! * **Gains** (input, phase, fader, pan, width, mute, sends, section on/off and insert
//!   return switching) are smoothed with [`SMOOTH_MS`] linear ramps: no zipper noise, no clicks.
//!   Section on/off switches crossfade between the dry and processed signal. Routing switches
//!   (`route.*`) and EQ shape switches (`eq.*.bell`) re-route at once.
//! * **Input gain:** −24..+24 dB. **Phase** flips the sign through a smoothed ramp.
//! * **Filters:** 18 dB/oct third-order Butterworth ([`atomik_dsp::PassFilter`]).
//! * **Dynamics** ([`atomik_dsp::Compressor`], [`atomik_dsp::Gate`]): one stereo-linked
//!   side-chain feeds both. The side-chain is `key_in` (mono, used on both sides) when `key.on`
//!   is set *and* `key_in` is connected; otherwise it is the signal entering the dynamics.
//!   Compressor attack is [`COMP_ATTACK_MS`] ([`COMP_FAST_ATTACK_MS`] with `comp.fast`), soft
//!   knee [`atomik_dsp::KNEE_DB`] dB, `comp.peak` selects peak instead of the default RMS
//!   detector. Gate attack is [`GATE_ATTACK_MS`] ([`GATE_FAST_ATTACK_MS`] with `gate.fast`).
//!   `gate.range` is the attenuation in dB when closed (the expander's floor).
//! * **EQ:** RBJ biquads with smoothed parameters. HF and LF are shelves (Q 1/√2, cookbook slope
//!   S = 1) or, with `*.bell`, bells with a fixed Q of [`OUTER_BELL_Q`]. HMF and LMF are bells.
//! * **Inserts:** `insert_send_*` always carry the signal at the insert point. When either
//!   `insert_return_*` socket is connected (and `insert.bypass` is off) the return replaces the
//!   signal; a single connected return side feeds both sides.
//! * **Width** (mid/side, 0 = mono, 1 = unchanged), then the **fader** (cubic taper,
//!   [`atomik_dsp::fader_gain`]: the param is the 0..1 *position*, unity at 0.75, +7.5 dB at 1),
//!   then the **pan** (constant power, −3.01 dB at centre, `L·cos θ`, `R·sin θ`, as the line
//!   mixer). Pan acts on the main output only.
//! * **Sends** tap the widened signal before the fader (`send{k}.pre`) or after it (default),
//!   before the pan (sends keep the stereo image). `send{k}.level` is in dB with a
//!   [`Taper::Decibel`] taper over −60..+6 dB (the minimum is −∞ / off); a send only sounds when
//!   `send{k}.on`.
//! * **CV inputs** (stage 1; each is the batch mean of the CV, clamped to −1..+1, times its
//!   `*_cv_trim` (−1..+1, default +1), applied through the same 5 ms smoothers; unconnected, the
//!   strip is bit-identical to before):
//!   `fader_cv_in` adds `trim · cv · 24` dB to the fader gain; `pan_cv_in` adds `trim · cv` to
//!   the pan position (clamped to −1..+1); `send{k}_level_cv_in` adds `trim · cv · 66` dB (the
//!   span of the send range) to `send{k}.level`, clamped to −60..+6 dB (−60 is off). A send
//!   whose `send{k}.on` is off stays silent.
//! * **Mute / solo:** `mute` (ignored while `solo` is on, per the §8 contract) and
//!   `internal.solo_silence` silence the main output *and every send, pre-fader included*. The
//!   insert send and the gain-reduction CV are unaffected. `solo` has no other DSP effect.
//! * **`latency.adjust`** is a *relative* trim of −[`MAX_LATENCY_ADJUST`]..=
//!   +[`MAX_LATENCY_ADJUST`] samples, default 0:
//!   * **positive `N`:** the channel really delays its signal by `N` samples (at the start of
//!     stage 1, so `out_*` and the sends carry it, the insert send and `gr_cv_out` do not) and
//!     does **not** report it, so it lands `N` samples *later* than the other channels;
//!   * **negative `N`:** no delay; [`Device::latency_frames`] reports `|N|` (phantom latency),
//!     so the engine's PDC delays every other channel instead and this one lands `|N|` samples
//!     *earlier*. The engine clips automatic + adjust at 0.
//! * **`gr_cv_out`:** `1 − g`, where `g` is the combined linear gain of gate × compressor
//!   (0 = no reduction, 0.5 ≈ 6 dB, → 1 when fully closed).

use crate::cv_in::batch_mean;
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit, db_to_gain};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    BiquadShape, Compressor, DelayLine, Detector, Gate, PassFilter, PassKind, SmoothedBiquad,
    Smoother, fader_gain, pan_constant_power, stereo_width,
};

/// Number of effect sends.
pub const SENDS: usize = 8;
/// Longest `latency.adjust` delay in samples (100 ms at 48 kHz).
pub const MAX_LATENCY_ADJUST: u32 = 4_800;
/// Ramp time of every smoothed gain.
pub const SMOOTH_MS: f32 = 5.0;
/// Compressor attack time constant.
pub const COMP_ATTACK_MS: f32 = 10.0;
/// Compressor attack time constant with `comp.fast`.
pub const COMP_FAST_ATTACK_MS: f32 = 0.5;
/// Gate opening time constant.
pub const GATE_ATTACK_MS: f32 = 1.0;
/// Gate opening time constant with `gate.fast`.
pub const GATE_FAST_ATTACK_MS: f32 = 0.05;
/// Q of the HF/LF bands in bell mode.
pub const OUTER_BELL_Q: f32 = 0.7;
/// Q of the HF/LF bands in shelf mode (cookbook slope S = 1).
pub const SHELF_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

// Const builder mirroring every `ParamDescriptor` field (const fns cannot take a struct
// update with defaults), so it needs one argument per field.
#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    taper: Taper,
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete: false,
    }
}

const fn toggle(id: u32, key: &'static str, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 1.0,
        default,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    }
}

const fn db(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    d: f32,
) -> ParamDescriptor {
    param(id, key, name, min, max, d, Taper::Linear, Unit::Db)
}

const fn hz(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    d: f32,
) -> ParamDescriptor {
    param(id, key, name, min, max, d, Taper::Exponential, Unit::Hz)
}

const fn ms(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    d: f32,
) -> ParamDescriptor {
    param(id, key, name, min, max, d, Taper::Exponential, Unit::Ms)
}

const fn q(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    param(id, key, name, 0.3, 8.0, 1.0, Taper::Exponential, Unit::None)
}

/// A send level: dB, decibel taper over −60 (= off) .. +6, default 0 dB.
pub(crate) const fn send_level(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    param(id, key, name, -60.0, 6.0, 0.0, Taper::Decibel, Unit::Db)
}

/// The fader position param (0..1 linear position, cubic gain taper, unity at 0.75).
pub(crate) const fn fader(id: u32) -> ParamDescriptor {
    param(
        id,
        "fader",
        "Fader",
        0.0,
        1.0,
        atomik_dsp::FADER_UNITY_POS,
        Taper::Linear,
        Unit::None,
    )
}

/// Pan, −1 (hard left) .. +1 (hard right).
pub(crate) const fn pan(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    param(id, key, name, -1.0, 1.0, 0.0, Taper::Linear, Unit::None)
}

macro_rules! send_params {
    ($k:literal, $base:expr) => {
        [
            toggle(
                $base,
                concat!("send", $k, ".on"),
                concat!("Send ", $k, " on"),
                0.0,
            ),
            send_level(
                $base + 1,
                concat!("send", $k, ".level"),
                concat!("Send ", $k, " level"),
            ),
            toggle(
                $base + 2,
                concat!("send", $k, ".pre"),
                concat!("Send ", $k, " pre"),
                0.0,
            ),
        ]
    };
}

// Param ids (index in `PARAMS`).
const P_INPUT_GAIN: usize = 0;
const P_INPUT_PHASE: usize = 1;
const P_HPF_ON: usize = 2;
const P_HPF_FREQ: usize = 3;
const P_LPF_ON: usize = 4;
const P_LPF_FREQ: usize = 5;
const P_COMP_ON: usize = 6;
const P_COMP_THRESHOLD: usize = 7;
const P_COMP_RATIO: usize = 8;
const P_COMP_RELEASE: usize = 9;
const P_COMP_PEAK: usize = 10;
const P_COMP_FAST: usize = 11;
const P_GATE_ON: usize = 12;
const P_GATE_THRESHOLD: usize = 13;
const P_GATE_RANGE: usize = 14;
const P_GATE_HOLD: usize = 15;
const P_GATE_RELEASE: usize = 16;
const P_GATE_FAST: usize = 17;
const P_GATE_EXPANDER: usize = 18;
const P_KEY_ON: usize = 19;
const P_FILTERS_TO_SC: usize = 20;
const P_EQ_ON: usize = 21;
/// First EQ band param (`eq.hf.gain`); bands are HF, HMF, LMF, LF with 3 params each.
const P_EQ_FIRST: usize = 22;
const P_DYN_POST_EQ: usize = 34;
const P_INSERT_PRE: usize = 35;
const P_INSERT_BYPASS: usize = 36;
/// First send param (`send1.on`); 3 params per send.
const P_SEND_FIRST: usize = 37;
const P_PAN: usize = P_SEND_FIRST + 3 * SENDS;
const P_WIDTH: usize = P_PAN + 1;
const P_FADER: usize = P_PAN + 2;
const P_MUTE: usize = P_PAN + 3;
const P_SOLO: usize = P_PAN + 4;
const P_LATENCY: usize = P_PAN + 5;
const P_SOLO_SILENCE: usize = P_PAN + 6;
/// CV trims (appended after the contract params so every existing id is unchanged).
const P_FADER_TRIM: usize = P_SOLO_SILENCE + 1;
const P_PAN_TRIM: usize = P_SOLO_SILENCE + 2;
/// First send-level CV trim (`send1.level_cv_trim`); one per send.
const P_SEND_TRIM_FIRST: usize = P_SOLO_SILENCE + 3;
const PARAM_COUNT: usize = P_SEND_TRIM_FIRST + SENDS;

/// Full-scale swing of `fader_cv_in` in dB.
const FADER_CV_DB: f32 = 24.0;
/// Full-scale swing of a send-level CV in dB (the −60..+6 dB range).
const SEND_CV_DB: f32 = 66.0;

/// A bipolar CV amount, −1..+1, default +1.
const fn cv_trim(id: usize, key: &'static str, name: &'static str) -> ParamDescriptor {
    param(
        id as u32,
        key,
        name,
        -1.0,
        1.0,
        1.0,
        Taper::Linear,
        Unit::Percent,
    )
}

macro_rules! send_trims {
    ($($k:literal),*) => {
        [$(cv_trim(
            P_SEND_TRIM_FIRST + $k - 1,
            concat!("send", $k, ".level_cv_trim"),
            concat!("Send ", $k, " level CV trim"),
        )),*]
    };
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] = {
    let head = [
        db(0, "input.gain", "Input gain", -24.0, 24.0, 0.0),
        toggle(1, "input.phase", "Phase invert", 0.0),
        toggle(2, "hpf.on", "HPF on", 0.0),
        hz(3, "hpf.freq", "HPF frequency", 20.0, 2_000.0, 80.0),
        toggle(4, "lpf.on", "LPF on", 0.0),
        hz(5, "lpf.freq", "LPF frequency", 1_000.0, 20_000.0, 12_000.0),
        toggle(6, "comp.on", "Compressor on", 0.0),
        db(7, "comp.threshold", "Comp threshold", -60.0, 0.0, -20.0),
        param(
            8,
            "comp.ratio",
            "Comp ratio",
            1.0,
            20.0,
            4.0,
            Taper::Exponential,
            Unit::None,
        ),
        ms(9, "comp.release", "Comp release", 50.0, 2_000.0, 200.0),
        toggle(10, "comp.peak", "Comp peak", 0.0),
        toggle(11, "comp.fast", "Comp fast attack", 0.0),
        toggle(12, "gate.on", "Gate on", 0.0),
        db(13, "gate.threshold", "Gate threshold", -80.0, 0.0, -50.0),
        db(14, "gate.range", "Gate range", 0.0, 80.0, 40.0),
        param(
            15,
            "gate.hold",
            "Gate hold",
            0.0,
            500.0,
            10.0,
            Taper::Linear,
            Unit::Ms,
        ),
        ms(16, "gate.release", "Gate release", 10.0, 4_000.0, 100.0),
        toggle(17, "gate.fast", "Gate fast attack", 0.0),
        toggle(18, "gate.expander", "Expander", 0.0),
        toggle(19, "key.on", "Key", 0.0),
        toggle(20, "route.filters_to_sc", "Filters to side-chain", 0.0),
        toggle(21, "eq.on", "EQ on", 0.0),
        db(22, "eq.hf.gain", "HF gain", -18.0, 18.0, 0.0),
        hz(23, "eq.hf.freq", "HF frequency", 1_500.0, 16_000.0, 8_000.0),
        toggle(24, "eq.hf.bell", "HF bell", 0.0),
        db(25, "eq.hmf.gain", "HMF gain", -18.0, 18.0, 0.0),
        hz(26, "eq.hmf.freq", "HMF frequency", 500.0, 8_000.0, 2_500.0),
        q(27, "eq.hmf.q", "HMF Q"),
        db(28, "eq.lmf.gain", "LMF gain", -18.0, 18.0, 0.0),
        hz(29, "eq.lmf.freq", "LMF frequency", 80.0, 2_000.0, 500.0),
        q(30, "eq.lmf.q", "LMF Q"),
        db(31, "eq.lf.gain", "LF gain", -18.0, 18.0, 0.0),
        hz(32, "eq.lf.freq", "LF frequency", 30.0, 500.0, 100.0),
        toggle(33, "eq.lf.bell", "LF bell", 0.0),
        toggle(34, "route.dyn_post_eq", "Dynamics after EQ", 0.0),
        toggle(35, "route.insert_pre", "Inserts before dynamics/EQ", 0.0),
        toggle(36, "insert.bypass", "Insert bypass", 0.0),
    ];
    let sends = [
        send_params!("1", 37),
        send_params!("2", 40),
        send_params!("3", 43),
        send_params!("4", 46),
        send_params!("5", 49),
        send_params!("6", 52),
        send_params!("7", 55),
        send_params!("8", 58),
    ];
    let tail = [
        pan(P_PAN as u32, "pan", "Pan"),
        param(
            P_WIDTH as u32,
            "width",
            "Width",
            0.0,
            1.0,
            1.0,
            Taper::Linear,
            Unit::Percent,
        ),
        fader(P_FADER as u32),
        toggle(P_MUTE as u32, "mute", "Mute", 0.0),
        toggle(P_SOLO as u32, "solo", "Solo", 0.0),
        ParamDescriptor {
            discrete: true,
            ..param(
                P_LATENCY as u32,
                "latency.adjust",
                "Latency adjust (samples)",
                -(MAX_LATENCY_ADJUST as f32),
                MAX_LATENCY_ADJUST as f32,
                0.0,
                Taper::Linear,
                Unit::None,
            )
        },
        toggle(
            P_SOLO_SILENCE as u32,
            "internal.solo_silence",
            "Solo silence",
            0.0,
        ),
    ];
    let cv = send_trims!(1, 2, 3, 4, 5, 6, 7, 8);
    let mut out = [tail[0]; PARAM_COUNT];
    let mut i = 0;
    while i < P_SEND_FIRST {
        out[i] = head[i];
        i += 1;
    }
    while i < P_PAN {
        out[i] = sends[(i - P_SEND_FIRST) / 3][(i - P_SEND_FIRST) % 3];
        i += 1;
    }
    while i <= P_SOLO_SILENCE {
        out[i] = tail[i - P_PAN];
        i += 1;
    }
    out[P_FADER_TRIM] = cv_trim(P_FADER_TRIM, "fader_cv_trim", "Fader CV trim");
    out[P_PAN_TRIM] = cv_trim(P_PAN_TRIM, "pan_cv_trim", "Pan CV trim");
    let mut k = 0;
    while k < SENDS {
        out[P_SEND_TRIM_FIRST + k] = cv[k];
        k += 1;
    }
    out
};

pub(crate) const fn socket(
    id: u16,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind,
        direction,
    }
}

const fn audio_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Audio, Direction::Input)
}

const fn audio_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Audio, Direction::Output)
}

macro_rules! send_sockets {
    ($k:literal, $base:expr) => {
        [
            audio_out($base, concat!("send", $k, "_l"), concat!("Send ", $k, " L")),
            audio_out(
                $base + 1,
                concat!("send", $k, "_r"),
                concat!("Send ", $k, " R"),
            ),
        ]
    };
}

const S_IN_L: u16 = 0;
const S_IN_R: u16 = 1;
const S_KEY: u16 = 2;
const S_INSERT_SEND_L: u16 = 3;
const S_INSERT_SEND_R: u16 = 4;
const S_INSERT_RETURN_L: u16 = 5;
const S_INSERT_RETURN_R: u16 = 6;
const S_SEND_FIRST: u16 = 7;
const S_OUT_L: u16 = S_SEND_FIRST + 2 * SENDS as u16;
const S_OUT_R: u16 = S_OUT_L + 1;
const S_GR: u16 = S_OUT_L + 2;
const S_FADER_CV: u16 = S_GR + 1;
const S_PAN_CV: u16 = S_GR + 2;
/// First send-level CV input (`send1_level_cv_in`); one per send.
const S_SEND_CV_FIRST: u16 = S_GR + 3;
const SOCKET_COUNT: usize = S_SEND_CV_FIRST as usize + SENDS;

const fn cv_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    socket(id, key, name, SocketKind::Cv, Direction::Input)
}

macro_rules! send_cv_sockets {
    ($($k:literal),*) => {
        [$(cv_in(
            S_SEND_CV_FIRST + $k - 1,
            concat!("send", $k, "_level_cv_in"),
            concat!("Send ", $k, " level CV"),
        )),*]
    };
}

static SOCKETS: [SocketDescriptor; SOCKET_COUNT] = {
    let head = [
        audio_in(S_IN_L, "in_l", "In L"),
        audio_in(S_IN_R, "in_r", "In R"),
        audio_in(S_KEY, "key_in", "Key in"),
        audio_out(S_INSERT_SEND_L, "insert_send_l", "Insert send L"),
        audio_out(S_INSERT_SEND_R, "insert_send_r", "Insert send R"),
        audio_in(S_INSERT_RETURN_L, "insert_return_l", "Insert return L"),
        audio_in(S_INSERT_RETURN_R, "insert_return_r", "Insert return R"),
    ];
    let sends = [
        send_sockets!("1", 7),
        send_sockets!("2", 9),
        send_sockets!("3", 11),
        send_sockets!("4", 13),
        send_sockets!("5", 15),
        send_sockets!("6", 17),
        send_sockets!("7", 19),
        send_sockets!("8", 21),
    ];
    let tail = [
        audio_out(S_OUT_L, "out_l", "Out L"),
        audio_out(S_OUT_R, "out_r", "Out R"),
        socket(
            S_GR,
            "gr_cv_out",
            "Gain reduction CV",
            SocketKind::Cv,
            Direction::Output,
        ),
    ];
    let cv = send_cv_sockets!(1, 2, 3, 4, 5, 6, 7, 8);
    let mut out = [tail[0]; SOCKET_COUNT];
    let mut i = 0;
    while i < S_SEND_FIRST as usize {
        out[i] = head[i];
        i += 1;
    }
    while i < S_OUT_L as usize {
        let k = i - S_SEND_FIRST as usize;
        out[i] = sends[k / 2][k % 2];
        i += 1;
    }
    while i <= S_GR as usize {
        out[i] = tail[i - S_OUT_L as usize];
        i += 1;
    }
    out[S_FADER_CV as usize] = cv_in(S_FADER_CV, "fader_cv_in", "Fader CV");
    out[S_PAN_CV as usize] = cv_in(S_PAN_CV, "pan_cv_in", "Pan CV");
    let mut k = 0;
    while k < SENDS {
        out[S_SEND_CV_FIRST as usize + k] = cv[k];
        k += 1;
    }
    out
};

/// Processing stage of each socket: stage 0 ends at the insert send, stage 1 starts
/// at the insert return.
static STAGES: [u8; SOCKET_COUNT] = {
    let mut st = [1u8; SOCKET_COUNT];
    st[S_IN_L as usize] = 0;
    st[S_IN_R as usize] = 0;
    st[S_KEY as usize] = 0;
    st[S_INSERT_SEND_L as usize] = 0;
    st[S_INSERT_SEND_R as usize] = 0;
    st
};

/// Static description of the channel strip.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &STAGES,
    type_id: "atomik.channel",
    name: "Console Channel",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Channel`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Channel::new()),
    }
}

/// One stereo block.
type Block = [f32; BATCH];

/// What a switchable section does this block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Fully off: skip.
    Skip,
    /// Running.
    Active,
    /// Just switched on: clear the section's state, then run.
    Fresh,
}

/// The on/off crossfade of a processing section (dry ↔ processed, [`SMOOTH_MS`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Section {
    mix: Smoother,
    active: bool,
}

impl Section {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            mix: Smoother::new(0.0, SMOOTH_MS, sample_rate),
            active: false,
        }
    }

    pub(crate) fn set_on(&mut self, on: bool) {
        self.mix.set_target(if on { 1.0 } else { 0.0 });
    }

    pub(crate) fn set_time(&mut self, sample_rate: f32) {
        self.mix.set_time(SMOOTH_MS, sample_rate);
    }

    pub(crate) fn snap(&mut self) {
        let t = self.mix.target();
        self.mix.set_immediate(t);
        self.active = t > 0.0;
    }

    /// Decides what to do this block.
    pub(crate) fn begin(&mut self) -> Step {
        let on = self.mix.target() > 0.0;
        if on && !self.active {
            self.active = true;
            return Step::Fresh;
        }
        if self.active && !on && self.mix.is_settled() {
            self.active = false;
        }
        if self.active {
            Step::Active
        } else {
            Step::Skip
        }
    }

    /// Next crossfade value (0 = dry, 1 = processed).
    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        self.mix.next_value()
    }
}

/// Runs a stereo filter section in place with its on/off crossfade.
fn run_filter(f: &mut PassFilter, sec: &mut Section, l: &mut Block, r: &mut Block) {
    match sec.begin() {
        Step::Skip => return,
        Step::Fresh => f.reset(),
        Step::Active => {}
    }
    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
        let m = sec.next();
        let (fa, fb) = f.process(*a, *b);
        *a += (fa - *a) * m;
        *b += (fb - *b) * m;
    }
}

/// Smoother at `v` with the strip's ramp time.
pub(crate) fn smoother(v: f32, sample_rate: f32) -> Smoother {
    Smoother::new(v, SMOOTH_MS, sample_rate)
}

/// Jumps a smoother to its target.
pub(crate) fn snap(s: &mut Smoother) {
    let t = s.target();
    s.set_immediate(t);
}

/// True when a smoother rests at exactly zero.
pub(crate) fn is_silent(s: &Smoother) -> bool {
    s.is_settled() && s.value() == 0.0
}

/// Linear gain of a `send{k}.level`-style dB param (−60 dB = off).
pub(crate) fn send_gain(db: f32) -> f32 {
    PARAMS[P_SEND_FIRST + 1].db_param_gain(db)
}

/// The channel strip device.
#[derive(Debug, Clone)]
pub struct Channel {
    values: [f32; PARAM_COUNT],
    sample_rate: f32,
    /// Real delay from a positive `latency.adjust` (not reported).
    latency: u32,
    /// Reported-only latency from a negative `latency.adjust` (no delay).
    phantom_latency: u32,
    delay_l: DelayLine,
    delay_r: DelayLine,
    input: Smoother,
    hpf: PassFilter,
    lpf: PassFilter,
    hpf_on: Section,
    lpf_on: Section,
    comp: Compressor,
    gate: Gate,
    comp_on: Section,
    gate_on: Section,
    eq: [SmoothedBiquad; 4],
    eq_on: Section,
    insert: Smoother,
    width: Smoother,
    fader: Smoother,
    pan_l: Smoother,
    pan_r: Smoother,
    audible: Smoother,
    send_pre: [Smoother; SENDS],
    send_post: [Smoother; SENDS],
    /// CV targets are currently overriding the smoothers: fader, pan, sends 1..8.
    cv_live: [bool; 2 + SENDS],
    /// Stage-0 → stage-1 hand-over: the signal at the insert point.
    mid: (Block, Block),
    /// Copy of `key_in` from stage 0 (stage 1 runs the dynamics with `route.insert_pre`).
    key: Block,
    key_active: bool,
    /// Insert routing latched in stage 0 for stage 1.
    insert_pre: bool,
    /// Dynamics gain-reduction CV, written in stage 1.
    gr: Block,
    /// Set by `prepare`/`reset`: the next stage 0 snaps every smoother to its target, so params
    /// set before the first batch apply at once instead of ramping from the defaults.
    fresh: bool,
    /// Like `fresh`, for the insert crossfade: its target (return connected or not) is only
    /// known in stage 1, so the first insert return after `prepare`/`reset` snaps it there.
    fresh_insert: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel {
    /// A channel with default params (call `prepare` before processing). Allocates the
    /// latency-adjust delay lines.
    pub fn new() -> Self {
        let sr = 48_000.0;
        let mut values = [0.0; PARAM_COUNT];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        let sm = smoother(0.0, sr);
        let band = |shape| SmoothedBiquad::new(shape, 1_000.0, 1.0, 0.0, sr);
        let mut c = Self {
            values,
            sample_rate: sr,
            latency: 0,
            phantom_latency: 0,
            delay_l: DelayLine::new(MAX_LATENCY_ADJUST as usize),
            delay_r: DelayLine::new(MAX_LATENCY_ADJUST as usize),
            input: sm,
            hpf: PassFilter::new(PassKind::Highpass, 80.0, sr),
            lpf: PassFilter::new(PassKind::Lowpass, 12_000.0, sr),
            hpf_on: Section::new(sr),
            lpf_on: Section::new(sr),
            comp: Compressor::new(sr),
            gate: Gate::new(sr),
            comp_on: Section::new(sr),
            gate_on: Section::new(sr),
            eq: [
                band(BiquadShape::HighShelf),
                band(BiquadShape::Peaking),
                band(BiquadShape::Peaking),
                band(BiquadShape::LowShelf),
            ],
            eq_on: Section::new(sr),
            insert: sm,
            width: sm,
            fader: sm,
            pan_l: sm,
            pan_r: sm,
            audible: sm,
            send_pre: [sm; SENDS],
            send_post: [sm; SENDS],
            cv_live: [false; 2 + SENDS],
            mid: ([0.0; BATCH], [0.0; BATCH]),
            key: [0.0; BATCH],
            key_active: false,
            insert_pre: false,
            gr: [0.0; BATCH],
            fresh: true,
            fresh_insert: true,
        };
        for i in 0..PARAM_COUNT {
            c.apply(i);
        }
        c.reset();
        c
    }

    fn on(&self, i: usize) -> bool {
        self.values[i] >= 0.5
    }

    /// Pushes param `i` into the DSP state (RT-safe).
    fn apply(&mut self, i: usize) {
        let v = self.values[i];
        match i {
            P_INPUT_GAIN | P_INPUT_PHASE => {
                let sign = if self.on(P_INPUT_PHASE) { -1.0 } else { 1.0 };
                self.input
                    .set_target(sign * db_to_gain(self.values[P_INPUT_GAIN]));
            }
            P_HPF_ON => self.hpf_on.set_on(self.on(i)),
            P_HPF_FREQ => self.hpf.set_freq(v),
            P_LPF_ON => self.lpf_on.set_on(self.on(i)),
            P_LPF_FREQ => self.lpf.set_freq(v),
            P_COMP_ON => self.comp_on.set_on(self.on(i)),
            P_COMP_THRESHOLD => self.comp.set_threshold_db(v),
            P_COMP_RATIO => self.comp.set_ratio(v),
            P_COMP_RELEASE => self.comp.set_release_ms(v),
            P_COMP_PEAK => self.comp.set_detector(if self.on(i) {
                Detector::Peak
            } else {
                Detector::Rms
            }),
            P_COMP_FAST => self.comp.set_attack_ms(if self.on(i) {
                COMP_FAST_ATTACK_MS
            } else {
                COMP_ATTACK_MS
            }),
            P_GATE_ON => self.gate_on.set_on(self.on(i)),
            P_GATE_THRESHOLD => self.gate.set_threshold_db(v),
            P_GATE_RANGE => self.gate.set_range_db(v),
            P_GATE_HOLD => self.gate.set_hold_ms(v),
            P_GATE_RELEASE => self.gate.set_release_ms(v),
            P_GATE_FAST => self.gate.set_attack_ms(if self.on(i) {
                GATE_FAST_ATTACK_MS
            } else {
                GATE_ATTACK_MS
            }),
            P_GATE_EXPANDER => self.gate.set_expander(self.on(i)),
            P_EQ_ON => self.eq_on.set_on(self.on(i)),
            _ if (P_EQ_FIRST..P_EQ_FIRST + 12).contains(&i) => {
                let b = (i - P_EQ_FIRST) / 3;
                let band = &mut self.eq[b];
                match (i - P_EQ_FIRST) % 3 {
                    0 => band.set_gain_db(v),
                    1 => band.set_freq(v),
                    _ if b == 0 || b == 3 => {
                        let bell = v >= 0.5;
                        let shelf = if b == 0 {
                            BiquadShape::HighShelf
                        } else {
                            BiquadShape::LowShelf
                        };
                        band.set_shape(if bell { BiquadShape::Peaking } else { shelf });
                        band.set_q(if bell { OUTER_BELL_Q } else { SHELF_Q });
                    }
                    _ => band.set_q(v),
                }
            }
            _ if (P_SEND_FIRST..P_PAN).contains(&i) => {
                self.set_send_target((i - P_SEND_FIRST) / 3, 0.0);
            }
            P_PAN => {
                let (l, r) = pan_constant_power(v);
                self.pan_l.set_target(l);
                self.pan_r.set_target(r);
            }
            P_WIDTH => self.width.set_target(v),
            P_FADER => self.fader.set_target(fader_gain(v)),
            P_MUTE | P_SOLO | P_SOLO_SILENCE => {
                let muted = self.on(P_MUTE) && !self.on(P_SOLO);
                let silent = muted || self.on(P_SOLO_SILENCE);
                self.audible.set_target(if silent { 0.0 } else { 1.0 });
            }
            P_LATENCY => {
                let n = v
                    .round()
                    .clamp(-(MAX_LATENCY_ADJUST as f32), MAX_LATENCY_ADJUST as f32);
                // Positive: a real delay, not reported. Negative: no delay, reported as
                // phantom latency so the engine's PDC delays every other channel instead.
                self.latency = n.max(0.0) as u32;
                self.phantom_latency = (-n).max(0.0) as u32;
            }
            // Routing switches, key and insert bypass are read in `process`.
            _ => {}
        }
    }

    /// Sets the pre/post targets of send `k` from its params, with `cv_db` added to the level.
    fn set_send_target(&mut self, k: usize, cv_db: f32) {
        let base = P_SEND_FIRST + 3 * k;
        let g = if self.on(base) {
            send_gain(self.values[base + 1] + cv_db)
        } else {
            0.0
        };
        let pre = self.on(base + 2);
        self.send_pre[k].set_target(if pre { g } else { 0.0 });
        self.send_post[k].set_target(if pre { 0.0 } else { g });
    }

    /// Applies the CV inputs (once per batch, stage 1) on top of the knob values. A CV that
    /// is not connected leaves the knob targets alone (restored once when it is unplugged).
    fn apply_cv(&mut self, ctx: &ProcessContext<'_>) {
        let cv_of = |s: u16| {
            ctx.is_connected(SocketId(s))
                .then(|| batch_mean(ctx.input(SocketId(s))))
        };
        match cv_of(S_FADER_CV) {
            Some(cv) => {
                let db = self.values[P_FADER_TRIM] * cv * FADER_CV_DB;
                let g = fader_gain(self.values[P_FADER]) * db_to_gain(db);
                self.fader.set_target(g);
                self.cv_live[0] = true;
            }
            None if self.cv_live[0] => {
                self.cv_live[0] = false;
                self.apply(P_FADER);
            }
            None => {}
        }
        match cv_of(S_PAN_CV) {
            Some(cv) => {
                let p = self.values[P_PAN] + self.values[P_PAN_TRIM] * cv;
                let (l, r) = pan_constant_power(p);
                self.pan_l.set_target(l);
                self.pan_r.set_target(r);
                self.cv_live[1] = true;
            }
            None if self.cv_live[1] => {
                self.cv_live[1] = false;
                self.apply(P_PAN);
            }
            None => {}
        }
        for k in 0..SENDS {
            match cv_of(S_SEND_CV_FIRST + k as u16) {
                Some(cv) => {
                    let db = self.values[P_SEND_TRIM_FIRST + k] * cv * SEND_CV_DB;
                    self.set_send_target(k, db);
                    self.cv_live[2 + k] = true;
                }
                None if self.cv_live[2 + k] => {
                    self.cv_live[2 + k] = false;
                    self.apply(P_SEND_FIRST + 3 * k);
                }
                None => {}
            }
        }
    }

    fn smoothers_mut(&mut self) -> impl Iterator<Item = &mut Smoother> {
        [
            &mut self.input,
            &mut self.insert,
            &mut self.width,
            &mut self.fader,
            &mut self.pan_l,
            &mut self.pan_r,
            &mut self.audible,
        ]
        .into_iter()
        .chain(self.send_pre.iter_mut())
        .chain(self.send_post.iter_mut())
    }

    fn sections_mut(&mut self) -> [&mut Section; 5] {
        [
            &mut self.hpf_on,
            &mut self.lpf_on,
            &mut self.comp_on,
            &mut self.gate_on,
            &mut self.eq_on,
        ]
    }

    /// Filters (audio path or side-chain, the same instances).
    fn filters(&mut self, l: &mut Block, r: &mut Block) {
        run_filter(&mut self.hpf, &mut self.hpf_on, l, r);
        run_filter(&mut self.lpf, &mut self.lpf_on, l, r);
    }

    fn eq_stage(&mut self, l: &mut Block, r: &mut Block) {
        match self.eq_on.begin() {
            Step::Skip => return,
            Step::Fresh => self.eq.iter_mut().for_each(SmoothedBiquad::reset),
            Step::Active => {}
        }
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let m = self.eq_on.next();
            let (mut x, mut y) = (*a, *b);
            for band in &mut self.eq {
                (x, y) = band.process(x, y);
            }
            *a += (x - *a) * m;
            *b += (y - *b) * m;
        }
    }

    fn dynamics_stage(
        &mut self,
        l: &mut Block,
        r: &mut Block,
        key: Option<&Block>,
        gr: &mut Block,
    ) {
        let gate = self.gate_on.begin();
        let comp = self.comp_on.begin();
        if gate == Step::Fresh {
            self.gate.reset();
        }
        if comp == Step::Fresh {
            self.comp.reset();
        }
        if gate == Step::Skip && comp == Step::Skip {
            return;
        }
        let (mut sl, mut sr) = match key {
            Some(k) => (*k, *k),
            None => (*l, *r),
        };
        if self.on(P_FILTERS_TO_SC) {
            self.filters(&mut sl, &mut sr);
        }
        for n in 0..BATCH {
            let mut g = 1.0;
            if gate != Step::Skip {
                let m = self.gate_on.next();
                g *= 1.0 + (self.gate.process(sl[n], sr[n]) - 1.0) * m;
            }
            if comp != Step::Skip {
                let m = self.comp_on.next();
                g *= 1.0 + (self.comp.process(sl[n], sr[n]) - 1.0) * m;
            }
            l[n] *= g;
            r[n] *= g;
            gr[n] = (1.0 - g).clamp(0.0, 1.0);
        }
    }

    /// Insert return: crossfades the stage-0 signal into the return when one is connected.
    fn insert_return(&mut self, l: &mut Block, r: &mut Block, ret: Option<(&Block, &Block)>) {
        let use_return = ret.is_some() && !self.on(P_INSERT_BYPASS);
        self.insert.set_target(if use_return { 1.0 } else { 0.0 });
        if self.fresh_insert {
            snap(&mut self.insert);
            self.fresh_insert = false;
        }
        if is_silent(&self.insert) {
            return;
        }
        let zero = [0.0; BATCH];
        let (rl, rr) = ret.unwrap_or((&zero, &zero));
        for n in 0..BATCH {
            let m = self.insert.next_value();
            l[n] += (rl[n] - l[n]) * m;
            r[n] += (rr[n] - r[n]) * m;
        }
    }

    /// Dynamics and EQ in the order chosen by `route.dyn_post_eq`.
    fn dyn_and_eq(&mut self, l: &mut Block, r: &mut Block) {
        let key_block = self.key;
        let key = self.key_active.then_some(&key_block);
        let mut gr = [0.0f32; BATCH];
        if self.on(P_DYN_POST_EQ) {
            self.eq_stage(l, r);
            self.dynamics_stage(l, r, key, &mut gr);
        } else {
            self.dynamics_stage(l, r, key, &mut gr);
            self.eq_stage(l, r);
        }
        self.gr = gr;
    }

    /// Jumps every smoothed gain, crossfade and filter/EQ parameter to its target.
    fn snap_all(&mut self) {
        for s in self.smoothers_mut() {
            snap(s);
        }
        for s in self.sections_mut() {
            s.snap();
        }
        self.hpf.snap();
        self.lpf.snap();
        for b in &mut self.eq {
            b.snap();
        }
    }

    /// Stage 0: input → gain/phase → filters (→ dynamics/EQ unless `route.insert_pre`) →
    /// `insert_send_*`.
    fn stage0(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.fresh {
            self.snap_all();
            self.fresh = false;
        }
        let (mut l, mut r, _) = stereo_in(ctx, S_IN_L, S_IN_R);
        self.key = *ctx.input(SocketId(S_KEY));
        self.key_active = self.on(P_KEY_ON) && ctx.is_connected(SocketId(S_KEY));
        self.gr = [0.0; BATCH];
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let g = self.input.next_value();
            *a *= g;
            *b *= g;
        }
        if !self.on(P_FILTERS_TO_SC) {
            self.filters(&mut l, &mut r);
        }
        self.insert_pre = self.on(P_INSERT_PRE);
        if !self.insert_pre {
            self.dyn_and_eq(&mut l, &mut r);
        }
        *ctx.output(SocketId(S_INSERT_SEND_L)) = l;
        *ctx.output(SocketId(S_INSERT_SEND_R)) = r;
        self.mid = (l, r);
    }

    /// Stage 1: insert return → latency adjust (→ dynamics/EQ with `route.insert_pre`) →
    /// width → fader → pan → `out_*`, sends and `gr_cv_out`.
    fn stage1(&mut self, ctx: &mut ProcessContext<'_>) {
        self.apply_cv(ctx);
        let (mut l, mut r) = self.mid;
        let (ret_l, ret_r, ret_on) = stereo_in(ctx, S_INSERT_RETURN_L, S_INSERT_RETURN_R);
        self.insert_return(&mut l, &mut r, ret_on.then_some((&ret_l, &ret_r)));
        // Latency adjust (the lines always record, so changing the delay replays history).
        let d = self.latency as f32;
        for n in 0..BATCH {
            let (x, y) = (l[n], r[n]);
            if self.latency > 0 {
                l[n] = self.delay_l.read(d);
                r[n] = self.delay_r.read(d);
            }
            self.delay_l.write(x);
            self.delay_r.write(y);
        }
        if self.insert_pre {
            self.dyn_and_eq(&mut l, &mut r);
        }
        *ctx.output(SocketId(S_GR)) = self.gr;

        // Width, fader, mute; then pan for the main output and the send taps.
        let mut fa = [0.0f32; BATCH]; // fader × audible
        let mut au = [0.0f32; BATCH]; // audible
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let (a, b) = stereo_width(l[n], r[n], self.width.next_value());
            l[n] = a;
            r[n] = b;
            au[n] = self.audible.next_value();
            fa[n] = self.fader.next_value() * au[n];
            out_l[n] = a * fa[n] * self.pan_l.next_value();
            out_r[n] = b * fa[n] * self.pan_r.next_value();
        }
        *ctx.output(SocketId(S_OUT_L)) = out_l;
        *ctx.output(SocketId(S_OUT_R)) = out_r;
        for k in 0..SENDS {
            let (pre, post) = (&mut self.send_pre[k], &mut self.send_post[k]);
            if is_silent(pre) && is_silent(post) {
                continue;
            }
            let mut sl = [0.0f32; BATCH];
            let mut sr = [0.0f32; BATCH];
            for n in 0..BATCH {
                let g = pre.next_value() * au[n] + post.next_value() * fa[n];
                sl[n] = l[n] * g;
                sr[n] = r[n] * g;
            }
            let base = S_SEND_FIRST + 2 * k as u16;
            *ctx.output(SocketId(base)) = sl;
            *ctx.output(SocketId(base + 1)) = sr;
        }
    }
}

/// Reads a stereo input pair with the mono fallback (one connected side feeds both).
/// Returns `(l, r, any_connected)`.
pub(crate) fn stereo_in(ctx: &ProcessContext<'_>, l: u16, r: u16) -> (Block, Block, bool) {
    let (has_l, has_r) = (ctx.is_connected(SocketId(l)), ctx.is_connected(SocketId(r)));
    let a = if has_l || !has_r {
        *ctx.input(SocketId(l))
    } else {
        *ctx.input(SocketId(r))
    };
    let b = if has_l && has_r {
        *ctx.input(SocketId(r))
    } else {
        a
    };
    (a, b, has_l || has_r)
}

impl Device for Channel {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        self.delay_l = DelayLine::new(MAX_LATENCY_ADJUST as usize);
        self.delay_r = DelayLine::new(MAX_LATENCY_ADJUST as usize);
        for s in self.smoothers_mut() {
            s.set_time(SMOOTH_MS, sr);
        }
        for s in self.sections_mut() {
            s.set_time(sr);
        }
        self.hpf.set_sample_rate(sr);
        self.lpf.set_sample_rate(sr);
        self.comp.set_sample_rate(sr);
        self.gate.set_sample_rate(sr);
        for b in &mut self.eq {
            b.set_sample_rate(sr);
        }
        for i in 0..PARAM_COUNT {
            self.apply(i);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.snap_all();
        self.fresh = true;
        self.fresh_insert = true;
        self.hpf.reset();
        self.lpf.reset();
        self.comp.reset();
        self.gate.reset();
        for b in &mut self.eq {
            b.reset();
        }
        self.delay_l.clear();
        self.delay_r.clear();
        self.mid = ([0.0; BATCH], [0.0; BATCH]);
        self.gr = [0.0; BATCH];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let i = id.0 as usize;
        if i < PARAM_COUNT {
            self.values[i] = plain;
            self.apply(i);
        }
    }

    fn latency_frames(&self) -> u32 {
        self.phantom_latency
    }

    /// Runs both stages (for hosts that do not split stages, e.g. the test harness).
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.stage0(ctx);
        self.stage1(ctx);
    }

    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        match stage {
            0 => self.stage0(ctx),
            1 => self.stage1(ctx),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
