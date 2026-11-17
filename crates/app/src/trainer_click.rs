//! The click of the chord trainer.
//!
//! The trainer works without the song transport, so its click cannot be the transport's
//! metronome. It uses the engine's preview voice (the one the file browser and the audio setup
//! tones play through): one continuous mono stream whose samples hold the clicks, so the beats are
//! as steady as the audio clock whatever the UI frame rate. [`ClickGen`] makes the samples (pure,
//! deterministic) and [`ClickStream`] is the thread that keeps the preview ring filled until it is
//! stopped or the engine lets go of the stream.

use atomik_engine::PreviewFeed;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// The rate of the click stream (the engine resamples it).
pub const CLICK_RATE: u32 = 44_100;
/// Silence before the first beat, so the voice's fade-in does not eat it, seconds.
const LEAD_SECONDS: f64 = 0.012;
/// How long a click rings, seconds.
const CLICK_SECONDS: f64 = 0.035;
/// Frames the thread writes at a time.
const CHUNK: usize = 1024;

/// Makes the samples of a click track: a beat every `60 / bpm` seconds, the first of every
/// `accent_every` beats higher and louder.
#[derive(Debug, Clone)]
pub struct ClickGen {
    rate: f64,
    frames_per_beat: f64,
    accent_every: u64,
    lead: u64,
    /// The next frame to make.
    frame: u64,
    /// The beat sounding (its index), once the first has started.
    beat: Option<u64>,
    /// The index of the next beat.
    next: u64,
}

impl ClickGen {
    /// A generator at `rate` Hz for `bpm` with an accent every `accent_every` beats.
    pub fn new(rate: u32, bpm: u16, accent_every: u8) -> Self {
        let rate = f64::from(rate.max(8_000));
        Self {
            rate,
            frames_per_beat: rate * 60.0 / f64::from(bpm.max(1)),
            accent_every: u64::from(accent_every.max(1)),
            lead: (LEAD_SECONDS * rate) as u64,
            frame: 0,
            beat: None,
            next: 0,
        }
    }

    /// The frame beat `index` starts at.
    pub fn beat_start(&self, index: u64) -> u64 {
        self.lead + (index as f64 * self.frames_per_beat) as u64
    }

    /// Whether beat `index` carries the accent.
    pub fn is_accent(&self, index: u64) -> bool {
        index.is_multiple_of(self.accent_every)
    }

    /// The next frame.
    pub fn sample(&mut self) -> f32 {
        let f = self.frame;
        self.frame += 1;
        while f >= self.beat_start(self.next) {
            self.beat = Some(self.next);
            self.next += 1;
        }
        let Some(i) = self.beat else {
            return 0.0;
        };
        let t = (f - self.beat_start(i)) as f64 / self.rate;
        if t >= CLICK_SECONDS {
            return 0.0;
        }
        let (hz, gain) = if self.is_accent(i) {
            (1_760.0, 0.9)
        } else {
            (1_100.0, 0.55)
        };
        let attack = (t / 0.0008).min(1.0);
        let env = attack * (-t * 160.0).exp();
        (gain * env * (std::f64::consts::TAU * hz * t).sin()) as f32
    }

    /// Fills `out` with the next frames.
    pub fn fill(&mut self, out: &mut [f32]) {
        for s in out {
            *s = self.sample();
        }
    }
}

/// The thread that feeds a click track into a preview ring until stopped.
pub struct ClickStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClickStream {
    /// Starts feeding `feed` with a click at `bpm` (an accent every `accent_every` beats). `None`
    /// when the thread cannot start.
    pub fn start(mut feed: PreviewFeed, bpm: u16, accent_every: u8) -> Option<Self> {
        feed.set_source_rate(CLICK_RATE);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("atomik-trainer-click".into())
            .spawn(move || {
                let mut gen_ = ClickGen::new(CLICK_RATE, bpm, accent_every);
                let mut block = vec![0.0f32; CHUNK];
                let mut pending = 0usize;
                while !flag.load(Ordering::Relaxed) && !feed.is_abandoned() {
                    if pending == 0 {
                        gen_.fill(&mut block);
                        pending = CHUNK;
                    }
                    let planar = [block.clone()];
                    let wrote = feed.push(&planar, CHUNK - pending);
                    pending -= wrote.min(pending);
                    if wrote == 0 {
                        std::thread::sleep(Duration::from_millis(4));
                    }
                }
                feed.finish();
            })
            .ok()?;
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the stream and waits for the thread (a few milliseconds).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ClickStream {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(bpm: u16, accent_every: u8, seconds: f64) -> Vec<f32> {
        let mut g = ClickGen::new(CLICK_RATE, bpm, accent_every);
        let mut out = vec![0.0; (seconds * f64::from(CLICK_RATE)) as usize];
        g.fill(&mut out);
        out
    }

    /// The frames a click starts at: a sample that is not zero after a run of zeros.
    fn onsets(samples: &[f32]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut quiet = CLICK_RATE as usize;
        for (i, s) in samples.iter().enumerate() {
            if s.abs() > 1e-4 {
                if quiet > 400 {
                    out.push(i);
                }
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
        out
    }

    #[test]
    fn the_beats_sit_a_beat_apart_after_the_lead() {
        let g = ClickGen::new(CLICK_RATE, 120, 4);
        assert_eq!(
            g.beat_start(0),
            (LEAD_SECONDS * f64::from(CLICK_RATE)) as u64
        );
        // 120 BPM: half a second a beat.
        assert_eq!(g.beat_start(2) - g.beat_start(0), CLICK_RATE as u64);
        let s = render(120, 4, 2.2);
        let on = onsets(&s);
        // Beats at 0, 0.5, 1.0, 1.5 and 2.0 seconds (the first sample of a beat is zero).
        assert_eq!(on.len(), 5, "{on:?}");
        for (i, at) in on.iter().enumerate() {
            let want = g.beat_start(i as u64) as usize;
            assert!(at.abs_diff(want) <= 2, "{at} vs {want}");
        }
    }

    #[test]
    fn the_accent_is_louder_than_the_other_beats() {
        let s = render(120, 2, 1.6);
        let g = ClickGen::new(CLICK_RATE, 120, 2);
        let peak = |i: u64| {
            let a = g.beat_start(i) as usize;
            s[a..a + 1500].iter().fold(0.0f32, |m, x| m.max(x.abs()))
        };
        assert!(peak(0) > peak(1) * 1.4);
        assert!(peak(2) > peak(1) * 1.4);
        assert!((peak(0) - peak(2)).abs() < 0.02);
    }

    #[test]
    fn it_is_silent_between_clicks_and_never_clips() {
        let s = render(60, 4, 3.0);
        assert!(s.iter().all(|x| x.abs() <= 1.0));
        let mid =
            &s[(0.5 * f64::from(CLICK_RATE)) as usize..(0.9 * f64::from(CLICK_RATE)) as usize];
        assert!(mid.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn the_same_settings_give_the_same_samples() {
        assert_eq!(render(90, 3, 1.0), render(90, 3, 1.0));
    }

    #[test]
    fn a_block_size_does_not_change_the_track() {
        let whole = render(100, 4, 1.0);
        let mut g = ClickGen::new(CLICK_RATE, 100, 4);
        let mut parts = Vec::new();
        for n in [1usize, 7, 1024, 333, 40_000] {
            let mut b = vec![0.0; n];
            g.fill(&mut b);
            parts.extend(b);
        }
        let n = parts.len().min(whole.len());
        assert_eq!(parts[..n], whole[..n]);
    }
}
