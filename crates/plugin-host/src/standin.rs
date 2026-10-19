//! The stand-in device of a plugin type: what stands in for a plugin that is not
//! loaded, missing or still being created. The device itself lives in the SDK
//! ([`PluginStandIn`]) because `Registry::create` builds it; this module is its host-side
//! entry point.

use atomik_device_sdk::{Device, DeviceDescriptor};

pub use atomik_device_sdk::PluginStandIn;

/// A stand-in for the plugin type described by `descriptor`: an instrument is silent, an effect
/// passes its dry main input through, a note effect emits nothing.
pub fn stand_in(descriptor: &'static DeviceDescriptor) -> Box<dyn Device> {
    Box::new(PluginStandIn::new(descriptor))
}
