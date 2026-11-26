//! A small OSC 1.0 encoder and decoder (the wire format, nothing else): messages and bundles,
//! with the argument types a control surface or a tablet app sends.
//!
//! A packet is a message (`/address`, a type tag string starting with a comma, the arguments)
//! or a bundle (`#bundle`, a time tag and size-prefixed elements, each a message or a bundle).
//! Every field is padded with zero bytes to a multiple of four. The decoder knows the tags `i`
//! (32 bit integer), `f` (32 bit float), `s` (string), `b` (blob), `h` (64 bit integer), `d`
//! (64 bit float), `T`, `F`, `N` and `I`; a packet with any other tag is refused whole, because
//! the size of what follows is unknown. It never panics, whatever the bytes. The time tag of a
//! bundle is ignored (its messages are handled when they arrive).

use std::fmt;

/// The most nesting of bundles inside bundles the decoder follows.
const MAX_DEPTH: usize = 4;
/// The most messages one packet may hold (a bundle of nothing but tiny messages).
const MAX_MESSAGES: usize = 4096;
/// The start of a bundle.
const BUNDLE: &[u8; 8] = b"#bundle\0";

/// One argument of a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// A 32 bit integer (`i`).
    Int(i32),
    /// A 32 bit float (`f`).
    Float(f32),
    /// A string (`s`).
    Str(String),
    /// A blob of bytes (`b`).
    Blob(Vec<u8>),
    /// A 64 bit integer (`h`).
    Long(i64),
    /// A 64 bit float (`d`).
    Double(f64),
    /// True or false (`T`, `F`): no data on the wire.
    Bool(bool),
    /// Nil (`N`) or infinitum (`I`): no data on the wire.
    Nil,
}

impl Arg {
    /// The argument as a number, when it is one (a boolean is 0 or 1).
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Int(v) => Some(*v as f32),
            Self::Float(v) => Some(*v),
            Self::Long(v) => Some(*v as f32),
            Self::Double(v) => Some(*v as f32),
            Self::Bool(b) => Some(f32::from(u8::from(*b))),
            Self::Str(_) | Self::Blob(_) | Self::Nil => None,
        }
    }

    /// The argument as a switch: a number is on when it is above one half.
    pub fn as_bool(&self) -> Option<bool> {
        self.as_f32().map(|v| v > 0.5)
    }

    /// The argument as text, when it is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// One OSC message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// The address pattern (`/strip/1/fader`).
    pub address: String,
    /// The arguments, in order.
    pub args: Vec<Arg>,
}

impl Message {
    /// A message with no arguments.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            args: Vec::new(),
        }
    }

    /// A message with one argument.
    pub fn with(address: impl Into<String>, arg: Arg) -> Self {
        Self {
            address: address.into(),
            args: vec![arg],
        }
    }
}

/// Why a packet could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscError {
    /// The packet ends inside a field.
    Truncated,
    /// The address does not start with a slash, or is not text.
    BadAddress,
    /// The type tag string is missing its comma, or has a tag this decoder does not know.
    BadTag,
    /// A string is not valid text or is not terminated.
    BadString,
    /// A bundle element has a size that does not fit, or is not a multiple of four.
    BadBundle,
}

impl fmt::Display for OscError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "the packet ends inside a field",
            Self::BadAddress => "the address is not a path",
            Self::BadTag => "the type tags are not understood",
            Self::BadString => "a string is not valid",
            Self::BadBundle => "a bundle is malformed",
        })
    }
}

impl std::error::Error for OscError {}

/// Zero bytes to pad `len` up to a multiple of four.
fn padding(len: usize) -> usize {
    (4 - len % 4) % 4
}

fn push_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    // At least one terminating zero, then padding to a multiple of four.
    let used = s.len() + 1;
    out.resize(out.len() + 1 + padding(used), 0);
}

/// The packet for one message.
pub fn encode(message: &Message) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + message.address.len());
    push_string(&mut out, &message.address);
    let mut tags = String::with_capacity(1 + message.args.len());
    tags.push(',');
    for arg in &message.args {
        tags.push(match arg {
            Arg::Int(_) => 'i',
            Arg::Float(_) => 'f',
            Arg::Str(_) => 's',
            Arg::Blob(_) => 'b',
            Arg::Long(_) => 'h',
            Arg::Double(_) => 'd',
            Arg::Bool(true) => 'T',
            Arg::Bool(false) => 'F',
            Arg::Nil => 'N',
        });
    }
    push_string(&mut out, &tags);
    for arg in &message.args {
        match arg {
            Arg::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Str(s) => push_string(&mut out, s),
            Arg::Blob(b) => {
                out.extend_from_slice(&(b.len() as i32).to_be_bytes());
                out.extend_from_slice(b);
                out.resize(out.len() + padding(b.len()), 0);
            }
            Arg::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
            Arg::Bool(_) | Arg::Nil => {}
        }
    }
    out
}

/// The packet for several messages: a bundle with the "immediately" time tag, or the bare
/// message when there is one.
pub fn encode_bundle(messages: &[Message]) -> Vec<u8> {
    match messages {
        [one] => encode(one),
        _ => {
            let mut out = BUNDLE.to_vec();
            out.extend_from_slice(&1u64.to_be_bytes());
            for m in messages {
                let packet = encode(m);
                out.extend_from_slice(&(packet.len() as i32).to_be_bytes());
                out.extend_from_slice(&packet);
            }
            out
        }
    }
}

/// Packs `messages` into as few packets as possible, none longer than `limit` bytes (a message
/// that is longer than that alone gets a packet of its own).
pub fn pack(messages: &[Message], limit: usize) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut batch: Vec<Message> = Vec::new();
    let mut size = BUNDLE.len() + 8;
    for m in messages {
        let len = encode(m).len() + 4;
        if !batch.is_empty() && size + len > limit {
            packets.push(encode_bundle(&batch));
            batch.clear();
            size = BUNDLE.len() + 8;
        }
        batch.push(m.clone());
        size += len;
    }
    if !batch.is_empty() {
        packets.push(encode_bundle(&batch));
    }
    packets
}

/// A cursor over the bytes of a packet.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], OscError> {
        let end = self.at.checked_add(n).ok_or(OscError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(OscError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], OscError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn string(&mut self) -> Result<String, OscError> {
        let rest = self.bytes.get(self.at..).ok_or(OscError::Truncated)?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(OscError::BadString)?;
        let text = std::str::from_utf8(&rest[..end]).map_err(|_| OscError::BadString)?;
        let used = end + 1;
        self.take(used + padding(used))?;
        Ok(text.to_owned())
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }
}

fn decode_message(bytes: &[u8]) -> Result<Message, OscError> {
    let mut r = Reader { bytes, at: 0 };
    let address = r.string()?;
    if !address.starts_with('/') {
        return Err(OscError::BadAddress);
    }
    let mut args = Vec::new();
    // OSC 1.0 allows a message with no type tag string at all.
    if !r.done() {
        let tags = r.string()?;
        let tags = tags.strip_prefix(',').ok_or(OscError::BadTag)?;
        for tag in tags.chars() {
            args.push(match tag {
                'i' => Arg::Int(i32::from_be_bytes(r.array()?)),
                'f' => Arg::Float(f32::from_be_bytes(r.array()?)),
                's' => Arg::Str(r.string()?),
                'b' => {
                    let len = i32::from_be_bytes(r.array()?);
                    let len = usize::try_from(len).map_err(|_| OscError::Truncated)?;
                    let data = r.take(len)?.to_vec();
                    r.take(padding(len))?;
                    Arg::Blob(data)
                }
                'h' => Arg::Long(i64::from_be_bytes(r.array()?)),
                'd' => Arg::Double(f64::from_be_bytes(r.array()?)),
                'T' => Arg::Bool(true),
                'F' => Arg::Bool(false),
                'N' | 'I' => Arg::Nil,
                _ => return Err(OscError::BadTag),
            });
        }
    }
    Ok(Message { address, args })
}

fn decode_into(bytes: &[u8], depth: usize, out: &mut Vec<Message>) -> Result<(), OscError> {
    if bytes.starts_with(BUNDLE) {
        if depth >= MAX_DEPTH {
            return Err(OscError::BadBundle);
        }
        let mut r = Reader { bytes, at: 16 };
        if bytes.len() < 16 {
            return Err(OscError::Truncated);
        }
        while !r.done() {
            let size = i32::from_be_bytes(r.array()?);
            let size = usize::try_from(size).map_err(|_| OscError::BadBundle)?;
            if size % 4 != 0 {
                return Err(OscError::BadBundle);
            }
            decode_into(r.take(size)?, depth + 1, out)?;
        }
        return Ok(());
    }
    if out.len() >= MAX_MESSAGES {
        return Err(OscError::BadBundle);
    }
    out.push(decode_message(bytes)?);
    Ok(())
}

/// The messages of a packet (one for a message, every one of a bundle, nested bundles
/// included), appended to `out`.
///
/// # Errors
/// What is wrong with the packet. Messages decoded before the fault stay in `out`.
pub fn decode(bytes: &[u8], out: &mut Vec<Message>) -> Result<(), OscError> {
    decode_into(bytes, 0, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(bytes: &[u8]) -> Message {
        let mut out = Vec::new();
        decode(bytes, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        out.remove(0)
    }

    #[test]
    fn a_message_has_the_exact_bytes_of_the_spec() {
        // The example of the OSC 1.0 spec: "/oscillator/4/frequency" with the float 440.0.
        let m = Message::with("/oscillator/4/frequency", Arg::Float(440.0));
        let mut want = b"/oscillator/4/frequency\0".to_vec();
        want.extend_from_slice(b",f\0\0");
        want.extend_from_slice(&440.0f32.to_be_bytes());
        assert_eq!(encode(&m), want);
        assert_eq!(one(&want), m);
    }

    #[test]
    fn every_argument_type_round_trips() {
        let m = Message {
            address: "/all".to_owned(),
            args: vec![
                Arg::Int(-7),
                Arg::Float(0.25),
                Arg::Str("hello".to_owned()),
                Arg::Blob(vec![1, 2, 3]),
                Arg::Long(1 << 40),
                Arg::Double(0.125),
                Arg::Bool(true),
                Arg::Bool(false),
                Arg::Nil,
            ],
        };
        let bytes = encode(&m);
        assert_eq!(bytes.len() % 4, 0, "every field is padded");
        assert_eq!(one(&bytes), m);
    }

    #[test]
    fn strings_are_padded_to_four_bytes_with_at_least_one_zero() {
        for (text, len) in [("", 4), ("a", 4), ("abc", 4), ("abcd", 8), ("abcdefg", 8)] {
            let mut out = Vec::new();
            push_string(&mut out, text);
            assert_eq!(out.len(), len, "{text:?}");
            assert_eq!(*out.last().unwrap(), 0);
        }
    }

    #[test]
    fn a_message_with_no_type_tags_has_no_arguments() {
        let mut bytes = Vec::new();
        push_string(&mut bytes, "/transport/play");
        assert_eq!(one(&bytes), Message::new("/transport/play"));
    }

    #[test]
    fn a_bundle_gives_its_messages_and_nested_bundles_in_order() {
        let a = Message::with("/a", Arg::Int(1));
        let b = Message::with("/b", Arg::Int(2));
        let c = Message::with("/c", Arg::Int(3));
        let inner = encode_bundle(&[b.clone(), c.clone()]);
        let mut outer = BUNDLE.to_vec();
        outer.extend_from_slice(&1u64.to_be_bytes());
        for packet in [encode(&a), inner] {
            outer.extend_from_slice(&(packet.len() as i32).to_be_bytes());
            outer.extend_from_slice(&packet);
        }
        let mut out = Vec::new();
        decode(&outer, &mut out).unwrap();
        assert_eq!(out, vec![a, b, c]);
    }

    #[test]
    fn packing_splits_at_the_limit_and_keeps_every_message() {
        let messages: Vec<Message> = (0..40)
            .map(|i| Message::with(format!("/strip/{i}/fader"), Arg::Float(i as f32)))
            .collect();
        let packets = pack(&messages, 200);
        assert!(packets.len() > 1);
        assert!(packets.iter().all(|p| p.len() <= 200), "{packets:?}");
        let mut back = Vec::new();
        for p in &packets {
            decode(p, &mut back).unwrap();
        }
        assert_eq!(back, messages);
        assert!(pack(&[], 200).is_empty());
        assert_eq!(pack(&messages[..1], 200), vec![encode(&messages[0])]);
    }

    #[test]
    fn damaged_packets_are_refused_and_never_panic() {
        let good = encode(&Message {
            address: "/x".to_owned(),
            args: vec![Arg::Int(1), Arg::Str("abc".to_owned())],
        });
        // Every prefix of a good packet, and every packet with one byte changed.
        for cut in 0..good.len() {
            let mut out = Vec::new();
            let _ = decode(&good[..cut], &mut out);
        }
        for i in 0..good.len() {
            for value in [0u8, 0x7F, 0xFF] {
                let mut bad = good.clone();
                bad[i] = value;
                let mut out = Vec::new();
                let _ = decode(&bad, &mut out);
            }
        }
        let mut out = Vec::new();
        assert_eq!(decode(b"nope\0\0\0\0", &mut out), Err(OscError::BadAddress));
        assert_eq!(decode(b"/x\0\0,q\0\0", &mut out), Err(OscError::BadTag));
        assert_eq!(decode(b"/x\0\0xx\0\0", &mut out), Err(OscError::BadTag));
        assert_eq!(
            decode(b"/x\0\0,i\0\0\0\0", &mut out),
            Err(OscError::Truncated)
        );
        assert_eq!(decode(b"/x", &mut out), Err(OscError::BadString));
        assert_eq!(decode(&[], &mut out), Err(OscError::BadString));
    }

    #[test]
    fn a_bundle_nested_too_deep_or_with_a_wrong_size_is_refused() {
        let mut deep = encode(&Message::new("/x"));
        for _ in 0..=MAX_DEPTH {
            let mut b = BUNDLE.to_vec();
            b.extend_from_slice(&1u64.to_be_bytes());
            b.extend_from_slice(&(deep.len() as i32).to_be_bytes());
            b.extend_from_slice(&deep);
            deep = b;
        }
        let mut out = Vec::new();
        assert_eq!(decode(&deep, &mut out), Err(OscError::BadBundle));
        let mut b = BUNDLE.to_vec();
        b.extend_from_slice(&1u64.to_be_bytes());
        b.extend_from_slice(&1000i32.to_be_bytes());
        assert_eq!(decode(&b, &mut out), Err(OscError::Truncated));
        let mut b = BUNDLE.to_vec();
        b.extend_from_slice(&1u64.to_be_bytes());
        b.extend_from_slice(&3i32.to_be_bytes());
        b.extend_from_slice(&[0, 0, 0]);
        assert_eq!(decode(&b, &mut out), Err(OscError::BadBundle));
    }

    #[test]
    fn arguments_convert_to_numbers_and_switches() {
        assert_eq!(Arg::Int(3).as_f32(), Some(3.0));
        assert_eq!(Arg::Bool(true).as_f32(), Some(1.0));
        assert_eq!(Arg::Str("x".into()).as_f32(), None);
        assert_eq!(Arg::Float(0.9).as_bool(), Some(true));
        assert_eq!(Arg::Int(0).as_bool(), Some(false));
        assert_eq!(Arg::Nil.as_bool(), None);
        assert_eq!(Arg::Str("x".into()).as_str(), Some("x"));
    }
}
