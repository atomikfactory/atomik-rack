//! Listening to a played instrument: a streaming note tracker for practice grading.
//!
//! A [`Listener`] takes the mono input of a guitar (or any plucked instrument) in whatever block
//! sizes the host delivers and returns [`PlayedNote`]s: one per onset, with the frame the note
//! started at, its pitch when the sound is monophonic enough to have one, and a 12-bin chroma of
//! the sound that follows (what a chord match reads for strums). It runs on the UI side from a
//! lock-free record tap, never on the audio thread, and allocates (it is not real-time code).
//!
//! **Onsets.** Each hop (256 frames at 48 kHz, scaled with the rate) gets an onset strength, the
//! larger of two level-independent measures: the *energy flux* of [`crate::onset`] made causal
//! (RMS of a 768-frame window, `f = max(0, (r_k - r_(k-1)) / (r_k + r_(k-1) + 5e-4))`) and 1.5
//! times the *spectral flux* (the share of the log spectrum up to 6 kHz that is new since the
//! previous hop, spectra scaled by the window's RMS), which sees a new pitch played over a ringing
//! one. A hop is an onset when its strength is a local peak (the next hop is known one hop
//! later), above `0.12 + 2 * mean(strength over the last 12 hops)`, its level above a gate and not
//! below the loudest of the six hops before (1.12 times that when the energy flux made the
//! strength: the beating of a ringing low note moves the RMS a little), and at least
//! [`MIN_GAP_MS`] after the previous onset. The position is then pulled back to the attack: the
//! first 1 ms block, in the last three hops, whose energy jump is at least half of the largest
//! jump.
//!
//! **Pitch.** [`SETTLE_MS`] after the onset (the attack is noisy) a window of up to 4096 frames
//! (at 48 kHz) goes to [`tuner::detect`]; a window is cut short by the next onset. A chord, noise
//! or a window under 1024 frames has no pitch (`key` is `None`).
//!
//! **Chroma.** A Hann-windowed FFT of up to 8192 frames from the same point; every bin between
//! 70 Hz and 2 kHz adds its magnitude to the pitch class of its nearest semitone. The 12 values
//! are scaled so the largest is 1 (all zero for silence). Low notes are blurred by the bin width
//! (5.9 Hz at 48 kHz); their harmonics carry the class.
//!
//! The result is deterministic for the same samples and rate whatever the block sizes are.

use crate::spectral::Fft;
use crate::tuner;

/// Onsets closer than this to the previous one are dropped, milliseconds.
pub const MIN_GAP_MS: f32 = 60.0;
/// Time after an onset before its pitch and chroma are read, milliseconds.
pub const SETTLE_MS: f32 = 12.0;
/// Frames the pitch window holds at 48 kHz.
const PITCH_FRAMES: usize = 4096;
/// Frames the chroma window holds at 48 kHz.
const CHROMA_FRAMES: usize = 8192;
/// A window shorter than this (frames at 48 kHz) is not analysed.
const MIN_WINDOW_FRAMES: usize = 1024;
/// RMS under which a hop cannot hold an onset (about -50 dBFS).
const ONSET_GATE: f32 = 0.003;
/// Hops before a candidate whose loudest RMS the rise is measured against, and the factor.
const RISE_HOPS: usize = 6;
const RISE_FACTOR: f32 = 1.12;
/// The weight of the spectral flux against the energy flux.
const SPECTRAL_GAIN: f32 = 1.5;
/// The spectral flux looks at the spectrum up to this frequency, Hz.
const SPECTRAL_HZ: f32 = 6000.0;
/// Hops the adaptive threshold averages.
const MEAN_HOPS: usize = 12;
/// The lowest and highest frequency the chroma counts, Hz.
const CHROMA_LO_HZ: f32 = 70.0;
const CHROMA_HI_HZ: f32 = 2000.0;

/// One played note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayedNote {
    /// The input frame the note started at (the same timeline as the frames fed in).
    pub frame: u64,
    /// The MIDI key of its pitch; `None` for a chord, noise or a note too short to read.
    pub key: Option<i32>,
    /// How far the pitch is off its key, cents (0 without a pitch).
    pub cents: f32,
    /// How clear the pitch is, 0 to 1 (0 without a pitch).
    pub clarity: f32,
    /// The RMS of the window the note was read from.
    pub level: f32,
    /// The chroma, pitch class 0 = C, the largest value 1 (all zero for silence).
    pub chroma: [f32; 12],
}

#[derive(Debug, Clone, Copy)]
struct Hop {
    /// Frame the hop's window ends at (exclusive).
    end: u64,
    rms: f32,
    /// The onset strength: the larger of the energy flux and the scaled spectral flux.
    flux: f32,
    /// The energy flux alone is what made `flux` (so the level must have risen).
    by_energy: bool,
}

/// The streaming note tracker (module docs).
#[derive(Debug)]
pub struct Listener {
    rate: u32,
    hop: usize,
    win: usize,
    pitch_len: usize,
    chroma_len: usize,
    settle: usize,
    min_gap: usize,
    /// The newest input; `buf[0]` is frame `base`.
    buf: Vec<f32>,
    base: u64,
    /// The end of the last analysed window; `None` before the first window is full.
    analysed: Option<u64>,
    hops: Vec<Hop>,
    last_onset: Option<u64>,
    /// Onsets whose pitch and chroma are not read yet.
    pending: Vec<u64>,
    ffts: Vec<(usize, Fft)>,
    /// The log spectrum of the previous hop (for the spectral flux).
    prev_spec: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

fn frames_at(rate: u32, at_48k: usize) -> usize {
    ((at_48k as f64 * f64::from(rate) / 48_000.0).round() as usize).max(1)
}

impl Listener {
    /// A tracker for input at `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.clamp(8_000, 384_000);
        let ms = |v: f32| (f64::from(v) * f64::from(rate) / 1000.0).round() as usize;
        Self {
            rate,
            hop: frames_at(rate, 256),
            win: frames_at(rate, 768),
            pitch_len: frames_at(rate, PITCH_FRAMES),
            chroma_len: frames_at(rate, CHROMA_FRAMES),
            settle: ms(SETTLE_MS),
            min_gap: ms(MIN_GAP_MS),
            buf: Vec::new(),
            base: 0,
            analysed: None,
            hops: Vec::new(),
            last_onset: None,
            pending: Vec::new(),
            ffts: Vec::new(),
            prev_spec: Vec::new(),
            re: Vec::new(),
            im: Vec::new(),
        }
    }

    /// Forgets everything heard (a new take, or a break in the input).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.base = 0;
        self.analysed = None;
        self.hops.clear();
        self.last_onset = None;
        self.pending.clear();
        self.prev_spec.clear();
    }

    /// The frame after the last one fed in (0 before any).
    pub fn next_frame(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    /// Frames between an onset and the moment its note is returned: the notes of a signal
    /// are all out `latency_frames()` after its last frame.
    pub fn latency_frames(&self) -> usize {
        self.settle + self.chroma_len + self.win
    }

    /// Feeds `mono`, the input frames from `first_frame` on, and returns the notes that became
    /// final. A `first_frame` that does not continue the previous block resets the tracker first.
    pub fn feed(&mut self, first_frame: u64, mono: &[f32]) -> Vec<PlayedNote> {
        if !self.buf.is_empty() && first_frame != self.next_frame() {
            self.reset();
        }
        if self.buf.is_empty() {
            self.base = first_frame;
        }
        self.buf
            .extend(mono.iter().map(|s| if s.is_finite() { *s } else { 0.0 }));
        self.detect_onsets();
        let out = self.finish_notes();
        self.trim();
        out
    }

    fn rms_of(&self, from: u64, to: u64) -> f32 {
        let (a, b) = (self.index(from), self.index(to));
        if b <= a {
            return 0.0;
        }
        let sum: f64 = self.buf[a..b].iter().map(|s| f64::from(*s).powi(2)).sum();
        (sum / (b - a) as f64).sqrt() as f32
    }

    fn index(&self, frame: u64) -> usize {
        usize::try_from(frame.saturating_sub(self.base))
            .unwrap_or(usize::MAX)
            .min(self.buf.len())
    }

    fn detect_onsets(&mut self) {
        let end = self.next_frame();
        let mut at = match self.analysed {
            Some(a) => a,
            None if end >= self.base + self.win as u64 => {
                self.base + self.win as u64 - self.hop as u64
            }
            None => return,
        };
        while at + self.hop as u64 <= end {
            at += self.hop as u64;
            let rms = self.rms_of(at - self.win as u64, at);
            let energy = match self.hops.last() {
                Some(p) => ((rms - p.rms) / (rms + p.rms + 5e-4)).max(0.0),
                None => 0.0,
            };
            let spectral = SPECTRAL_GAIN * self.spectral_flux(at, rms);
            let (flux, by_energy) = if spectral > energy {
                (spectral, false)
            } else {
                (energy, true)
            };
            self.hops.push(Hop {
                end: at,
                rms,
                flux,
                by_energy,
            });
            self.judge_previous();
            if self.hops.len() > MEAN_HOPS + RISE_HOPS {
                self.hops.remove(0);
            }
        }
        self.analysed = Some(at);
    }

    /// The spectral flux of the window ending at `at`: how much of its log spectrum (shape, not
    /// level: the window's RMS divides the magnitudes) is new since the previous hop, 0 to 1.
    fn spectral_flux(&mut self, at: u64, rms: f32) -> f32 {
        let n = frames_at(self.rate, 1024).next_power_of_two();
        let fi = self.fft(n);
        self.re.clear();
        self.re.resize(n, 0.0);
        self.im.clear();
        self.im.resize(n, 0.0);
        let a = self.index(at.saturating_sub(n as u64));
        let b = self.index(at);
        let off = n - (b - a);
        let denom = (n.max(2) - 1) as f64;
        for (i, s) in self.buf[a..b].iter().enumerate() {
            let w = 0.5 - 0.5 * crate::det_trig::cos_cycles((off + i) as f64 / denom);
            self.re[off + i] = *s * w as f32;
        }
        self.ffts[fi].1.forward(&mut self.re, &mut self.im);
        let bins = ((f64::from(SPECTRAL_HZ) * n as f64 / f64::from(self.rate)) as usize).min(n / 2);
        let scale = 1.0 / (rms.max(1e-4) * (n as f32).sqrt());
        let spec: Vec<f32> = (1..bins)
            .map(|k| (1.0 + 2.0 * self.re[k].hypot(self.im[k]) * scale).ln())
            .collect();
        let total: f32 = spec.iter().sum();
        let flux = if self.prev_spec.len() == spec.len() && total > 0.0 {
            spec.iter()
                .zip(&self.prev_spec)
                .map(|(c, p)| (c - p).max(0.0))
                .sum::<f32>()
                / total
        } else {
            0.0
        };
        self.prev_spec = spec;
        flux
    }

    /// Decides whether the hop before the newest one is an onset (its peak is known now).
    fn judge_previous(&mut self) {
        let n = self.hops.len();
        if n < 3 {
            return;
        }
        let (before, cand, after) = (self.hops[n - 3], self.hops[n - 2], self.hops[n - 1]);
        if !(cand.flux >= before.flux && cand.flux > after.flux) || cand.rms < ONSET_GATE {
            return;
        }
        // The level must have risen well above what the last few hops held: the beating of a
        // low ringing note moves the RMS of a short window a little without being a new note.
        let recent = self.hops[n.saturating_sub(2 + RISE_HOPS)..n - 2]
            .iter()
            .fold(0.0f32, |m, h| m.max(h.rms));
        let need = if cand.by_energy { RISE_FACTOR } else { 1.0 };
        if cand.rms.max(after.rms) < need * recent {
            return;
        }
        let from = n.saturating_sub(MEAN_HOPS);
        let mean = self.hops[from..].iter().map(|h| h.flux).sum::<f32>() / (n - from) as f32;
        if cand.flux <= 0.12 + 2.0 * mean {
            return;
        }
        let frame = self.foot_of_attack(cand.end);
        if let Some(last) = self.last_onset
            && frame < last + self.min_gap as u64
        {
            return;
        }
        self.last_onset = Some(frame);
        self.pending.push(frame);
    }

    /// The start of the attack in the three hops before `end`.
    fn foot_of_attack(&self, end: u64) -> u64 {
        let block = frames_at(self.rate, 48).max(8) as u64; // 1 ms
        let start = end.saturating_sub(3 * self.hop as u64).max(self.base);
        let blocks = ((end - start) / block) as usize;
        if blocks == 0 {
            return start;
        }
        let energy: Vec<f32> = (0..blocks)
            .map(|b| {
                let a = start + b as u64 * block;
                let r = self.rms_of(a, a + block);
                r * r
            })
            .collect();
        // The largest jump of block energy is the attack; its start is the first block whose
        // jump is at least half of it (a ringing note's beating jumps less).
        let jump = |b: usize| energy[b] - if b > 0 { energy[b - 1] } else { 0.0 };
        let biggest = (1..blocks).fold(0.0f32, |m, b| m.max(jump(b)));
        let first = (1..blocks).find(|b| jump(*b) >= 0.5 * biggest).unwrap_or(0);
        start + first as u64 * block
    }

    /// Reads pitch and chroma of every onset whose window is complete.
    fn finish_notes(&mut self) -> Vec<PlayedNote> {
        let end = self.next_frame();
        let mut out = Vec::new();
        let ready = (self.settle + self.chroma_len) as u64;
        while let Some(&frame) = self.pending.first() {
            if end < frame + ready {
                break;
            }
            self.pending.remove(0);
            let next = self.pending.first().copied();
            out.push(self.read_note(frame, next));
        }
        out
    }

    fn read_note(&mut self, frame: u64, next: Option<u64>) -> PlayedNote {
        let guard = self.settle as u64 / 4;
        let start = frame + self.settle as u64;
        let cap = |len: usize| {
            let mut e = start + len as u64;
            if let Some(n) = next {
                e = e.min(n.saturating_sub(guard));
            }
            e
        };
        let min = frames_at(self.rate, MIN_WINDOW_FRAMES) as u64;
        let (pe, ce) = (cap(self.pitch_len), cap(self.chroma_len));
        let (a, pb, cb) = (self.index(start), self.index(pe), self.index(ce));
        let mut note = PlayedNote {
            frame,
            key: None,
            cents: 0.0,
            clarity: 0.0,
            level: 0.0,
            chroma: [0.0; 12],
        };
        if pe.saturating_sub(start) >= min && start >= self.base {
            let window = &self.buf[a..pb];
            note.level = tuner::rms(window);
            if let Some(p) = tuner::detect(window, self.rate as f32) {
                let n = tuner::note_of(p.hz, tuner::REF_DEFAULT_HZ);
                note.key = Some(n.key);
                note.cents = n.cents;
                note.clarity = p.clarity;
            }
        }
        if ce.saturating_sub(start) >= min && start >= self.base {
            let window: Vec<f32> = self.buf[a..cb].to_vec();
            note.chroma = self.chroma_of(&window);
        }
        note
    }

    fn fft(&mut self, n: usize) -> usize {
        match self.ffts.iter().position(|f| f.0 == n) {
            Some(i) => i,
            None => {
                self.ffts.push((n, Fft::new(n)));
                self.ffts.len() - 1
            }
        }
    }

    fn chroma_of(&mut self, window: &[f32]) -> [f32; 12] {
        let len = window.len();
        let n = len.next_power_of_two().max(2048);
        let fi = self.fft(n);
        self.re.clear();
        self.re.resize(n, 0.0);
        self.im.clear();
        self.im.resize(n, 0.0);
        let denom = (len.max(2) - 1) as f64;
        for (i, s) in window.iter().enumerate() {
            let w = 0.5 - 0.5 * crate::det_trig::cos_cycles(i as f64 / denom);
            self.re[i] = *s * w as f32;
        }
        self.ffts[fi].1.forward(&mut self.re, &mut self.im);
        let bin_hz = f64::from(self.rate) / n as f64;
        let lo = (f64::from(CHROMA_LO_HZ) / bin_hz).ceil() as usize;
        let hi = ((f64::from(CHROMA_HI_HZ) / bin_hz) as usize).min(n / 2 - 1);
        let mut chroma = [0.0f32; 12];
        for k in lo..=hi {
            let mag = self.re[k].hypot(self.im[k]);
            let key = 69.0 + 12.0 * (k as f64 * bin_hz / 440.0).log2();
            let pc = (key.round() as i64).rem_euclid(12) as usize;
            chroma[pc] += mag;
        }
        let peak = chroma.iter().fold(0.0f32, |m, v| m.max(*v));
        // Silence (the window holds no more than rounding noise) has no chroma.
        if peak <= 1e-6 {
            return [0.0; 12];
        }
        for v in &mut chroma {
            *v /= peak;
        }
        chroma
    }

    /// Drops the input nothing needs any more.
    fn trim(&mut self) {
        let end = self.next_frame();
        let keep_from = (self.win * 2 + self.hop) as u64;
        let mut from = end.saturating_sub(keep_from);
        if let Some(&p) = self.pending.first() {
            from = from.min(p.saturating_sub(self.win as u64));
        }
        // A cap: never keep more than two seconds.
        from = from.max(end.saturating_sub(2 * u64::from(self.rate)));
        let drop = self.index(from);
        if drop > 0 {
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }
}

#[cfg(test)]
mod tests;
