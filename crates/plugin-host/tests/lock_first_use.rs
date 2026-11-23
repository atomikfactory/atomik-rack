//! HI-1 for the locks the audio thread may take: the first lock must not allocate. `std`'s
//! mutex on Apple targets boxes its pthread mutex on the first lock, so on macOS these fail
//! unless the mutex was warmed when it was created.

atomik_testkit::install_alloc_guard!();

use atomik_plugin_host::{InstanceId, PluginMessage, plugin_channel, warmed};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::sync::Mutex;

#[test]
fn a_warmed_mutex_does_not_allocate_on_its_first_real_lock() {
    let m = warmed(Mutex::new(0u32));
    assert_no_alloc(|| *m.lock().unwrap() += 1);
    assert_eq!(*m.lock().unwrap(), 1);
}

#[test]
fn the_first_post_of_a_host_callback_does_not_allocate() {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let messenger = manager.messenger();
    // A fresh channel: this is the first lock of its producer, taken inside the guard.
    let posted = assert_no_alloc(|| messenger.post(PluginMessage::StateDirty));
    assert!(posted);
}

#[test]
fn the_first_post_from_inside_process_does_not_allocate() {
    use atomik_plugin_host::AudioThreadGuard;
    let (_device, manager) = plugin_channel(InstanceId(2));
    let messenger = manager.messenger();
    let _audio = AudioThreadGuard::enter();
    // The ring's producer is the mutex the audio path locks: warmed when the channel was made.
    let posted = assert_no_alloc(|| {
        messenger.post(PluginMessage::GuiClosed)
            && messenger.post(PluginMessage::ParamEdit {
                id: 1,
                value: 0.5,
                phase: atomik_plugin_host::ParamPhase::Change,
            })
    });
    assert!(posted);
}
