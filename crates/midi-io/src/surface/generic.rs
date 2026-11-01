//! The generic controller: a surface with no fixed layout. Its controls are whatever the user
//! maps (MIDI learn), so the driver itself shows nothing and does nothing with a message; the
//! app hands the messages of a generic surface to the mapping engine, tagged with the surface.

use super::{SurfaceAction, SurfaceDriver, SurfaceState};

/// A controller whose controls feed the MIDI mappings.
#[derive(Debug, Default, Clone, Copy)]
pub struct Generic;

impl Generic {
    /// A generic controller driver.
    pub fn new() -> Self {
        Self
    }
}

impl SurfaceDriver for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn feeds_mappings(&self) -> bool {
        true
    }

    fn connect(&mut self, _state: &SurfaceState, _out: &mut Vec<Vec<u8>>) {}

    fn on_message(
        &mut self,
        _bytes: &[u8],
        _state: &SurfaceState,
        _actions: &mut Vec<SurfaceAction>,
        _replies: &mut Vec<Vec<u8>>,
    ) {
    }

    fn feedback(&mut self, _state: &SurfaceState, _out: &mut Vec<Vec<u8>>) {}

    fn disconnect(&mut self, _out: &mut Vec<Vec<u8>>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{DRIVERS, driver_by_id};

    #[test]
    fn the_generic_controller_sends_nothing_and_leaves_messages_to_the_mappings() {
        let mut d = Generic::new();
        let state = SurfaceState::default();
        let (mut out, mut actions, mut replies) = (Vec::new(), Vec::new(), Vec::new());
        d.connect(&state, &mut out);
        d.on_message(&[0xB0, 7, 99], &state, &mut actions, &mut replies);
        d.feedback(&state, &mut out);
        d.disconnect(&mut out);
        assert!(out.is_empty() && actions.is_empty() && replies.is_empty());
        assert!(d.feeds_mappings());
        assert_eq!(d.name(), "generic");
    }

    #[test]
    fn the_driver_list_offers_it_and_the_motor_surface_does_not_feed_mappings() {
        assert!(DRIVERS.iter().any(|(id, _)| *id == "generic"));
        assert!(driver_by_id("generic").is_some_and(|d| d.feeds_mappings()));
        assert!(driver_by_id("mcu").is_some_and(|d| !d.feeds_mappings()));
    }
}
