//! The Peak Surgeon in the app core: the Peaks page of the Delivery panel (**Edit > Peak
//! Surgeon...**) lists the highest peaks of the selected audio clip and trims them by gain dips a
//! few samples wide ([`atomik_model::peak_surgeon`] has the maths and the rules).
//!
//! [`AppCore::surgeon_scan`] reads the clip the way it plays (the span of its asset, with the
//! clip's gain, fades and envelope, forwards or backwards) in chunks from the engine's assets,
//! exactly as Normalise does, and keeps the highest peaks. [`AppCore::surgeon_trim`] dips one
//! peak by one step and [`AppCore::surgeon_apply_all`] dips every peak above the threshold; both
//! write the dips into the clip's gain envelope (one `SetAudioClip`, one undo step, the file is
//! never touched), read the clip again and say in the status line how far the highest peak went
//! down. A warped or looped clip, a missing file and an asset at another rate are refused.

use super::*;
use atomik_engine::SongAsset;
use atomik_model::peak_surgeon::{
    DEFAULT_STEP_DB, DEFAULT_WIDTH, Dip, MAX_STEP_DB, MIN_STEP_DB, Plan, Scan, Scanner, Settings,
    dips_that_fit, plan, trimmed,
};
use atomik_model::{AudioClipContent, ClipContent};
use atomik_ui::{PeakRow, PeaksData};

use super::audio_edit_ops::for_each_chunk;

/// The step the cycle button goes through.
const STEPS: [f32; 3] = [MIN_STEP_DB, 0.3, MAX_STEP_DB];
/// The half-widths the cycle button goes through.
const WIDTHS: [u32; 4] = [4, DEFAULT_WIDTH, 16, 32];
/// Rows of the list.
const LIST_ROWS: usize = atomik_ui::delivery_panel::ROWS;
/// Frames read from an asset at a time when reading backwards.
const BLOCK: u64 = 1 << 16;

/// The peak surgeon state of an open project.
pub(super) struct SurgeonState {
    clip: Option<ClipId>,
    name: String,
    scan: Option<Scan>,
    /// The clip's length in frames.
    span: u64,
    message: String,
    chosen: usize,
    threshold_db: f32,
    step_db: f32,
    width: u32,
}

impl Default for SurgeonState {
    fn default() -> Self {
        Self {
            clip: None,
            name: String::new(),
            scan: None,
            span: 0,
            message: "Select an audio clip to trim its peaks".to_owned(),
            chosen: 0,
            threshold_db: -1.0,
            step_db: DEFAULT_STEP_DB,
            width: DEFAULT_WIDTH,
        }
    }
}

impl SurgeonState {
    /// Forgets the clip and its peaks (the project changed).
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Reads the levels `content` plays from `asset` (a clip `clip_frames` long, its source starting at
/// `content.source_offset`) into a [`Scanner`] of dips `width` wide, in clip time, and returns the
/// peaks. `None` when the clip has nothing to read.
pub(super) fn scan_clip(
    asset: &SongAsset,
    content: &AudioClipContent,
    clip_frames: u64,
    width: u32,
) -> Option<Scan> {
    let from = content.source_offset;
    let len = clip_frames.min(asset.frames().saturating_sub(from));
    if len == 0 {
        return None;
    }
    let gain = content.played_gain(clip_frames);
    let mut scanner = Scanner::new(width);
    let highest =
        |planes: &[&[f32]], i: usize| planes.iter().map(|p| p[i].abs()).fold(0.0, f32::max);
    if content.reverse {
        // Backwards: blocks from the end of the span, each turned round, so the scanner sees the
        // clip's own time.
        let mut hi = from + len;
        while hi > from {
            let lo = hi.saturating_sub(BLOCK).max(from);
            let mut raw: Vec<f32> = Vec::with_capacity((hi - lo) as usize);
            for_each_chunk(asset, lo, hi, |planes| {
                let n = planes.first().map_or(0, |p| p.len());
                raw.extend((0..n).map(|i| highest(planes, i)));
            });
            raw.resize((hi - lo) as usize, 0.0);
            let levels: Vec<f32> = (lo..hi)
                .rev()
                .map(|s| raw[(s - lo) as usize] * gain.at(from + len - 1 - s))
                .collect();
            scanner.push(&levels);
            hi = lo;
        }
    } else {
        let mut k = 0u64;
        for_each_chunk(asset, from, from + len, |planes| {
            let n = planes.first().map_or(0, |p| p.len());
            let levels: Vec<f32> = (0..n)
                .map(|i| highest(planes, i) * gain.at(k + i as u64))
                .collect();
            scanner.push(&levels);
            k += n as u64;
        });
    }
    Some(scanner.finish())
}

/// A frame as `m:ss.mmm` at `rate`.
fn clock(frame: u64, rate: u32) -> String {
    let ms = frame * 1000 / u64::from(rate.max(1));
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

impl<S: EngineSink> AppCore<S> {
    /// Whether a clip has been scanned.
    pub(super) fn surgeon_has_scan(&self) -> bool {
        self.surgeon.scan.is_some()
    }

    /// Reads the selected audio clip (see the module docs). A refusal is the page's message.
    pub fn surgeon_scan(&mut self) {
        let previous = self.surgeon.clip;
        self.surgeon.scan = None;
        self.surgeon.clip = None;
        self.surgeon.chosen = 0;
        let result = self.scan_selected();
        match result {
            Ok((id, name, span, scan)) => {
                if previous != Some(id) || self.surgeon.name != name {
                    // A new clip starts with a threshold a dB under its highest peak.
                    if let Some(top) = scan.top_db() {
                        self.surgeon.threshold_db = ((top - 1.0) * 10.0).floor() / 10.0;
                    }
                }
                self.surgeon.clip = Some(id);
                self.surgeon.name = name;
                self.surgeon.span = span;
                self.surgeon.message = if scan.peaks.is_empty() {
                    "The clip is silent: there is no peak to trim".to_owned()
                } else {
                    String::new()
                };
                self.surgeon.scan = Some(scan);
            }
            Err(why) => self.surgeon.message = why,
        }
    }

    #[allow(clippy::type_complexity)]
    fn scan_selected(&self) -> Result<(ClipId, String, u64, Scan), String> {
        let selected = self.selected_audio();
        let s = selected
            .first()
            .ok_or_else(|| "Select an audio clip to trim its peaks".to_owned())?;
        let doc = self.doc();
        if s.content.warp.enabled {
            return Err(
                "Peak Surgeon needs a clip at its own speed: render the warped clip first"
                    .to_owned(),
            );
        }
        let clip = doc
            .song
            .clip(s.id)
            .map(|(_, _, c)| c)
            .ok_or_else(|| "The clip is gone".to_owned())?;
        if clip.loop_length.is_some() {
            return Err("Peak Surgeon needs a clip that is not looped".to_owned());
        }
        let ClipContent::Audio(_) = &clip.content else {
            return Err("Select an audio clip to trim its peaks".to_owned());
        };
        let rate_ok = doc
            .assets
            .get(&s.content.asset)
            .is_none_or(|m| m.sample_rate == doc.sample_rate);
        if !rate_ok {
            return Err("The audio file has another sample rate than the project".to_owned());
        }
        let asset = self
            .audio
            .assets
            .get(&s.content.asset)
            .ok_or_else(|| "The audio file is missing".to_owned())?;
        let name = clip.name.clone().unwrap_or_else(|| {
            doc.assets
                .get(&s.content.asset)
                .map_or_else(|| "Audio clip".to_owned(), |m| m.original_name.clone())
        });
        let scan = scan_clip(asset, &s.content, s.frames, self.surgeon.width)
            .ok_or_else(|| "The clip plays no audio".to_owned())?;
        Ok((s.id, name, s.frames, scan))
    }

    /// The settings the planner takes now.
    fn surgeon_settings(&self) -> Settings {
        let existing = self
            .surgeon
            .clip
            .and_then(|id| self.doc().song.clip(id))
            .and_then(|(_, _, c)| match &c.content {
                ClipContent::Audio(a) => Some(a.envelope.len()),
                _ => None,
            })
            .unwrap_or(0);
        Settings {
            threshold_db: self.surgeon.threshold_db,
            step_db: self.surgeon.step_db,
            width: self.surgeon.width,
            max_dips: dips_that_fit(existing),
        }
    }

    fn surgeon_plan(&self) -> Option<Plan> {
        let scan = self.surgeon.scan.as_ref()?;
        Some(plan(&scan.peaks, &self.surgeon_settings()))
    }

    /// Chooses peak `n` of the list (clamped).
    pub fn surgeon_choose(&mut self, n: usize) {
        let count = self
            .surgeon
            .scan
            .as_ref()
            .map_or(0, |s| s.peaks.len().min(LIST_ROWS));
        self.surgeon.chosen = n.min(count.saturating_sub(1));
    }

    /// Moves the threshold by `delta` dB (kept within -60..=0).
    pub fn surgeon_threshold(&mut self, delta: f32) {
        let t = ((self.surgeon.threshold_db + delta) * 10.0).round() / 10.0;
        self.surgeon.threshold_db = t.clamp(-60.0, 0.0);
    }

    /// Takes the next dip step of 0.2, 0.3 and 0.4 dB.
    pub fn surgeon_cycle_step(&mut self) {
        let at = STEPS
            .iter()
            .position(|&s| (s - self.surgeon.step_db).abs() < 1e-4)
            .unwrap_or(0);
        self.surgeon.step_db = STEPS[(at + 1) % STEPS.len()];
    }

    /// Takes the next dip width; the clip is read again, as the width decides what a peak is.
    pub fn surgeon_cycle_width(&mut self) {
        let at = WIDTHS
            .iter()
            .position(|&w| w == self.surgeon.width)
            .unwrap_or(0);
        self.surgeon.width = WIDTHS[(at + 1) % WIDTHS.len()];
        if self.surgeon.clip.is_some() {
            let keep = self.surgeon.threshold_db;
            let chosen = self.surgeon.chosen;
            self.surgeon_scan();
            self.surgeon.threshold_db = keep;
            self.surgeon.chosen = chosen;
            self.surgeon_choose(chosen);
        }
    }

    /// What the Peaks page shows.
    pub(super) fn peaks_data(&self) -> PeaksData {
        let s = &self.surgeon;
        let mut data = PeaksData {
            message: s.message.clone(),
            threshold_db: s.threshold_db,
            step_db: s.step_db,
            width: s.width,
            ..PeaksData::default()
        };
        let Some(scan) = &s.scan else {
            return data;
        };
        let rate = self.doc().sample_rate;
        let top = scan.top_db().unwrap_or(0.0);
        data.clip = s.name.clone();
        data.total = scan.peaks.len();
        data.chosen = s.chosen.min(LIST_ROWS - 1);
        data.rows = scan
            .peaks
            .iter()
            .take(LIST_ROWS)
            .map(|p| PeakRow {
                time: clock(p.frame, rate),
                db: p.db(),
                headroom: top - p.db(),
            })
            .collect();
        let p = plan(&scan.peaks, &self.surgeon_settings());
        let above = scan
            .peaks
            .iter()
            .filter(|p| p.db() > s.threshold_db)
            .count();
        data.can_apply = !p.dips.is_empty();
        data.preview = if above == 0 {
            format!("No peak above {:.1} dBFS: nothing to trim", s.threshold_db)
        } else {
            let left = if p.left > 0 {
                format!("; {} too sharp to trim", p.left)
            } else {
                String::new()
            };
            format!(
                "Above {:.1} dBFS: {above} peak{}, {} dip{}: the highest goes from {:.2} to {:.2} \
                 dBFS, {:.2} dB of headroom{left}",
                s.threshold_db,
                if above == 1 { "" } else { "s" },
                p.dips.len(),
                if p.dips.len() == 1 { "" } else { "s" },
                p.before_db.unwrap_or(top),
                p.after_db.unwrap_or(top),
                p.gained_db()
            )
        };
        data
    }

    /// Dips the chosen peak by one step.
    pub fn surgeon_trim(&mut self) {
        let Some(peak) = self
            .surgeon
            .scan
            .as_ref()
            .and_then(|s| s.peaks.get(self.surgeon.chosen))
            .copied()
        else {
            return self.reject("Select an audio clip with a peak to trim".to_owned());
        };
        let dip = Dip {
            frame: peak.frame,
            depth_db: self.surgeon.step_db,
        };
        self.surgeon_apply(&[dip]);
    }

    /// Dips every peak above the threshold (see [`atomik_model::peak_surgeon::plan`]).
    pub fn surgeon_apply_all(&mut self) {
        let Some(plan) = self.surgeon_plan() else {
            return self.reject("Select an audio clip to trim its peaks".to_owned());
        };
        if plan.dips.is_empty() {
            return self.reject(format!(
                "No peak above {:.1} dBFS: nothing to trim",
                self.surgeon.threshold_db
            ));
        }
        self.surgeon_apply(&plan.dips);
    }

    /// Writes `dips` into the scanned clip's envelope, one undo step, then reads the clip again.
    fn surgeon_apply(&mut self, dips: &[Dip]) {
        let Some(id) = self.surgeon.clip else {
            return self.reject("Select an audio clip to trim its peaks".to_owned());
        };
        let Some((_, _, clip)) = self.doc().song.clip(id) else {
            return self.reject("The clip is gone".to_owned());
        };
        let ClipContent::Audio(content) = &clip.content else {
            return self.reject("The clip is not audio".to_owned());
        };
        let new = match trimmed(content, dips, self.surgeon.width, self.surgeon.span) {
            Ok(c) => c,
            Err(e) => return self.reject(format!("Peak Surgeon: {e}")),
        };
        let command = Command::Song(SongCommand::SetAudioClip {
            id,
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content: new,
            merge: None,
        });
        let before = self.surgeon.scan.as_ref().and_then(Scan::top_db);
        let keep = (self.surgeon.threshold_db, self.surgeon.chosen);
        if self.apply_command(command).is_err() {
            return;
        }
        self.surgeon_scan();
        (self.surgeon.threshold_db, self.surgeon.chosen) = keep;
        self.surgeon_choose(keep.1);
        let after = self.surgeon.scan.as_ref().and_then(Scan::top_db);
        if let (Some(b), Some(a)) = (before, after) {
            self.reject(format!(
                "Peak Surgeon: {} dip{}, the highest peak went from {b:.2} to {a:.2} dBFS ({:.2} dB of headroom)",
                dips.len(),
                if dips.len() == 1 { "" } else { "s" },
                (b - a).max(0.0)
            ));
        }
    }

    /// The step the Peaks page uses, for tests.
    #[cfg(test)]
    pub(super) fn surgeon_step_for_test(&self) -> f32 {
        self.surgeon.step_db
    }
}
