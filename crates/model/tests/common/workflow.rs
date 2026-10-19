//! Helpers of the workflow command tests: the saved text of a document, wrappers,
//! and the rejection check.

use super::*;
use atomik_model::{ModelError, WorkflowCommand, save_project};

/// The canonical saved text of a document; undo and redo must reproduce it byte for byte.
pub fn text(d: &Document) -> String {
    save_project(d.doc())
}

pub fn wf(c: WorkflowCommand) -> Command {
    Command::Workflow(c)
}

pub fn keys_of(list: &[&str]) -> Vec<DeviceKey> {
    list.iter().map(|k| key(k)).collect()
}

/// Applies `c` and expects a rejection that changes nothing (not the text, not the history).
pub fn rejected(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = text(d);
    let (undo, redo) = (d.can_undo(), d.can_redo());
    let e = d.apply(c, r).expect_err("the command must be rejected");
    assert_eq!(text(d), before, "a rejected command changes nothing");
    assert_eq!((d.can_undo(), d.can_redo()), (undo, redo));
    e
}

/// Applies `c`, checks that it is exactly one undo step (undo returns to the saved text of
/// before, redo to the text of after), and returns the sync ops of the first application.
pub fn one_step(d: &mut Document, r: &Registry, c: Command) -> Vec<SyncOp> {
    let before = text(d);
    let ops = d
        .apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = text(d);
    assert_ne!(before, after, "the command changed something");
    d.undo(r).expect("one step to undo");
    assert_eq!(text(d), before, "undo restores the saved text");
    d.redo(r).expect("one step to redo");
    assert_eq!(text(d), after, "redo restores the saved text");
    ops
}
