//! The application icon of the windows (taskbar, Alt-Tab, title bar).
//!
//! The 128 pixel PNG of `assets/icon` is embedded in the program and decoded once, on first use,
//! on the thread that opens a window (never on the audio path). The result is a plain RGBA
//! buffer; the platform layer wraps it in a window-system icon. macOS ignores window icons (its
//! Dock icon comes from the `.app` bundle) and so do some Wayland compositors.

use std::sync::OnceLock;

/// The embedded icon, a square 8-bit RGBA PNG.
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/atomik-rack-128.png");

/// A decoded icon: `size` x `size` pixels, straight alpha, RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Width and height in pixels.
    pub size: u32,
    /// `size * size * 4` bytes, row by row.
    pub rgba: Vec<u8>,
}

/// Decodes a square 8-bit RGBA PNG; `None` when it is anything else.
pub fn decode_icon(png_bytes: &[u8]) -> Option<IconImage> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
        || info.width != info.height
    {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some(IconImage {
        size: info.width,
        rgba: buf,
    })
}

/// The application icon, decoded the first time it is asked for. `None` if the embedded file
/// could not be decoded (the windows then keep the system's default icon).
pub fn app_icon() -> Option<&'static IconImage> {
    static ICON: OnceLock<Option<IconImage>> = OnceLock::new();
    ICON.get_or_init(|| decode_icon(ICON_PNG)).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes_to_a_square_rgba_picture() {
        let icon = app_icon().expect("the embedded icon decodes");
        assert_eq!(icon.size, 128);
        assert_eq!(icon.rgba.len(), 128 * 128 * 4);
        // The icon sits on an opaque black background (it reads on any taskbar): every pixel is
        // opaque, the corners are black and the artwork's green is there.
        assert!(icon.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert_eq!(&icon.rgba[..4], &[0, 0, 0, 255]);
        assert!(icon.rgba.as_chunks::<4>().0.iter().any(|p| p[1] > 150));
    }

    #[test]
    fn a_picture_that_is_not_square_rgba_is_refused() {
        assert_eq!(decode_icon(b"not a png"), None);
        let mut gray = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut gray, 2, 2);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[0; 4]).unwrap();
        }
        assert_eq!(decode_icon(&gray), None);
    }
}
