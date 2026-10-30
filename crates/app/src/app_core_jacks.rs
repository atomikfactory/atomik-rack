//! The choices of a jack's pop-up menu ([`atomik_ui::JackMenu`]) in the app core: a cable, a
//! replacement, a disconnect or a new mixer channel is one command and so one undo step; a
//! scroll to a connected device only moves the rack view.

use super::*;
use atomik_ui::JackOp;

impl<S: EngineSink> AppCore<S> {
    /// Does what the jack menu entry `op` (the `rack.jack` argument, see [`JackOp::encode`])
    /// says. Returns whether it did something; a refusal of the model is shown in the view.
    pub fn jack_op(&mut self, op: &str) -> bool {
        let Some(op) = JackOp::parse(op) else {
            return false;
        };
        if let JackOp::Reveal(key) = &op {
            return self.view.reveal_key(key);
        }
        match op.command(self.doc()) {
            Some(command) => self.apply_command(command).is_ok(),
            None => false,
        }
    }
}
