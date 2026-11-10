//! Finishing an export for a purpose: a mono mix-down and a loudness target.
//!
//! An export preset ("Streaming", "Podcast", ...) asks for audio that arrives at a given
//! integrated loudness with its true peaks under a ceiling. The render of the export is measured
//! with the loudness meter of the delivery targets ([`crate::loudness::measure`]), then:
//!
//! * [`Loudness::Target`]: the whole render is turned up or down by the difference between the
//!   target and the measured loudness. When the gain would push the true peak past the ceiling,
//!   the house look-ahead true-peak limiter
//!   ([`atomik_devices::mastering::limiter::Limiter`], the one of the Mastering device) holds the
//!   peaks under the ceiling, and because limiting costs a little loudness the gain is raised
//!   again (a few passes, from the unlimited signal each time) until the result is within
//!   [`TOLERANCE_LU`] of the target or no more can be had under the ceiling. A render that
//!   needs no limiting is only multiplied by one gain: the shape of the waveform is untouched.
//! * [`Loudness::Guide`]: nothing is changed. The master is measured and the report says how far
//!   it stands from the guide, with a warning when its peaks clip. No gain and no limiter are
//!   added: the loudness of such a master is the engineer's.
//! * [`Loudness::Off`]: nothing at all.
//!
//! [`Finish::mono`] first mixes every channel into one (their mean), so the loudness is measured
//! on the audio that is written. The measurement is a pure function of the samples and the
//! limiter is deterministic, so the same export gives the same bits on every run.
//!
//! The limiter links two channels; the channels of a multi-output document are limited in pairs
//! (a last single channel as a pair with itself).

use crate::export::ExportRender;
use crate::loudness::{LoudnessReport, measure};
use atomik_audio_file::AudioData;
use atomik_core::BATCH;
use atomik_devices::mastering::limiter::Limiter;

/// The true-peak ceiling of the loudness targets of the presets, in dBTP.
pub const CEILING_DBTP: f32 = -1.0;

/// How close to the target the result is taken to be, in LU. A limited render is raised until it
/// stands within this of the target.
pub const TOLERANCE_LU: f32 = 0.1;

/// The release time of the limiter that holds the ceiling, in seconds.
const RELEASE_SECONDS: f32 = 0.15;

/// The margin the limiter keeps under the ceiling, in dB, so the meter reads the ceiling or less.
const LIMITER_MARGIN_DB: f32 = 0.05;

/// The most passes of limit-and-raise.
const MAX_PASSES: usize = 6;

/// What to do about loudness.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Loudness {
    /// Leave the level as it is.
    #[default]
    Off,
    /// Bring the integrated loudness to `lufs` with the true peak at or under `ceiling_dbtp`.
    Target {
        /// The integrated loudness aimed for, in LUFS.
        lufs: f32,
        /// The true-peak ceiling, in dBTP.
        ceiling_dbtp: f32,
    },
    /// Measure only: report the distance to `lufs` and warn when the peaks clip.
    Guide {
        /// The integrated loudness of the guide, in LUFS.
        lufs: f32,
    },
}

/// What is done to a render before it is written.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Finish {
    /// Mix every channel into one.
    pub mono: bool,
    /// The loudness.
    pub loudness: Loudness,
}

impl Finish {
    /// Whether anything is done at all.
    pub fn is_noop(&self) -> bool {
        !self.mono && self.loudness == Loudness::Off
    }
}

/// What finishing did, for the report after the export.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NormaliseReport {
    /// The loudness of the render before the gain.
    pub before: LoudnessReport,
    /// The loudness of the audio that is written.
    pub after: LoudnessReport,
    /// The gain the render was turned up (positive) or down by, in dB; 0 for a guide.
    pub gain_db: f32,
    /// The most the limiter turned the audio down, in dB; `None` when it did not run.
    pub limited_db: Option<f32>,
    /// The target that was aimed for, in LUFS.
    pub target_lufs: Option<f32>,
    /// The ceiling that was kept, in dBTP.
    pub ceiling_dbtp: Option<f32>,
    /// The target was only a guide: nothing was changed.
    pub guide: bool,
    /// Samples reach full scale, so the file clips.
    pub clipped: bool,
}

impl NormaliseReport {
    /// What happened, in a few words for the toast and the log: `was -18.2 LUFS, -3.1 dBTP;
    /// turned up 4.2 dB, limited by up to 1.1 dB`.
    pub fn summary(&self) -> String {
        let was = if self.before.is_silent() {
            "was silent".to_owned()
        } else {
            let mut parts = Vec::new();
            if let Some(l) = self.before.integrated_lufs {
                parts.push(format!("{l:.1} LUFS"));
            }
            if let Some(p) = self.before.true_peak_dbtp {
                parts.push(format!("{p:.1} dBTP"));
            }
            format!("was {}", parts.join(", "))
        };
        if self.guide {
            let guide = self.target_lufs.unwrap_or_default();
            let distance = match (self.before.integrated_lufs, self.target_lufs) {
                (Some(l), Some(t)) if (l - t).abs() >= 0.05 => format!(
                    "{:.1} LU {} than the {guide:.0} LUFS guide",
                    (l - t).abs(),
                    if l < t { "quieter" } else { "louder" }
                ),
                (Some(_), Some(_)) => format!("on the {guide:.0} LUFS guide"),
                _ => format!("no reading against the {guide:.0} LUFS guide"),
            };
            let clip = if self.clipped {
                "; WARNING: the peaks clip"
            } else {
                ""
            };
            return format!("{distance}, nothing changed{clip}");
        }
        let did = if self.before.integrated_lufs.is_none() {
            "nothing to measure, left as it is".to_owned()
        } else if self.gain_db.abs() < 0.05 {
            "level kept".to_owned()
        } else if self.gain_db > 0.0 {
            format!("turned up {:.1} dB", self.gain_db)
        } else {
            format!("turned down {:.1} dB", -self.gain_db)
        };
        let limited = self.limited_db.map_or_else(String::new, |db| {
            format!(
                ", limited by up to {db:.1} dB to keep the peaks under {:.1} dBTP",
                self.ceiling_dbtp.unwrap_or(CEILING_DBTP)
            )
        });
        let short = match (self.after.integrated_lufs, self.target_lufs) {
            (Some(l), Some(t)) if t - l > 0.5 => format!(
                "; {:.1} LU short of {t:.0} LUFS, the peaks leave no more room",
                t - l
            ),
            _ => String::new(),
        };
        format!("{was}; {did}{limited}{short}")
    }
}

fn gain_of(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Mixes every channel of `channels` into their mean.
fn mix_down(channels: &[Vec<f32>]) -> Vec<f32> {
    let n = channels.len().max(1) as f32;
    let frames = channels.first().map_or(0, Vec::len);
    (0..frames)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / n)
        .collect()
}

/// Limits `channels` to the linear `ceiling` in pairs, in place, and returns the most gain
/// reduction in dB. The limiter's delay is taken out, so the audio stays where it was.
fn limit(channels: &mut [Vec<f32>], rate: u32, ceiling: f32) -> f32 {
    let alpha = (-1.0 / (RELEASE_SECONDS * rate as f32)).exp();
    let mut most = 0.0f32;
    let mut at = 0;
    while at < channels.len() {
        let pair = if at + 1 < channels.len() {
            [at, at + 1]
        } else {
            [at, at]
        };
        let frames = channels[at].len();
        let mut lim = Limiter::new(rate as f32);
        let latency = lim.latency() as usize;
        let total = frames + latency;
        let mut out = [Vec::with_capacity(total), Vec::with_capacity(total)];
        let mut env = [1.0f32; BATCH];
        let mut start = 0;
        while start < total {
            let mut input = [[0.0f32; BATCH]; 2];
            for (k, &c) in pair.iter().enumerate() {
                let take = frames.saturating_sub(start).min(BATCH);
                input[k][..take].copy_from_slice(&channels[c][start.min(frames)..][..take]);
            }
            let mut block = [[0.0f32; BATCH]; 2];
            let [l, r] = &mut block;
            most = most.max(lim.process([&input[0], &input[1]], ceiling, alpha, [l, r], &mut env));
            out[0].extend_from_slice(&block[0]);
            out[1].extend_from_slice(&block[1]);
            start += BATCH;
        }
        for (k, &c) in pair.iter().enumerate() {
            if k == 1 && pair[0] == pair[1] {
                break;
            }
            channels[c].copy_from_slice(&out[k][latency..latency + frames]);
        }
        at += 2;
    }
    most
}

fn audio(sample_rate: u32, channels: Vec<Vec<f32>>) -> AudioData {
    // The channels all have the length of the render and its rate is not zero.
    AudioData {
        sample_rate,
        channels,
    }
}

/// Brings `source` to the loudness `target` (see the module docs): the result and the report.
fn to_target(
    source: &AudioData,
    before: LoudnessReport,
    (lufs, ceiling_dbtp): (f32, f32),
) -> (AudioData, NormaliseReport) {
    let mut report = NormaliseReport {
        before,
        after: before,
        target_lufs: Some(lufs),
        ceiling_dbtp: Some(ceiling_dbtp),
        ..NormaliseReport::default()
    };
    let (Some(measured), Some(peak)) = (before.integrated_lufs, before.true_peak_dbtp) else {
        return (source.clone(), report);
    };
    let rate = source.sample_rate;
    let scaled = |db: f32| -> Vec<Vec<f32>> {
        let g = gain_of(db);
        source
            .channels
            .iter()
            .map(|c| c.iter().map(|x| x * g).collect())
            .collect()
    };
    let mut gain_db = lufs - measured;
    report.gain_db = gain_db;
    if peak + gain_db <= ceiling_dbtp {
        let out = audio(rate, scaled(gain_db));
        report.after = measure(&out);
        return (out, report);
    }
    // The peaks pass the ceiling: limit, and raise the gain while loudness is left on the table.
    let ceiling = gain_of(ceiling_dbtp - LIMITER_MARGIN_DB);
    let mut best: Option<(AudioData, LoudnessReport, f32, f32)> = None;
    for _ in 0..MAX_PASSES {
        let mut channels = scaled(gain_db);
        let reduction = limit(&mut channels, rate, ceiling);
        let out = audio(rate, channels);
        let after = measure(&out);
        let shortfall = after.integrated_lufs.map_or(0.0, |l| lufs - l);
        let better = best.as_ref().is_none_or(|(_, b, _, _)| {
            let miss =
                |r: &LoudnessReport| r.integrated_lufs.map_or(f32::MAX, |l| (lufs - l).abs());
            miss(&after) < miss(b)
        });
        if better {
            best = Some((out, after, gain_db, reduction));
        }
        if shortfall.abs() <= TOLERANCE_LU {
            break;
        }
        // Raise (or lower) the gain by what is missing, from the unlimited signal.
        gain_db += shortfall;
    }
    let Some((out, after, gain, reduction)) = best else {
        return (source.clone(), report);
    };
    report.after = after;
    report.gain_db = gain;
    report.limited_db = Some(reduction);
    (out, report)
}

/// Finishes `render` as `finish` says (see the module docs). The report is `None` when nothing
/// was asked for the loudness.
pub fn finish_render(render: &mut ExportRender, finish: &Finish) -> Option<NormaliseReport> {
    let rate = render.audio.sample_rate;
    if finish.mono && render.audio.channels.len() > 1 {
        let mixed = mix_down(&render.audio.channels);
        render.audio = audio(rate, vec![mixed]);
    }
    match finish.loudness {
        Loudness::Off => None,
        Loudness::Guide { lufs } => {
            let before = measure(&render.audio);
            let clipped = render
                .audio
                .channels
                .iter()
                .flatten()
                .any(|x| x.abs() >= 1.0);
            Some(NormaliseReport {
                before,
                after: before,
                target_lufs: Some(lufs),
                guide: true,
                clipped,
                ..NormaliseReport::default()
            })
        }
        Loudness::Target { lufs, ceiling_dbtp } => {
            let before = measure(&render.audio);
            let (out, report) = to_target(&render.audio, before, (lufs, ceiling_dbtp));
            render.audio = out;
            Some(report)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn render(channels: Vec<Vec<f32>>) -> ExportRender {
        let audio = AudioData::new(RATE, channels).unwrap();
        ExportRender {
            span_frames: audio.frames() as u64,
            tail_frames: 0,
            warnings: Vec::new(),
            audio,
        }
    }

    /// A stereo tone at `level` (linear peak), `seconds` long.
    fn tone(level: f32, seconds: f32) -> Vec<Vec<f32>> {
        let n = (RATE as f32 * seconds) as usize;
        let one: Vec<f32> = (0..n)
            .map(|i| level * (std::f32::consts::TAU * 220.0 * i as f32 / RATE as f32).sin())
            .collect();
        vec![one.clone(), one]
    }

    /// A tone with a sharp click every half second: a high crest factor.
    fn peaky(seconds: f32) -> Vec<Vec<f32>> {
        let mut c = tone(0.05, seconds);
        for ch in &mut c {
            for beat in (0..ch.len()).step_by(RATE as usize / 2) {
                for k in 0..24 {
                    if let Some(x) = ch.get_mut(beat + k) {
                        *x += 0.9 * (1.0 - k as f32 / 24.0);
                    }
                }
            }
        }
        c
    }

    fn target(lufs: f32) -> Finish {
        Finish {
            mono: false,
            loudness: Loudness::Target {
                lufs,
                ceiling_dbtp: CEILING_DBTP,
            },
        }
    }

    #[test]
    fn a_quiet_tone_is_brought_to_the_target_without_the_limiter() {
        let mut r = render(tone(0.05, 8.0));
        let report = finish_render(&mut r, &target(-14.0)).unwrap();
        let after = measure(&r.audio);
        assert!(
            (after.integrated_lufs.unwrap() + 14.0).abs() <= 0.5,
            "{after:?}"
        );
        assert!(after.true_peak_dbtp.unwrap() <= -1.0, "{after:?}");
        assert!(
            report.limited_db.is_none(),
            "a tone this quiet needs no limiter"
        );
        assert!(report.gain_db > 0.0);
        // The shape is a pure gain.
        let g = gain_of(report.gain_db);
        let original = tone(0.05, 8.0);
        assert!((r.audio.channels[0][1000] - original[0][1000] * g).abs() < 1e-6);
    }

    #[test]
    fn a_loud_tone_is_turned_down() {
        let mut r = render(tone(0.9, 8.0));
        let report = finish_render(&mut r, &target(-14.0)).unwrap();
        let after = measure(&r.audio);
        assert!(
            (after.integrated_lufs.unwrap() + 14.0).abs() <= 0.5,
            "{after:?}"
        );
        assert!(report.gain_db < 0.0);
        assert!(report.limited_db.is_none());
    }

    #[test]
    fn a_peaky_signal_reaches_the_target_under_the_ceiling_by_limiting() {
        let mut r = render(peaky(10.0));
        let report = finish_render(&mut r, &target(-14.0)).unwrap();
        let after = measure(&r.audio);
        assert!(report.limited_db.is_some_and(|db| db > 0.0), "{report:?}");
        assert!(
            (after.integrated_lufs.unwrap() + 14.0).abs() <= 0.5,
            "{after:?}"
        );
        assert!(after.true_peak_dbtp.unwrap() <= -1.0, "{after:?}");
        assert!(report.summary().contains("limited by up to"));
    }

    #[test]
    fn the_same_render_normalises_to_the_same_bits() {
        let mut a = render(peaky(4.0));
        let mut b = render(peaky(4.0));
        finish_render(&mut a, &target(-14.0));
        finish_render(&mut b, &target(-14.0));
        assert_eq!(a.audio, b.audio);
    }

    #[test]
    fn silence_is_left_alone() {
        let mut r = render(vec![vec![0.0; 48_000], vec![0.0; 48_000]]);
        let report = finish_render(&mut r, &target(-14.0)).unwrap();
        assert!(report.before.is_silent());
        assert!(r.audio.channels[0].iter().all(|x| *x == 0.0));
        assert!(report.summary().contains("silent"));
    }

    #[test]
    fn a_guide_changes_nothing_and_warns_when_the_peaks_clip() {
        let finish = Finish {
            mono: false,
            loudness: Loudness::Guide { lufs: -9.0 },
        };
        let original = tone(0.3, 6.0);
        let mut quiet = render(original.clone());
        let report = finish_render(&mut quiet, &finish).unwrap();
        assert_eq!(quiet.audio.channels, original);
        assert!(report.guide && !report.clipped);
        assert!(report.summary().contains("quieter than the -9 LUFS guide"));
        assert!(!report.summary().contains("WARNING"));

        let mut hot = tone(0.3, 6.0);
        hot[0][100] = 1.0;
        let mut r = render(hot);
        let report = finish_render(&mut r, &finish).unwrap();
        assert!(report.clipped);
        assert!(report.summary().contains("WARNING: the peaks clip"));
    }

    #[test]
    fn off_has_no_report_and_mono_mixes_the_channels_down() {
        let mut r = render(vec![vec![0.5; 100], vec![-0.5; 100]]);
        let none = finish_render(&mut r, &Finish::default());
        assert!(none.is_none() && r.audio.channels.len() == 2);
        let report = finish_render(
            &mut r,
            &Finish {
                mono: true,
                loudness: Loudness::Off,
            },
        );
        assert!(report.is_none());
        assert_eq!(r.audio.channels.len(), 1);
        assert!(r.audio.channels[0].iter().all(|x| x.abs() < 1e-7));
        assert!(Finish::default().is_noop());
    }

    #[test]
    fn mono_is_measured_after_the_mix_down() {
        let mut r = render(tone(0.2, 6.0));
        let finish = Finish {
            mono: true,
            ..target(-16.0)
        };
        let report = finish_render(&mut r, &finish).unwrap();
        assert_eq!(r.audio.channels.len(), 1);
        let after = measure(&r.audio);
        assert!(
            (after.integrated_lufs.unwrap() + 16.0).abs() <= 0.5,
            "{after:?}"
        );
        assert!(report.after.integrated_lufs == after.integrated_lufs);
    }
}
