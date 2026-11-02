//! Derives the application icon files from the master logo.
//!
//! ```sh
//! cargo run --release -p atomik-app --example make_icons -- assets/icon/atomik-rack.png assets/icon assets/logo
//! ```
//!
//! The icons are composited on an opaque black background, so they read on any taskbar or dock.
//! The optional third argument is a folder that gets `monogram.png` (transparent) and
//! `monogram-black.png`, both 512 pixels.
//!
//! Reads the master PNG (8-bit RGBA), crops it to the square around the artwork (the pixels
//! that are not transparent, plus a small margin, so the logo fills the icon) and writes, into
//! the output folder: `atomik-rack-{128,256,512}.png`,
//! `atomik-rack.ico` (bitmap frames 16 to 128, a PNG frame at 256) and
//! `atomik-rack.icns` (PNG frames, 16 to 512 including the 2x sizes). The artwork is only
//! cropped and scaled, with area averaging on premultiplied colour, so small sizes stay crisp
//! and no colour bleeds out of the transparent pixels.

use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Margin around the artwork, as a fraction of its larger side.
const MARGIN: f64 = 0.04;
/// Sizes of the `.ico` frames.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// Size of the logo files `monogram.png` and `monogram-black.png`.
const LOGO_SIZE: u32 = 512;
/// PNG files that are kept next to the `.ico` and `.icns`.
const PNG_SIZES: [u32; 3] = [128, 256, 512];
/// `.icns` entries that hold a PNG: type code and pixel size.
const ICNS_ENTRIES: [(&[u8; 4], u32); 8] = [
    (b"icp4", 16),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
];

/// A square RGBA8 picture.
struct Image {
    size: u32,
    rgba: Vec<u8>,
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or("usage: make_icons <master.png> <out-dir>")?,
    );
    let out_dir = PathBuf::from(
        args.next()
            .ok_or("usage: make_icons <master.png> <out-dir>")?,
    );
    let logo_dir = args.next().map(PathBuf::from);
    let (width, height, pixels) = read_png(&input)?;
    let square = crop_to_artwork(width, height, &pixels)?;
    println!(
        "master {width}x{height}, artwork square {0}x{0}",
        square.size
    );

    // The logo files: the artwork on transparent and on an opaque black background.
    if let Some(dir) = logo_dir {
        let logo = scale(&square, LOGO_SIZE);
        write(&dir.join("monogram.png"), &encode_png(&logo, LOGO_SIZE)?)?;
        let black = on_black(&logo);
        write(
            &dir.join("monogram-black.png"),
            &encode_png(&black, LOGO_SIZE)?,
        )?;
    }

    // The icons sit on black so they read on any taskbar or dock.
    let mut frames: std::collections::BTreeMap<u32, Vec<u8>> = std::collections::BTreeMap::new();
    let mut images: std::collections::BTreeMap<u32, Image> = std::collections::BTreeMap::new();
    for size in ICO_SIZES
        .iter()
        .copied()
        .chain(ICNS_ENTRIES.iter().map(|e| e.1))
        .chain(PNG_SIZES)
    {
        if let std::collections::btree_map::Entry::Vacant(slot) = frames.entry(size) {
            let image = on_black(&scale(&square, size));
            slot.insert(encode_png(&image, size)?);
            images.insert(size, image);
        }
    }

    for size in PNG_SIZES {
        write(
            &out_dir.join(format!("atomik-rack-{size}.png")),
            &frames[&size],
        )?;
    }
    write(&out_dir.join("atomik-rack.ico"), &ico(&images, &frames))?;
    write(&out_dir.join("atomik-rack.icns"), &icns(&frames))?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "the master must be 8-bit RGBA, not {:?} {:?}",
            info.color_type, info.bit_depth
        ));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// The square around the pixels with any alpha, plus the margin (kept inside the picture).
fn crop_to_artwork(width: u32, height: u32, rgba: &[u8]) -> Result<Image, String> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if rgba[((y * width + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Err("the master has no visible pixels".into());
    }
    let side = f64::from((x1 - x0).max(y1 - y0));
    let side = (side * (1.0 + 2.0 * MARGIN)).round() as u32;
    let side = side.min(width).min(height);
    // Centre the square on the artwork, then keep it inside the picture.
    let cx = (x0 + x1) / 2;
    let cy = (y0 + y1) / 2;
    let left = cx.saturating_sub(side / 2).min(width - side);
    let top = cy.saturating_sub(side / 2).min(height - side);
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    for y in top..top + side {
        let start = ((y * width + left) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (side * 4) as usize]);
    }
    Ok(Image {
        size: side,
        rgba: out,
    })
}

/// Area-averaged resize on premultiplied colour (every output pixel is the exact average of the
/// source area it covers, with fractional edges).
fn scale(src: &Image, size: u32) -> Image {
    let s = src.size as usize;
    let n = size as usize;
    // Premultiplied copy.
    let mut pre = vec![0.0f32; s * s * 4];
    for (i, px) in src.rgba.as_chunks::<4>().0.iter().enumerate() {
        let a = f32::from(px[3]) / 255.0;
        for c in 0..3 {
            pre[i * 4 + c] = f32::from(px[c]) / 255.0 * a;
        }
        pre[i * 4 + 3] = a;
    }
    let ratio = s as f64 / n as f64;
    // Horizontal pass, then vertical.
    let weights = |o: usize| -> Vec<(usize, f32)> {
        let from = o as f64 * ratio;
        let to = from + ratio;
        let first = from.floor() as usize;
        let last = (to.ceil() as usize).min(s);
        (first..last)
            .map(|i| {
                let w = (to.min((i + 1) as f64) - from.max(i as f64)) / ratio;
                (i, w as f32)
            })
            .collect()
    };
    let table: Vec<Vec<(usize, f32)>> = (0..n).map(weights).collect();
    let mut tmp = vec![0.0f32; n * s * 4];
    for y in 0..s {
        for (ox, taps) in table.iter().enumerate() {
            for &(ix, w) in taps {
                for c in 0..4 {
                    tmp[(y * n + ox) * 4 + c] += pre[(y * s + ix) * 4 + c] * w;
                }
            }
        }
    }
    let mut out = vec![0u8; n * n * 4];
    for (oy, taps) in table.iter().enumerate() {
        for ox in 0..n {
            let mut acc = [0.0f32; 4];
            for &(iy, w) in taps {
                for c in 0..4 {
                    acc[c] += tmp[(iy * n + ox) * 4 + c] * w;
                }
            }
            let a = acc[3];
            let o = (oy * n + ox) * 4;
            for c in 0..3 {
                let v = if a > 0.0 { acc[c] / a } else { 0.0 };
                out[o + c] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    Image { size, rgba: out }
}

/// The picture composited over opaque black (straight-alpha output, alpha 255 everywhere).
fn on_black(src: &Image) -> Image {
    let mut rgba = src.rgba.clone();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
        px[3] = 255;
    }
    Image {
        size: src.size,
        rgba,
    }
}

fn encode_png(image: &Image, size: u32) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&image.rgba)
            .map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

/// An `.ico`. The 256 pixel frame is a PNG (Vista and later; its size is stored as 0) and the
/// smaller ones are classic 32-bit bitmaps (BGRA, bottom-up, plus the unused AND mask), the
/// layout every icon reader and resource compiler accepts.
fn ico(
    images: &std::collections::BTreeMap<u32, Image>,
    pngs: &std::collections::BTreeMap<u32, Vec<u8>>,
) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = ICO_SIZES
        .iter()
        .map(|&size| {
            if size >= 256 {
                pngs[&size].clone()
            } else {
                dib(&images[&size])
            }
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(ICO_SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * ICO_SIZES.len() as u32;
    for (size, frame) in ICO_SIZES.iter().zip(&frames) {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in &frames {
        out.extend_from_slice(frame);
    }
    out
}

/// A bitmap icon frame: a `BITMAPINFOHEADER` (height doubled for the mask), the BGRA pixels
/// from the bottom row up (straight alpha) and a zero AND mask, rows padded to 4 bytes.
fn dib(image: &Image) -> Vec<u8> {
    let n = image.size as usize;
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(n as i32).to_le_bytes());
    out.extend_from_slice(&(2 * n as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&[0; 24]); // BI_RGB, image size, resolution, colour counts
    for row in (0..n).rev() {
        for px in image.rgba[row * n * 4..(row + 1) * n * 4]
            .as_chunks::<4>()
            .0
        {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    let mask_row = n.div_ceil(32) * 4;
    out.resize(out.len() + mask_row * n, 0);
    out
}

/// An `.icns` whose entries are PNG files; every length is big-endian and counts its header.
fn icns(frames: &std::collections::BTreeMap<u32, Vec<u8>>) -> Vec<u8> {
    let body: usize = ICNS_ENTRIES.iter().map(|e| 8 + frames[&e.1].len()).sum();
    let mut out = Vec::with_capacity(8 + body);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((8 + body) as u32).to_be_bytes());
    for (code, size) in ICNS_ENTRIES {
        let png = &frames[&size];
        out.extend_from_slice(code);
        out.extend_from_slice(&((8 + png.len()) as u32).to_be_bytes());
        out.extend_from_slice(png);
    }
    out
}
