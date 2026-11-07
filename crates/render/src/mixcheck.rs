//! The mix check: renders the strips of a song, measures them and turns the numbers into findings
//! ([`atomik_dsp::mixcheck`] holds the measurements and every threshold).
//!
//! # Rendering
//!
//! [`plan`] lists what is read: every console channel (up to [`MAX_TRACKS`]), the FX returns and
//! the master, each as a stem ([`crate::stems`]), and the **feed** of every non-linear device on
//! an insert chain (see [`NONLINEAR_DEVICES`]): the signal at the output of the stage before it,
//! or at the insert send for the first. [`render_check`] renders them in the deterministic stem
//! passes and keeps, as each pass ends, a [`SignalStats`] and a band map per stem and a
//! [`SignalStats`] per feed, so the audio of a long song is never held at once.
//!
//! # Findings
//!
//! [`findings`] is a pure function of the document and the measurements, so it is tested without
//! a render. Each [`Finding`] names the track, carries the measured number and a one-line reason,
//! and may carry a [`Fix`] (one undo step in the app) or the [`Tool`] that works on the problem.
//!
//! * [`Kind::BassRoot`]: the root of a bass part (the key at the middle of its held time) is
//!   outside [`BASS_ROOT_HZ`]. A track is a bass when its name has the word bass, sub or 808 or
//!   its instrument is the mono bass.
//! * [`Kind::WeakFundamental`]: the fundamental of a bass (its root, or the loudest low band when
//!   the part has no notes) or of a kick (the loudest band in [`KICK_HZ`]; a track is a kick when
//!   its name has the word kick) is more than [`WEAK_FUNDAMENTAL_DB`] under its strongest harmonic.
//! * [`Kind::LowMid`]: the share of the master's power in [`LOW_MID_HZ`] is over
//!   [`LOW_MID_SHARE`]; the tracks (returns included) are ranked by their share of the power in
//!   that region.
//! * [`Kind::Level`]: a non-linear device is fed more than [`OFF_REFERENCE_DB`] from
//!   [`REFERENCE_RMS_DB`].
//! * [`Kind::Peaks`]: the master peaks over [`PEAK_REPORT_DB`]; for each of its highest spots the
//!   tracks whose own peak there is the highest are named.
//! * [`Kind::Mono`]: the master's low end loses at least [`MONO_LOSS_DB`] in mono; the tracks that
//!   lose most of it are named.

use crate::RenderError;
use crate::export::ExportSpan;
use crate::plugin_pass::RenderControl;
use crate::project::{PluginBounce, ProjectOptions};
use crate::stems::{Stem, StemRequest, StemStrip, render_stems};
use atomik_device_sdk::Registry;
use atomik_dsp::masking::{BANDS, BandMap, analyse, band_centre};
use atomik_dsp::mixcheck::{
    BASS_ROOT_HZ, BASS_SEARCH_HZ, KICK_HZ, LOW_MID_HZ, LOW_MID_SHARE, LOW_MID_TRACKS, MONO_LOSS_DB,
    MONO_MIN_DB, MONO_TRACK_LOSS_DB, NONLINEAR_DEVICES, OFF_REFERENCE_DB, PEAK_REPORT_DB,
    PEAK_SPOTS, PEAK_SPREAD_DB, PEAK_TRACKS, PEAK_WINDOW, REFERENCE_RMS_DB, SignalStats,
    WEAK_FUNDAMENTAL_DB, amp_db, bass_root, fundamental_of, key_hz, key_name, loudest_in,
    low_mid_bands, low_mid_power, measure, off_reference, peak_spots, root_in_range, trim_for,
};
use atomik_model::{
    DeviceKey, RackDoc, SocketRef, StemKind, StemSource, insert_chain, song::flatten_notes,
    stem_sources,
};
use std::path::Path;

/// The most channels the check reads.
pub const MAX_TRACKS: usize = atomik_dsp::masking::MAX_TRACKS;
/// The most device feeds the check reads.
pub const MAX_PROBES: usize = 16;

/// The feed of one non-linear device on an insert chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The strip (channel, bus or master) whose chain the device is on.
    pub strip: DeviceKey,
    /// The name of the strip.
    pub strip_name: String,
    /// The device.
    pub device: DeviceKey,
    /// The name of the device.
    pub device_name: String,
    /// The device's type id.
    pub type_id: String,
}

/// What a check reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The strips to take as stems: channels, then the FX returns, then the master.
    pub sources: Vec<StemSource>,
    /// The device feeds.
    pub probes: Vec<Probe>,
    probe_sources: Vec<StemSource>,
}

/// One strip, measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    /// The strip.
    pub source: StemSource,
    /// Its level, peaks and low end.
    pub stats: SignalStats,
    /// Its band map.
    pub map: BandMap,
}

/// What a check measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The sample rate of the project.
    pub rate: u32,
    /// The strips, in the order of [`Plan::sources`].
    pub signals: Vec<Measured>,
    /// The feeds, in the order of [`Plan::probes`].
    pub feeds: Vec<(Probe, SignalStats)>,
}

fn device_label(doc: &RackDoc, key: &DeviceKey) -> String {
    doc.device(key).map_or_else(
        || key.0.clone(),
        |d| {
            let name = d.name.trim();
            if name.is_empty() {
                d.type_id
                    .strip_prefix("atomik.")
                    .unwrap_or(&d.type_id)
                    .replace('-', " ")
            } else {
                name.to_owned()
            }
        },
    )
}

/// What a check of `doc` reads (see the module docs).
///
/// # Errors
/// The song has no mixer channel or no master.
pub fn plan(doc: &RackDoc, registry: &Registry) -> Result<Plan, RenderError> {
    let all = stem_sources(doc, registry);
    let mut sources: Vec<StemSource> = all
        .iter()
        .filter(|s| s.kind == StemKind::Channel)
        .take(MAX_TRACKS)
        .cloned()
        .collect();
    if sources.is_empty() {
        return Err(RenderError::Project(
            "the mix check reads the mixer channels: the song has none".to_owned(),
        ));
    }
    sources.extend(all.iter().filter(|s| s.kind == StemKind::Return).cloned());
    let Some(master) = all.iter().find(|s| s.kind == StemKind::Master) else {
        return Err(RenderError::Project("the song has no master".to_owned()));
    };
    sources.push(master.clone());
    let mut probes = Vec::new();
    let mut probe_sources = Vec::new();
    let strips = all
        .iter()
        .filter(|s| matches!(s.kind, StemKind::Channel | StemKind::Bus | StemKind::Master));
    'strips: for strip in strips {
        let chain = insert_chain(doc, registry, &strip.key);
        for (slot, device) in chain.iter().enumerate() {
            let Some(type_id) = doc.device(device).map(|d| d.type_id.as_str()) else {
                continue;
            };
            if !NONLINEAR_DEVICES.contains(&type_id) {
                continue;
            }
            if probes.len() == MAX_PROBES {
                break 'strips;
            }
            let (from, left, right) = if slot == 0 {
                (&strip.key, "insert_send_l", "insert_send_r")
            } else {
                (&chain[slot - 1], "out_l", "out_r")
            };
            let at = |socket: &str| SocketRef {
                device: from.clone(),
                socket: socket.to_owned(),
            };
            probe_sources.push(StemSource {
                kind: StemKind::Channel,
                key: device.clone(),
                name: device_label(doc, device),
                tap: [at(left), at(right)],
                dry: None,
            });
            probes.push(Probe {
                strip: strip.key.clone(),
                strip_name: strip.name.clone(),
                device: device.clone(),
                device_name: device_label(doc, device),
                type_id: type_id.to_owned(),
            });
        }
    }
    Ok(Plan {
        sources,
        probes,
        probe_sources,
    })
}

/// Renders `plan` over `span` and measures it. A stopped render returns `None`. `bundle_root` is
/// the folder the project's audio lives in, as for a bounce.
///
/// # Errors
/// The span is out of range or the bounce of the document fails.
pub fn render_check(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    (plan, span): (&Plan, ExportSpan),
    control: RenderControl<'_>,
) -> Result<Option<Rendered>, RenderError> {
    let signals = plan.sources.len();
    let stems = StemRequest {
        span,
        tail_seconds: 0.0,
        strip: StemStrip::Full,
        sources: plan
            .sources
            .iter()
            .chain(&plan.probe_sources)
            .cloned()
            .collect(),
    };
    let mut measured: Vec<(usize, Measured)> = Vec::new();
    let mut fed: Vec<(usize, SignalStats)> = Vec::new();
    let mut take = |stem: Stem| {
        let planes = &stem.audio.channels;
        let right = planes.get(1).map_or(&[][..], Vec::as_slice);
        let rate = stem.audio.sample_rate;
        let stats = measure(&planes[0], right, rate);
        if stem.index < signals {
            let map = analyse(&planes[0], right, rate);
            measured.push((
                stem.index,
                Measured {
                    source: stem.source,
                    stats,
                    map,
                },
            ));
        } else {
            fed.push((stem.index - signals, stats));
        }
        Ok(())
    };
    let done = render_stems(
        doc,
        registry,
        (options, bundle_root),
        plugins,
        &stems,
        (control, &mut take),
    )?;
    if done.is_none() {
        return Ok(None);
    }
    measured.sort_by_key(|(i, _)| *i);
    fed.sort_by_key(|(i, _)| *i);
    Ok(Some(Rendered {
        rate: doc.sample_rate,
        signals: measured.into_iter().map(|(_, m)| m).collect(),
        feeds: fed
            .into_iter()
            .filter_map(|(i, s)| Some((plan.probes.get(i)?.clone(), s)))
            .collect(),
    }))
}

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The root note of a bass is outside the range.
    BassRoot,
    /// A bass or a kick has a weak fundamental.
    WeakFundamental,
    /// The low mids are built up.
    LowMid,
    /// A non-linear device is fed far from the reference level.
    Level,
    /// The master's highest peaks and the tracks that drive them.
    Peaks,
    /// The low end loses level in mono.
    Mono,
}

/// What one click fixes (one undo step in the app).
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// A Gain Tool before `device` on the chain of `strip` that brings its feed to the reference
    /// level.
    Trim {
        /// The strip whose chain the device is on.
        strip: DeviceKey,
        /// The device that is fed too loud or too soft.
        device: DeviceKey,
        /// The trim, dB.
        db: f32,
    },
    /// A static cut of `atomik_dsp::mixcheck::LOW_MID_CUT_DB` on a Dynamic EQ band of `channel`, centred at `hz`.
    CutLowMids {
        /// The channel.
        channel: DeviceKey,
        /// The centre of the cut, Hz.
        hz: f32,
    },
    /// A Stereo Tool at the end of the chain of `channel` with Mono on.
    MonoLow {
        /// The channel.
        channel: DeviceKey,
    },
}

/// The tool that works on a finding. Only [`Tool::MaskingMap`] exists in the application today;
/// the others are the places where tools that are built separately are wired in: until then the
/// finding gives its number and its reason and offers no button ([`Tool::available`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// The masking map (Mixer > Masking Map).
    MaskingMap,
    /// A clipper or limiter on the master or the track that drives the peaks.
    Clipper,
    /// A check of the kick and the bass against each other.
    KickBassCheck,
}

impl Tool {
    /// Whether the application can open the tool now.
    pub fn available(self) -> bool {
        matches!(self, Self::MaskingMap)
    }

    /// The label of the button that opens the tool.
    pub fn label(self) -> &'static str {
        match self {
            Self::MaskingMap => "MASKING MAP",
            Self::Clipper => "CLIPPER",
            Self::KickBassCheck => "KICK AND BASS",
        }
    }
}

/// One finding of the mix check.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// What it is about.
    pub kind: Kind,
    /// The track (or tracks, or the strip and device) it concerns.
    pub track: String,
    /// The channel the fix or the tool works on, when there is one.
    pub channel: Option<DeviceKey>,
    /// The measured number, as a sentence.
    pub measured: String,
    /// The number itself: a frequency in Hz, a level or a gap in dB, a share in percent.
    pub value: f32,
    /// Why it matters, in a line.
    pub why: &'static str,
    /// Seconds from the start of the analysed range where it happens, for a peak.
    pub at: Option<f64>,
    /// The one-click fix.
    pub fix: Option<Fix>,
    /// The tool that works on it.
    pub tool: Option<Tool>,
}

const WHY_BASS_ROOT: &str = "Below 40 Hz small speakers give nothing and above 60 Hz the root crowds the kick: keep the bass root between about 40 and 60 Hz";
const WHY_FUNDAMENTAL: &str = "The ear reads a bass's pitch and weight from the fundamental: when the harmonics carry the level it sounds thin and drops out on small speakers";
const WHY_LOW_MID: &str = "The body of many tracks piles up between 150 and 500 Hz and the mix turns muddy; reverb returns add to it";
const WHY_LEVEL: &str = "Compressors, distortion and amps react to the level they are fed: around -18 dBFS RMS they work in the range they were voiced for";
const WHY_PEAKS: &str = "The loudest peaks of the master are what a limiter or clipper has to catch: the tracks that drive them are where to take level off";
const WHY_MONO: &str = "Below 120 Hz a mono playback (a club bass bin, a phone) adds the channels: out-of-phase low end cancels there";

fn words(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// The device a channel plays.
fn instrument_of(doc: &RackDoc, source: &StemSource) -> Option<DeviceKey> {
    doc.console
        .channels
        .iter()
        .find(|c| c.channel == source.key)
        .and_then(|c| c.source.clone())
}

/// What a channel is to the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Bass,
    Kick,
    Other,
}

fn role_of(doc: &RackDoc, source: &StemSource) -> Role {
    let named = |f: &dyn Fn(&str) -> bool| words(&source.name).iter().any(|w| f(w));
    if named(&|w| w.starts_with("kick") || w == "bd") {
        return Role::Kick;
    }
    let instrument =
        instrument_of(doc, source).and_then(|k| doc.device(&k).map(|d| d.type_id.clone()));
    if named(&|w| w.starts_with("bass") || w == "sub" || w == "808")
        || instrument.as_deref() == Some("atomik.mono-bass")
    {
        return Role::Bass;
    }
    Role::Other
}

/// The `(length, key)` of the notes a channel plays.
fn notes_of(doc: &RackDoc, source: &StemSource) -> Vec<(i64, u8)> {
    let Some(device) = instrument_of(doc, source) else {
        return Vec::new();
    };
    doc.song
        .track(&device)
        .map(flatten_notes)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, len, key, _)| (len, key))
        .collect()
}

fn ordinal(n: u32) -> String {
    match n {
        2 => "2nd".to_owned(),
        3 => "3rd".to_owned(),
        n => format!("{n}th"),
    }
}

/// `1:05.3` for 65.3 seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let minutes = (s / 60.0).floor();
    format!("{}:{:04.1}", minutes as u64, s - minutes * 60.0)
}

fn joined(names: &[String]) -> String {
    names.join(", ")
}

/// The findings of a check of `doc` (see the module docs), in the order of [`Kind`].
pub fn findings(doc: &RackDoc, rendered: &Rendered) -> Vec<Finding> {
    let mut out = Vec::new();
    let signals = &rendered.signals;
    let strips = signals
        .iter()
        .filter(|m| matches!(m.source.kind, StemKind::Channel | StemKind::Return));
    // Bass and kick: the root, then the fundamental.
    let mut weak = Vec::new();
    for m in signals
        .iter()
        .filter(|m| m.source.kind == StemKind::Channel)
    {
        let role = role_of(doc, &m.source);
        if role == Role::Other {
            continue;
        }
        let mean = m.map.mean(&[]);
        let channel = Some(m.source.key.clone());
        let mut root = None;
        if role == Role::Bass
            && let Some(key) = bass_root(&notes_of(doc, &m.source))
        {
            let hz = key_hz(key);
            root = Some(hz);
            if !root_in_range(hz) {
                out.push(Finding {
                    kind: Kind::BassRoot,
                    track: m.source.name.clone(),
                    channel: channel.clone(),
                    measured: format!(
                        "root {} at {hz:.1} Hz, outside {:.0}-{:.0} Hz",
                        key_name(key),
                        BASS_ROOT_HZ.0,
                        BASS_ROOT_HZ.1
                    ),
                    value: hz,
                    why: WHY_BASS_ROOT,
                    at: None,
                    fix: None,
                    tool: Some(Tool::KickBassCheck),
                });
            }
        }
        let f0 = match role {
            Role::Kick => loudest_in(&mean, KICK_HZ),
            _ => root.or_else(|| loudest_in(&mean, BASS_SEARCH_HZ)),
        };
        if let Some(f) = f0.and_then(|hz| fundamental_of(&mean, hz))
            && f.weak()
        {
            weak.push(Finding {
                kind: Kind::WeakFundamental,
                track: m.source.name.clone(),
                channel,
                measured: format!(
                    "fundamental {:.0} Hz is {:.1} dB under its {} harmonic (limit {:.0} dB)",
                    f.hz,
                    f.gap_db(),
                    ordinal(f.harmonic),
                    WEAK_FUNDAMENTAL_DB
                ),
                value: f.gap_db(),
                why: WHY_FUNDAMENTAL,
                at: None,
                fix: None,
                tool: Some(Tool::KickBassCheck),
            });
        }
    }
    out.extend(weak);
    let master = signals.iter().find(|m| m.source.kind == StemKind::Master);
    // The low mids of the master, by track.
    if let Some(master) = master {
        let (low, all) = low_mid_power(&master.map.mean(&[]));
        let share = if all > 0.0 { (low / all) as f32 } else { 0.0 };
        if share > LOW_MID_SHARE {
            let mut parts: Vec<(&Measured, f64)> = strips
                .clone()
                .map(|m| (m, low_mid_power(&m.map.mean(&[])).0))
                .filter(|(_, p)| *p > 0.0)
                .collect();
            parts.sort_by(|a, b| b.1.total_cmp(&a.1));
            let total: f64 = parts.iter().map(|(_, p)| p).sum();
            let names: Vec<String> = parts
                .iter()
                .take(LOW_MID_TRACKS)
                .map(|(m, p)| format!("{} {:.0}%", m.source.name, 100.0 * p / total.max(1e-30)))
                .collect();
            let fix = parts
                .iter()
                .find(|(m, _)| m.source.kind == StemKind::Channel)
                .map(|(m, _)| {
                    let mean = m.map.mean(&[]);
                    let band = low_mid_bands()
                        .max_by(|a, b| mean[*a].total_cmp(&mean[*b]))
                        .unwrap_or(0);
                    Fix::CutLowMids {
                        channel: m.source.key.clone(),
                        hz: band_centre(band.min(BANDS - 1)),
                    }
                });
            out.push(Finding {
                kind: Kind::LowMid,
                track: joined(&names),
                channel: match &fix {
                    Some(Fix::CutLowMids { channel, .. }) => Some(channel.clone()),
                    _ => None,
                },
                measured: format!(
                    "{:.0}% of the master's power is in {:.0}-{:.0} Hz (limit {:.0}%)",
                    share * 100.0,
                    LOW_MID_HZ.0,
                    LOW_MID_HZ.1,
                    LOW_MID_SHARE * 100.0
                ),
                value: share * 100.0,
                why: WHY_LOW_MID,
                at: None,
                fix,
                tool: Some(Tool::MaskingMap),
            });
        }
    }
    // The feed of the non-linear devices.
    for (probe, stats) in &rendered.feeds {
        if !off_reference(stats.rms_db) {
            continue;
        }
        let db = trim_for(stats.rms_db);
        if db == 0.0 {
            continue;
        }
        out.push(Finding {
            kind: Kind::Level,
            track: format!("{} / {}", probe.strip_name, probe.device_name),
            channel: doc
                .console
                .channel_index(&probe.strip)
                .map(|_| probe.strip.clone()),
            measured: format!(
                "{} is fed {:.1} dBFS RMS (reference {:.0}, allowed +/-{:.0} dB)",
                probe.device_name, stats.rms_db, REFERENCE_RMS_DB, OFF_REFERENCE_DB
            ),
            value: stats.rms_db,
            why: WHY_LEVEL,
            at: None,
            fix: Some(Fix::Trim {
                strip: probe.strip.clone(),
                device: probe.device.clone(),
                db,
            }),
            tool: None,
        });
    }
    if let Some(master) = master {
        // The peaks of the master and who drives them.
        if master.stats.peak_db() > PEAK_REPORT_DB {
            for window in peak_spots(&master.stats.peaks, PEAK_SPOTS) {
                let at = |m: &Measured| {
                    let from = window.saturating_sub(1);
                    m.stats
                        .peaks
                        .get(from..=window + 1)
                        .or_else(|| m.stats.peaks.get(from..))
                        .map_or(0.0, |p| p.iter().fold(0.0f32, |a, b| a.max(*b)))
                };
                let mut who: Vec<(&Measured, f32)> = strips
                    .clone()
                    .map(|m| (m, at(m)))
                    .filter(|(_, p)| *p > 0.0)
                    .collect();
                who.sort_by(|a, b| b.1.total_cmp(&a.1));
                let top = who.first().map_or(0.0, |(_, p)| amp_db(*p));
                who.retain(|(_, p)| amp_db(*p) >= top - PEAK_SPREAD_DB);
                who.truncate(PEAK_TRACKS);
                let seconds = (window * PEAK_WINDOW) as f64 / f64::from(rendered.rate.max(1));
                let peak = amp_db(master.stats.peaks.get(window).copied().unwrap_or(0.0));
                let names: Vec<String> = who.iter().map(|(m, _)| m.source.name.clone()).collect();
                let numbers: Vec<String> = who
                    .iter()
                    .map(|(m, p)| format!("{} {:.1}", m.source.name, amp_db(*p)))
                    .collect();
                out.push(Finding {
                    kind: Kind::Peaks,
                    track: joined(&names),
                    channel: who
                        .iter()
                        .find(|(m, _)| m.source.kind == StemKind::Channel)
                        .map(|(m, _)| m.source.key.clone()),
                    measured: format!(
                        "master peaks at {peak:.1} dBFS at {}; at that point {} dBFS",
                        clock(seconds),
                        joined(&numbers)
                    ),
                    value: peak,
                    why: WHY_PEAKS,
                    at: Some(seconds),
                    fix: None,
                    tool: Some(Tool::Clipper),
                });
            }
        }
        // Mono compatibility of the low end.
        let lows = master.stats.lows;
        if lows.lost_db() >= MONO_LOSS_DB && lows.stereo_db() >= MONO_MIN_DB {
            let mut lost: Vec<(&Measured, f64)> = signals
                .iter()
                .filter(|m| m.source.kind == StemKind::Channel)
                .filter(|m| {
                    m.stats.lows.lost_db() >= MONO_TRACK_LOSS_DB
                        && m.stats.lows.stereo_db() >= MONO_MIN_DB
                })
                .map(|m| (m, m.stats.lows.lost_power()))
                .collect();
            lost.sort_by(|a, b| b.1.total_cmp(&a.1));
            lost.truncate(2);
            let names: Vec<String> = lost
                .iter()
                .map(|(m, _)| format!("{} {:.1} dB", m.source.name, m.stats.lows.lost_db()))
                .collect();
            let top = lost.first().map(|(m, _)| m.source.key.clone());
            out.push(Finding {
                kind: Kind::Mono,
                track: if names.is_empty() {
                    "Master".to_owned()
                } else {
                    joined(&names)
                },
                channel: top.clone(),
                measured: format!(
                    "the master's low end (under {:.0} Hz) loses {:.1} dB in mono (limit {:.0} dB)",
                    atomik_dsp::mixcheck::LOW_HZ,
                    lows.lost_db(),
                    MONO_LOSS_DB
                ),
                value: lows.lost_db(),
                why: WHY_MONO,
                at: None,
                fix: top.map(|channel| Fix::MonoLow { channel }),
                tool: None,
            });
        }
    }
    out
}
