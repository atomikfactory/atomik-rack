//! A reader of the legacy PyTorch checkpoint format (`.pth` written before the zip format), which
//! is what the published Open-Unmix weights are.
//!
//! The file is five pickles and a blob: a magic number, the protocol version, the system info,
//! the state dictionary (tensors refer to their storage by a key), the list of storage keys, and
//! then for each key its element count (`i64`, little endian) and its raw values.
//!
//! The pickle reader here is a tiny interpreter of the handful of opcodes such a file uses. It
//! never imports or runs anything: the only two "calls" it understands (an ordered dictionary
//! and a tensor rebuild) are built in, and any other opcode or call is an error. So even a
//! damaged or hostile file can only make [`read_state`] fail.

use std::collections::BTreeMap;

/// Why a checkpoint cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TorchError {
    /// The file is cut short.
    #[error("the weight file ends too soon")]
    Truncated,
    /// The file uses something this reader does not know.
    #[error("the weight file is not in the expected format: {0}")]
    Format(String),
}

fn bad<T>(what: &str) -> Result<T, TorchError> {
    Err(TorchError::Format(what.to_string()))
}

/// One tensor of the state dictionary (32-bit floats, contiguous).
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    /// The shape.
    pub shape: Vec<usize>,
    /// The values, row major.
    pub data: Vec<f32>,
}

/// The tensors by name.
pub type State = BTreeMap<String, Tensor>;

/// The magic number every checkpoint starts with, as the ten little-endian bytes of its LONG1.
const MAGIC: [u8; 10] = [0x6c, 0xfc, 0x9c, 0x46, 0xf9, 0x20, 0x6a, 0xa8, 0x50, 0x19];

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Mark,
    None,
    Bool(bool),
    Int(i64),
    Big(Vec<u8>),
    Str(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Global(String),
    Storage { key: String, wide: bool },
    Tensor(TensorRef),
}

#[derive(Debug, Clone, PartialEq)]
struct TensorRef {
    key: String,
    /// 64-bit integers (the sample rate, counters), read as numbers.
    wide: bool,
    offset: usize,
    shape: Vec<usize>,
    stride: Vec<usize>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], TorchError> {
        let end = self.pos.checked_add(n).ok_or(TorchError::Truncated)?;
        let s = self.bytes.get(self.pos..end).ok_or(TorchError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, TorchError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, TorchError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, TorchError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn line(&mut self) -> Result<&'a str, TorchError> {
        let rest = &self.bytes[self.pos.min(self.bytes.len())..];
        let n = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or(TorchError::Truncated)?;
        self.pos += n + 1;
        std::str::from_utf8(&rest[..n]).or_else(|_| bad("text"))
    }
}

/// The most values a pickle may hold, so a hostile file cannot ask for endless memory.
const MAX_ITEMS: usize = 1 << 20;

fn top(stack: &[Value], what: &str) -> Result<Value, TorchError> {
    stack
        .last()
        .cloned()
        .ok_or_else(|| TorchError::Format(what.to_string()))
}

fn pop(stack: &mut Vec<Value>, what: &str) -> Result<Value, TorchError> {
    stack
        .pop()
        .ok_or_else(|| TorchError::Format(what.to_string()))
}

/// Runs one pickle from the cursor and returns its value.
fn pickle(c: &mut Cursor<'_>) -> Result<Value, TorchError> {
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: BTreeMap<u32, Value> = BTreeMap::new();
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > 4 * MAX_ITEMS || stack.len() > MAX_ITEMS {
            return bad("too large");
        }
        let op = c.u8()?;
        match op {
            0x80 => {
                c.u8()?; // PROTO
            }
            b'.' => return pop(&mut stack, "empty"),
            b'(' => stack.push(Value::Mark),
            b'N' => stack.push(Value::None),
            0x88 => stack.push(Value::Bool(true)),
            0x89 => stack.push(Value::Bool(false)),
            b'K' => {
                let v = c.u8()?;
                stack.push(Value::Int(i64::from(v)));
            }
            b'M' => {
                let v = c.u16()?;
                stack.push(Value::Int(i64::from(v)));
            }
            b'J' => {
                let v = c.u32()? as i32;
                stack.push(Value::Int(i64::from(v)));
            }
            0x8a => {
                // LONG1: a little-endian two's complement integer.
                let n = c.u8()? as usize;
                let b = c.take(n)?;
                if n > 8 {
                    stack.push(Value::Big(b.to_vec()));
                } else {
                    let mut v: i64 = 0;
                    for (i, &x) in b.iter().enumerate() {
                        v |= i64::from(x) << (8 * i);
                    }
                    if n > 0 && n < 8 && b[n - 1] & 0x80 != 0 {
                        v |= -1i64 << (8 * n);
                    }
                    stack.push(Value::Int(v));
                }
            }
            b'X' => {
                let n = c.u32()? as usize;
                let s = std::str::from_utf8(c.take(n)?).or_else(|_| bad("text"))?;
                stack.push(Value::Str(s.to_string()));
            }
            0x8c => {
                let n = c.u8()? as usize;
                let s = std::str::from_utf8(c.take(n)?).or_else(|_| bad("text"))?;
                stack.push(Value::Str(s.to_string()));
            }
            b'c' => {
                let module = c.line()?.to_string();
                let name = c.line()?;
                stack.push(Value::Global(format!("{module} {name}")));
            }
            b'q' => {
                let k = u32::from(c.u8()?);
                memo.insert(k, top(&stack, "memo")?);
            }
            b'r' => {
                let k = c.u32()?;
                memo.insert(k, top(&stack, "memo")?);
            }
            b'h' => {
                let k = u32::from(c.u8()?);
                stack.push(
                    memo.get(&k)
                        .cloned()
                        .ok_or_else(|| TorchError::Format("memo".into()))?,
                );
            }
            b'j' => {
                let k = c.u32()?;
                stack.push(
                    memo.get(&k)
                        .cloned()
                        .ok_or_else(|| TorchError::Format("memo".into()))?,
                );
            }
            b')' => stack.push(Value::Tuple(Vec::new())),
            0x85..=0x87 => {
                let n = usize::from(op - 0x84);
                if stack.len() < n {
                    return bad("tuple");
                }
                let items = stack.split_off(stack.len() - n);
                stack.push(Value::Tuple(items));
            }
            b't' => {
                let items = pop_to_mark(&mut stack)?;
                stack.push(Value::Tuple(items));
            }
            b']' => stack.push(Value::List(Vec::new())),
            b'}' => stack.push(Value::Dict(Vec::new())),
            b'e' => {
                let items = pop_to_mark(&mut stack)?;
                match stack.last_mut() {
                    Some(Value::List(l)) => l.extend(items),
                    _ => return bad("appends"),
                }
            }
            b's' => {
                let v = pop(&mut stack, "setitem")?;
                let k = pop(&mut stack, "setitem")?;
                match stack.last_mut() {
                    Some(Value::Dict(d)) => d.push((k, v)),
                    _ => return bad("setitem"),
                }
            }
            b'u' => {
                let items = pop_to_mark(&mut stack)?;
                if items.len() % 2 != 0 {
                    return bad("setitems");
                }
                match stack.last_mut() {
                    Some(Value::Dict(d)) => {
                        let mut it = items.into_iter();
                        while let (Some(k), Some(v)) = (it.next(), it.next()) {
                            d.push((k, v));
                        }
                    }
                    _ => return bad("setitems"),
                }
            }
            b'b' => {
                // BUILD sets attributes of an object (here the dictionary's metadata): ignored.
                pop(&mut stack, "build")?;
            }
            b'R' => {
                let args = pop(&mut stack, "reduce")?;
                let f = pop(&mut stack, "reduce")?;
                stack.push(call(&f, args)?);
            }
            b'Q' => {
                let pid = pop(&mut stack, "persistent id")?;
                stack.push(persistent(pid)?);
            }
            _ => return bad(&format!("opcode 0x{op:02x}")),
        }
    }
}

fn pop_to_mark(stack: &mut Vec<Value>) -> Result<Vec<Value>, TorchError> {
    let at = stack
        .iter()
        .rposition(|v| *v == Value::Mark)
        .ok_or_else(|| TorchError::Format("mark".into()))?;
    let items = stack.split_off(at + 1);
    stack.pop();
    Ok(items)
}

fn ints(t: &[Value]) -> Result<Vec<usize>, TorchError> {
    t.iter()
        .map(|v| match v {
            Value::Int(i) if *i >= 0 => Ok(*i as usize),
            _ => bad("tensor shape"),
        })
        .collect()
}

fn call(f: &Value, args: Value) -> Result<Value, TorchError> {
    let Value::Global(name) = f else {
        return bad("call of a value");
    };
    let Value::Tuple(args) = args else {
        return bad("call arguments");
    };
    match name.as_str() {
        "collections OrderedDict" => Ok(Value::Dict(Vec::new())),
        "torch._utils _rebuild_tensor_v2" => {
            let [
                Value::Storage { key, wide },
                Value::Int(offset),
                Value::Tuple(shape),
                Value::Tuple(stride),
                ..,
            ] = args.as_slice()
            else {
                return bad("tensor arguments");
            };
            Ok(Value::Tensor(TensorRef {
                key: key.clone(),
                wide: *wide,
                offset: usize::try_from(*offset).or_else(|_| bad("offset"))?,
                shape: ints(shape)?,
                stride: ints(stride)?,
            }))
        }
        other => bad(&format!("call of {other}")),
    }
}

fn persistent(pid: Value) -> Result<Value, TorchError> {
    let Value::Tuple(t) = pid else {
        return bad("storage reference");
    };
    match t.as_slice() {
        [Value::Str(kind), Value::Global(class), Value::Str(key), ..] if kind == "storage" => {
            let wide = match class.as_str() {
                "torch FloatStorage" => false,
                "torch LongStorage" => true,
                other => return bad(&format!("storage type {other}")),
            };
            Ok(Value::Storage {
                key: key.clone(),
                wide,
            })
        }
        _ => bad("storage reference"),
    }
}

/// Reads the state dictionary of a legacy checkpoint. Float tensors and 64-bit integer tensors (read as numbers) are accepted.
pub fn read_state(bytes: &[u8]) -> Result<State, TorchError> {
    let mut c = Cursor { bytes, pos: 0 };
    // Magic number, protocol version, system info.
    if pickle(&mut c)? != Value::Big(MAGIC.to_vec()) {
        return bad("magic number");
    }
    pickle(&mut c)?;
    pickle(&mut c)?;
    let dict = pickle(&mut c)?;
    let keys = match pickle(&mut c)? {
        Value::List(l) => l,
        _ => return bad("storage keys"),
    };
    let Value::Dict(items) = dict else {
        return bad("state dictionary");
    };
    // Which storages hold 64-bit integers (eight bytes an element).
    let wide: Vec<&str> = items
        .iter()
        .filter_map(|(_, v)| match v {
            Value::Tensor(t) if t.wide => Some(t.key.as_str()),
            _ => None,
        })
        .collect();
    let mut blobs: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    for k in keys {
        let Value::Str(key) = k else {
            return bad("storage key");
        };
        let b = c.take(8)?;
        let n = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        let n = usize::try_from(n).or_else(|_| bad("storage size"))?;
        let size = if wide.contains(&key.as_str()) { 8 } else { 4 };
        let raw = c.take(n.checked_mul(size).ok_or(TorchError::Truncated)?)?;
        let data = if size == 4 {
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|q| f32::from_le_bytes(*q))
                .collect()
        } else {
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|q| i64::from_le_bytes(*q) as f32)
                .collect()
        };
        blobs.insert(key, data);
    }
    let mut state = State::new();
    for (k, v) in items {
        let (Value::Str(name), Value::Tensor(t)) = (k, v) else {
            continue;
        };
        let blob = blobs
            .get(&t.key)
            .ok_or_else(|| TorchError::Format("missing storage".into()))?;
        let count: usize = t.shape.iter().product();
        // Only contiguous tensors: each stride is the product of the sizes after it.
        let mut expect = 1usize;
        for (size, stride) in t.shape.iter().zip(&t.stride).rev() {
            if *size != 1 && *stride != expect {
                return bad("a tensor that is not contiguous");
            }
            expect = expect.saturating_mul(*size);
        }
        let end = t.offset.checked_add(count).ok_or(TorchError::Truncated)?;
        let data = blob
            .get(t.offset..end)
            .ok_or(TorchError::Truncated)?
            .to_vec();
        state.insert(
            name,
            Tensor {
                shape: t.shape,
                data,
            },
        );
    }
    Ok(state)
}

/// Writes a legacy checkpoint of `tensors`: (name, shape, values). Tests use it to build small
/// weight files; it writes exactly the structure the real files have.
pub fn write_state(tensors: &[(&str, &[usize], &[f32])]) -> Vec<u8> {
    fn text(out: &mut Vec<u8>, s: &str) {
        out.push(b'X');
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }
    fn int(out: &mut Vec<u8>, v: usize) {
        out.push(b'J');
        out.extend((v as u32).to_le_bytes());
    }
    let mut out: Vec<u8> = Vec::new();
    out.extend([0x80, 2, 0x8a, 10]);
    out.extend(MAGIC);
    out.push(b'.');
    out.extend([0x80, 2, b'M', 0xe9, 0x03, b'.']);
    out.extend([0x80, 2, b'}', b'.']);
    // The state dictionary.
    out.extend([0x80, 2]);
    out.extend(b"ccollections\nOrderedDict\n)R(");
    for (i, (name, shape, _)) in tensors.iter().enumerate() {
        text(&mut out, name);
        out.extend(b"ctorch._utils\n_rebuild_tensor_v2\n((");
        text(&mut out, "storage");
        out.extend(b"ctorch\nFloatStorage\n");
        text(&mut out, &i.to_string());
        text(&mut out, "cpu");
        int(&mut out, shape.iter().product::<usize>());
        out.extend(*b"NtQ");
        int(&mut out, 0);
        out.push(b'(');
        for s in *shape {
            int(&mut out, *s);
        }
        out.extend(*b"t(");
        let mut stride = vec![1usize; shape.len()];
        for d in (0..shape.len().saturating_sub(1)).rev() {
            stride[d] = stride[d + 1] * shape[d + 1];
        }
        for s in &stride {
            int(&mut out, *s);
        }
        out.push(b't');
        out.push(0x89);
        out.extend(b"ccollections\nOrderedDict\n)Rt");
        out.push(b'R');
    }
    out.extend(*b"u.");
    // The storage keys, then each storage: its element count and its values.
    out.extend([0x80, 2, b']', b'(']);
    for i in 0..tensors.len() {
        text(&mut out, &i.to_string());
    }
    out.extend(*b"e.");
    for (_, _, data) in tensors {
        out.extend((data.len() as u64).to_le_bytes());
        for v in *data {
            out.extend(v.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_checkpoint_reads_back() {
        let a = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = [0.5f32];
        let bytes = write_state(&[("fc.weight", &[2, 3], &a), ("scale", &[1], &b)]);
        let state = read_state(&bytes).expect("reads");
        assert_eq!(state["fc.weight"].shape, vec![2, 3]);
        assert_eq!(state["fc.weight"].data, a.to_vec());
        assert_eq!(state["scale"].data, vec![0.5]);
    }

    #[test]
    fn every_cut_of_the_file_is_an_error_and_never_a_panic() {
        let a = [1.0f32, 2.0, 3.0, 4.0];
        let bytes = write_state(&[("w", &[2, 2], &a)]);
        for cut in 0..bytes.len() {
            assert!(read_state(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn an_unknown_call_is_refused() {
        // A pickle that calls something else than the two built-in calls is refused, not run.
        let mut bytes = write_state(&[("w", &[1], &[1.0])]);
        let at = bytes
            .windows(8)
            .position(|w| w == b"torch._u")
            .expect("the call is there");
        bytes[at] = b'o';
        bytes[at + 1] = b's';
        assert!(read_state(&bytes).is_err());
    }

    #[test]
    fn garbage_is_refused() {
        assert!(read_state(&[]).is_err());
        assert!(read_state(b"PK\x03\x04 not a legacy file").is_err());
        assert!(read_state(&[0xff; 64]).is_err());
    }
}
