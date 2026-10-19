//! Moving owned state (the engine, the input ring writer) into stream callbacks and getting it
//! back: when a callback is dropped (stream stopped, or a build attempt rejected), its payload
//! is sent back through a one-slot channel so it is never lost and never dropped on the audio
//! thread while a receiver exists.

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// Holds a value inside a callback; sends it back through `tx` when dropped.
pub(crate) struct ReturnOnDrop<T> {
    value: Option<T>,
    tx: SyncSender<T>,
}

impl<T> ReturnOnDrop<T> {
    /// The held value (always `Some` until drop).
    pub(crate) fn get_mut(&mut self) -> Option<&mut T> {
        self.value.as_mut()
    }
}

impl<T> Drop for ReturnOnDrop<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            // One slot per payload: never blocks. If the receiver is gone the value is dropped
            // here (only when the owner itself was dropped without stopping).
            let _ = self.tx.try_send(value);
        }
    }
}

/// Builds with a fixed buffer size of `fixed` frames first; if that is rejected, recovers the
/// payload from the dropped callback and retries with the host default. Returns the stream, the
/// fixed size if it was accepted, and the receiver the payload comes back through when the
/// stream's callback is dropped.
pub(crate) fn build_with_fallback<P, S, E: std::fmt::Display>(
    payload: P,
    fixed: u32,
    mut build: impl FnMut(Option<u32>, ReturnOnDrop<P>) -> Result<S, E>,
) -> Result<(S, Option<u32>, Receiver<P>), String> {
    let (tx, rx) = sync_channel(1);
    let first = ReturnOnDrop {
        value: Some(payload),
        tx: tx.clone(),
    };
    let fixed_error = match build(Some(fixed), first) {
        Ok(stream) => return Ok((stream, Some(fixed), rx)),
        Err(e) => e.to_string(),
    };
    let payload = rx.try_recv().map_err(|_| {
        format!("fixed buffer of {fixed} frames rejected ({fixed_error}) and the backend kept the callback")
    })?;
    let second = ReturnOnDrop {
        value: Some(payload),
        tx,
    };
    match build(None, second) {
        Ok(stream) => Ok((stream, None, rx)),
        Err(e) => Err(format!(
            "fixed buffer of {fixed} frames: {fixed_error}; default buffer: {e}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake "stream" that owns its callback payload, like cpal does.
    struct FakeStream(#[allow(dead_code)] ReturnOnDrop<String>);

    #[test]
    fn return_on_drop_sends_the_value_back() {
        let (stream, fixed, rx) = build_with_fallback("engine".to_string(), 128, |size, cb| {
            assert_eq!(size, Some(128));
            Ok::<_, String>(FakeStream(cb))
        })
        .unwrap();
        assert_eq!(fixed, Some(128));
        assert!(rx.try_recv().is_err(), "still owned by the stream");
        drop(stream);
        assert_eq!(rx.try_recv().unwrap(), "engine");
    }

    #[test]
    fn rejected_fixed_size_falls_back_to_default_with_the_same_payload() {
        let mut sizes = Vec::new();
        let (mut stream, fixed, rx) = build_with_fallback("engine".to_string(), 64, |size, cb| {
            sizes.push(size);
            match size {
                Some(_) => {
                    drop(cb); // the backend drops the rejected callback
                    Err("fixed size rejected".to_string())
                }
                None => Ok(FakeStream(cb)),
            }
        })
        .unwrap();
        assert_eq!(sizes, [Some(64), None]);
        assert_eq!(fixed, None);
        assert_eq!(stream.0.get_mut().map(|s| s.as_str()), Some("engine"));
        drop(stream);
        assert_eq!(rx.try_recv().unwrap(), "engine");
    }

    #[test]
    fn both_attempts_failing_reports_both_errors() {
        let err = build_with_fallback("engine".to_string(), 64, |size, _cb| {
            Err::<FakeStream, _>(format!("nope {size:?}"))
        })
        .err()
        .unwrap();
        assert!(
            err.contains("nope Some(64)") && err.contains("nope None"),
            "{err}"
        );
    }

    #[test]
    fn dropping_after_the_receiver_is_gone_drops_the_value_in_place() {
        let (stream, _, rx) = build_with_fallback(5u8, 1, |_, cb| Ok::<_, String>(cb)).unwrap();
        drop(rx);
        drop(stream); // must not panic
    }
}
