//! Meter panels on console strips: they open and close per strip, a deleted strip closes its
//! panels, the tap of a strip is live only while a panel looks at it, the trace is the analysis of
//! the tap, the second curve can be another strip or the reference track, a panel never changes
//! the project or a bounce.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{Command, ConsoleCommand, DeviceKey};
use atomik_ui::{CurveSource, MeterKind, MeterPanelAction};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn two_channels() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .expect("instrument added");
    }
    c.frame();
    c
}

fn channel_of(c: &AppCore<RecordingSink>, source: &str) -> DeviceKey {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("automatic channel")
        .channel
        .clone()
}

fn tone(hz: f64, frames: usize, amp: f64) -> (Vec<f32>, Vec<f32>, u64) {
    let l: Vec<f32> = (0..frames)
        .map(|i| (amp * (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin()) as f32)
        .collect();
    (l.clone(), l, frames as u64)
}

/// Puts a tone in the tap of the engine device behind `strip`.
fn feed(c: &mut AppCore<RecordingSink>, strip: &DeviceKey, scope: (Vec<f32>, Vec<f32>, u64)) {
    let id = c.device_id(strip).expect("in the engine");
    c.sink_mut().device_scopes.insert(id, scope);
}

fn state_of(c: &AppCore<RecordingSink>, id: u32) -> atomik_ui::MeterPanelState {
    c.meter_panel_states()
        .into_iter()
        .find(|s| s.id == id)
        .expect("panel open")
}

#[test]
fn panels_open_and_close_per_strip_and_a_second_open_of_the_same_kind_is_the_same_panel() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let spectrum_a = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    let phase_a = c.open_meter_panel(&a, MeterKind::Phase).unwrap();
    let spectrum_b = c.open_meter_panel(&b, MeterKind::Spectrum).unwrap();
    assert_ne!(spectrum_a, phase_a);
    assert_ne!(spectrum_a, spectrum_b);
    assert_eq!(c.meter_panel_ids(), [spectrum_a, phase_a, spectrum_b]);
    assert_eq!(
        c.open_meter_panel(&a, MeterKind::Spectrum).unwrap(),
        spectrum_a,
        "no second panel of a kind for one strip"
    );
    assert_eq!(c.meter_panel_ids().len(), 3);
    c.close_meter_panel(phase_a);
    assert_eq!(c.meter_panel_ids(), [spectrum_a, spectrum_b]);
    // The master opens one too.
    let master = c.doc().console.master.clone().expect("a master");
    assert!(c.open_meter_panel(&master, MeterKind::Phase).is_ok());
    // An instrument is not a strip and not a plugin.
    assert!(c.open_meter_panel(&key("a"), MeterKind::Spectrum).is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("strip")));
}

#[test]
fn deleting_the_strip_closes_its_panels_and_only_those() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    c.open_meter_panel(&a, MeterKind::Phase).unwrap();
    let keep = c.open_meter_panel(&b, MeterKind::Spectrum).unwrap();
    c.apply_command(Command::Console(ConsoleCommand::RemoveChannel {
        channel: a.clone(),
    }))
    .expect("the channel goes");
    c.frame();
    assert_eq!(c.meter_panel_ids(), [keep]);
    assert_eq!(c.meter_panel_target(keep), Some(&b));
}

#[test]
fn a_new_project_closes_every_panel() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    c.new_document();
    assert!(c.meter_panel_ids().is_empty());
}

#[test]
fn the_tap_is_live_only_while_a_panel_looks_at_the_strip() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let (ia, ib) = (c.device_id(&a).unwrap(), c.device_id(&b).unwrap());
    c.frame();
    assert!(c.sink().scopes_live.is_empty(), "no panel, nothing live");
    let pa = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    assert_eq!(
        c.sink().scopes_live.iter().copied().collect::<Vec<_>>(),
        [ia]
    );
    assert!(c.meter_tap_live(&a) && !c.meter_tap_live(&b));
    let pb = c.open_meter_panel(&b, MeterKind::Phase).unwrap();
    assert!(c.sink().scopes_live.contains(&ib));
    c.close_meter_panel(pa);
    assert!(!c.sink().scopes_live.contains(&ia), "closed: off again");
    assert!(c.sink().scopes_live.contains(&ib));
    c.close_meter_panel(pb);
    assert!(c.sink().scopes_live.is_empty());
}

#[test]
fn the_flag_is_sent_again_after_an_engine_restart() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    // A new engine has new taps, all off: the sink forgets and the next frame sets it again.
    c.sink_mut().scopes_live.clear();
    c.frame();
    let ia = c.device_id(&a).unwrap();
    assert!(c.sink().scopes_live.contains(&ia));
}

#[test]
fn the_trace_is_the_analysis_of_the_tap_and_the_size_steps_within_the_ring() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    let id = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    feed(&mut c, &a, tone(1000.0, 20_000, 0.5));
    c.frame();
    let s = state_of(&c, id);
    assert_eq!(s.size, 4096);
    assert_eq!(s.trace.len(), atomik_ui::analyser::TRACE_COLUMNS);
    assert!((s.peak_hz - 1000.0).abs() < 25.0, "{}", s.peak_hz);
    assert!((s.peak_db + 6.0).abs() < 2.0, "{}", s.peak_db);
    // Up to 16384 (half the ring of a strip), no further.
    for expect in [8192, 16_384, 16_384] {
        c.step_meter_panel_size(id, true);
        assert_eq!(c.meter_panel_size(id), Some(expect));
    }
    for expect in [8192, 4096, 2048, 1024, 1024] {
        c.step_meter_panel_size(id, false);
        assert_eq!(c.meter_panel_size(id), Some(expect));
    }
    // A tap that did not move is skipped: the trace stays.
    let before = state_of(&c, id).trace;
    c.frame();
    assert_eq!(state_of(&c, id).trace, before);
}

#[test]
fn the_phase_panel_measures_the_correlation_and_the_levels_of_the_frames() {
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    let id = c.open_meter_panel(&a, MeterKind::Phase).unwrap();
    let (l, _, head) = tone(440.0, 4096, 0.5);
    feed(&mut c, &a, (l.clone(), l.clone(), head));
    c.frame();
    let s = state_of(&c, id);
    assert!((s.correlation - 1.0).abs() < 1e-3, "{}", s.correlation);
    assert!((s.left_db + 6.0).abs() < 0.2 && (s.right_db + 6.0).abs() < 0.2);
    assert_eq!(
        s.scope_l.len(),
        1024,
        "every second frame of the newest 2048"
    );
    let inverted: Vec<f32> = l.iter().map(|x| -x).collect();
    feed(&mut c, &a, (l, inverted, head + 4096));
    c.frame();
    assert!((state_of(&c, id).correlation + 1.0).abs() < 1e-3);
}

#[test]
fn the_second_curve_can_be_picked_from_the_other_strips_and_the_reference() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let id = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    let s = state_of(&c, id);
    // Off, the other channel, the master: never the strip itself, no reference yet.
    assert_eq!(s.source, 0);
    assert_eq!(s.sources[0], CurveSource::Off);
    assert!(
        !s.sources
            .iter()
            .any(|x| *x == CurveSource::Strip(c.meter_curve_choices(&a)[0].1.clone()))
    );
    let choices = c.meter_curve_choices(&a);
    assert!(
        choices
            .iter()
            .any(|(f, _)| *f == CurveFrom::Strip(b.clone()))
    );
    assert!(
        !choices
            .iter()
            .any(|(f, _)| *f == CurveFrom::Strip(a.clone()))
    );
    assert!(!choices.iter().any(|(f, _)| *f == CurveFrom::Reference));
    // Pick the other strip: its tap goes live and its trace is the second curve.
    feed(&mut c, &a, tone(1000.0, 20_000, 0.5));
    feed(&mut c, &b, tone(4000.0, 20_000, 0.25));
    let row = choices
        .iter()
        .position(|(f, _)| *f == CurveFrom::Strip(b.clone()))
        .unwrap();
    c.meter_panel_action(MeterPanelAction::Source { id, index: row });
    assert_eq!(c.meter_panel_curve(id), Some(CurveFrom::Strip(b.clone())));
    assert!(
        c.meter_tap_live(&b),
        "the strip drawn as a curve writes its tap"
    );
    c.frame();
    let s = state_of(&c, id);
    assert_eq!(s.source, row);
    assert_eq!(s.second.len(), atomik_ui::analyser::TRACE_COLUMNS);
    let peak_col = |t: &[f32]| {
        t.iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .map(|(i, _)| i)
            .unwrap()
    };
    assert!(
        peak_col(&s.second) > peak_col(&s.trace),
        "4 kHz sits right of 1 kHz"
    );
    // Back to off: the other strip stops writing.
    c.meter_panel_action(MeterPanelAction::Source { id, index: 0 });
    assert!(c.state_second_is_empty(id));
    assert!(!c.meter_tap_live(&b));
    // A row that does not exist changes nothing.
    c.meter_panel_action(MeterPanelAction::Source { id, index: 99 });
    assert_eq!(c.meter_panel_curve(id), Some(CurveFrom::Off));
}

impl AppCore<RecordingSink> {
    fn state_second_is_empty(&self, id: u32) -> bool {
        state_of(self, id).second.is_empty()
    }
}

#[test]
fn a_deleted_second_strip_clears_the_curve() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let id = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    c.set_meter_panel_curve(id, CurveFrom::Strip(b.clone()));
    assert_eq!(c.meter_panel_curve(id), Some(CurveFrom::Strip(b.clone())));
    c.apply_command(Command::Console(ConsoleCommand::RemoveChannel {
        channel: b,
    }))
    .unwrap();
    c.frame();
    assert_eq!(c.meter_panel_curve(id), Some(CurveFrom::Off));
}

/// A temp folder that removes itself.
struct Tmp(std::path::PathBuf);

impl Tmp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-meter-panels-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_reference_track_is_a_choice_once_the_song_has_one_and_its_curve_follows_the_trim() {
    let tmp = Tmp::new("reference");
    let rate = 48_000usize;
    let data: Vec<f32> = (0..rate * 4)
        .map(|n| 0.3 * (std::f32::consts::TAU * 2000.0 * n as f32 / rate as f32).sin())
        .collect();
    let path = tmp.0.join("ref.wav");
    write_wav_f32(
        &path,
        &AudioData::new(48_000, vec![data.clone(), data]).unwrap(),
    )
    .unwrap();
    let mut c = two_channels();
    let a = channel_of(&c, "a");
    let id = c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    assert!(
        !c.meter_curve_choices(&a)
            .iter()
            .any(|(f, _)| *f == CurveFrom::Reference)
    );
    c.add_reference(&path).unwrap();
    let choices = c.meter_curve_choices(&a);
    let row = choices
        .iter()
        .position(|(f, _)| *f == CurveFrom::Reference)
        .expect("the reference is a choice");
    assert_eq!(row, choices.len() - 1, "last in the list");
    c.meter_panel_action(MeterPanelAction::Source { id, index: row });
    // Not decoded yet: nothing to draw.
    assert!(state_of(&c, id).second.is_empty());
    wait_reference(&mut c);
    let s = state_of(&c, id);
    assert_eq!(s.sources.last(), Some(&CurveSource::Reference));
    assert_eq!(s.second.len(), atomik_ui::analyser::TRACE_COLUMNS);
    let loudest_col = s
        .second
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map(|(i, _)| i)
        .unwrap();
    let hz = atomik_dsp::spectrum::column_edge_hz(loudest_col as f64 + 0.5, s.second.len());
    assert!((hz - 2000.0).abs() < 150.0, "{hz}");
    // The trim moves the curve by exactly its dB.
    let before = s.second.clone();
    c.set_reference_trim(-30);
    let after = state_of(&c, id).second;
    assert!(
        before
            .iter()
            .zip(&after)
            .all(|(x, y)| (x - 3.0 - y).abs() < 1e-3),
        "a trim of -3 dB lowers the curve by 3 dB"
    );
}

#[allow(clippy::disallowed_methods)] // a real worker is waited for in real time
fn wait_reference(c: &mut AppCore<RecordingSink>) {
    let start = std::time::Instant::now();
    loop {
        c.frame();
        let info = c.reference_info();
        if !info.loading && (info.loudest_lufs.is_some() || info.error.is_some()) {
            return;
        }
        assert!(start.elapsed().as_secs() < 20, "the decode did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn open_panels_change_neither_the_project_nor_a_bounce() {
    let mut c = two_channels();
    let (a, b) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let bounce = |c: &AppCore<RecordingSink>| {
        atomik_render::project::render_doc_in(
            c.doc(),
            &atomik_render::default_registry(),
            &atomik_render::project::ProjectOptions::default(),
            None,
        )
        .unwrap()
        .channels
    };
    let before_text = atomik_model::save_project(c.doc());
    let before = bounce(&c);
    let depth = c.document().undo_depth();
    c.open_meter_panel(&a, MeterKind::Spectrum).unwrap();
    let id = c.open_meter_panel(&b, MeterKind::Phase).unwrap();
    feed(&mut c, &b, tone(300.0, 8192, 0.4));
    c.frame();
    c.step_meter_panel_size(id, true);
    assert_eq!(c.document().undo_depth(), depth, "never an undo step");
    assert_eq!(
        atomik_model::save_project(c.doc()),
        before_text,
        "never saved"
    );
    let after = bounce(&c);
    assert!(
        before.len() == after.len()
            && before.iter().zip(&after).all(|(x, y)| x.len() == y.len()
                && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits())),
        "the bounce is the same to the bit"
    );
}
