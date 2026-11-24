//! Dead-end pruning through the app: with a real offline engine that prunes (as the live session
//! does), a synth whose cable into its channel is pulled is left out of the routing and runs again
//! when the cable is back; what a tap on its notes reads keeps it in.

use super::*;
use crate::offline_sink::OfflineSink;
use atomik_model::{Cable, Command, DeviceKey, SocketRef};

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn socket(d: &DeviceKey, s: &str) -> SocketRef {
    SocketRef {
        device: d.clone(),
        socket: s.to_string(),
    }
}

fn pruning_core() -> AppCore<OfflineSink> {
    let mut sink = OfflineSink::new();
    sink.prune = true;
    AppCore::new(sink)
}

#[test]
fn a_synth_whose_cable_is_pulled_is_left_out_and_runs_again_when_it_is_back() {
    let mut c = pruning_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("lead")),
    })
    .unwrap();
    let channel = c
        .doc()
        .console
        .channel_of(&key("lead"))
        .expect("automatic channel")
        .channel
        .clone();
    let lead = c.device_id(&key("lead")).unwrap();
    c.sink_mut().render(256);
    assert!(c.sink().pruned_devices().is_empty(), "cabled: it runs");
    for side in ["l", "r"] {
        c.apply_command(Command::Disconnect {
            to: socket(&channel, &format!("in_{side}")),
        })
        .unwrap();
    }
    c.sink_mut().render(256);
    assert_eq!(c.sink().pruned_devices(), vec![lead], "nothing hears it");
    for side in ["l", "r"] {
        c.apply_command(Command::Connect(Cable {
            from: socket(&key("lead"), &format!("out_{side}")),
            to: socket(&channel, &format!("in_{side}")),
        }))
        .unwrap();
    }
    c.sink_mut().render(256);
    assert!(c.sink().pruned_devices().is_empty(), "cabled again");
}

#[test]
fn a_sink_that_does_not_prune_runs_every_device() {
    let mut c = AppCore::new(OfflineSink::new());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("lead")),
    })
    .unwrap();
    let channel = c
        .doc()
        .console
        .channel_of(&key("lead"))
        .expect("automatic channel")
        .channel
        .clone();
    for side in ["l", "r"] {
        c.apply_command(Command::Disconnect {
            to: socket(&channel, &format!("in_{side}")),
        })
        .unwrap();
    }
    c.sink_mut().render(256);
    assert!(c.sink().pruned_devices().is_empty());
}
