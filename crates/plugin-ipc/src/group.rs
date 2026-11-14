//! The wire of a plugin group: one child process that hosts several plugin instances.
//!
//! A group child reads and writes **slot frames** on its stdin and stdout instead of plain
//! control frames:
//!
//! ```text
//! u32 little endian   the slot (0 is the group itself, 1 and up are the instances)
//! u32 little endian   length of the inner frame, in bytes
//! inner frame         one complete control frame (see the frame module), exactly as an
//!                     ungrouped child reads or writes it
//! ```
//!
//! Every instance of the group talks to its own `Link` through its slot, so a slot carries the
//! same Hello, the same requests and the same answers an ungrouped child does. Slot 0 carries the
//! few group messages ([`ToGroup`], [`FromGroup`]). An ungrouped child never sees a slot frame:
//! with the default setting nothing on the wire changes.

use crate::layout::{MAX_FRAME_BYTES, MAX_REPLY_JSON_BYTES};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// The slot of the group itself.
pub const GROUP_SLOT: u32 = 0;

/// Most instances one group child hosts; the next one starts another child.
pub const MAX_GROUP_INSTANCES: usize = 16;

/// The longest inner frame a slot frame may carry: a control frame whose JSON and blob are both
/// at their largest.
pub const MAX_SLOT_PAYLOAD: usize = MAX_FRAME_BYTES + MAX_REPLY_JSON_BYTES + 16;

/// Parent to child, on slot 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ToGroup {
    /// Adds an instance to the group: its slot, the plugin it hosts and the shared block and
    /// doorbell the parent made for it. The instance then greets on its own slot like an
    /// ungrouped child does.
    Attach {
        /// The slot of the instance (1 and up).
        slot: u32,
        /// The plugin file (ignored by the built-in test plugins).
        plugin_file: String,
        /// The plugin's id inside the file.
        plugin_id: String,
        /// The shared block file of the instance.
        block: String,
        /// The doorbell name of the instance.
        doorbell: String,
    },
}

/// Child to parent, on slot 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum FromGroup {
    /// The instance could not be added (the block or the doorbell would not open, no audio
    /// thread). The slot is closed.
    AttachFailed {
        /// The slot.
        slot: u32,
        /// Why.
        text: String,
    },
    /// The instance left the group (it was asked to end, or its channel broke). The slot is
    /// closed.
    Detached {
        /// The slot.
        slot: u32,
    },
}

/// Writes one slot frame. Flushes `w`.
///
/// # Errors
/// `InvalidData` for an inner frame over [`MAX_SLOT_PAYLOAD`]; the writer's own errors.
pub fn write_slot_frame(w: &mut impl Write, slot: u32, inner: &[u8]) -> io::Result<()> {
    if inner.len() > MAX_SLOT_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "a slot frame of {} bytes is over the limit of {MAX_SLOT_PAYLOAD}",
                inner.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(8 + inner.len());
    out.extend_from_slice(&slot.to_le_bytes());
    out.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    out.extend_from_slice(inner);
    w.write_all(&out)?;
    w.flush()
}

/// Reads one slot frame: the slot and the inner frame's bytes. A stream that ends inside the
/// frame or before it is `UnexpectedEof`; an inner frame over [`MAX_SLOT_PAYLOAD`] is
/// `InvalidData`. The buffer grows as bytes arrive, so a lying length cannot make the reader
/// allocate the whole limit for a stream that ends early.
///
/// # Errors
/// As above.
pub fn read_slot_frame(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let slot = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_SLOT_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a slot frame of {len} bytes is over the limit of {MAX_SLOT_PAYLOAD}"),
        ));
    }
    let mut inner = Vec::new();
    Read::take(&mut *r, len as u64).read_to_end(&mut inner)?;
    if inner.len() != len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the stream ended inside a slot frame",
        ));
    }
    Ok((slot, inner))
}

/// A writer that gathers what one frame writes and sends it as a slot frame on `flush`, so the
/// several `write_all` calls of a frame (its length and JSON, then its blob) leave as one.
#[derive(Debug)]
pub struct SlotWriter<W: Write> {
    slot: u32,
    out: W,
    pending: Vec<u8>,
}

impl<W: Write> SlotWriter<W> {
    /// A writer for `slot` that sends its frames to `out`.
    pub fn new(slot: u32, out: W) -> Self {
        Self {
            slot,
            out,
            pending: Vec::new(),
        }
    }
}

impl<W: Write> Write for SlotWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let inner = std::mem::take(&mut self.pending);
        write_slot_frame(&mut self.out, self.slot, &inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{read_frame, write_frame};
    use crate::message::ToChild;
    use std::io::Cursor;

    #[test]
    fn a_slot_frame_round_trips_with_its_slot_and_its_bytes() {
        let mut wire = Vec::new();
        write_slot_frame(&mut wire, 7, b"hello").unwrap();
        write_slot_frame(&mut wire, 0, b"").unwrap();
        let mut r = Cursor::new(wire);
        assert_eq!(read_slot_frame(&mut r).unwrap(), (7, b"hello".to_vec()));
        assert_eq!(read_slot_frame(&mut r).unwrap(), (0, Vec::new()));
        assert_eq!(
            read_slot_frame(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn a_control_frame_with_a_blob_survives_the_slot_envelope() {
        let mut inner = Vec::new();
        write_frame(&mut inner, &ToChild::RestoreState { id: 4 }, Some(b"state")).unwrap();
        let mut wire = Vec::new();
        write_slot_frame(&mut wire, 3, &inner).unwrap();
        let (slot, bytes) = read_slot_frame(&mut Cursor::new(wire)).unwrap();
        assert_eq!(slot, 3);
        let (msg, blob) = read_frame::<ToChild>(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(msg, ToChild::RestoreState { id: 4 });
        assert_eq!(blob.as_deref(), Some(&b"state"[..]));
    }

    #[test]
    fn a_slot_writer_sends_one_slot_frame_per_flush_whatever_the_write_calls() {
        let mut wire = Vec::new();
        {
            let mut w = SlotWriter::new(2, &mut wire);
            write_frame(&mut w, &ToChild::Shutdown, None).unwrap();
            write_frame(&mut w, &ToChild::Flush { id: 9 }, Some(b"xy")).unwrap();
        }
        let mut r = Cursor::new(wire);
        let (s1, a) = read_slot_frame(&mut r).unwrap();
        let (s2, b) = read_slot_frame(&mut r).unwrap();
        assert_eq!((s1, s2), (2, 2));
        assert_eq!(
            read_frame::<ToChild>(&mut Cursor::new(a)).unwrap().0,
            ToChild::Shutdown
        );
        let (msg, blob) = read_frame::<ToChild>(&mut Cursor::new(b)).unwrap();
        assert_eq!(msg, ToChild::Flush { id: 9 });
        assert_eq!(blob.as_deref(), Some(&b"xy"[..]));
        assert!(read_slot_frame(&mut r).is_err(), "nothing else was written");
    }

    #[test]
    fn a_frame_over_the_limit_is_refused_before_it_is_read_or_written() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&1u32.to_le_bytes());
        wire.extend_from_slice(&((MAX_SLOT_PAYLOAD + 1) as u32).to_le_bytes());
        assert_eq!(
            read_slot_frame(&mut Cursor::new(wire)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let big = vec![0u8; MAX_SLOT_PAYLOAD + 1];
        assert_eq!(
            write_slot_frame(&mut Vec::new(), 1, &big)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_stream_that_ends_inside_a_frame_is_an_unexpected_eof() {
        let mut wire = Vec::new();
        write_slot_frame(&mut wire, 1, b"abcdef").unwrap();
        wire.truncate(wire.len() - 2);
        assert_eq!(
            read_slot_frame(&mut Cursor::new(wire)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn the_group_messages_round_trip() {
        let attach = ToGroup::Attach {
            slot: 2,
            plugin_file: "a.clap".to_string(),
            plugin_id: "x.y".to_string(),
            block: "b".to_string(),
            doorbell: "d".to_string(),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &attach, None).unwrap();
        assert_eq!(
            read_frame::<ToGroup>(&mut Cursor::new(bytes)).unwrap().0,
            attach
        );
        for msg in [
            FromGroup::Detached { slot: 4 },
            FromGroup::AttachFailed {
                slot: 5,
                text: "no".to_string(),
            },
        ] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &msg, None).unwrap();
            assert_eq!(
                read_frame::<FromGroup>(&mut Cursor::new(bytes)).unwrap().0,
                msg
            );
        }
    }

    #[test]
    fn a_group_holds_sixteen_instances() {
        assert_eq!(MAX_GROUP_INSTANCES, 16);
    }
}
