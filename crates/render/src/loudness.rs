//! The loudness report of an export: integrated loudness, loudness range and true peak of the
//! audio that was written.
//!
//! The numbers are those of the loudness meter device ([`atomik_devices::mastering::loudness`]:
//! K-weighting and gating of ITU-R BS.1770-4, the true-peak detector of its Annex 2), run offline
//! over the finished audio, after any sample rate conversion and before the file's quantisation.
//!
//! * **Integrated loudness** in LUFS: the gated loudness of the whole programme (absolute gate
//!   -70 LUFS, relative gate -10 LU).
//! * **Loudness range** in LU (EBU Tech 3342): the spread of the short-term loudness (3 s windows
//!   every 100 ms) between its 10th and 95th percentile, after an absolute gate at -70 LUFS and a
//!   relative gate 20 LU below the gated mean. A programme shorter than 3 s has none.
//! * **True peak** in dBTP: the highest 4x oversampled peak of any channel; the **sample peak**
//!   in dBFS is the highest sample.
//!
//! The first two channels are measured (the stereo mix); one channel is measured as it is. A
//! value that does not exist (silence, a programme too short for a range) is `None`. The
//! measurement is a pure function of the samples, so the same export reports the same numbers.

use atomik_audio_file::AudioData;
use atomik_devices::mastering::loudness::{FLOOR_DB, LoudnessMeter, PeakDetector};

/// The short-term windows a range needs: 3 s in steps of 100 ms.
const SHORT_WINDOWS: usize = 30;

/// Zero frames fed after the audio so the peak detector's delay line is flushed.
const FLUSH_FRAMES: usize = 64;

/// What was measured. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoudnessReport {
    /// Gated integrated loudness in LUFS.
    pub integrated_lufs: Option<f32>,
    /// Loudness range in LU.
    pub range_lu: Option<f32>,
    /// True peak in dBTP.
    pub true_peak_dbtp: Option<f32>,
    /// Sample peak in dBFS.
    pub sample_peak_dbfs: Option<f32>,
}

impl LoudnessReport {
    /// True when nothing was audible: no loudness and no peak.
    pub fn is_silent(&self) -> bool {
        self.integrated_lufs.is_none() && self.true_peak_dbtp.is_none()
    }

    /// One line, for the toast and the log: `-14.2 LUFS, -0.8 dBTP, range 6.3 LU`.
    pub fn summary(&self) -> String {
        if self.is_silent() {
            return "silent".to_owned();
        }
        let mut parts = Vec::new();
        if let Some(l) = self.integrated_lufs {
            parts.push(format!("{l:.1} LUFS"));
        }
        if let Some(p) = self.true_peak_dbtp {
            parts.push(format!("{p:.1} dBTP"));
        }
        if let Some(r) = self.range_lu {
            parts.push(format!("range {r:.1} LU"));
        }
        parts.join(", ")
    }
}

fn db(linear: f32) -> Option<f32> {
    (linear > 0.0).then(|| 20.0 * linear.log10())
}

/// The mean-square power a loudness reading in LUFS stands for.
fn power(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

/// The loudness range of a list of short-term readings (see the module docs).
fn range_of(readings: &[f64]) -> Option<f32> {
    let audible: Vec<f64> = readings.iter().copied().filter(|&l| l > -70.0).collect();
    if audible.is_empty() {
        return None;
    }
    let mean = audible.iter().map(|&l| power(l)).sum::<f64>() / audible.len() as f64;
    let gate = -0.691 + 10.0 * mean.log10() - 20.0;
    let mut kept: Vec<f64> = audible.into_iter().filter(|&l| l >= gate).collect();
    if kept.is_empty() {
        return None;
    }
    kept.sort_by(f64::total_cmp);
    let at = |q: f64| {
        let x = (kept.len() - 1) as f64 * q;
        let (lo, frac) = (x.floor() as usize, x - x.floor());
        let hi = (lo + 1).min(kept.len() - 1);
        kept[lo] * (1.0 - frac) + kept[hi] * frac
    };
    Some((at(0.95) - at(0.10)) as f32)
}

/// Measures `audio` (see the module docs).
pub fn measure(audio: &AudioData) -> LoudnessReport {
    let frames = audio.frames();
    let Some(left) = audio.channels.first() else {
        return LoudnessReport::default();
    };
    let silence = vec![0.0f32; frames];
    let right = audio.channels.get(1).unwrap_or(&silence);
    let rate = f64::from(audio.sample_rate);
    let mut meter = LoudnessMeter::new(rate);
    let step = ((rate * 0.1).round() as usize).max(1);
    let mut short_term = Vec::new();
    let mut windows = 0usize;
    for start in (0..frames).step_by(step) {
        let end = (start + step).min(frames);
        meter.process(&left[start..end], &right[start..end]);
        if end - start == step {
            windows += 1;
            if windows >= SHORT_WINDOWS {
                short_term.push(f64::from(meter.short_term()));
            }
        }
    }
    let integrated = meter.integrated();
    let sample_peak = audio.channels.iter().flatten().fold(0.0f32, |m, &x| {
        if x.is_finite() { m.max(x.abs()) } else { m }
    });
    const BLOCK: usize = 4096;
    let mut true_peak = sample_peak;
    let tail = [0.0f32; FLUSH_FRAMES];
    let mut peaks = vec![0.0f32; BLOCK];
    for channel in &audio.channels {
        let mut detector = PeakDetector::new(BLOCK);
        let mut feed = |block: &[f32]| {
            detector.process(block, &mut peaks[..block.len()], true);
            for &p in &peaks[..block.len()] {
                true_peak = true_peak.max(p);
            }
        };
        for block in channel.chunks(BLOCK) {
            feed(block);
        }
        feed(&tail);
    }
    LoudnessReport {
        integrated_lufs: (integrated > FLOOR_DB).then_some(integrated),
        range_lu: range_of(&short_term),
        true_peak_dbtp: db(true_peak),
        sample_peak_dbfs: db(sample_peak),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f32, hz: f32, amp: f32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn stereo(channel: Vec<f32>, rate: u32) -> AudioData {
        AudioData::new(rate, vec![channel.clone(), channel]).unwrap()
    }

    #[test]
    fn a_full_scale_stereo_sine_reads_its_known_loudness_and_peak() {
        // BS.1770: a 997 Hz sine at -20 dBFS in both channels is -20 LUFS (the channel powers
        // add 3 dB, and the K-weighting gain at that frequency cancels the 0.691 offset).
        for rate in [44_100, 48_000] {
            let audio = stereo(tone(rate, 8.0, 997.0, 0.1), rate);
            let r = measure(&audio);
            let lufs = r.integrated_lufs.expect("audible");
            assert!((lufs - -20.0).abs() < 0.15, "{rate}: {lufs}");
            let peak = r.sample_peak_dbfs.expect("audible");
            assert!((peak - -20.0).abs() < 0.01, "{peak}");
            let tp = r.true_peak_dbtp.expect("audible");
            assert!((-20.0..-19.8).contains(&tp), "{tp}");
        }
    }

    #[test]
    fn the_true_peak_sees_between_the_samples() {
        // A sine a quarter of the sample rate, a 45 degree phase: every sample is 0.707 of the
        // peak, the waveform reaches 1.0.
        let rate = 48_000;
        let wave: Vec<f32> = (0..rate)
            .map(|i| {
                let phase = std::f64::consts::FRAC_PI_2 * (i % 4) as f64;
                (phase + std::f64::consts::FRAC_PI_4).sin() as f32
            })
            .collect();
        let r = measure(&stereo(wave, rate as u32));
        let sample = r.sample_peak_dbfs.unwrap();
        let tp = r.true_peak_dbtp.unwrap();
        assert!((sample - -3.01).abs() < 0.02, "{sample}");
        assert!(tp > -0.5 && tp < 0.5, "{tp}");
    }

    #[test]
    fn silence_has_nothing_to_report() {
        let r = measure(&stereo(vec![0.0; 48_000 * 4], 48_000));
        assert_eq!(r, LoudnessReport::default());
        assert!(r.is_silent());
        assert_eq!(r.summary(), "silent");
        assert!(measure(&AudioData::new(48_000, vec![Vec::new()]).unwrap()).is_silent());
    }

    #[test]
    fn a_steady_tone_has_no_range_and_a_short_one_has_none_at_all() {
        let steady = measure(&stereo(tone(48_000, 10.0, 440.0, 0.3), 48_000));
        let range = steady.range_lu.expect("ten seconds have windows");
        assert!(range < 0.1, "{range}");
        let short = measure(&stereo(tone(48_000, 2.0, 440.0, 0.3), 48_000));
        assert_eq!(short.range_lu, None);
        assert!(short.integrated_lufs.is_some());
    }

    #[test]
    fn a_loud_and_a_quiet_part_make_a_range() {
        // Loudness range of two steady parts 10 LU apart is about 10 LU.
        let mut wave = tone(48_000, 12.0, 440.0, 0.3);
        wave.extend(tone(48_000, 12.0, 440.0, 0.3 * 0.316_227_77));
        let r = measure(&stereo(wave, 48_000));
        let range = r.range_lu.expect("range");
        assert!((range - 10.0).abs() < 1.0, "{range}");
    }

    #[test]
    fn one_channel_is_measured_alone_and_the_summary_names_each_value() {
        let mono = AudioData::new(48_000, vec![tone(48_000, 5.0, 997.0, 0.1)]).unwrap();
        let r = measure(&mono);
        let lufs = r.integrated_lufs.unwrap();
        assert!((lufs - -23.0).abs() < 0.15, "{lufs}");
        let summary = measure(&stereo(tone(48_000, 8.0, 997.0, 0.1), 48_000)).summary();
        assert!(
            summary.contains("LUFS") && summary.contains("dBTP"),
            "{summary}"
        );
        assert!(summary.contains("range"), "{summary}");
    }

    #[test]
    fn the_measurement_is_repeatable() {
        let audio = stereo(tone(44_100, 7.0, 300.0, 0.5), 44_100);
        assert_eq!(measure(&audio), measure(&audio));
    }
}
