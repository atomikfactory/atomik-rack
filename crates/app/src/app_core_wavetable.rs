//! The Wavetable Synth's table editor in the app core: the table is the device's one sample, so
//! an edited table is stored the way a dropped sample is, as an asset of the project's audio
//! behind one zone, and every edit is one `SetZones` undo step.
//!
//! * **Stored form.** [`AppCore::commit_wavetable`] bakes the editor's key frames
//!   ([`atomik_dsp::wavetable::edit::bake`]) into [`atomik_dsp::wavetable::FRAMES`] frames of
//!   2048 samples, one mono asset of 32768 samples, and gives it to the device in one step. The
//!   synth cuts that asset into exactly those sixteen frames, so what the editor drew is what
//!   plays (each frame scaled to peak at 1, as for every imported table), and the table is saved
//!   with the project's audio and reloads like a dropped sample. The asset's name records how
//!   many key frames the table was drawn with and whether they were aligned (`Table frames 4
//!   aligned`), so reopening the editor shows the key frames again and not their sixteen blends.
//!   A sample that was dropped has another name: its frames are listed as they are cut, its cycle
//!   length is detected and the cleanup tools of the editor are offered.
//! * **Reading.** [`AppCore::wavetable_data`] gives the editor what the device holds now (kept in
//!   step with undo and redo, since it is read from the document each frame). A device without a
//!   sample starts from four frames of the table oscillator 1 shows, and the first edit stores
//!   them.
//! * **Cleanup.** [`AppCore::cut_wavetable_cycles`] cuts the dropped sample again with one
//!   detected cycle per frame, drops silent and near-identical frames and stores the result.
//!
//! Nothing here touches the audio thread: the engine receives the new sample map like any other
//! (`attach_table` builds the table on the worker that makes maps).

use super::*;
use atomik_audio_file::AudioData;
use atomik_dsp::wavetable::edit::{
    self, Cycle, DUPLICATE_TOLERANCE, FRAME_LEN, Frame, MAX_KEYS, key_position,
};
use atomik_dsp::wavetable::{FRAMES, IMPORTED_TABLE, TABLE_COUNT, display_frame};
use atomik_ui::WavetableData;

/// The name of an asset stored by the editor: this, the key frame count and `aligned` or
/// `plain`.
pub const TABLE_NAME: &str = "Table frames";

/// Key frames shown for a device that has no sample yet.
const START_KEYS: usize = 4;

/// The name an edited table's asset gets.
pub(in crate::app_core) fn table_name(keys: usize, phase_safe: bool) -> String {
    format!(
        "{TABLE_NAME} {keys} {}",
        if phase_safe { "aligned" } else { "plain" }
    )
}

/// What an asset name says about an edited table: the key frame count and whether the frames
/// were aligned.
fn parse_table_name(name: &str) -> Option<(usize, bool)> {
    let rest = name.strip_prefix(TABLE_NAME)?.trim();
    let (count, mode) = rest.split_once(' ')?;
    let count: usize = count.parse().ok()?;
    let aligned = match mode {
        "aligned" => true,
        "plain" => false,
        _ => return None,
    };
    (1..=MAX_KEYS).contains(&count).then_some((count, aligned))
}

/// The samples of a zone as the synth reads them for its table, and what was found in them.
struct Source {
    asset: String,
    start: u64,
    end: u64,
    mono: Arc<[f32]>,
    cycle: Option<Cycle>,
    /// The frames the synth cuts the sample into ([`edit::source_frames`]).
    frames: Vec<Frame>,
}

/// The editor's state in the core.
#[derive(Default)]
pub(super) struct WavetableState {
    /// The sample of the device being edited, read once per asset.
    source: Option<Source>,
    /// The starting frames of a device without a sample, for the table they were made from.
    start: Option<(usize, Vec<Frame>)>,
}

impl WavetableState {
    /// Forgets everything (the project changed or the editor closed).
    pub(super) fn reset(&mut self) {
        self.source = None;
        self.start = None;
    }
}

/// The first frames of factory table `table`, as key frames: [`START_KEYS`] evenly spaced
/// positions (a sine for the imported table).
fn start_frames(table: usize) -> Vec<Frame> {
    if table >= TABLE_COUNT {
        let mut sine = vec![0.0f32; FRAME_LEN];
        let turn = std::f64::consts::TAU / FRAME_LEN as f64;
        for (n, x) in sine.iter_mut().enumerate() {
            *x = (turn * n as f64).sin() as f32;
        }
        return vec![sine];
    }
    (0..START_KEYS)
        .map(|i| {
            let mut frame = vec![0.0f32; FRAME_LEN];
            display_frame(table, i as f32 / (START_KEYS - 1) as f32, &mut frame);
            frame
        })
        .collect()
}

impl<S: EngineSink> AppCore<S> {
    fn wavetable_device(&self, device: &DeviceKey) -> Option<&atomik_model::RackDevice> {
        self.doc()
            .resolve(device)
            .filter(|d| d.type_id == super::samples::WAVETABLE)
    }

    /// Reads the sample behind the device's zone into `state.source` (once per asset). `false`
    /// while the asset is still loading.
    fn load_wavetable_source(&mut self, asset: &str, start: u64, end: u64) -> bool {
        if self
            .wavetable
            .source
            .as_ref()
            .is_some_and(|s| (s.asset.as_str(), s.start, s.end) == (asset, start, end))
        {
            return true;
        }
        let Some(buffer) = self.zone_buffer(asset) else {
            return false;
        };
        let first = buffer.data.first().map_or(0, Vec::len);
        let end = usize::try_from(end).unwrap_or(usize::MAX).min(first);
        let from = usize::try_from(start).unwrap_or(usize::MAX).min(end);
        let channels = buffer.data.len().max(1) as f32;
        let mono: Arc<[f32]> = (from..end)
            .map(|i| buffer.data.iter().map(|c| c[i]).sum::<f32>() / channels)
            .collect();
        let cycle = edit::detect_cycle(&mono);
        let frames = edit::source_frames(&mono);
        self.wavetable.source = Some(Source {
            asset: asset.to_owned(),
            start: from as u64,
            end: end as u64,
            mono,
            cycle,
            frames,
        });
        true
    }

    /// What the table editor shows for the Wavetable Synth `device`: its key frames and what is
    /// known about them. `None` when the device is gone or is no wavetable synth.
    pub fn wavetable_data(&mut self, device: &DeviceKey) -> Option<WavetableData> {
        let dev = self.wavetable_device(device)?;
        let name = dev.name.clone();
        let plays = ["osc1.table", "osc2.table"].iter().any(|k| {
            dev.params
                .get(*k)
                .is_some_and(|v| v.round() as usize == IMPORTED_TABLE)
        });
        let table = dev
            .params
            .get("osc1.table")
            .map_or(0, |v| v.round().max(0.0) as usize);
        let zone = dev.zones.first().map(|z| (z.asset.clone(), z.start, z.end));
        let mut data = WavetableData {
            device: name,
            plays,
            ..WavetableData::default()
        };
        let Some((asset, start, end)) = zone else {
            let reuse = self
                .wavetable
                .start
                .as_ref()
                .is_some_and(|(t, _)| *t == table);
            if !reuse {
                self.wavetable.start = Some((table, start_frames(table)));
            }
            data.keys = self
                .wavetable
                .start
                .as_ref()
                .map_or_else(Vec::new, |(_, k)| k.clone());
            return Some(data);
        };
        data.stored = true;
        if !self.load_wavetable_source(&asset, start, end) {
            return Some(data);
        }
        let label = self
            .doc()
            .assets
            .get(&asset)
            .map(|m| m.original_name.clone())
            .unwrap_or_default();
        let source = self.wavetable.source.as_ref()?;
        let frames = &source.frames;
        match parse_table_name(&label) {
            Some((count, aligned)) if frames.len() == FRAMES => {
                data.phase_safe = aligned;
                data.keys = (0..count)
                    .map(|i| frames[key_position(i, count)].clone())
                    .collect();
            }
            _ => {
                data.imported = true;
                data.cycle = source
                    .cycle
                    .map(|c| (c.length, c.hz(self.doc().sample_rate as f32)));
                data.keys = frames.clone();
            }
        }
        Some(data)
    }

    /// Stores `keys` as the table of the Wavetable Synth `device` in one undo step (see the module
    /// documentation). `phase_safe` aligns the fundamentals of the frames first.
    pub fn commit_wavetable(
        &mut self,
        device: &DeviceKey,
        keys: &[Frame],
        phase_safe: bool,
    ) -> Result<(), String> {
        if self.wavetable_device(device).is_none() {
            return Err("the Wavetable Synth is gone".to_owned());
        }
        if keys.is_empty() || keys.len() > MAX_KEYS {
            return Err(format!("a table has 1 to {MAX_KEYS} frames"));
        }
        let table = edit::bake(keys, phase_safe);
        let rate = self.doc().sample_rate;
        let data = AudioData::new(rate, vec![table]).map_err(|e| e.to_string())?;
        let name = table_name(keys.len(), phase_safe);
        self.store_sample(data, &name, device.clone(), None)?;
        Ok(())
    }

    /// Cuts the sample the table was made from with one detected cycle per frame, drops silent
    /// and near-identical frames and stores the rest (one undo step). The status line says what
    /// came of it.
    pub fn cut_wavetable_cycles(&mut self, device: &DeviceKey) {
        let Some(source) = &self.wavetable.source else {
            return self.reject("The table has no sample to cut".to_owned());
        };
        let Some(cycle) = source.cycle else {
            return self.reject("No cycle was found in the sample".to_owned());
        };
        let frames = edit::cut_cycles(&source.mono, cycle.length, MAX_KEYS);
        let keys = edit::drop_near_duplicates(&frames, DUPLICATE_TOLERANCE);
        if keys.is_empty() {
            return self.reject("The cycles of the sample are silent: nothing to keep".to_owned());
        }
        let count = keys.len();
        match self.commit_wavetable(device, &keys, true) {
            Ok(()) => self.reject(format!(
                "Cut {count} frame(s) at {:.1} samples per cycle",
                cycle.length
            )),
            Err(e) => self.reject(format!("table: {e}")),
        }
    }

    /// Makes oscillator 1 of `device` play the imported table (its table choice becomes the
    /// last entry), as one undo step.
    pub fn wavetable_use_on_osc1(&mut self, device: &DeviceKey) {
        if self.wavetable_device(device).is_none() {
            return;
        }
        let result = self.apply_command(Command::SetParam {
            key: device.clone(),
            param: "osc1.table".to_owned(),
            value: IMPORTED_TABLE as f32,
            merge: None,
        });
        match result {
            Ok(_) => self.reject("Oscillator 1 plays the table".to_owned()),
            Err(e) => self.reject(format!("table: {e}")),
        }
    }

    /// Puts a message of the editor on the status line.
    pub fn report_wavetable(&mut self, message: String) {
        self.reject(message);
    }

    /// Lets go of what the editor read (it closed).
    pub fn release_wavetable(&mut self) {
        self.wavetable.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_name_round_trips_and_nothing_else_parses() {
        for (n, aligned) in [(1, true), (4, false), (16, true)] {
            assert_eq!(
                parse_table_name(&table_name(n, aligned)),
                Some((n, aligned))
            );
        }
        for bad in [
            "",
            "Table frames",
            "Table frames 0 aligned",
            "Table frames 17 aligned",
            "Table frames 4 loud",
            "Table frames x plain",
            "Sample 1",
            "drop.wav",
        ] {
            assert_eq!(parse_table_name(bad), None, "{bad}");
        }
    }
}
