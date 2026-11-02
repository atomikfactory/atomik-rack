//! Descriptor contracts a device author can break: the stage table, and what the registry
//! hands out for a plugin type.

use atomik_core::{BATCH, NoteBuffer, SampleRate, Tempo, TimeInfo};
use atomik_device_sdk::{
    Channels, DescriptorError, DeviceDescriptor, DeviceKind, Direction, PluginFormat,
    PluginKindHint, PluginLayout, PluginRef, ProcessContext, Registry, SocketDescriptor, SocketId,
    SocketKind, descriptor_for,
};

const fn socket(id: u16, key: &'static str, direction: Direction) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind: SocketKind::Audio,
        direction,
    }
}

const SOCKETS: &[SocketDescriptor] = &[
    socket(0, "in", Direction::Input),
    socket(1, "mid", Direction::Output),
    socket(2, "out", Direction::Output),
];

const fn with_stages(stages: &'static [u8]) -> DeviceDescriptor {
    DeviceDescriptor {
        type_id: "test.stages",
        name: "Stages",
        kind: DeviceKind::Effect,
        version: 1,
        params: &[],
        sockets: SOCKETS,
        stages,
    }
}

#[test]
fn consecutive_stages_and_single_stage_devices_are_accepted() {
    assert_eq!(with_stages(&[]).validate(), Ok(()));
    assert_eq!(with_stages(&[0, 0, 0]).validate(), Ok(()));
    assert_eq!(with_stages(&[0, 1, 2]).validate(), Ok(()));
    assert_eq!(with_stages(&[1, 0, 1]).validate(), Ok(()));
    assert_eq!(with_stages(&[0, 1, 2]).stage_count(), 3);
}

#[test]
fn a_lone_stage_above_zero_is_refused() {
    assert_eq!(
        with_stages(&[7, 7, 7]).validate(),
        Err(DescriptorError::StageGap(0))
    );
}

#[test]
fn a_skipped_stage_is_refused() {
    assert_eq!(
        with_stages(&[0, 2, 2]).validate(),
        Err(DescriptorError::StageGap(1))
    );
}

#[test]
fn stage_255_is_refused_and_does_not_overflow_the_count() {
    let d = with_stages(&[0, 1, 255]);
    assert_eq!(d.validate(), Err(DescriptorError::StageOverflow));
    assert_eq!(
        d.stage_count(),
        255,
        "the count saturates instead of overflowing"
    );
}

#[test]
fn stage_254_is_the_last_one_allowed_but_needs_every_stage_below_it() {
    const LONG: &[SocketDescriptor] = &[socket(0, "a", Direction::Input)];
    let one = DeviceDescriptor {
        sockets: LONG,
        stages: &[254],
        ..with_stages(&[])
    };
    assert_eq!(one.validate(), Err(DescriptorError::StageGap(0)));
}

#[test]
fn the_registry_does_not_hand_out_a_constructor_for_a_plugin_type() {
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        id: "contract.plugin".into(),
    };
    let layout = PluginLayout {
        name: "Contract".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: false,
        notes_out: false,
        kind_hint: PluginKindHint::Effect,
        params: Vec::new(),
    };
    let descriptor = descriptor_for(&plugin, &layout).unwrap();
    let mut registry = Registry::new();
    registry
        .register_plugin(plugin.type_id(), descriptor)
        .unwrap();
    let entry = registry.get(&plugin.type_id()).unwrap();
    assert!(std::ptr::eq(entry.descriptor, descriptor));
    assert!(registry.factory(&plugin.type_id()).is_none());
    // The instance is the stand-in of the right descriptor, with its sockets.
    let device = registry.create(&plugin.type_id()).unwrap();
    assert!(std::ptr::eq(device.descriptor(), descriptor));
    assert!(!device.descriptor().sockets.is_empty());
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "outside the device's 2 sockets")]
fn a_socket_id_outside_the_descriptor_is_named_in_debug_builds() {
    let mut signals = vec![[0.0; BATCH]; 2];
    let mut notes = vec![NoteBuffer::with_capacity(0), NoteBuffer::with_capacity(0)];
    let connected = [false, false];
    let time = TimeInfo::at(
        0,
        SampleRate::new(48_000.0).unwrap(),
        Tempo::new(120.0).unwrap(),
        true,
    );
    let ctx = ProcessContext::new(time, &mut signals, &mut notes, &connected);
    let _ = ctx.input(SocketId(5));
}
