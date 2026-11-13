//! The decision zone: where the mix stands among the song's reference tracks.
//!
//! An analysis measures the mix and every reference with [`atomik_render::zone`] (loudness, the
//! loudest part, punch, brightness, low end, width, correlation). It runs on a worker thread
//! (`atomik-zone`), never on the audio or the UI thread:
//!
//! * the **mix** is a render of the song with the renderer a bounce uses
//!   ([`atomik_render::project::render_doc_in`]), so it is the song as it is now, after the master
//!   chain and without the reference voice, and the same on every platform within the resolution
//!   of the readings. A song with third-party plugins is refused here (a headless render cannot
//!   run them);
//! * a **reference** is decoded from its file. Its readings are kept by path while it stays in
//!   the list, so a refresh measures only files that are new.
//!
//! [`AppCore::refresh_zone`] starts an analysis (one at a time); [`AppCore::zone_data`] is what the
//! panel shows: the range of every attribute over the references, the mix as a marker and the
//! verdict ("inside", "above by 1.4 dB"). The analysis never changes the song: no command, no
//! undo step. It measures the mix as it was when the render started, and the panel says when the
//! song has changed since.

use super::*;
use atomik_audio_file::decode_file_with;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_render::zone::{self, Attribute, Verdict, ZoneMetrics};
use atomik_ui::{Standing, ZoneData, ZoneReference, ZoneRowData};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// One measurement, or why there is none.
type Reading = Result<ZoneMetrics, String>;

/// The largest file a reference reading decodes, in samples over all channels.
const MAX_SAMPLES: u64 = 180_000_000;

/// What the worker brings back.
struct Outcome {
    /// The document revision the render started from.
    revision: u64,
    mix: Reading,
    references: Vec<(String, Reading)>,
}

/// The decision-zone state of one song (not saved).
pub(crate) struct ZoneState {
    /// The readings of the references by path.
    references: BTreeMap<String, Reading>,
    /// The mix's reading and the document revision it was rendered from.
    mix: Option<(u64, Reading)>,
    /// The analysis that is running.
    job: Option<Receiver<Outcome>>,
}

impl ZoneState {
    pub(crate) fn new() -> Self {
        Self {
            references: BTreeMap::new(),
            mix: None,
            job: None,
        }
    }
}

/// Decodes `path` and measures it.
fn measure_file(path: &str) -> Reading {
    let decoded = decode_file_with(Path::new(path), MAX_SAMPLES).map_err(|e| e.to_string())?;
    if decoded.audio.channels.first().is_none_or(Vec::is_empty) {
        return Err("the file has no audio".to_owned());
    }
    catch_unwind(AssertUnwindSafe(|| zone::measure(&decoded.audio)))
        .map_err(|_| "the measurement failed".to_owned())
}

/// Renders `doc` as a bounce does and measures the result.
fn measure_mix(doc: &RackDoc, root: Option<&Path>) -> Reading {
    let registry = atomik_render::default_registry();
    let rendered = catch_unwind(AssertUnwindSafe(|| {
        render_doc_in(doc, &registry, &ProjectOptions::default(), root)
    }))
    .map_err(|_| "a device failed while the mix rendered".to_owned())?
    .map_err(|e| e.to_string())?;
    Ok(zone::measure_channels(
        &rendered.channels,
        rendered.sample_rate,
    ))
}

/// How a reading shows in the list of references.
fn detail(reading: Option<&Reading>, busy: bool) -> String {
    match reading {
        Some(Ok(m)) => match m.loudest_lufs {
            Some(l) => format!("loudest {l:.1} LUFS"),
            None => "silent".to_owned(),
        },
        Some(Err(e)) => e.clone(),
        None if busy => "measuring".to_owned(),
        None => String::new(),
    }
}

fn standing(verdict: Option<Verdict>) -> Standing {
    match verdict {
        None => Standing::Unknown,
        Some(Verdict::Inside) => Standing::Inside,
        Some(Verdict::Above(_)) => Standing::Above,
        Some(Verdict::Below(_)) => Standing::Below,
    }
}

impl<S: EngineSink> AppCore<S> {
    /// True while an analysis runs.
    pub fn zone_busy(&self) -> bool {
        self.zone.job.is_some()
    }

    /// True once the mix has been measured (whether or not the song changed since).
    pub fn zone_measured(&self) -> bool {
        self.zone.mix.is_some()
    }

    /// Measures the mix and the references that have no reading yet, on a worker. Returns false
    /// when nothing started: an analysis is running, or the song has no reference.
    pub fn refresh_zone(&mut self) -> bool {
        if self.zone.job.is_some() {
            return false;
        }
        let listed: Vec<String> = self
            .doc()
            .references
            .iter()
            .map(|r| r.path.clone())
            .collect();
        if listed.is_empty() {
            self.report("Add a reference track to measure the mix against");
            return false;
        }
        // A file that left the list is forgotten; one that is new gets measured.
        self.zone.references.retain(|path, _| listed.contains(path));
        let wanted: Vec<String> = listed
            .into_iter()
            .filter(|p| !matches!(self.zone.references.get(p), Some(Ok(_))))
            .collect();
        let revision = self.document.revision();
        let plugins = !self.doc().plugin_devices().is_empty();
        let doc = self.doc().clone();
        let root = self.bundle_root().map(Path::to_path_buf);
        let (send, receive) = channel();
        let started = std::thread::Builder::new()
            .name("atomik-zone".into())
            .spawn(move || {
                let references = wanted
                    .into_iter()
                    .map(|p| {
                        let reading = measure_file(&p);
                        (p, reading)
                    })
                    .collect();
                let mix = if plugins {
                    Err("the song uses third-party plugins, which this render cannot run".into())
                } else {
                    measure_mix(&doc, root.as_deref())
                };
                let _ = send.send(Outcome {
                    revision,
                    mix,
                    references,
                });
            });
        if started.is_err() {
            self.report("The decision zone could not start its worker");
            return false;
        }
        self.zone.job = Some(receive);
        self.report("Measuring the mix and the references");
        true
    }

    /// One frame of the analysis: takes the worker's result when it is there.
    pub(super) fn zone_frame(&mut self) {
        let Some(job) = &self.zone.job else { return };
        let outcome = match job.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.zone.job = None;
                self.report("The decision zone worker stopped");
                return;
            }
        };
        self.zone.job = None;
        for (path, reading) in outcome.references {
            self.zone.references.insert(path, reading);
        }
        self.zone.mix = Some((outcome.revision, outcome.mix));
        let message = self.zone_summary();
        self.report(message);
    }

    /// The readings of the references that could be measured, in list order.
    fn zone_references(&self) -> Vec<ZoneMetrics> {
        self.doc()
            .references
            .iter()
            .filter_map(|r| match self.zone.references.get(&r.path) {
                Some(Ok(m)) => Some(*m),
                _ => None,
            })
            .collect()
    }

    /// The mix's readings (none before the first analysis or when it failed).
    fn zone_mix(&self) -> ZoneMetrics {
        match &self.zone.mix {
            Some((_, Ok(m))) => *m,
            _ => ZoneMetrics::default(),
        }
    }

    /// The rows of the zone as the model computes them.
    pub fn zone_rows(&self) -> Vec<zone::ZoneRow> {
        zone::zone(&self.zone_references(), &self.zone_mix())
    }

    /// The status-line text when an analysis lands: how many attributes sit inside the zone and
    /// which ones do not.
    pub(super) fn zone_summary(&self) -> String {
        let Some((_, mix)) = &self.zone.mix else {
            return String::new();
        };
        if let Err(e) = mix {
            return format!("The mix could not be measured: {e}");
        }
        let rows = self.zone_rows();
        let known = rows.iter().filter(|r| r.verdict.is_some()).count();
        if known == 0 {
            return "Decision zone: nothing to compare (the mix or the references are silent)"
                .to_owned();
        }
        let inside = rows
            .iter()
            .filter(|r| r.verdict == Some(Verdict::Inside))
            .count();
        let outside: Vec<String> = rows
            .iter()
            .filter_map(|r| match r.verdict {
                Some(Verdict::Above(_)) => Some(format!("{} above", r.attribute.label())),
                Some(Verdict::Below(_)) => Some(format!("{} below", r.attribute.label())),
                _ => None,
            })
            .collect();
        if outside.is_empty() {
            format!("Decision zone: the mix is inside on all {known}")
        } else {
            format!(
                "Decision zone: {inside} of {known} inside ({})",
                outside.join(", ")
            )
        }
    }

    /// What the references and decision zone panel shows now.
    pub fn zone_data(&self) -> ZoneData {
        let info = self.reference_info();
        let busy = self.zone_busy();
        let references: Vec<ZoneReference> = info
            .items
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let selected = i == info.selected;
                let mut text = detail(self.zone.references.get(&r.path), busy);
                if selected && let Some(gain) = info.gain_db {
                    if !text.is_empty() {
                        text.push_str("   ");
                    }
                    text.push_str(&format!("matched {gain:+.1} dB"));
                }
                if selected && r.trim_tenths != 0 {
                    if !text.is_empty() {
                        text.push_str("   ");
                    }
                    text.push_str(&format!("trim {:+.1} dB", r.trim_db()));
                }
                ZoneReference {
                    name: r.name().to_owned(),
                    selected,
                    detail: text,
                }
            })
            .collect();
        let rows = self
            .zone_rows()
            .into_iter()
            .map(|row| {
                let attribute = row.attribute;
                ZoneRowData {
                    label: attribute.label().to_owned(),
                    unit: attribute.unit().to_owned(),
                    any_level: attribute.normalised(),
                    range: row.range,
                    mix: row.mix,
                    standing: standing(row.verdict),
                    verdict: row.verdict.map(|v| v.text(attribute)).unwrap_or_default(),
                    min_span: attribute.tolerance() * 10.0,
                    digits: if attribute == Attribute::Correlation {
                        2
                    } else {
                        1
                    },
                }
            })
            .collect();
        ZoneData {
            references,
            rows,
            status: self.zone_status(busy),
            busy,
            listening: info.listening,
            trim_db: (!info.items.is_empty()).then_some(info.trim_db),
            matching: info.matching,
        }
    }

    /// The line at the foot of the panel.
    fn zone_status(&self, busy: bool) -> String {
        let count = self.doc().references.len();
        if busy {
            return "Measuring: the mix is rendered again and the references are read".to_owned();
        }
        if count == 0 {
            return "Add a reference track (Mixer > Reference) to see the zone".to_owned();
        }
        let unreadable = self
            .doc()
            .references
            .iter()
            .filter(|r| matches!(self.zone.references.get(&r.path), Some(Err(_))))
            .count();
        let Some((revision, mix)) = &self.zone.mix else {
            return "Press REFRESH to render the mix and measure it".to_owned();
        };
        if let Err(e) = mix {
            return format!("The mix could not be measured: {e}");
        }
        let mut text = if *revision == self.document.revision() {
            format!(
                "The mix is measured against {count} reference{}",
                if count == 1 { "" } else { "s" }
            )
        } else {
            "The song changed since the mix was measured: REFRESH renders it again".to_owned()
        };
        if unreadable > 0 {
            text.push_str(&format!(" ({unreadable} could not be read)"));
        }
        text
    }
}
