//! Loading `examples/patches/slice-demo.v2.json` into `AppCore` with the recording sink: one
//! engine reset, inserts with ids 1..n in rack order carrying the stored params, one routing.

use atomik_app::AppCore;
use atomik_app::sink::{RecordingSink, SinkCall};
use atomik_engine::DeviceInstanceId;
use std::path::Path;

fn demo() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    ))
}

#[test]
fn slice_demo_v2_builds_the_engine_in_rack_order() {
    let mut core = AppCore::new(RecordingSink::new());
    core.sink_mut().take();
    core.open(demo()).expect("the demo opens");

    let calls = core.sink_mut().take();
    let n = core.doc().devices.len();
    assert_eq!(n, 4);
    assert!(matches!(calls[0], SinkCall::Reset { seed: 1, .. }));
    assert_eq!(
        calls.len(),
        n + 5,
        "reset, {n} inserts, one routing, then the song, loop and metronome: {calls:?}"
    );
    assert!(matches!(calls[n + 2], SinkCall::Song(_)));

    for (i, (call, device)) in calls[1..=n].iter().zip(&core.doc().devices).enumerate() {
        let SinkCall::Insert {
            id,
            type_id,
            params,
        } = call
        else {
            panic!("call {} is not an insert: {call:?}", i + 1);
        };
        assert_eq!(
            *id,
            DeviceInstanceId(i as u32 + 1),
            "ids 1..n in rack order"
        );
        assert_eq!(*type_id, device.type_id);
        let desc = core.registry().get(type_id).unwrap().descriptor;
        let expected: Vec<_> = device
            .params
            .iter()
            .map(|(k, v)| (desc.param_by_key(k).unwrap().id, *v))
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(
            params, &expected,
            "{} carries its stored params",
            device.key
        );
    }

    let SinkCall::Routing(order) = &calls[n + 1] else {
        panic!("last call is not a routing: {:?}", calls[n + 1]);
    };
    let mut ids = order.clone();
    ids.sort();
    assert_eq!(
        ids,
        (1..=n as u32).map(DeviceInstanceId).collect::<Vec<_>>()
    );
    assert_eq!(core.topology().cables().count(), core.doc().cables.len());
    assert_eq!(
        core.title(),
        "Atomik Rack \u{2014} slice-demo.v2.json",
        "clean after open"
    );
}
