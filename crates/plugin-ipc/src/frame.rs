//! Control-channel framing: one message per frame on the child's stdin and stdout.
//!
//! ```text
//! u32 little endian   length of the JSON, in bytes
//! JSON                one object (serde_json)
//! blob                only when the JSON has a numeric "blob" field: exactly that many bytes
//! ```
//!
//! The writer sets `"blob"` itself when it is given a payload. Both the JSON and the blob are
//! limited to [`MAX_FRAME_BYTES`], checked before anything is allocated on the reading side.

use crate::layout::MAX_FRAME_BYTES;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::{self, Read, Write};

fn invalid_data(text: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text.into())
}

/// Writes one frame: `msg` as JSON, and `blob` after it when given (the `"blob"` field of the
/// JSON is set to its length). The message must serialise to a JSON object to carry a blob
/// (`InvalidInput` otherwise); a JSON or a blob over [`MAX_FRAME_BYTES`] is `InvalidData`.
/// Flushes `w`.
pub fn write_frame(
    w: &mut impl Write,
    msg: &impl Serialize,
    blob: Option<&[u8]>,
) -> io::Result<()> {
    let mut value = serde_json::to_value(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    if let Some(blob) = blob {
        if blob.len() > MAX_FRAME_BYTES {
            return Err(invalid_data(format!(
                "a frame blob of {} bytes is over the limit of {MAX_FRAME_BYTES}",
                blob.len()
            )));
        }
        let Some(object) = value.as_object_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a message that is a JSON object can carry a blob",
            ));
        };
        object.insert("blob".to_string(), Value::from(blob.len() as u64));
    }
    let json = serde_json::to_vec(&value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    if json.len() > MAX_FRAME_BYTES {
        return Err(invalid_data(format!(
            "a frame of {} bytes is over the limit of {MAX_FRAME_BYTES}",
            json.len()
        )));
    }
    // One write for the length and the JSON: one system call on a pipe.
    let mut head = Vec::with_capacity(4 + json.len());
    head.extend_from_slice(&(json.len() as u32).to_le_bytes());
    head.extend_from_slice(&json);
    w.write_all(&head)?;
    if let Some(blob) = blob {
        w.write_all(blob)?;
    }
    w.flush()
}

/// Reads one frame. A stream that ends anywhere inside the frame, or before it, is
/// `UnexpectedEof` (at a frame boundary that is the peer closing the channel); a frame over
/// [`MAX_FRAME_BYTES`], malformed JSON or JSON that is not a `T` is `InvalidData`.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<(T, Option<Vec<u8>>)> {
    read_frame_with(r, MAX_FRAME_BYTES, |_| MAX_FRAME_BYTES)
}

/// Reads one frame with tighter limits: the JSON may be at most `json_limit` bytes, and
/// `blob_limit` says, from the decoded message, how long its blob may be (0: none). Both are
/// checked before the bytes are allocated, so a hostile peer cannot make the reader grow past
/// what its message kind may carry.
pub fn read_frame_with<T: DeserializeOwned>(
    r: &mut impl Read,
    json_limit: usize,
    blob_limit: impl FnOnce(&T) -> usize,
) -> io::Result<(T, Option<Vec<u8>>)> {
    let json_limit = json_limit.min(MAX_FRAME_BYTES);
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > json_limit {
        return Err(invalid_data(format!(
            "a frame of {len} bytes is over the limit of {json_limit}"
        )));
    }
    let mut json = vec![0u8; len];
    r.read_exact(&mut json)?;
    let mut value: Value =
        serde_json::from_slice(&json).map_err(|e| invalid_data(format!("bad frame: {e}")))?;

    let blob_len = match value.get("blob") {
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .ok_or_else(|| invalid_data("a frame blob length is not a size"))?,
        ),
        _ => None,
    };
    if blob_len.is_some()
        && let Some(object) = value.as_object_mut()
    {
        object.remove("blob");
    }
    let msg: T =
        serde_json::from_value(value).map_err(|e| invalid_data(format!("bad frame: {e}")))?;
    let blob = match blob_len {
        None => None,
        Some(n) => {
            let limit = blob_limit(&msg).min(MAX_FRAME_BYTES);
            if n > limit as u64 {
                return Err(invalid_data(format!(
                    "a frame blob of {n} bytes is over the limit of {limit}"
                )));
            }
            // `take` grows the buffer as bytes arrive, so a lying length cannot make us
            // allocate the whole limit for a stream that ends early.
            let mut buf = Vec::new();
            Read::take(&mut *r, n).read_to_end(&mut buf)?;
            if buf.len() as u64 != n {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a frame blob",
                ));
            }
            Some(buf)
        }
    };
    Ok((msg, blob))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::io::{Cursor, ErrorKind};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Msg {
        t: String,
        id: u64,
    }

    fn msg(id: u64) -> Msg {
        Msg {
            t: "Ping".to_string(),
            id,
        }
    }

    fn write(m: &Msg, blob: Option<&[u8]>) -> Vec<u8> {
        let mut out = Vec::new();
        write_frame(&mut out, m, blob).unwrap();
        out
    }

    #[test]
    fn a_frame_is_a_little_endian_length_then_the_json() {
        let bytes = write(&msg(7), None);
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + len);
        let json: serde_json::Value = serde_json::from_slice(&bytes[4..]).unwrap();
        assert_eq!(json["t"], "Ping");
        assert_eq!(json["id"], 7);
        assert!(json.get("blob").is_none());
    }

    #[test]
    fn a_frame_without_a_blob_round_trips() {
        let bytes = write(&msg(7), None);
        let (back, blob): (Msg, _) = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(back, msg(7));
        assert_eq!(blob, None);
    }

    #[test]
    fn a_frame_with_a_blob_names_its_length_and_carries_the_bytes() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let bytes = write(&msg(1), Some(&payload));
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let json: serde_json::Value = serde_json::from_slice(&bytes[4..4 + len]).unwrap();
        assert_eq!(json["blob"], 5000);
        assert_eq!(bytes.len(), 4 + len + 5000);
        let (back, blob): (Msg, _) = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(back, msg(1));
        assert_eq!(blob.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn an_empty_blob_is_still_a_blob() {
        let bytes = write(&msg(1), Some(&[]));
        let (_, blob): (Msg, _) = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(blob, Some(Vec::new()));
    }

    #[test]
    fn frames_follow_each_other_on_one_stream() {
        let mut bytes = write(&msg(1), Some(&[1, 2, 3]));
        bytes.extend(write(&msg(2), None));
        bytes.extend(write(&msg(3), Some(&[9])));
        let mut r = Cursor::new(bytes);
        let (a, ab): (Msg, _) = read_frame(&mut r).unwrap();
        let (b, bb): (Msg, _) = read_frame(&mut r).unwrap();
        let (c, cb): (Msg, _) = read_frame(&mut r).unwrap();
        assert_eq!((a.id, ab), (1, Some(vec![1, 2, 3])));
        assert_eq!((b.id, bb), (2, None));
        assert_eq!((c.id, cb), (3, Some(vec![9])));
        let end = read_frame::<Msg>(&mut r).unwrap_err();
        assert_eq!(end.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_truncated_frame_is_an_unexpected_eof_wherever_it_stops() {
        let full = write(&msg(1), Some(&[1, 2, 3, 4]));
        for cut in [0, 2, 4, 6, full.len() - 2, full.len() - 1] {
            let err = read_frame::<Msg>(&mut Cursor::new(full[..cut].to_vec())).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[test]
    fn a_json_longer_than_the_limit_is_refused_before_it_is_read() {
        let mut bytes = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        bytes.extend(b"{}");
        let err = read_frame::<Msg>(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn a_blob_longer_than_the_limit_is_refused_by_the_reader() {
        let json = format!(r#"{{"t":"Ping","id":1,"blob":{}}}"#, MAX_FRAME_BYTES + 1);
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json.as_bytes());
        let err = read_frame::<Msg>(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn a_blob_longer_than_the_limit_is_refused_by_the_writer() {
        let big = vec![0u8; MAX_FRAME_BYTES + 1];
        let err = write_frame(&mut Vec::new(), &msg(1), Some(&big)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn a_blob_at_exactly_the_limit_is_accepted() {
        let exact = vec![7u8; MAX_FRAME_BYTES];
        let mut out = Vec::new();
        write_frame(&mut out, &msg(1), Some(&exact)).unwrap();
        let (_, blob): (Msg, _) = read_frame(&mut Cursor::new(out)).unwrap();
        assert_eq!(blob.map(|b| b.len()), Some(MAX_FRAME_BYTES));
    }

    #[test]
    fn a_json_that_is_not_a_message_is_invalid_data() {
        for text in [
            "not json",
            r#"{"t":"Ping"}"#,
            "[1,2]",
            r#"{"t":"Ping","id":"x"}"#,
        ] {
            let mut bytes = (text.len() as u32).to_le_bytes().to_vec();
            bytes.extend(text.as_bytes());
            let err = read_frame::<Msg>(&mut Cursor::new(bytes)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidData, "{text}");
        }
    }

    #[test]
    fn a_message_that_is_not_an_object_cannot_carry_a_blob() {
        let err = write_frame(&mut Vec::new(), &[1, 2, 3], Some(&[1])).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn a_blob_field_that_is_not_a_number_is_not_a_blob() {
        let text = r#"{"t":"Ping","id":1,"blob":"yes"}"#;
        let mut bytes = (text.len() as u32).to_le_bytes().to_vec();
        bytes.extend(text.as_bytes());
        let (m, blob): (Msg, _) = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(m.id, 1);
        assert_eq!(blob, None);
    }

    #[test]
    fn a_json_over_the_tighter_limit_is_refused_before_it_is_read() {
        let bytes = write(&msg(1), None);
        let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let err =
            read_frame_with::<Msg>(&mut Cursor::new(bytes.clone()), len - 1, |_| 0).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        // Only the four length bytes were needed to refuse it.
        let mut r = Cursor::new(bytes);
        let _ = read_frame_with::<Msg>(&mut r, len - 1, |_| 0);
        assert_eq!(r.position(), 4);
        let (ok, _) =
            read_frame_with::<Msg>(&mut Cursor::new(write(&msg(1), None)), len, |_| 0).unwrap();
        assert_eq!(ok, msg(1));
    }

    #[test]
    fn the_blob_limit_comes_from_the_message_and_is_checked_before_the_blob_is_read() {
        let bytes = write(&msg(1), Some(&[0u8; 100]));
        let mut r = Cursor::new(bytes.clone());
        let err =
            read_frame_with::<Msg>(&mut r, 1 << 20, |m| usize::from(m.id == 2) * 1000).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert!(
            (r.position() as usize) < bytes.len() - 99,
            "the blob was not read"
        );
        let (_, blob) = read_frame_with::<Msg>(&mut Cursor::new(bytes), 1 << 20, |_| 100).unwrap();
        assert_eq!(blob.map(|b| b.len()), Some(100));
    }
}
