//! CV activity: the UI reads the source endpoint of every CV cable through the lock-free meter
//! bank. A cable with signal reads its peak, a silent one reads zero, reading is allocation-free
//! and metering never changes what the engine renders.

mod common;

use atomik_core::BATCH;
use atomik_engine::{DeviceInstanceId, Topology, new_engine};
use atomik_testkit::fakes::{CONSTANT_SOURCE, ConstantSource, SUMMING_BUS, SummingBus};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::{cfg, ep, render_block};

atomik_testkit::install_alloc_guard!();

const LOUD: DeviceInstanceId = DeviceInstanceId(1);
const QUIET: DeviceInstanceId = DeviceInstanceId(2);
const BUS: DeviceInstanceId = DeviceInstanceId(3);

/// Two CV sources (0.5 and 0.0) cabled into one bus; the audio of the loud one goes to out_1.
fn topology(metered: bool) -> (atomik_engine::Engine, atomik_engine::EngineHandle) {
    let c = cfg(0, 1);
    let (engine, mut handle) = new_engine(c);
    let mut t = Topology::new(&c);
    for (id, v) in [(LOUD, 0.5), (QUIET, 0.0)] {
        handle
            .insert_device(id, ConstantSource::boxed(v), &[])
            .unwrap();
        t.add_node(id, &CONSTANT_SOURCE).unwrap();
    }
    handle.insert_device(BUS, SummingBus::boxed(), &[]).unwrap();
    t.add_node(BUS, &SUMMING_BUS).unwrap();
    for id in [LOUD, QUIET] {
        t.connect(
            ep(id, ConstantSource::CV_OUT),
            ep(BUS, SummingBus::CV_SUM_IN),
        )
        .unwrap();
        if metered {
            t.add_meter(ep(id, ConstantSource::CV_OUT)).unwrap();
        }
    }
    t.connect(
        ep(LOUD, ConstantSource::AUDIO_OUT),
        ep(
            atomik_engine::Graph::INTERFACE,
            atomik_device_sdk::SocketId(0),
        ),
    )
    .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    (engine, handle)
}

#[test]
fn cable_source_reads_its_peak_and_a_silent_cable_reads_zero() {
    let (mut engine, handle) = topology(true);
    render_block(&mut engine, &[], 1, BATCH);
    let meters = handle.meters();
    let loud = assert_no_alloc(|| meters.read(ep(LOUD, ConstantSource::CV_OUT))).unwrap();
    let quiet = assert_no_alloc(|| meters.read(ep(QUIET, ConstantSource::CV_OUT))).unwrap();
    assert_eq!(loud.peak.to_bits(), 0.5f32.to_bits());
    assert_eq!(quiet.peak.to_bits(), 0.0f32.to_bits());
    // The peak is spent by the read; the next batch publishes it again.
    render_block(&mut engine, &[], 1, BATCH);
    let again = meters.read(ep(LOUD, ConstantSource::CV_OUT)).unwrap();
    assert_eq!(again.peak.to_bits(), 0.5f32.to_bits());
}

#[test]
fn rendering_with_cv_meters_is_allocation_free_and_bit_identical_to_without() {
    let (mut with, _h1) = topology(true);
    let (mut without, _h2) = topology(false);
    for _ in 0..4 {
        let a = render_block(&mut with, &[], 1, BATCH);
        let b = render_block(&mut without, &[], 1, BATCH);
        assert_eq!(common::bits(&a), common::bits(&b));
    }
}
