//! A device's `ScopeTap` is registered with the engine when the device is inserted or its plan
//! is submitted, and forgotten when it is removed or the plan replaced; the UI reads the frames
//! the device pushed, lock-free, through the handle or the shared `Meters`. Own binary: the
//! worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{BATCH, ParamId};
use atomik_device_sdk::{Device, DeviceDescriptor, PrepareInfo, ProcessContext, ScopeTap};
use atomik_engine::{DeviceInstanceId, Graph, new_engine_with_arena};
use atomik_testkit::fakes::CONSTANT_SOURCE;
use common::*;
use std::sync::Arc;

/// Pushes `(n, -n)` for frame `n` into its tap, like a metering device would.
struct Tapper {
    tap: Arc<ScopeTap>,
    frame: usize,
}

impl Tapper {
    fn boxed() -> (Box<dyn Device>, Arc<ScopeTap>) {
        let tap = Arc::new(ScopeTap::new());
        let device = Box::new(Self {
            tap: Arc::clone(&tap),
            frame: 0,
        });
        (device, tap)
    }
}

impl Device for Tapper {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &CONSTANT_SOURCE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _ctx: &mut ProcessContext<'_>) {
        let l: [f32; BATCH] = std::array::from_fn(|i| (self.frame + i) as f32);
        let r: [f32; BATCH] = std::array::from_fn(|i| -((self.frame + i) as f32));
        self.tap.push(&l, &r);
        self.frame += BATCH;
    }
    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }
}

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn route(handle: &mut atomik_engine::EngineHandle, n: u32) {
    let c = cfg(0, 2);
    let mut t = atomik_engine::Topology::new(&c);
    t.add_node(id(n), &CONSTANT_SOURCE).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
}

#[test]
fn an_inserted_device_hands_its_tap_to_the_meter_bank_and_loses_it_on_removal() {
    let (mut engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let (device, own) = Tapper::boxed();
    assert!(handle.device_scope_tap(id(1)).is_none());
    handle.insert_device(id(1), device, &[]).unwrap();
    let tap = handle
        .device_scope_tap(id(1))
        .expect("registered at insert");
    assert!(Arc::ptr_eq(&tap, &own), "the device's own tap, not a copy");
    assert!(Arc::ptr_eq(
        &handle.meters().scope_tap(id(1)).unwrap(),
        &own
    ));
    route(&mut handle, 1);
    render_blocks(&mut engine, &[], 2, 10 * BATCH, 64);
    let (mut l, mut r) = (vec![0.0; 100], vec![0.0; 100]);
    let (n, head) = tap.read_latest(&mut l, &mut r);
    assert_eq!((n, head), (100, (10 * BATCH) as u64));
    for (i, (x, y)) in l.iter().zip(&r).enumerate() {
        let frame = 10 * BATCH - 100 + i;
        assert_eq!(x.to_bits(), (frame as f32).to_bits());
        assert_eq!(y.to_bits(), (-(frame as f32)).to_bits());
    }
    // Removal needs the routing without it.
    handle
        .submit_routing(atomik_engine::Topology::new(&cfg(0, 2)).compile().unwrap())
        .unwrap();
    handle.remove_device(id(1)).unwrap();
    assert!(handle.device_scope_tap(id(1)).is_none());
}

#[test]
fn a_plan_registers_its_taps_and_replaces_the_old_ones() {
    let (mut engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let mut g = Graph::new(&cfg(0, 2));
    let (device, own) = Tapper::boxed();
    let k = g.add_device(device);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let tap = handle.device_scope_tap(k).expect("registered by the plan");
    assert!(Arc::ptr_eq(&tap, &own));
    render_blocks(&mut engine, &[], 2, 4 * BATCH, 64);
    assert_eq!(tap.head(), (4 * BATCH) as u64);
    // A plan without it forgets the tap.
    handle
        .submit_plan(Graph::new(&cfg(0, 2)).compile().unwrap())
        .unwrap();
    assert!(handle.device_scope_tap(k).is_none());
}

#[test]
fn a_device_without_a_tap_registers_nothing() {
    let (_engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let mut g = Graph::new(&cfg(0, 2));
    let k = g.add_device(atomik_testkit::fakes::ConstantSource::boxed(0.5));
    handle.submit_plan(g.compile().unwrap()).unwrap();
    assert!(handle.device_scope_tap(k).is_none());
}

#[test]
fn an_on_demand_tap_asks_the_allocator_for_nothing_until_it_is_looked_at() {
    use atomik_testkit::no_alloc::{allocations_of, assert_no_alloc, peak_heap_of};
    let (tap, bytes) = peak_heap_of(|| Arc::new(ScopeTap::on_demand()));
    assert!(bytes < 1024, "{bytes} bytes for a header");
    let (eager, eager_bytes) = peak_heap_of(|| Arc::new(ScopeTap::new()));
    assert!(eager_bytes >= 2 * 4 * atomik_device_sdk::SCOPE_FRAMES);
    drop(eager);
    // A device that writes while nobody asked: nothing is allocated, the head moves.
    let (l, r) = ([0.5f32; BATCH], [-0.5f32; BATCH]);
    assert_no_alloc(|| {
        for _ in 0..8 {
            tap.push(&l, &r);
        }
    });
    assert_eq!(tap.head(), (8 * BATCH) as u64);
    assert!(!tap.has_ring());
    // Looking at it makes the ring once, here (not the writer); asking again costs nothing.
    let ((), made) = allocations_of(|| tap.set_live(true));
    assert!(made > 0, "the ring is made on the asking thread");
    let ((), again) = allocations_of(|| {
        tap.set_live(true);
        tap.reserve();
    });
    assert_eq!(again, 0, "once");
    // From here the writer fills it without allocating, and a read sees only the new frames.
    assert_no_alloc(|| tap.push(&l, &r));
    let (mut a, mut b) = (vec![0.0; 256], vec![0.0; 256]);
    assert_eq!(tap.read_latest(&mut a, &mut b), (BATCH, (9 * BATCH) as u64));
    assert!(
        a[..BATCH].iter().all(|v| v.to_bits() == 0.5f32.to_bits())
            && b[..BATCH]
                .iter()
                .all(|v| v.to_bits() == (-0.5f32).to_bits())
    );
}

#[test]
fn an_on_demand_tap_of_a_device_in_the_engine_is_written_after_the_ring_appears() {
    struct Lazy(Arc<ScopeTap>);
    impl Device for Lazy {
        fn descriptor(&self) -> &'static DeviceDescriptor {
            &CONSTANT_SOURCE
        }
        fn prepare(&mut self, _: &PrepareInfo) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: ParamId, _: f32) {}
        fn process(&mut self, _ctx: &mut ProcessContext<'_>) {
            // Like a console strip: writes only while something looks.
            if self.0.is_live() {
                self.0.push(&[1.0; BATCH], &[1.0; BATCH]);
            }
        }
        fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
            Some(Arc::clone(&self.0))
        }
    }
    let (mut engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let own = Arc::new(ScopeTap::on_demand());
    handle
        .insert_device(id(1), Box::new(Lazy(Arc::clone(&own))), &[])
        .unwrap();
    route(&mut handle, 1);
    let tap = handle.device_scope_tap(id(1)).unwrap();
    render_blocks(&mut engine, &[], 2, 4 * BATCH, 64);
    let (mut a, mut b) = (vec![0.0; 64], vec![0.0; 64]);
    assert_eq!(
        tap.read_latest(&mut a, &mut b),
        (0, 0),
        "nobody looked: nothing"
    );
    tap.set_live(true);
    render_blocks(&mut engine, &[], 2, 4 * BATCH, 64);
    assert_eq!(tap.read_latest(&mut a, &mut b), (64, (4 * BATCH) as u64));
    assert!(a.iter().all(|v| v.to_bits() == 1.0f32.to_bits()));
}
