//! Cutting FFmpeg's piped JPEG stream into frames, and reading a frame's size.
//!
//! `-f image2pipe -c:v mjpeg` writes one complete JPEG file after the other with nothing between
//! them. A frame is cut by walking its markers (never by searching for the end marker bytes: a
//! quantisation or Huffman table may contain them), and inside the entropy-coded data the end
//! marker is the first `FF D9` (a data byte `FF` is always followed by `00`).

use crate::VideoError;

/// The biggest frame accepted, in bytes. A 360-line frame is some tens of kilobytes; more than
/// this is a stream that is not JPEG.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Where the walk of the current frame stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the start marker.
    Start,
    /// Between segments, at the byte where a marker's `FF` is expected.
    Markers,
    /// Inside entropy-coded data, scanning for the next marker.
    Scan,
}

/// Cuts a byte stream of JPEG files into frames.
#[derive(Debug)]
pub struct FrameSplitter {
    buf: Vec<u8>,
    pos: usize,
    state: State,
}

impl Default for FrameSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSplitter {
    /// A splitter at the start of a stream.
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
            state: State::Start,
        }
    }

    /// Adds `bytes` and calls `frame` with every frame that is complete, in order. `frame`
    /// returns `false` to stop (the rest of the input is dropped).
    ///
    /// # Errors
    /// [`VideoError::Proxy`] when the bytes are not a JPEG stream or a frame is too big.
    pub fn push(
        &mut self,
        bytes: &[u8],
        frame: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<bool, VideoError> {
        self.buf.extend_from_slice(bytes);
        while let Some(end) = self.next_end()? {
            let keep_going = frame(&self.buf[..end]);
            self.buf.drain(..end);
            self.pos = 0;
            self.state = State::Start;
            if !keep_going {
                self.buf.clear();
                return Ok(false);
            }
        }
        if self.buf.len() > MAX_FRAME_BYTES {
            return Err(VideoError::Proxy("a video frame is too big".into()));
        }
        Ok(true)
    }

    /// Whether bytes of an unfinished frame are held (at the end of a stream that means the
    /// picture was cut short).
    pub fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }

    /// The end (exclusive) of the frame at the start of the buffer, `None` when it is not
    /// complete yet.
    fn next_end(&mut self) -> Result<Option<usize>, VideoError> {
        let b = &self.buf;
        loop {
            match self.state {
                State::Start => {
                    if b.len() < 2 {
                        return Ok(None);
                    }
                    if b[0] != 0xFF || b[1] != 0xD8 {
                        return Err(VideoError::Proxy("the video stream is not JPEG".into()));
                    }
                    self.pos = 2;
                    self.state = State::Markers;
                }
                State::Markers => {
                    if b.len() < self.pos + 2 {
                        return Ok(None);
                    }
                    if b[self.pos] != 0xFF {
                        return Err(VideoError::Proxy("a video frame is damaged".into()));
                    }
                    let marker = b[self.pos + 1];
                    match marker {
                        // Fill bytes before a marker.
                        0xFF => self.pos += 1,
                        0xD9 => return Ok(Some(self.pos + 2)),
                        // Markers without a length: restart markers, TEM, a second SOI.
                        0x00 | 0x01 | 0xD0..=0xD8 => self.pos += 2,
                        _ => {
                            if b.len() < self.pos + 4 {
                                return Ok(None);
                            }
                            let len =
                                usize::from(b[self.pos + 2]) << 8 | usize::from(b[self.pos + 3]);
                            if len < 2 {
                                return Err(VideoError::Proxy("a video frame is damaged".into()));
                            }
                            if b.len() < self.pos + 2 + len {
                                return Ok(None);
                            }
                            self.pos += 2 + len;
                            if marker == 0xDA {
                                self.state = State::Scan;
                            }
                        }
                    }
                }
                State::Scan => {
                    // Skip data bytes; stop at an FF that is not a stuffed zero.
                    let mut i = self.pos;
                    while i + 1 < b.len() {
                        if b[i] == 0xFF && b[i + 1] != 0x00 && !(0xD0..=0xD7).contains(&b[i + 1]) {
                            break;
                        }
                        i += if b[i] == 0xFF { 2 } else { 1 };
                    }
                    if i + 1 >= b.len() {
                        // The last byte may be the FF of a marker: look at it again next time.
                        self.pos = if b.last() == Some(&0xFF) {
                            b.len() - 1
                        } else {
                            b.len()
                        };
                        return Ok(None);
                    }
                    self.pos = i;
                    self.state = State::Markers;
                }
            }
        }
    }
}

/// The width and height a JPEG frame states in its start-of-frame segment.
pub fn jpeg_size(frame: &[u8]) -> Option<(u32, u32)> {
    if frame.len() < 4 || frame[0] != 0xFF || frame[1] != 0xD8 {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= frame.len() {
        if frame[pos] != 0xFF {
            return None;
        }
        let marker = frame[pos + 1];
        match marker {
            0xFF => {
                pos += 1;
                continue;
            }
            0x00 | 0x01 | 0xD0..=0xD8 => {
                pos += 2;
                continue;
            }
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(frame[pos + 2]) << 8 | usize::from(frame[pos + 3]);
        // The frame headers: baseline, extended, progressive, lossless and the arithmetic ones.
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let seg = frame.get(pos + 4..pos + 2 + len)?;
            if seg.len() < 5 {
                return None;
            }
            let height = u32::from(seg[1]) << 8 | u32::from(seg[2]);
            let width = u32::from(seg[3]) << 8 | u32::from(seg[4]);
            return (width > 0 && height > 0).then_some((width, height));
        }
        pos += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::tiny_jpeg;

    fn split_all(chunks: &[&[u8]]) -> (Vec<Vec<u8>>, FrameSplitter) {
        let mut s = FrameSplitter::new();
        let mut frames = Vec::new();
        for c in chunks {
            s.push(c, &mut |f| {
                frames.push(f.to_vec());
                true
            })
            .unwrap();
        }
        (frames, s)
    }

    #[test]
    fn a_stream_is_cut_into_its_frames_at_any_chunk_size() {
        let frames: Vec<Vec<u8>> = (0..5).map(|i| tiny_jpeg(64, 36, i)).collect();
        let stream: Vec<u8> = frames.concat();
        for size in [1usize, 2, 3, 7, 50, stream.len()] {
            let chunks: Vec<&[u8]> = stream.chunks(size).collect();
            let (got, s) = split_all(&chunks);
            assert_eq!(got, frames, "chunk size {size}");
            assert!(!s.has_partial());
        }
    }

    #[test]
    fn end_marker_bytes_inside_a_table_do_not_end_a_frame() {
        // `tiny_jpeg` puts FF D9 inside its quantisation table and FF 00 in its data.
        let one = tiny_jpeg(8, 8, 1);
        assert!(one.windows(2).filter(|w| *w == [0xFF, 0xD9]).count() >= 2);
        let (got, _) = split_all(&[&one, &one]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], one);
    }

    #[test]
    fn a_cut_short_frame_is_held_not_delivered() {
        let one = tiny_jpeg(8, 8, 1);
        let (got, s) = split_all(&[&one[..one.len() - 1]]);
        assert!(got.is_empty());
        assert!(s.has_partial());
    }

    #[test]
    fn a_stream_that_is_not_jpeg_is_an_error() {
        let mut s = FrameSplitter::new();
        let err = s.push(b"not a picture", &mut |_| true).unwrap_err();
        assert!(matches!(err, VideoError::Proxy(_)));
    }

    #[test]
    fn the_consumer_can_stop_the_walk() {
        let stream = [tiny_jpeg(8, 8, 1), tiny_jpeg(8, 8, 2)].concat();
        let mut s = FrameSplitter::new();
        let mut seen = 0;
        let more = s
            .push(&stream, &mut |_| {
                seen += 1;
                false
            })
            .unwrap();
        assert!(!more);
        assert_eq!(seen, 1);
    }

    #[test]
    fn the_size_comes_from_the_frame_header() {
        assert_eq!(jpeg_size(&tiny_jpeg(640, 360, 0)), Some((640, 360)));
        assert_eq!(jpeg_size(&tiny_jpeg(1, 1, 0)), Some((1, 1)));
        assert_eq!(jpeg_size(b"no"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xD9]), None);
    }
}
