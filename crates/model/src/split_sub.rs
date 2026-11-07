//! `WorkflowCommand::SplitSub`: a Mono Bass track gets a clean, mono sub under its sound, so the
//! width and the unison of the bass can stay wide above it.
//!
//! The track's bass moves into a container (the same combine as [`crate::ContainerCommand::Combine`],
//! so the track, the console channel and the note entry follow it) and three devices join it:
//!
//! * a Poly Synth **sub layer**: one voice, two sines (one for each oscillator of the bass, at its
//!   pitch, without detune, balanced as the fundamentals of the bass's oscillators are), their phase reset
//!   at every note (`osc.phase` reset, start 0), no unison, spread, chorus or echo, a 12 dB state
//!   variable low-pass at 1.25 times the split frequency (flat below it, so the notes next to the
//!   split lose little, and no more than a little where the bass's low cut takes over) with no
//!   resonance, envelope or key tracking, and the amp envelope,
//!   glide, legato and bend range of the bass. It hears the same notes as the bass (a full-range
//!   note split from the container's input) and plays the fundamentals of the bass's oscillators at
//!   their level (a wave's fundamental is 2/pi of its peak for a saw, 4/pi for a square, 8/pi^2 for
//!   a triangle and 1 for a sine);
//! * an **EQ** after the bass with its low cut on (24 dB per octave at the split frequency), so
//!   what the bass plays under the split frequency, where its phase wanders with the unison, is gone;
//! * a **Line Mixer** that sums the two into the container's output. The mixer's centred pan takes
//!   3 dB off each channel, so both channels are set 3.0103 dB up: the sum keeps the level.
//!
//! **Level.** With the bass's filter open the sum is within 1 dB of the bass (measured on bounces of
//! several patches); the bass's own filter, its resonance and drive change the weight of its
//! fundamental in ways that are not known without playing it, so a patch with a strong resonance can
//! move by up to about 3 dB either way: the sub layer's `master.level` is the trim.
//!
//! The bass itself is not touched: its params stay as they were, and it keeps its splits. All of it
//! is one planned edit built on a scratch copy of the document and checked as a whole, so it is one
//! undo step and a refusal changes nothing. The automation lanes of the track name params of the
//! bass, which is now inside the container: the combine drops them and says so in [`Planned::notes`].

use crate::console_cmd::Planner;
use crate::container::{CONTAINER_TYPE, ContainerCommand};
use crate::container_move::{self, Planned};
use crate::history::Edit;
use crate::{Cable, DeviceKey, ModelError, RackDevice, RackDoc, SocketRef, container_cmd, rules};
use atomik_device_sdk::Registry;

/// The only device type Split Sub works on.
pub const BASS_TYPE: &str = "atomik.mono-bass";
/// Where the sub layer's low-pass sits, as a multiple of the split frequency (chosen on bounces:
/// the level of the sum next to the split is flattest there).
const SUB_LOWPASS_RATIO: f32 = 1.25;
/// The sub layer's device type.
const SUB_TYPE: &str = "atomik.poly-synth";
const EQ_TYPE: &str = "atomik.eq";
const MIXER_TYPE: &str = "atomik.line-mixer";
/// The lowest split frequency in Hz.
pub const MIN_SPLIT_HZ: f32 = 60.0;
/// The highest split frequency in Hz.
pub const MAX_SPLIT_HZ: f32 = 250.0;
/// The split frequency the app uses unless it is told another one, in Hz.
pub const DEFAULT_SPLIT_HZ: f32 = 120.0;
/// What a mixer channel gives back of the -3 dB its centred pan takes (20 log10 of the square root
/// of 2), in dB.
const PAN_COMPENSATION_DB: f32 = 3.0103;
/// The amplitude of the fundamental of a wave of unit peak: saw, square, triangle, sine (the
/// order of `osc1.wave`).
const FUNDAMENTAL: [f32; 4] = [
    2.0 / std::f32::consts::PI,
    4.0 / std::f32::consts::PI,
    8.0 / (std::f32::consts::PI * std::f32::consts::PI),
    1.0,
];

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// The value of param `param` of `device`: its own, else the default of the descriptor.
fn value_of(device: &RackDevice, registry: &Registry, param: &str) -> Result<f32, ModelError> {
    let desc = rules::descriptor(registry, &device.type_id)?;
    let spec = desc
        .param_by_key(param)
        .ok_or_else(|| ModelError::UnknownKey(param.to_owned()))?;
    Ok(device.params.get(param).copied().unwrap_or(spec.default))
}

/// Sets `param` of the device `local` inside `container` in the plan (clamped to its range; nothing
/// is planned when it already has the value).
fn set(
    p: &mut Planner<'_>,
    container: &DeviceKey,
    local: &DeviceKey,
    param: &str,
    value: f32,
) -> Result<(), ModelError> {
    let key = DeviceKey(format!("{container}/{local}"));
    let registry = p.registry;
    let device = rules::resolve(p.doc(), &key)?;
    let desc = rules::descriptor(registry, &device.type_id)?;
    let spec = desc
        .param_by_key(param)
        .ok_or_else(|| ModelError::UnknownKey(param.to_owned()))?;
    let new = spec.clamp(value);
    let old = device.params.get(param).copied();
    if old.unwrap_or(spec.default).to_bits() == new.to_bits() {
        return Ok(());
    }
    let edit = Edit::Param {
        key,
        param: param.to_owned(),
        old,
        new: Some(new),
        default: spec.default,
    };
    p.push(edit);
    Ok(())
}

/// Adds `type_id` as `key` inside `container` (with a full-range note split when `split`).
fn add(
    p: &mut Planner<'_>,
    container: &DeviceKey,
    type_id: &str,
    key: &str,
    split: bool,
) -> Result<DeviceKey, ModelError> {
    let local = DeviceKey(key.to_owned());
    let edit = container_cmd::plan(
        p.doc(),
        p.registry,
        ContainerCommand::AddInner {
            container: container.clone(),
            index: usize::MAX,
            type_id: type_id.to_owned(),
            key: Some(local.clone()),
            auto_split: split,
        },
    )?;
    p.push(edit);
    Ok(local)
}

/// A cable between two devices of `container` (`None` for the container's own edge).
fn wire(
    p: &mut Planner<'_>,
    container: &DeviceKey,
    (from, out): (Option<&DeviceKey>, &str),
    (to, input): (Option<&DeviceKey>, &str),
) -> Result<(), ModelError> {
    let at = |device: Option<&DeviceKey>, socket: &str| SocketRef {
        device: DeviceKey(match device {
            Some(d) => format!("{container}/{d}"),
            None => format!("{container}/boundary"),
        }),
        socket: socket.to_owned(),
    };
    let cable = Cable {
        from: at(from, out),
        to: at(to, input),
    };
    let edit = container_cmd::connect(p.doc(), p.registry, &cable)?;
    p.push(edit);
    Ok(())
}

/// The two oscillators of `bass` as the sub layer plays them: for each its octave and semitone
/// offsets and the amplitude of its fundamental (its weight in the mix times the fundamental of
/// its wave).
fn oscillators(bass: &RackDevice, registry: &Registry) -> Result<[(f32, f32, f32); 2], ModelError> {
    let get = |k: &str| value_of(bass, registry, k);
    let mix = get("osc.mix")?.clamp(0.0, 1.0);
    let fundamental = |wave: f32| FUNDAMENTAL[(wave.round().max(0.0) as usize).min(3)];
    Ok([
        (
            get("osc1.octave")?,
            get("osc1.semi")?,
            (1.0 - mix) * fundamental(get("osc1.wave")?),
        ),
        (
            get("osc2.octave")?,
            get("osc2.semi")?,
            mix * fundamental(get("osc2.wave")?),
        ),
    ])
}

/// Plans the split of the bass `device` at `freq` Hz (see the module docs).
///
/// # Errors
/// The frequency is outside [`MIN_SPLIT_HZ`]..=[`MAX_SPLIT_HZ`], the device is not a top-level Mono
/// Bass with a song track, a device the plan needs is not in the registry, or the combine or one of
/// the additions is refused.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    freq: f32,
) -> Result<Planned, ModelError> {
    if !(MIN_SPLIT_HZ..=MAX_SPLIT_HZ).contains(&freq) {
        return Err(ModelError::OutOfRange(format!(
            "the split frequency {freq} Hz is outside {MIN_SPLIT_HZ} to {MAX_SPLIT_HZ} Hz"
        )));
    }
    if device.0.contains('/') {
        return Err(refuse(format!(
            "{device} is inside a container: Split Sub works on a track of the rack"
        )));
    }
    let bass = rules::device(doc, device)?;
    if bass.type_id != BASS_TYPE {
        return Err(refuse(format!(
            "Split Sub works on a Mono Bass track, not on {}",
            bass.name
        )));
    }
    if doc.song.track(device).is_none() {
        return Err(refuse(format!("{} has no track", bass.name)));
    }
    for type_id in [SUB_TYPE, EQ_TYPE, MIXER_TYPE] {
        rules::descriptor(registry, type_id)?;
    }
    let name = bass.name.clone();
    let [(octave1, semi1, a1), (octave2, semi2, a2)] = oscillators(bass, registry)?;
    let level = value_of(bass, registry, "master.level")?;
    // The two sines carry the fundamentals of the two oscillators: their sum is the level and
    // their balance the crossfade.
    let total = (a1 + a2).max(1.0e-3);
    let sub_level = level + 20.0 * total.log10();
    let sub_mix = a2 / total;

    let combined = container_move::combine(doc, registry, std::slice::from_ref(device), &name)?;
    let mut p = Planner::new(doc, registry);
    p.push(combined.edit);
    let container = p
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == CONTAINER_TYPE && doc.device(&d.key).is_none())
        .map(|d| d.key.clone())
        .ok_or_else(|| refuse("the bass could not be put in a container"))?;
    let original = p
        .doc()
        .resolve(&container)
        .and_then(|c| c.inner.as_ref())
        .and_then(|inner| inner.devices.first().map(|d| d.key.clone()))
        .ok_or_else(|| refuse("the container holds nothing"))?;

    let sub = add(&mut p, &container, SUB_TYPE, "sub", true)?;
    let eq = add(&mut p, &container, EQ_TYPE, "split-eq", false)?;
    let mix = add(&mut p, &container, MIXER_TYPE, "split-mix", false)?;

    // The sub layer: a mono sine at the bass's own envelope, low-passed at the split.
    let from_bass = |param: &str| value_of(bass, registry, param);
    for (param, value) in [
        ("voices", 1.0),
        ("legato", from_bass("legato")?),
        ("glide", from_bass("glide")?),
        ("osc1.wave", 3.0),
        ("osc1.octave", octave1),
        ("osc1.semi", semi1),
        ("osc1.fine", 0.0),
        ("osc2.wave", 3.0),
        ("osc2.octave", octave2),
        ("osc2.semi", semi2),
        ("osc2.fine", 0.0),
        ("osc2.mode", 0.0),
        ("osc.mix", sub_mix),
        ("osc.phase", 0.0),
        ("osc.phase_start", 0.0),
        ("noise.level", 0.0),
        ("spread", 0.0),
        ("drift", 0.0),
        ("chorus.mix", 0.0),
        ("delay.mix", 0.0),
        ("filter.type", 0.0),
        ("filter.mode", 0.0),
        ("filter.cutoff", freq * SUB_LOWPASS_RATIO),
        ("filter.resonance", 0.0),
        ("filter.drive", 0.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter2.mode", 0.0),
        ("env.amp.attack", from_bass("env.amp.attack")?),
        ("env.amp.decay", from_bass("env.amp.decay")?),
        ("env.amp.sustain", from_bass("env.amp.sustain")?),
        ("env.amp.release", from_bass("env.amp.release")?),
        ("velocity.amp", from_bass("velocity.amp")?),
        ("bend.range", from_bass("bend.range")?),
        ("master.level", sub_level),
    ] {
        set(&mut p, &container, &sub, param, value)?;
    }
    // The bass above the split: the EQ's low cut at 24 dB per octave.
    for (param, value) in [
        ("band1.on", 1.0),
        ("band1.freq", freq),
        ("lowcut.slope", 1.0),
    ] {
        set(&mut p, &container, &eq, param, value)?;
    }
    for param in ["ch1.level", "ch2.level"] {
        set(&mut p, &container, &mix, param, PAN_COMPENSATION_DB)?;
    }

    // The bass goes through the EQ instead of straight to the container's output.
    for side in ["out_l", "out_r"] {
        let to = SocketRef {
            device: DeviceKey(format!("{container}/boundary")),
            socket: side.to_owned(),
        };
        if p.doc()
            .resolve(&container)
            .and_then(|c| c.inner.as_ref())
            .is_some_and(|inner| {
                inner
                    .cables
                    .iter()
                    .any(|c| c.to.device.0 == "boundary" && c.to.socket == side)
            })
        {
            let edit = container_cmd::disconnect(p.doc(), registry, &to)?;
            p.push(edit);
        }
    }
    for (side, mixer_side) in [("l", "l"), ("r", "r")] {
        let out = format!("out_{side}");
        let input = format!("in_{side}");
        wire(
            &mut p,
            &container,
            (Some(&original), &out),
            (Some(&eq), &input),
        )?;
        wire(
            &mut p,
            &container,
            (Some(&eq), &out),
            (Some(&mix), &format!("ch1_in_{mixer_side}")),
        )?;
        wire(
            &mut p,
            &container,
            (Some(&sub), &out),
            (Some(&mix), &format!("ch2_in_{mixer_side}")),
        )?;
        wire(
            &mut p,
            &container,
            (Some(&mix), &format!("master_{side}")),
            (None, &out),
        )?;
    }

    let edit = p.finish();
    let mut scratch = doc.clone();
    edit.perform(&mut scratch)?;
    scratch.check_containers(registry)?;
    Ok(Planned {
        edit,
        notes: combined.notes,
    })
}

impl RackDoc {
    /// What Split Sub would leave behind (the automation lanes of the track, which name params of
    /// the bass that goes inside the container), as lines for the user.
    ///
    /// # Errors
    /// The error the command itself would give.
    pub fn split_sub_notes(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        freq: f32,
    ) -> Result<Vec<String>, ModelError> {
        plan(self, registry, device, freq).map(|p| p.notes)
    }
}
