//! `WorkflowCommand::AddReverbSend`: the reverb send wizard of a track, in one undo step.
//!
//! The track's console channel gets a send to a new FX return. The return holds, in series, an
//! optional de-esser (for a vocal), an EQ with a high-pass and a low-pass, and a Space Reverb at
//! 100 % wet:
//!
//! ```text
//! channel send k -> [de-esser] -> EQ (high-pass, low-pass) -> Space Reverb -> return k
//! channel out ----------------------------------------------> Space Reverb key (sc_in_l/r)
//! ```
//!
//! * **Send.** `send{k}.on` is set, the level is [`ReverbSendOptions::send_db`] and the send is
//!   post-fader unless [`ReverbSendOptions::pre_fader`] says otherwise: with a post-fader send
//!   the reverb follows the fader, so the balance of dry and wet stays when the track is turned
//!   up or down.
//! * **EQ.** The high-pass (band 1, 24 dB per octave) and the low-pass (band 6, 24 dB per octave)
//!   keep the lows and the extreme highs of the source out of the reverb: low-mid mud and harsh
//!   air in a tail are the usual trouble.
//! * **Decay.** The reverb's decay is [`ReverbSendOptions::decay`], or, when that is `None`,
//!   [`suggested_decay`]: twice the median inter-onset interval of the track's played notes, kept
//!   between [`MIN_DECAY`] and [`MAX_DECAY`] seconds ([`MAX_VOCAL_DECAY`] for a vocal). The
//!   onsets are the distinct start ticks of the notes (a chord is one onset), the intervals are
//!   converted to seconds through the tempo map, and a tail that rings for about twice the gap
//!   between notes has faded by about 30 dB when the next one arrives. A track without enough
//!   notes (fewer than four distinct onsets: an audio track, a sparse clip) gets
//!   [`DEFAULT_DECAY`].
//! * **Duck.** With [`ReverbSendOptions::duck`] the channel's output is cabled to the reverb's key
//!   sockets and the duck is on ([`DUCK_AMOUNT`], broadband); its release is the median interval
//!   (three quarters of it, between [`MIN_RELEASE_MS`] and [`MAX_RELEASE_MS`], or the reverb's own
//!   default when the track has no rhythm to follow), so the reverb comes back between the notes.
//!
//! The whole plan is built on a scratch copy of the document and applied as one edit; a refusal
//! (no channel for the track, no free slot, a cable the rack does not take) changes nothing.

use crate::ConsoleCommand;
use crate::console_cmd::{self, Planner};
use crate::history::Edit;
use crate::song::{Song, Track, flatten_track_notes};
use crate::workflow_create::{
    SEND_SLOTS_FULL, first_free_send_slot, send_effect_index, series_after,
};
use crate::{Command, DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::Registry;

/// The reverb type of the wizard.
pub const REVERB_TYPE: &str = "atomik.space-reverb";
const EQ_TYPE: &str = "atomik.eq";
const DE_ESSER_TYPE: &str = "atomik.de-esser";

/// The decay of a track without a rhythm to read, in seconds.
pub const DEFAULT_DECAY: f32 = 2.0;
/// The shortest suggested decay, in seconds.
pub const MIN_DECAY: f32 = 0.4;
/// The longest suggested decay, in seconds.
pub const MAX_DECAY: f32 = 4.0;
/// The longest suggested decay of a vocal, in seconds.
pub const MAX_VOCAL_DECAY: f32 = 3.0;
/// The fewest distinct onsets the suggestion reads a rhythm from.
pub const MIN_ONSETS: usize = 4;
/// How much of the reverb the duck takes out at a loud key.
pub const DUCK_AMOUNT: f32 = 0.5;
/// The shortest and the longest duck release the wizard sets, in ms.
pub const MIN_RELEASE_MS: f32 = 120.0;
/// See [`MIN_RELEASE_MS`].
pub const MAX_RELEASE_MS: f32 = 800.0;
/// The range of the EQ's high-pass the wizard accepts, in Hz.
pub const HIGH_PASS_RANGE: (f32, f32) = (20.0, 1_000.0);
/// The range of the EQ's low-pass the wizard accepts, in Hz.
pub const LOW_PASS_RANGE: (f32, f32) = (1_000.0, 20_000.0);
/// The range of the send level the wizard accepts, in dB.
pub const SEND_RANGE_DB: (f32, f32) = (-60.0, 6.0);
/// The sample rate the note intervals are converted at (only the ratio of frames matters).
const SAMPLE_RATE: u32 = 48_000;

/// The choices of the wizard.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverbSendOptions {
    /// Put a de-esser before the EQ: sibilance in a tail is harsh.
    pub vocal: bool,
    /// The reverb's decay in seconds; `None` suggests it from the track ([`suggested_decay`]).
    pub decay: Option<f32>,
    /// Cable the track to the reverb's key and turn the duck on.
    pub duck: bool,
    /// The EQ's high-pass in Hz.
    pub high_pass: f32,
    /// The EQ's low-pass in Hz.
    pub low_pass: f32,
    /// The send level in dB.
    pub send_db: f32,
    /// The send taps the signal before the fader instead of after it.
    pub pre_fader: bool,
}

impl Default for ReverbSendOptions {
    fn default() -> Self {
        Self {
            vocal: false,
            decay: None,
            duck: true,
            high_pass: 300.0,
            low_pass: 8_000.0,
            send_db: -12.0,
            pre_fader: false,
        }
    }
}

/// The distinct onsets of `track`'s played notes as seconds, ascending.
fn onsets(song: &Song, track: &Track) -> Vec<f64> {
    let clock = song.frame_clock(SAMPLE_RATE);
    let mut ticks: Vec<i64> = flatten_track_notes(song, track)
        .into_iter()
        .map(|(tick, ..)| tick)
        .collect();
    ticks.dedup();
    ticks
        .into_iter()
        .map(|t| clock.at(t) as f64 / f64::from(SAMPLE_RATE))
        .collect()
}

/// The median gap between the onsets of `track`'s played notes, in seconds; `None` with fewer
/// than [`MIN_ONSETS`] distinct onsets.
pub fn median_onset_gap(song: &Song, track: &Track) -> Option<f32> {
    let onsets = onsets(song, track);
    if onsets.len() < MIN_ONSETS {
        return None;
    }
    let mut gaps: Vec<f64> = onsets.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.retain(|g| *g > 0.0);
    gaps.sort_by(f64::total_cmp);
    let mid = gaps.len() / 2;
    let median = if gaps.len() % 2 == 1 {
        gaps[mid]
    } else {
        0.5 * (gaps[mid - 1] + gaps[mid])
    };
    Some(median as f32)
}

/// The decay the wizard suggests for `track`, in seconds (see the module docs): twice the median
/// onset gap between [`MIN_DECAY`] and [`MAX_DECAY`] ([`MAX_VOCAL_DECAY`] for a vocal), or
/// [`DEFAULT_DECAY`] when there is no rhythm to read.
pub fn suggested_decay(song: &Song, track: &Track, vocal: bool) -> f32 {
    let top = if vocal { MAX_VOCAL_DECAY } else { MAX_DECAY };
    match median_onset_gap(song, track) {
        Some(gap) => (2.0 * gap).clamp(MIN_DECAY, top),
        None => DEFAULT_DECAY.min(top),
    }
}

/// The Space Reverb at the end of the chain on return `slot` (the one the wizard made), found by
/// following the return's cables from its first effect. `None` when the slot is empty or its
/// chain holds no Space Reverb.
pub fn reverb_of_return(doc: &RackDoc, slot: usize) -> Option<DeviceKey> {
    let mut at = doc.console.returns.get(slot)?.clone()?;
    // A return holds a handful of effects; the bound keeps a hand-made loop from spinning.
    for _ in 0..16 {
        if doc.device(&at)?.type_id == REVERB_TYPE {
            return Some(at);
        }
        at = doc
            .cables
            .iter()
            .find(|c| c.from.device == at && c.from.socket == "out_l")
            .map(|c| c.to.device.clone())
            .filter(|next| Some(next) != doc.console.master.as_ref())?;
    }
    None
}

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// Sets `param` of the top-level device `key` in the plan (clamped; nothing when unchanged).
fn set(p: &mut Planner<'_>, key: &DeviceKey, param: &str, value: f32) -> Result<(), ModelError> {
    let registry = p.registry;
    let device = rules::resolve(p.doc(), key)?;
    let desc = rules::descriptor(registry, &device.type_id)?;
    let spec = desc
        .param_by_key(param)
        .ok_or_else(|| ModelError::UnknownKey(param.to_owned()))?;
    let value = spec.clamp(value);
    let old = device.params.get(param).copied();
    if old.unwrap_or(spec.default).to_bits() == value.to_bits() {
        return Ok(());
    }
    // A value that equals the default is stored as no value, as everywhere else.
    let new = (spec.default.to_bits() != value.to_bits()).then_some(value);
    p.push(Edit::Param {
        key: key.clone(),
        param: param.to_owned(),
        old,
        new,
        default: spec.default,
    });
    Ok(())
}

/// Adds a device of `type_id` at rack `index` in the plan and returns its key.
fn add(p: &mut Planner<'_>, type_id: &str, index: usize) -> Result<DeviceKey, ModelError> {
    let key = rules::generate_key(p.doc(), type_id);
    let command = Command::AddDevice {
        index,
        type_id: type_id.to_owned(),
        key: Some(key.clone()),
    };
    let edit = crate::Document::plan(p.doc(), command, p.registry)?.0;
    p.push(edit);
    Ok(key)
}

/// Plans the reverb send of the track whose device is `track` (see the module docs).
///
/// # Errors
/// The device is inside a container, has no song track or no console channel, the send/return
/// slots are all in use, an option is out of its range, or a device or cable is refused.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    track: &DeviceKey,
    options: &ReverbSendOptions,
) -> Result<Edit, ModelError> {
    if track.0.contains('/') {
        return Err(refuse(format!(
            "{track} is inside a container: a reverb send works on a track of the rack"
        )));
    }
    let song_track = doc
        .song
        .track(track)
        .ok_or_else(|| refuse(format!("{track} has no track")))?;
    let channel = doc
        .console
        .channel_of(track)
        .ok_or_else(|| refuse(format!("{track} has no console channel to send from")))?
        .channel
        .clone();
    let master = doc
        .console
        .master
        .clone()
        .ok_or_else(|| refuse("the console has no master"))?;
    let in_range = |v: f32, (lo, hi): (f32, f32)| v.is_finite() && (lo..=hi).contains(&v);
    if !in_range(options.high_pass, HIGH_PASS_RANGE)
        || !in_range(options.low_pass, LOW_PASS_RANGE)
        || !in_range(options.send_db, SEND_RANGE_DB)
        || options.decay.is_some_and(|d| !in_range(d, (0.2, 20.0)))
    {
        return Err(ModelError::OutOfRange(
            "a reverb send option is outside its range".to_owned(),
        ));
    }
    let slot = first_free_send_slot(doc).ok_or_else(|| refuse(SEND_SLOTS_FULL))?;
    let first = send_effect_index(doc, &master).ok_or_else(|| refuse("the master is missing"))?;

    let mut p = Planner::new(doc, registry);
    let mut types = Vec::new();
    if options.vocal {
        types.push(DE_ESSER_TYPE);
    }
    types.extend([EQ_TYPE, REVERB_TYPE]);
    let mut keys: Vec<DeviceKey> = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        keys.push(add(&mut p, ty, first + i)?);
    }
    // The return: send k -> chain -> return k.
    let set_return = console_cmd::plan(
        p.doc(),
        registry,
        ConsoleCommand::SetReturn {
            slot,
            effect: Some(keys[0].clone()),
        },
    )?;
    p.push(set_return);
    for pair in keys.windows(2) {
        series_after(&mut p, &master, slot, &pair[0], &pair[1])
            .ok_or_else(|| refuse("the effects could not be put in series on the return"))?;
    }
    let (eq, reverb) = (&keys[keys.len() - 2], &keys[keys.len() - 1]);

    // The EQ keeps the lows and the extreme highs out of the reverb.
    set(&mut p, eq, "band1.on", 1.0)?;
    set(&mut p, eq, "band1.freq", options.high_pass)?;
    set(&mut p, eq, "lowcut.slope", 1.0)?;
    set(&mut p, eq, "band6.on", 1.0)?;
    set(&mut p, eq, "band6.freq", options.low_pass)?;
    set(&mut p, eq, "highcut.slope", 1.0)?;

    // The reverb is all wet; the return is where the dry/wet balance is made.
    let vocal = options.vocal;
    let decay = options
        .decay
        .unwrap_or_else(|| suggested_decay(&doc.song, song_track, vocal));
    set(&mut p, reverb, "mix", 1.0)?;
    set(&mut p, reverb, "decay", decay)?;

    if options.duck {
        set(&mut p, reverb, "duck", DUCK_AMOUNT)?;
        if let Some(gap) = median_onset_gap(&doc.song, song_track) {
            let release = (gap * 750.0).clamp(MIN_RELEASE_MS, MAX_RELEASE_MS);
            set(&mut p, reverb, "duck_release", release)?;
        }
        for side in ["l", "r"] {
            p.connect(console_cmd::wire(
                console_cmd::at(&channel, format!("out_{side}")),
                console_cmd::at(reverb, format!("sc_in_{side}")),
            ))?;
        }
    }

    // The send of the channel.
    let k = slot + 1;
    set(&mut p, &channel, &format!("send{k}.on"), 1.0)?;
    set(&mut p, &channel, &format!("send{k}.level"), options.send_db)?;
    set(
        &mut p,
        &channel,
        &format!("send{k}.pre"),
        if options.pre_fader { 1.0 } else { 0.0 },
    )?;
    Ok(p.finish())
}
