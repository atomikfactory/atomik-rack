//! A bounds-checked reader over the bytes of a binary Guitar Pro file.

use crate::gp::GpError;

/// Reads little-endian numbers and the three string layouts of the format. Every read checks the
/// bytes that are left and fails with [`GpError::Truncated`], so a short file never panics.
pub(super) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The bytes that are left.
    pub(super) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(super) fn take(&mut self, n: usize) -> Result<&'a [u8], GpError> {
        if n > self.remaining() {
            return Err(GpError::Truncated);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(super) fn skip(&mut self, n: usize) -> Result<(), GpError> {
        self.take(n).map(|_| ())
    }

    pub(super) fn u8(&mut self) -> Result<u8, GpError> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn i8(&mut self) -> Result<i8, GpError> {
        Ok(i8::from_le_bytes([self.u8()?]))
    }

    pub(super) fn i16(&mut self) -> Result<i16, GpError> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub(super) fn i32(&mut self) -> Result<i32, GpError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A length or count: an `i32` that must not be negative.
    pub(super) fn count(&mut self) -> Result<usize, GpError> {
        let v = self.i32()?;
        usize::try_from(v).map_err(|_| GpError::Corrupt("a negative length".into()))
    }

    /// A string stored as one length byte, then a field of `size` bytes of which the first `length`
    /// are the text.
    pub(super) fn byte_size_string(&mut self, size: usize) -> Result<String, GpError> {
        let len = usize::from(self.u8()?);
        let field = self.take(size)?;
        Ok(decode(&field[..len.min(size)]))
    }

    /// A string stored as an `i32` (the field size plus one), a length byte, then the field.
    pub(super) fn int_byte_size_string(&mut self) -> Result<String, GpError> {
        let total = self.count()?;
        if total == 0 {
            return Ok(String::new());
        }
        self.byte_size_string(total - 1)
    }

    /// A string stored as an `i32` length, then that many bytes.
    pub(super) fn int_size_string(&mut self) -> Result<String, GpError> {
        let n = self.count()?;
        Ok(decode(self.take(n)?))
    }
}

/// Text of the file: UTF-8 when it is valid, else one character per byte (Latin-1).
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|b| char::from(*b)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_little_endian() {
        let mut c = Cursor::new(&[1, 0xFF, 0x34, 0x12, 0xFE, 0xFF, 0xFF, 0xFF]);
        assert_eq!(c.u8().unwrap(), 1);
        assert_eq!(c.i8().unwrap(), -1);
        assert_eq!(c.i16().unwrap(), 0x1234);
        assert_eq!(c.i32().unwrap(), -2);
        assert_eq!(c.remaining(), 0);
        assert_eq!(c.u8(), Err(GpError::Truncated));
    }

    #[test]
    fn strings_in_the_three_layouts() {
        let data = [
            3, b'a', b'b', b'c', 0, 0, // byte size string in a 5 byte field
            5, 0, 0, 0, 4, b'g', b'i', b't', b'a', // int byte size string: field of 4
            2, 0, 0, 0, 0xE9, b'x', // int size string, Latin-1
        ];
        let mut c = Cursor::new(&data);
        assert_eq!(c.byte_size_string(5).unwrap(), "abc");
        assert_eq!(c.int_byte_size_string().unwrap(), "gita");
        assert_eq!(c.int_size_string().unwrap(), "\u{e9}x");
    }

    #[test]
    fn lengths_beyond_the_data_fail() {
        assert_eq!(
            Cursor::new(&[0xFF, 0xFF, 0xFF, 0x7F]).int_size_string(),
            Err(GpError::Truncated)
        );
        assert!(matches!(
            Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF]).int_size_string(),
            Err(GpError::Corrupt(_))
        ));
        // The length byte may say more than the field holds.
        let mut c = Cursor::new(&[200, b'a', b'b']);
        assert_eq!(c.byte_size_string(2).unwrap(), "ab");
    }
}
