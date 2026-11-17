//! Test fixture writers for the Guitar Pro readers: a small encoder for the binary formats
//! (versions 3.00, 4.00, 5.00 and 5.10), a builder for GPIF XML, and writers for the zip and
//! `.gpx` containers. No real song is used anywhere; everything is made up here.
#![allow(dead_code)]

pub mod binary;
pub mod containers;
pub mod gpif;

/// A little-endian byte writer.
#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn i8(&mut self, v: i8) {
        self.0.push(v as u8);
    }
    pub fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn zeros(&mut self, n: usize) {
        self.0.extend(std::iter::repeat_n(0u8, n));
    }
    /// A length byte, then a field of `size` bytes.
    pub fn byte_size_string(&mut self, s: &str, size: usize) {
        let b = s.as_bytes();
        let n = b.len().min(size);
        self.u8(n as u8);
        self.0.extend_from_slice(&b[..n]);
        self.zeros(size - n);
    }
    /// An `i32` of (length + 1), a length byte, then the text.
    pub fn int_byte_size_string(&mut self, s: &str) {
        self.i32(s.len() as i32 + 1);
        self.u8(s.len() as u8);
        self.0.extend_from_slice(s.as_bytes());
    }
    /// An `i32` length, then the text.
    pub fn int_size_string(&mut self, s: &str) {
        self.i32(s.len() as i32);
        self.0.extend_from_slice(s.as_bytes());
    }
}

/// The CRC-32 of the zip format.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A deterministic pseudo-random stream for mutation tests.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
