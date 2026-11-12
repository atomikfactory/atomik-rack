//! Pixel formats to RGB8, and the shrinking of a big frame. Pure code, the same on every system.

use super::{CameraError, MAX_HEIGHT, MAX_WIDTH};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// How the camera delivers its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Motion JPEG: one JPEG picture per frame.
    Mjpeg,
    /// Packed 4:2:2, `Y0 U Y1 V` for each pair of pixels.
    Yuyv,
    /// Planar 4:2:0: the `Y` plane, then interleaved `U V` at half size.
    Nv12,
    /// One byte of grey per pixel.
    Gray,
    /// `R G B` per pixel.
    Rgb,
    /// `B G R` per pixel.
    Bgr,
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// One pixel of the usual video-range conversion (BT.601, integer maths).
fn yuv(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    [
        clamp((298 * c + 409 * e + 128) >> 8),
        clamp((298 * c - 100 * d - 208 * e + 128) >> 8),
        clamp((298 * c + 516 * d + 128) >> 8),
    ]
}

fn size_error(format: PixelFormat, got: usize, want: usize) -> CameraError {
    CameraError::Frame(format!("{format:?} frame has {got} bytes, expected {want}"))
}

/// Converts one frame of `format` (`width` by `height`, which must be even for the subsampled
/// formats) to RGB8 in `out`. Returns the size of the picture, which for Motion JPEG is the one
/// the picture itself says.
pub fn to_rgb(
    format: PixelFormat,
    width: u32,
    height: u32,
    bytes: &[u8],
    out: &mut Vec<u8>,
) -> Result<(u32, u32), CameraError> {
    let (w, h) = (width as usize, height as usize);
    out.clear();
    match format {
        PixelFormat::Mjpeg => {
            let options = DecoderOptions::default()
                .set_max_width(4096)
                .set_max_height(4096)
                .jpeg_set_out_colorspace(ColorSpace::RGB);
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
            let pixels = decoder
                .decode()
                .map_err(|e| CameraError::Frame(format!("bad JPEG: {e:?}")))?;
            let info = decoder
                .info()
                .ok_or_else(|| CameraError::Frame("bad JPEG: no header".into()))?;
            *out = pixels;
            Ok((u32::from(info.width), u32::from(info.height)))
        }
        PixelFormat::Yuyv => {
            if w % 2 != 0 || bytes.len() < w * h * 2 {
                return Err(size_error(format, bytes.len(), w * h * 2));
            }
            out.reserve(w * h * 3);
            for quad in bytes[..w * h * 2].as_chunks::<4>().0 {
                out.extend_from_slice(&yuv(quad[0], quad[1], quad[3]));
                out.extend_from_slice(&yuv(quad[2], quad[1], quad[3]));
            }
            Ok((width, height))
        }
        PixelFormat::Nv12 => {
            let need = w * h + w * h / 2;
            if w % 2 != 0 || h % 2 != 0 || bytes.len() < need {
                return Err(size_error(format, bytes.len(), need));
            }
            let (luma, chroma) = bytes.split_at(w * h);
            out.reserve(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    let c = (y / 2) * w + (x / 2) * 2;
                    out.extend_from_slice(&yuv(luma[y * w + x], chroma[c], chroma[c + 1]));
                }
            }
            Ok((width, height))
        }
        PixelFormat::Gray => {
            if bytes.len() < w * h {
                return Err(size_error(format, bytes.len(), w * h));
            }
            out.reserve(w * h * 3);
            for &g in &bytes[..w * h] {
                out.extend_from_slice(&[g, g, g]);
            }
            Ok((width, height))
        }
        PixelFormat::Rgb => {
            if bytes.len() < w * h * 3 {
                return Err(size_error(format, bytes.len(), w * h * 3));
            }
            out.extend_from_slice(&bytes[..w * h * 3]);
            Ok((width, height))
        }
        PixelFormat::Bgr => {
            if bytes.len() < w * h * 3 {
                return Err(size_error(format, bytes.len(), w * h * 3));
            }
            out.reserve(w * h * 3);
            for px in bytes[..w * h * 3].as_chunks::<3>().0 {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            Ok((width, height))
        }
    }
}

/// A copy of `rgb` (`width` by `height`) shrunk by a whole `factor`, each new pixel the average of
/// the block of `factor` by `factor` pixels it covers. The size is `width / factor` by
/// `height / factor`.
pub fn shrink_by(rgb: &[u8], width: u32, height: u32, factor: usize) -> Vec<u8> {
    let factor = factor.max(1);
    let (w, h) = (width as usize, height as usize);
    let (nw, nh) = (w / factor, h / factor);
    let area = (factor * factor) as u32;
    let mut out = Vec::with_capacity(nw * nh * 3);
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 3];
            for dy in 0..factor {
                let row = ((y * factor + dy) * w + x * factor) * 3;
                for dx in 0..factor {
                    for (k, s) in sum.iter_mut().enumerate() {
                        *s += u32::from(rgb[row + dx * 3 + k]);
                    }
                }
            }
            out.extend(sum.iter().map(|s| ((s + area / 2) / area) as u8));
        }
    }
    out
}

/// Shrinks `rgb` in place by the whole factor that brings it within 640 by 480. A picture within
/// the limits is left as it is. Returns the new size.
pub fn shrink_to_fit(rgb: &mut Vec<u8>, width: u32, height: u32) -> (u32, u32) {
    let factor = width
        .div_ceil(MAX_WIDTH)
        .max(height.div_ceil(MAX_HEIGHT))
        .max(1) as usize;
    if factor == 1 {
        return (width, height);
    }
    *rgb = shrink_by(rgb, width, height, factor);
    (width / factor as u32, height / factor as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_video_levels_map_to_grey() {
        // Video-range black, mid grey and white with neutral chroma.
        assert_eq!(yuv(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv(235, 128, 128), [255, 255, 255]);
        let mid = yuv(126, 128, 128);
        assert!(mid[0] == mid[1] && mid[1] == mid[2] && (mid[0] as i32 - 128).abs() <= 1);
    }

    #[test]
    fn yuyv_gives_two_pixels_per_four_bytes() {
        let mut out = Vec::new();
        // A red pair: Y 81, U 90, V 240 in video range.
        let (w, h) = to_rgb(PixelFormat::Yuyv, 2, 1, &[81, 90, 81, 240], &mut out).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(out.len(), 6);
        assert!(out[0] > 230 && out[1] < 25 && out[2] < 25, "{out:?}");
    }

    #[test]
    fn nv12_uses_one_chroma_pair_per_two_by_two_pixels() {
        let mut out = Vec::new();
        let mut bytes = vec![81u8; 4]; // four red pixels
        bytes.extend_from_slice(&[90, 240]);
        to_rgb(PixelFormat::Nv12, 2, 2, &bytes, &mut out).unwrap();
        assert_eq!(out.len(), 12);
        for px in out.chunks(3) {
            assert!(px[0] > 230 && px[1] < 25 && px[2] < 25, "{px:?}");
        }
    }

    #[test]
    fn short_frames_are_errors_not_panics() {
        let mut out = Vec::new();
        for f in [
            PixelFormat::Yuyv,
            PixelFormat::Nv12,
            PixelFormat::Gray,
            PixelFormat::Rgb,
            PixelFormat::Bgr,
        ] {
            assert!(to_rgb(f, 4, 4, &[0; 3], &mut out).is_err(), "{f:?}");
        }
        assert!(to_rgb(PixelFormat::Mjpeg, 4, 4, b"not a jpeg", &mut out).is_err());
    }

    #[test]
    fn bgr_is_swapped_and_gray_is_spread() {
        let mut out = Vec::new();
        to_rgb(PixelFormat::Bgr, 1, 1, &[1, 2, 3], &mut out).unwrap();
        assert_eq!(out, [3, 2, 1]);
        to_rgb(PixelFormat::Gray, 2, 1, &[7, 9], &mut out).unwrap();
        assert_eq!(out, [7, 7, 7, 9, 9, 9]);
    }

    #[test]
    fn a_big_frame_is_shrunk_to_fit_and_a_small_one_is_left() {
        let mut big = vec![100u8; 1280 * 720 * 3];
        let (w, h) = shrink_to_fit(&mut big, 1280, 720);
        assert_eq!((w, h), (640, 360));
        assert_eq!(big.len(), 640 * 360 * 3);
        assert!(big.iter().all(|&b| b == 100));
        let mut small = vec![1u8; 320 * 240 * 3];
        assert_eq!(shrink_to_fit(&mut small, 320, 240), (320, 240));
        assert_eq!(small.len(), 320 * 240 * 3);
        // 1920 by 1080 shrinks by three (a whole factor): 640 by 360.
        let mut full_hd = vec![0u8; 1920 * 1080 * 3];
        assert_eq!(shrink_to_fit(&mut full_hd, 1920, 1080), (640, 360));
    }

    #[test]
    fn shrinking_averages_each_block() {
        let mut px = vec![0u8; 1280 * 480 * 3];
        // Width 1280 shrinks by two: the pair (0, 200) averages to 100.
        for y in 0..480 {
            px[(y * 1280 + 1) * 3] = 200;
        }
        let (w, h) = shrink_to_fit(&mut px, 1280, 480);
        assert_eq!((w, h), (640, 240));
        assert_eq!(px[0], 100); // the two by two block holds 0, 200, 0, 200
    }
}
