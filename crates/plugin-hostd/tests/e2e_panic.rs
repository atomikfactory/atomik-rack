//! A plugin that panics on the child's audio thread: the app gets the panic's message, place and
//! operation as a log line, and the device reports FAILED.

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{DeviceState, LogLevel, ManagerEvent};
use atomik_plugin_ipc::PANIC_LOG_PREFIX;
use sandbox_support::{METER_STATE, Sandbox, config, effect, meters, null_plugin, request};
use std::time::Duration;

#[test]
fn a_panic_of_the_plugin_reaches_the_manager_with_message_place_and_operation_and_fails_the_device()
{
    let mut sb = Sandbox::null(config());
    let handle = sb
        .manager
        .instantiate(&request(&null_plugin("null.panic"), RunMode::Realtime))
        .unwrap();
    let mut h = effect(handle, 0.25);
    // About a minute at most.
    let mut tries = 0;
    let mut events = Vec::new();
    let line = loop {
        h.process();
        events.extend(sb.manager.poll());
        let found = events.iter().find_map(|e| match e {
            ManagerEvent::Log { level, text, .. } if text.starts_with(PANIC_LOG_PREFIX) => {
                Some((*level, text.clone()))
            }
            _ => None,
        });
        if let Some(found) = found {
            break found;
        }
        tries += 1;
        assert!(tries < 30_000, "no panic line: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(line.0, LogLevel::Warning);
    let text = line.1;
    assert!(
        text.starts_with(
            "panicked in the plugin host during Process: null.panic panics on purpose at "
        ),
        "{text}"
    );
    assert!(text.contains("builtin.rs:"), "the place is named: {text}");
    // The device failed because of the error status the panic became.
    for _ in 0..50 {
        h.process();
    }
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Failed.as_f32());
    drop(h);
    sb.manager.poll();
    sb.finish();
}
