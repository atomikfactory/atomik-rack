//! Decoding a proxy frame to pixels, in pure Rust.
//!
//! The proxy holds every frame as a baseline JPEG (see [`crate::proxy`]); the viewer needs RGBA
//! pixels to hand to the renderer. [`decode_jpeg`] does that with the `zune-jpeg` crate (safe to
//! feed, no C code, no FFmpeg): the picture comes back as 8-bit RGBA with every alpha at 255, in
//! a shared buffer so the cache, the worker and the window hold one copy.

use crate::VideoError;
use std::sync::Arc;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// The widest and tallest picture accepted, in pixels. A proxy frame is 360 lines; this only
/// stops a damaged header from asking for gigabytes.
pub const MAX_SIDE: usize = 4_096;

/// A decoded frame: 8-bit RGBA, rows top to bottom, alpha always 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Arc<Vec<u8>>,
}

impl Picture {
    /// The size of the pixel data in bytes.
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }

    /// The pixel at `x`, `y` as `[r, g, b, a]`; `None` outside the picture.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba.get(at..at + 4)?.try_into().ok()
    }
}

/// Decodes one JPEG file to RGBA.
///
/// # Errors
/// [`VideoError::Proxy`] when the bytes are not a JPEG this decoder reads (the message says
/// what the decoder found).
pub fn decode_jpeg(bytes: &[u8]) -> Result<Picture, VideoError> {
    let bad = |why: String| VideoError::Proxy(format!("a video frame cannot be decoded: {why}"));
    let options = DecoderOptions::default()
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| bad(format!("{e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| bad("it has no header".to_owned()))?;
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| bad("it has no size".to_owned()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if width == 0 || height == 0 || size != width as usize * height as usize * 4 {
        return Err(bad(format!("an unexpected size {width}x{height}")));
    }
    let mut rgba = vec![0u8; size];
    decoder
        .decode_into(&mut rgba)
        .map_err(|e| bad(format!("{e:?}")))?;
    Ok(Picture {
        width,
        height,
        rgba: Arc::new(rgba),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Solid, solid_jpeg, split_jpeg};

    fn near(a: [u8; 4], b: [u8; 3]) -> bool {
        a[3] == 255
            && a[..3]
                .iter()
                .zip(b)
                .all(|(x, y)| (i16::from(*x) - i16::from(y)).abs() <= 12)
    }

    #[test]
    fn a_solid_frame_decodes_to_its_colour() {
        for (colour, rgb) in [
            (Solid::Red, [255, 0, 0]),
            (Solid::Green, [0, 255, 0]),
            (Solid::Blue, [0, 0, 255]),
            (Solid::White, [255, 255, 255]),
        ] {
            let p = decode_jpeg(solid_jpeg(colour)).unwrap();
            assert_eq!((p.width, p.height), (16, 16));
            assert_eq!(p.bytes(), 16 * 16 * 4);
            assert!(near(p.pixel(8, 8).unwrap(), rgb), "{colour:?}");
        }
    }

    #[test]
    fn rows_run_top_to_bottom_and_channels_are_red_green_blue() {
        let p = decode_jpeg(split_jpeg()).unwrap();
        assert_eq!((p.width, p.height), (32, 16));
        assert!(near(p.pixel(4, 8).unwrap(), [255, 0, 0]));
        assert!(near(p.pixel(27, 8).unwrap(), [0, 0, 255]));
        assert_eq!(p.pixel(32, 0), None);
    }

    #[test]
    fn bytes_that_are_not_a_jpeg_are_an_error_not_a_panic() {
        assert!(matches!(decode_jpeg(b""), Err(VideoError::Proxy(_))));
        assert!(matches!(
            decode_jpeg(b"not a picture at all"),
            Err(VideoError::Proxy(_))
        ));
        let whole = solid_jpeg(Solid::Red);
        // Cut short (the decoder may fill what is missing): it must not panic. Then with the
        // header damaged: an error.
        let _ = decode_jpeg(&whole[..whole.len() / 2]);
        let mut damaged = whole.to_vec();
        damaged[2..40].fill(0);
        assert!(decode_jpeg(&damaged).is_err());
    }
}
