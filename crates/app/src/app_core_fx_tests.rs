//! The effect meter feed, effects on
//! channel inserts and FX returns, latency epochs of the dynamic-latency effects, and the
//! sidechain helper end to end (with a real offline engine for the audio checks).

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Note, SocketRef,
    SongCommand,
};
use atomik_ui::sidechain::{ScSource, sidechain_command};

const SYNTH: &str = "atomik.poly-synth";
const EFFECTS: [&str; 8] = [
    "atomik.reverb",
    "atomik.compressor",
    "atomik.eq",
    "atomik.distortion",
    "atomik.modulation",
    "atomik.mastering",
    "atomik.character",
    "atomik.guitar-amp",
];

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

fn set<S: EngineSink>(c: &mut AppCore<S>, k: &DeviceKey, p: &str, v: f32) {
    c.apply_command(Command::SetParam {
        key: k.clone(),
        param: p.into(),
        value: v,
        merge: None,
    })
    .unwrap_or_else(|e| panic!("{k} {p}: {e}"));
}

fn channel_of<S: EngineSink>(c: &AppCore<S>, source: &str) -> DeviceKey {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("automatic channel")
        .channel
        .clone()
}

fn cable(from: (&DeviceKey, &str), to: (&DeviceKey, &str)) -> Command {
    let r = |(d, s): (&DeviceKey, &str)| SocketRef {
        device: d.clone(),
        socket: s.to_string(),
    };
    Command::Connect(Cable {
        from: r(from),
        to: r(to),
    })
}

/// Puts `effect` on the insert of `channel`: send -> effect -> return, both sides.
fn insert<S: EngineSink>(c: &mut AppCore<S>, channel: &DeviceKey, effect: &DeviceKey) {
    for side in ["l", "r"] {
        c.apply_command(cable(
            (channel, &format!("insert_send_{side}")),
            (effect, &format!("in_{side}")),
        ))
        .expect("send into the effect");
        c.apply_command(cable(
            (effect, &format!("out_{side}")),
            (channel, &format!("insert_return_{side}")),
        ))
        .expect("effect into the return");
    }
}

fn routings(calls: &[SinkCall]) -> usize {
    calls
        .iter()
        .filter(|x| matches!(x, SinkCall::Routing(_)))
        .count()
}

fn notes_clip(notes: Vec<Note>, length: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    }
}

// ------------------------------------------------------------------ meter feed

#[test]
fn published_values_reach_the_rack_view_each_frame() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "atomik.compressor", "comp");
    let id = c.device_id(&key("comp")).unwrap();
    c.frame();
    assert!(
        c.view().device_meters(&key("comp")).is_empty(),
        "nothing yet"
    );
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.5, -12.0], 1));
    c.frame();
    assert!(
        !c.view().device_meters(&key("comp")).is_empty(),
        "the values reached the view"
    );
}

#[test]
fn a_stale_sequence_number_is_not_applied_twice() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "atomik.compressor", "comp");
    let id = c.device_id(&key("comp")).unwrap();
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.5, -12.0], 1));
    c.frame();
    let first = c.view().device_meters(&key("comp")).to_vec();
    // Different values under the same sequence number: the engine did not publish them.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.001, 0.001, 0.0], 1));
    c.frame();
    assert_eq!(c.view().device_meters(&key("comp")), &first[..]);
    // A new sequence number is applied.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.001, 0.001, 0.0], 2));
    c.frame();
    assert_ne!(c.view().device_meters(&key("comp")), &first[..]);
}

#[test]
fn every_effect_type_that_publishes_is_fed_and_the_others_are_left_alone() {
    let mut c = AppCore::new(RecordingSink::new());
    for (i, t) in EFFECTS.iter().enumerate() {
        add(&mut c, t, &format!("fx{i}"));
    }
    add(&mut c, SYNTH, "synth");
    for (i, t) in EFFECTS.iter().enumerate() {
        let id = c.device_id(&key(&format!("fx{i}"))).unwrap();
        let n = atomik_ui::fx::FxKind::of(t).unwrap().meter_count().max(1);
        c.sink_mut().device_values.insert(id, (vec![0.25; n], 3));
    }
    let synth = c.device_id(&key("synth")).unwrap();
    c.sink_mut().device_values.insert(synth, (vec![0.9; 4], 3));
    c.frame();
    for (i, t) in EFFECTS.iter().enumerate() {
        let shown = c.view().device_meters(&key(&format!("fx{i}")));
        let publishes = atomik_ui::fx::FxKind::of(t).unwrap().meter_count() > 0;
        assert_eq!(!shown.is_empty(), publishes, "{t}");
    }
    assert!(c.view().device_meters(&key("synth")).is_empty());
}

#[test]
fn a_rebuilt_engine_restarts_the_sequence_tracking() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "atomik.compressor", "comp");
    let id = c.device_id(&key("comp")).unwrap();
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.5, -12.0], 9));
    c.frame();
    let loud = c.view().device_meters(&key("comp")).to_vec();
    c.rebuild_engine();
    let id = c.device_id(&key("comp")).unwrap();
    // A fresh engine counts from 1 again: the lower number must still be applied.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.001, 0.001, 0.0], 1));
    c.frame();
    assert_ne!(c.view().device_meters(&key("comp")), &loud[..]);
}

#[test]
fn a_real_engine_feeds_the_compressor_meters_while_it_works() {
    let mut c = AppCore::new(OfflineSink::new());
    add(&mut c, SYNTH, "synth");
    add(&mut c, "atomik.compressor", "comp");
    let ch = channel_of(&c, "synth");
    insert(&mut c, &ch, &key("comp"));
    set(&mut c, &key("comp"), "threshold", -60.0);
    set(&mut c, &key("comp"), "ratio", 20.0);
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    let len = 4 * atomik_core::PPQ;
    let note = Note::new(0, len, 60, 1.0);
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: notes_clip(vec![note], len),
    }))
    .unwrap();
    c.transport_action(atomik_ui::TransportAction::Play);
    c.sink_mut().render(48_000 / 2);
    c.frame();
    assert!(
        !c.view().device_meters(&key("comp")).is_empty(),
        "the engine's values reached the view"
    );
}

// ------------------------------------------------------------------ inserts and returns

#[test]
fn each_effect_goes_on_a_channel_insert_and_on_an_fx_return() {
    for (i, t) in EFFECTS.iter().enumerate() {
        let mut c = AppCore::new(RecordingSink::new());
        add(&mut c, SYNTH, "synth");
        add(&mut c, t, "fx");
        let ch = channel_of(&c, "synth");
        c.sink_mut().take();
        insert(&mut c, &ch, &key("fx"));
        assert!(
            routings(&c.sink_mut().take()) >= 1,
            "{t}: insert recompiles"
        );
        // Then onto a return (the way the console menu does it).
        c.apply_command(Command::Console(ConsoleCommand::SetReturn {
            slot: i,
            effect: Some(key("fx")),
        }))
        .unwrap_or_else(|e| panic!("{t}: {e}"));
        assert_eq!(c.doc().console.returns[i], Some(key("fx")), "{t}");
        assert!(
            c.last_rejection().is_none(),
            "{t}: {:?}",
            c.last_rejection()
        );
    }
}

#[test]
fn a_latency_change_of_an_insert_effect_realigns_the_other_channels() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    add(&mut c, "atomik.compressor", "comp");
    add(&mut c, "atomik.distortion", "dist");
    let (ca, cb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    insert(&mut c, &cb, &key("comp"));
    insert(&mut c, &ca, &key("dist"));
    c.frame();
    let comp = c.device_id(&key("comp")).unwrap();
    // The compressor's lookahead (5 ms at 48 kHz) goes live: the epoch moves.
    c.sink_mut().latencies.insert(comp, 240);
    c.sink_mut().epoch += 1;
    c.sink_mut().take();
    c.frame();
    assert_eq!(routings(&c.sink_mut().take()), 1, "re-routed once");
    assert_eq!(c.topology().latency(comp), Some(240));
    let pdc = c.sink().pdc.as_ref().expect("report");
    assert!(pdc.active);
    assert!(
        pdc.cable_delays.iter().any(|(_, _, frames)| *frames == 240),
        "the path without the compressor is delayed by 240: {pdc:?}"
    );
    // The distortion's oversampling latency (35) follows with the next epoch.
    let dist = c.device_id(&key("dist")).unwrap();
    c.sink_mut().latencies.insert(dist, 35);
    c.sink_mut().epoch += 1;
    c.frame();
    let pdc = c.sink().pdc.as_ref().unwrap();
    assert!(
        pdc.cable_delays
            .iter()
            .any(|(_, _, frames)| *frames == 240 - 35),
        "{pdc:?}"
    );
}

// ------------------------------------------------------------------ sidechain, end to end

const SR: usize = 48_000;
const BEAT: usize = SR / 2;

fn source(c: &mut AppCore<OfflineSink>, k: &str, sustain: f32, note: u8, len: i64) {
    add(c, SYNTH, k);
    let key = key(k);
    for (p, v) in [
        ("osc1.wave", 1.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.resonance", 0.0),
        ("env.amp.attack", 0.5),
        ("env.amp.decay", 60.0),
        ("env.amp.sustain", sustain),
        ("env.amp.release", 5.0),
        ("voices", 1.0),
    ] {
        set(c, &key, p, v);
    }
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key.clone(),
    }))
    .unwrap();
    let ppq = atomik_core::PPQ;
    let notes = (0..4).map(|b| Note::new(b * ppq, len, note, 1.0)).collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key,
        lane: 0,
        clip: notes_clip(notes, 4 * ppq),
    }))
    .unwrap();
}

/// A muted kick channel (its insert send still keys) and a bass channel with a compressor
/// insert (`mix` as given); the master carries only the bass.
fn ducking_core(mix: f32) -> AppCore<OfflineSink> {
    let mut c = AppCore::new(OfflineSink::new());
    source(&mut c, "kick", 0.0, 48, atomik_core::PPQ / 8);
    source(&mut c, "bass", 1.0, 36, atomik_core::PPQ);
    add(&mut c, "atomik.compressor", "comp");
    let (kick, bass) = (channel_of(&c, "kick"), channel_of(&c, "bass"));
    set(&mut c, &kick, "mute", 1.0);
    insert(&mut c, &bass, &key("comp"));
    let comp = key("comp");
    for (p, v) in [
        ("threshold", -40.0),
        ("ratio", 20.0),
        ("knee", 0.0),
        ("attack", 0.1),
        ("release", 60.0),
        ("mix", mix),
    ] {
        set(&mut c, &comp, p, v);
    }
    c
}

fn bounce(c: &mut AppCore<OfflineSink>) -> Vec<f32> {
    c.transport_action(atomik_ui::TransportAction::Play);
    let out = c.sink_mut().render_blocks(4 * BEAT, 256);
    out[0].clone()
}

fn rms(x: &[f32], from: usize, to: usize) -> f64 {
    let w = &x[from..to];
    (w.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
}

fn apply_helper(c: &mut AppCore<OfflineSink>, source: ScSource) {
    let cmd = sidechain_command(c.doc(), c.registry(), &key("comp"), &source).expect("a change");
    c.handle_action(atomik_ui::UiAction::Command(cmd));
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
}

#[test]
fn the_helper_batch_cables_the_kick_into_the_bass_compressor_and_ducks_it() {
    // Baseline: the compressor passes the bass untouched (mix 0).
    let baseline = bounce(&mut ducking_core(0.0));
    let mut c = ducking_core(1.0);
    let kick = channel_of(&c, "kick");
    apply_helper(&mut c, ScSource::Channel(kick.clone()));
    assert!(c.doc().cables.iter().any(|x| x.from.device == kick
        && x.from.socket == "insert_send_l"
        && x.to.device == key("comp")
        && x.to.socket == "sc_in_l"));
    // One undo step removes the cables and the enable together.
    assert!(c.undo());
    assert!(
        !c.doc()
            .cables
            .iter()
            .any(|x| x.to.device == key("comp") && x.to.socket.starts_with("sc_in"))
    );
    assert!(c.redo());
    let ducked = bounce(&mut c);
    // Skip the first beat (startup); compare the 40 ms after the 2nd and 3rd kick with the
    // same windows of the baseline, and the quiet end of the same beats.
    let mut worst = 0.0f64;
    for beat in 1..3 {
        let hit = beat * BEAT + SR / 100;
        let (d, b) = (
            rms(&ducked, hit, hit + SR / 25),
            rms(&baseline, hit, hit + SR / 25),
        );
        assert!(b > 1e-3, "the baseline has bass: {b}");
        worst = worst.max(d / b);
        let late = beat * BEAT + BEAT - SR / 10;
        let (dl, bl) = (
            rms(&ducked, late, late + SR / 20),
            rms(&baseline, late, late + SR / 20),
        );
        assert!(dl > 0.7 * bl, "the bass is back between hits: {dl} vs {bl}");
    }
    assert!(
        worst < 0.5,
        "ducked to {worst:.3} of the baseline during the kick"
    );
    eprintln!(
        "ducking: bass RMS during the kick = {:.1} dB of the baseline",
        20.0 * worst.log10()
    );
}

#[test]
fn a_helper_batch_that_would_close_a_cycle_is_rejected_and_reported() {
    let mut c = ducking_core(1.0);
    // Keying the compressor from its own outputs closes a loop; the helper does not guard it,
    // the model rejects the whole batch.
    let cmd = sidechain_command(
        c.doc(),
        c.registry(),
        &key("comp"),
        &ScSource::Device(key("comp")),
    );
    // The compressor itself is not offered, but the command builder accepts any source.
    let cmd = cmd.expect("a batch");
    let before = c.doc().clone();
    c.handle_action(atomik_ui::UiAction::Command(cmd));
    assert_eq!(c.doc(), &before, "nothing changed");
    let why = c.last_rejection().expect("reported to the view");
    assert!(!why.is_empty());
}

#[test]
fn a_real_compressor_lookahead_and_distortion_oversampling_move_the_latency_epoch() {
    let mut c = AppCore::new(OfflineSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    add(&mut c, "atomik.compressor", "comp");
    add(&mut c, "atomik.distortion", "dist");
    let (ca, cb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    insert(&mut c, &cb, &key("comp"));
    insert(&mut c, &ca, &key("dist"));
    set(&mut c, &key("dist"), "oversample", 0.0);
    c.sink_mut().render(256);
    c.frame();
    let (comp, dist) = (
        c.device_id(&key("comp")).unwrap(),
        c.device_id(&key("dist")).unwrap(),
    );
    assert_eq!(c.topology().latency(comp), Some(0));
    assert_eq!(c.topology().latency(dist), Some(0));
    // The engine republishes the latency after a batch with the new param.
    set(&mut c, &key("comp"), "lookahead", 5.0);
    set(&mut c, &key("dist"), "oversample", 2.0);
    c.sink_mut().render(256);
    c.frame();
    assert_eq!(c.topology().latency(comp), Some(240));
    assert_eq!(c.topology().latency(dist), Some(35));
}

#[test]
fn the_fx_demo_opens_plays_and_feeds_every_metered_effect_to_the_view() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/fx-demo/fx-demo.atomik");
    let mut c = AppCore::new(OfflineSink::new());
    c.open(&path).unwrap();
    c.wait_background();
    assert!(c.missing_assets().is_empty());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    c.transport_action(atomik_ui::TransportAction::Play);
    // Four seconds: the engine publishes while the song plays; one frame per 100 ms.
    for _ in 0..40 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    for k in ["bass-comp", "hall", "lead-dist", "lead-eq", "master-fx"] {
        assert!(!c.view().device_meters(&key(k)).is_empty(), "{k} was fed");
    }
    // The modulation publishes nothing.
    assert!(c.view().device_meters(&key("pad-chorus")).is_empty());
    assert!(c.last_rejection().is_none());
}

// ------------------------------------------------------------------ insert chains (console)

fn set_insert(
    channel: &DeviceKey,
    slot: usize,
    effect: Option<atomik_model::InsertEffect>,
) -> atomik_ui::UiAction {
    atomik_ui::UiAction::Command(Command::Console(ConsoleCommand::SetInsert {
        channel: channel.clone(),
        slot,
        effect,
    }))
}

fn new_fx(type_id: &str) -> Option<atomik_model::InsertEffect> {
    Some(atomik_model::InsertEffect::New {
        type_id: type_id.into(),
    })
}

fn chain(c: &AppCore<impl EngineSink>, channel: &DeviceKey) -> Vec<DeviceKey> {
    atomik_model::insert_chain(c.doc(), c.registry(), channel)
}

fn voice(c: &mut AppCore<OfflineSink>) {
    source(c, "synth", 1.0, 48, atomik_core::PPQ);
}

fn shape(c: &mut AppCore<OfflineSink>, comp: &DeviceKey, eq: &DeviceKey) {
    for (p, v) in [("threshold", -30.0), ("ratio", 8.0), ("attack", 1.0)] {
        set(c, comp, p, v);
    }
    set(c, eq, "output", -3.0);
}

#[test]
fn a_compressor_and_eq_added_from_the_console_bounce_like_the_hand_cabled_chain() {
    // Through the console action.
    let mut a = AppCore::new(OfflineSink::new());
    voice(&mut a);
    let ch = channel_of(&a, "synth");
    a.handle_action(set_insert(&ch, 0, new_fx("atomik.compressor")));
    a.handle_action(set_insert(&ch, 1, new_fx("atomik.eq")));
    assert!(a.last_rejection().is_none(), "{:?}", a.last_rejection());
    let keys = chain(&a, &ch);
    assert_eq!(keys.len(), 2);
    shape(&mut a, &keys[0], &keys[1]);

    // By hand: the same devices right after the channel, cabled send -> comp -> eq -> return.
    let mut b = AppCore::new(OfflineSink::new());
    voice(&mut b);
    let chb = channel_of(&b, "synth");
    let at = b
        .doc()
        .devices
        .iter()
        .position(|d| d.key == chb)
        .expect("channel")
        + 1;
    for (i, (t, k)) in [("atomik.compressor", "comp"), ("atomik.eq", "eq")]
        .into_iter()
        .enumerate()
    {
        b.apply_command(Command::AddDevice {
            index: at + i,
            type_id: t.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    let (comp, eq) = (key("comp"), key("eq"));
    for side in ["l", "r"] {
        for (f, t) in [
            (
                (&chb, format!("insert_send_{side}")),
                (&comp, format!("in_{side}")),
            ),
            ((&comp, format!("out_{side}")), (&eq, format!("in_{side}"))),
            (
                (&eq, format!("out_{side}")),
                (&chb, format!("insert_return_{side}")),
            ),
        ] {
            b.apply_command(cable((f.0, &f.1), (t.0, &t.1))).unwrap();
        }
    }
    shape(&mut b, &comp, &eq);

    let (xa, xb) = (bounce(&mut a), bounce(&mut b));
    assert!(
        rms(&xa, BEAT / 4, BEAT) > 1e-3,
        "the chain passes the voice"
    );
    assert!(
        xa.iter().zip(&xb).all(|(p, q)| p.to_bits() == q.to_bits()),
        "the console-built chain renders bit for bit like the hand-cabled one"
    );
}

#[test]
fn insert_edits_undo_redo_and_remove_slots() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "synth");
    let ch = channel_of(&c, "synth");
    let start = c.doc().clone();
    c.sink_mut().take();
    c.handle_action(set_insert(&ch, 0, new_fx("atomik.compressor")));
    assert!(
        routings(&c.sink_mut().take()) >= 1,
        "the engine is re-routed"
    );
    c.handle_action(set_insert(&ch, 1, new_fx("atomik.eq")));
    let two = c.doc().clone();
    let both = chain(&c, &ch);
    assert_eq!(both.len(), 2);
    // Remove the first slot: the device stays in the rack, the eq closes the chain.
    c.handle_action(set_insert(&ch, 0, None));
    assert_eq!(chain(&c, &ch), [both[1].clone()]);
    assert!(c.doc().device(&both[0]).is_some());
    assert!(c.undo());
    assert_eq!(c.doc(), &two);
    assert!(c.redo());
    assert_eq!(chain(&c, &ch), [both[1].clone()]);
    assert!(c.undo() && c.undo() && c.undo());
    assert_eq!(c.doc(), &start);
}

#[test]
fn rejected_insert_edits_are_reported_and_change_nothing() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    add(&mut c, "atomik.compressor", "comp");
    let (ca, cb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let existing = |k: &str| Some(atomik_model::InsertEffect::Existing(key(k)));
    c.handle_action(set_insert(&ca, 0, existing("comp")));
    assert!(c.last_rejection().is_none());
    let before = c.doc().clone();
    // Double membership: the compressor is already on a's chain.
    c.handle_action(set_insert(&cb, 0, existing("comp")));
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().expect("reported").contains("already"));
    // Not an effect.
    c.handle_action(set_insert(&cb, 0, existing("a")));
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().is_some());
    // A cycle: a compressor keyed from b's output cannot join b's chain.
    add(&mut c, "atomik.compressor", "comp2");
    c.apply_command(cable((&cb, "out_l"), (&key("comp2"), "sc_in_l")))
        .unwrap();
    let before = c.doc().clone();
    c.handle_action(set_insert(&cb, 0, existing("comp2")));
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().expect("reported").contains("cycle"));
}

#[test]
fn a_chain_effect_with_lookahead_realigns_the_other_channels() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    let (ca, cb) = (channel_of(&c, "a"), channel_of(&c, "b"));
    let _ = ca;
    c.handle_action(set_insert(&cb, 0, new_fx("atomik.compressor")));
    c.handle_action(set_insert(&cb, 1, new_fx("atomik.eq")));
    c.frame();
    let comp = chain(&c, &cb)[0].clone();
    let id = c.device_id(&comp).unwrap();
    c.sink_mut().latencies.insert(id, 240);
    c.sink_mut().epoch += 1;
    c.sink_mut().take();
    c.frame();
    assert_eq!(routings(&c.sink_mut().take()), 1, "re-routed once");
    assert_eq!(c.topology().latency(id), Some(240));
    let pdc = c.sink().pdc.as_ref().expect("report");
    assert!(
        pdc.cable_delays.iter().any(|(_, _, frames)| *frames == 240),
        "the other channel is delayed by the chain's latency: {pdc:?}"
    );
}

#[test]
fn picking_an_insert_in_the_console_selects_it_in_the_rack() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "synth");
    let ch = channel_of(&c, "synth");
    c.handle_action(set_insert(&ch, 0, new_fx("atomik.eq")));
    let fx = chain(&c, &ch)[0].clone();
    c.handle_action(atomik_ui::UiAction::Select(fx.clone()));
    assert_eq!(c.view().selected(), Some(&fx));
}

// ------------------------------------------------------------------ Mod Source scope

#[test]
fn the_mod_source_scope_is_fed_from_published_values() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, "atomik.mod-source", "mod");
    let id = c.device_id(&key("mod")).unwrap();
    let mut v = vec![0.0f32; 32];
    v[6] = 8.0;
    v[31] = 2.5;
    c.sink_mut().device_values.insert(id, (v, 1));
    c.frame();
    let scope = c.view().modsrc_scope(&key("mod")).expect("scope");
    assert_eq!(scope.len(), 8);
}

#[test]
fn a_real_engine_feeds_the_mod_source_scope_while_it_plays() {
    let mut c = AppCore::new(OfflineSink::new());
    add(&mut c, "atomik.mod-source", "mod");
    set(&mut c, &key("mod"), "lfo1_rate", 5.0);
    c.transport_action(atomik_ui::TransportAction::Play);
    c.sink_mut().render(48_000 / 2);
    c.frame();
    let scope = c.view().modsrc_scope(&key("mod")).expect("scope");
    assert!(!scope.is_empty(), "the engine's values reached the scope");
    let moving = (0..scope.len())
        .filter_map(|a| scope.point(0, a))
        .any(|v| v.abs() > 0.1);
    assert!(moving, "LFO 1 swings");
}
