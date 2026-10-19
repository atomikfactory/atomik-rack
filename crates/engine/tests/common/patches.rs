//! Patch builders shared by the multicore tests: random fake graphs with fan-out and
//! parallel branches, and a real-device patch (synth -> delay -> mixer, plus a second synth).

use super::{ep, iface};
use atomik_core::NoteKind;
use atomik_device_sdk::{DeviceFactory, SocketId};
use atomik_engine::{DeviceInstanceId, EngineConfig, Graph, RenderPlan, ScheduledNote};
use atomik_testkit::fakes::{ConstantSource, ImpulseOnNote, NoteThru, Passthrough};

/// One node of a random fake graph: `(kind, input selector, value)`.
pub type NodeSpec = (u8, usize, f32);

/// A built patch plus the targets notes should be sent to.
pub struct Built {
    pub plan: RenderPlan,
    /// `(device, notes input socket)` of every note receiver.
    pub note_targets: Vec<(DeviceInstanceId, SocketId)>,
}

/// Builds a random fake graph with 1 host input and 4 host outputs.
///
/// Kinds (mod 5): 0 ConstantSource, 1 ImpulseOnNote (notes cabled from the latest NoteThru if
/// any), 2 NoteThru, 3-4 Passthrough fed by any earlier audio producer (fan-out, branches).
/// `outs[k]` picks the producer cabled to `out_{k+1}`.
pub fn fake_graph(c: &EngineConfig, spec: &[NodeSpec], outs: [usize; 4]) -> Built {
    let mut g = Graph::new(c);
    let mut audio = vec![iface(&g, "in_1")];
    let mut thru: Option<DeviceInstanceId> = None;
    let mut note_targets = Vec::new();
    for &(kind, sel, x) in spec {
        match kind % 5 {
            0 => {
                let d = g.add_device(ConstantSource::boxed(x));
                audio.push(ep(d, ConstantSource::AUDIO_OUT));
            }
            1 => {
                let d = g.add_device(ImpulseOnNote::boxed());
                if let Some(t) = thru {
                    g.connect(ep(t, NoteThru::NOTES_OUT), ep(d, ImpulseOnNote::NOTES_IN))
                        .unwrap();
                }
                note_targets.push((d, ImpulseOnNote::NOTES_IN));
                audio.push(ep(d, ImpulseOnNote::AUDIO_OUT));
            }
            2 => {
                let d = g.add_device(NoteThru::boxed());
                note_targets.push((d, NoteThru::NOTES_IN));
                thru = Some(d);
            }
            _ => {
                let d = g.add_device(Passthrough::boxed(x));
                let from = audio[sel % audio.len()];
                g.connect(from, ep(d, Passthrough::AUDIO_IN)).unwrap();
                audio.push(ep(d, Passthrough::AUDIO_OUT));
            }
        }
    }
    for (k, sel) in outs.into_iter().enumerate() {
        let to = iface(&g, ["out_1", "out_2", "out_3", "out_4"][k]);
        g.connect(audio[sel % audio.len()], to).unwrap();
    }
    Built {
        plan: g.compile().unwrap(),
        note_targets,
    }
}

/// Notes for every target: a short On/Off pattern spread over `frames`.
pub fn notes_for(targets: &[(DeviceInstanceId, SocketId)], frames: u64) -> Vec<ScheduledNote> {
    let mut notes = Vec::new();
    for (k, &(device, socket)) in targets.iter().enumerate() {
        let k = k as u64;
        for step in 0..8u64 {
            let frame = (step * 389 + k * 61) % frames;
            let kind = if step % 2 == 0 {
                NoteKind::On {
                    key: 40 + (k as u8 % 30) + step as u8,
                    velocity: 0.8,
                }
            } else {
                NoteKind::Off {
                    key: 40 + (k as u8 % 30) + step as u8 - 1,
                }
            };
            notes.push(ScheduledNote {
                frame,
                device,
                socket,
                kind,
            });
        }
    }
    notes.sort_by_key(|n| n.frame);
    notes
}

fn add(g: &mut Graph, f: DeviceFactory) -> DeviceInstanceId {
    g.add_device((f.create)())
}

fn sock(g: &Graph, d: DeviceInstanceId, key: &str) -> atomik_engine::Endpoint {
    ep(d, g.descriptor(d).unwrap().socket_by_key(key).unwrap().id)
}

fn param(g: &mut Graph, d: DeviceInstanceId, key: &str, v: f32) {
    let p = g.descriptor(d).unwrap().param_by_key(key).unwrap().id;
    g.set_param(d, p, v).unwrap();
}

/// Real devices: `synths[0]` -> delay -> mixer ch1; `synths[1..]` -> mixer ch2..; mixer -> out.
/// With `synth_count` up to 6. Returns the plan and the synths' `notes_in` targets.
pub fn real_patch(c: &EngineConfig, synth_count: usize) -> Built {
    use atomik_devices::{line_mixer, poly_synth, stereo_delay};
    assert!((1..=6).contains(&synth_count));
    let mut g = Graph::new(c);
    let synths: Vec<_> = (0..synth_count)
        .map(|_| add(&mut g, poly_synth::factory()))
        .collect();
    let delay = add(&mut g, stereo_delay::factory());
    let mixer = add(&mut g, line_mixer::factory());
    for (k, &s) in synths.iter().enumerate() {
        param(&mut g, s, "osc1.wave", (k % 4) as f32);
        param(&mut g, s, "filter.cutoff", 800.0 + 900.0 * k as f32);
        param(&mut g, s, "lfo.to_cutoff", 1.0);
    }
    param(&mut g, delay, "time.mode", 0.0);
    param(&mut g, delay, "time.ms", 37.0);
    param(&mut g, delay, "feedback", 0.6);
    for side in ["l", "r"] {
        g.connect(
            sock(&g, synths[0], &format!("out_{side}")),
            sock(&g, delay, &format!("in_{side}")),
        )
        .unwrap();
        g.connect(
            sock(&g, delay, &format!("out_{side}")),
            sock(&g, mixer, &format!("ch1_in_{side}")),
        )
        .unwrap();
        for (k, &s) in synths.iter().enumerate().skip(1) {
            g.connect(
                sock(&g, s, &format!("out_{side}")),
                sock(&g, mixer, &format!("ch{}_in_{side}", k + 1)),
            )
            .unwrap();
        }
    }
    g.connect(sock(&g, mixer, "master_l"), iface(&g, "out_1"))
        .unwrap();
    g.connect(sock(&g, mixer, "master_r"), iface(&g, "out_2"))
        .unwrap();
    let note_targets = synths
        .iter()
        .map(|&s| (s, sock(&g, s, "notes_in").socket))
        .collect();
    Built {
        plan: g.compile().unwrap(),
        note_targets,
    }
}

/// 16 synths in groups of 4 into 4 mixers, then a master mixer (benchmark patch).
pub fn sixteen_synths(c: &EngineConfig) -> Built {
    use atomik_devices::{line_mixer, poly_synth};
    let mut g = Graph::new(c);
    let master = add(&mut g, line_mixer::factory());
    let mut note_targets = Vec::new();
    for group in 0..4 {
        let sub = add(&mut g, line_mixer::factory());
        for k in 0..4 {
            let s = add(&mut g, poly_synth::factory());
            param(&mut g, s, "filter.cutoff", 1500.0 + 300.0 * k as f32);
            for side in ["l", "r"] {
                g.connect(
                    sock(&g, s, &format!("out_{side}")),
                    sock(&g, sub, &format!("ch{}_in_{side}", k + 1)),
                )
                .unwrap();
            }
            note_targets.push((s, sock(&g, s, "notes_in").socket));
        }
        for side in ["l", "r"] {
            g.connect(
                sock(&g, sub, &format!("master_{side}")),
                sock(&g, master, &format!("ch{}_in_{side}", group + 1)),
            )
            .unwrap();
        }
    }
    g.connect(sock(&g, master, "master_l"), iface(&g, "out_1"))
        .unwrap();
    g.connect(sock(&g, master, "master_r"), iface(&g, "out_2"))
        .unwrap();
    Built {
        plan: g.compile().unwrap(),
        note_targets,
    }
}
