//! Perceptual pixel goldens: a pane is rasterised by the CPU canvas at a fixed size and scale
//! and compared with a PNG in `tests/pixel/`.
//!
//! The comparison is tolerant, not byte equality: a pixel differs when any channel moves by more
//! than [`Tolerance::channel`]; the image fails when too large a share of its pixels differ, or
//! when the mean structural similarity of its luminance (8x8 blocks) falls under a floor. The CPU
//! rasteriser is close to the GPU path but not equal to it, and text is shaped with the fonts the
//! machine has, so a golden made on one OS is compared more loosely on another (see
//! [`Tolerance::for_this_machine`]); a PNG named `<name>.<os>.png` pins one OS exactly.
//!
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1` (it writes the reference PNG, or the OS-pinned one
//! when that exists). A failure writes the actual image and a diff image (differing pixels in
//! red over a dimmed copy of the golden) to `target/pixel-diff/`, which CI uploads.
#![allow(dead_code, reason = "each test binary uses a subset")]

use super::cpu::CpuCanvas;
use atomik_ui::kurbo::{Affine, Size};
use atomik_ui::{Canvas, TextEngine};
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

/// An RGBA8 image (rows top to bottom, straight alpha; the panes are opaque).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The same image moved `dx` pixels right and `dy` down, the uncovered edge copied from the
    /// nearest row or column (a layout shift in a test).
    pub fn shifted(&self, dx: i32, dy: i32) -> Image {
        let mut out = self.clone();
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let sx = (x - dx).clamp(0, self.width as i32 - 1) as u32;
                let sy = (y - dy).clamp(0, self.height as i32 - 1) as u32;
                let from = ((sy * self.width + sx) * 4) as usize;
                let to = ((y as u32 * self.width + x as u32) * 4) as usize;
                out.rgba[to..to + 4].copy_from_slice(&self.rgba[from..from + 4]);
            }
        }
        out
    }

    /// The image with the pixels of `rect` (x0, y0, x1, y1) set to `rgba` (a clipped label in a
    /// test).
    pub fn painted(&self, rect: (u32, u32, u32, u32), rgba: [u8; 4]) -> Image {
        let mut out = self.clone();
        for y in rect.1..rect.3.min(self.height) {
            for x in rect.0..rect.2.min(self.width) {
                let at = ((y * self.width + x) * 4) as usize;
                out.rgba[at..at + 4].copy_from_slice(&rgba);
            }
        }
        out
    }

    fn luma(&self, x: u32, y: u32) -> f64 {
        let at = ((y * self.width + x) * 4) as usize;
        let p = &self.rgba[at..at + 4];
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
    }
}

/// How far an image may stray from its golden.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// A pixel differs when any channel is further than this from the golden (0..=255).
    pub channel: u8,
    /// The most pixels that may differ, as a share of the image.
    pub max_share: f64,
    /// The least mean luminance SSIM (1.0 = identical).
    pub min_ssim: f64,
}

impl Tolerance {
    /// The same OS and rasteriser as the golden: only rounding noise passes.
    pub const SAME_OS: Tolerance = Tolerance {
        channel: 12,
        max_share: 0.002,
        min_ssim: 0.995,
    };
    /// Another OS: the same shapes, but text from other fonts and hinting.
    pub const OTHER_OS: Tolerance = Tolerance {
        channel: 40,
        max_share: 0.04,
        min_ssim: 0.90,
    };

    /// The tolerance for comparing on this machine with a golden made on `golden_os`.
    pub fn for_this_machine(golden_os: &str) -> Tolerance {
        if golden_os == std::env::consts::OS {
            Self::SAME_OS
        } else {
            Self::OTHER_OS
        }
    }
}

/// The measurements of one comparison.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub differing: usize,
    pub share: f64,
    pub ssim: f64,
}

fn differs(a: &[u8; 4], g: &[u8; 4], tolerance: Tolerance) -> bool {
    a.iter()
        .zip(g)
        .any(|(a, g)| a.abs_diff(*g) > tolerance.channel)
}

/// The mean SSIM of the luminance of two same-size images over 8x8 blocks.
pub fn ssim(a: &Image, b: &Image) -> f64 {
    const BLOCK: u32 = 8;
    let (c1, c2) = ((0.01_f64 * 255.0).powi(2), (0.03_f64 * 255.0).powi(2));
    let (mut sum, mut blocks) = (0.0, 0_u32);
    for by in (0..a.height).step_by(BLOCK as usize) {
        for bx in (0..a.width).step_by(BLOCK as usize) {
            let (w, h) = (BLOCK.min(a.width - bx), BLOCK.min(a.height - by));
            let n = f64::from(w * h);
            let (mut ma, mut mb) = (0.0, 0.0);
            for y in by..by + h {
                for x in bx..bx + w {
                    ma += a.luma(x, y);
                    mb += b.luma(x, y);
                }
            }
            let (ma, mb) = (ma / n, mb / n);
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for y in by..by + h {
                for x in bx..bx + w {
                    let (da, db) = (a.luma(x, y) - ma, b.luma(x, y) - mb);
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            let (va, vb, cov) = (va / n, vb / n, cov / n);
            sum += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            blocks += 1;
        }
    }
    if blocks == 0 {
        1.0
    } else {
        sum / f64::from(blocks)
    }
}

/// Compares `actual` with `golden`: `Ok` with the measurements when inside `tolerance`, `Err`
/// with a message otherwise (a different size always fails).
pub fn compare(actual: &Image, golden: &Image, tolerance: Tolerance) -> Result<Report, String> {
    if (actual.width, actual.height) != (golden.width, golden.height) {
        return Err(format!(
            "size {}x{} against the golden's {}x{}",
            actual.width, actual.height, golden.width, golden.height
        ));
    }
    let differing = actual
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(golden.rgba.as_chunks::<4>().0)
        .filter(|(a, g)| differs(a, g, tolerance))
        .count();
    let total = (actual.width * actual.height).max(1) as usize;
    let report = Report {
        differing,
        share: differing as f64 / total as f64,
        ssim: ssim(actual, golden),
    };
    if report.share > tolerance.max_share {
        return Err(format!(
            "{:.2}% of the pixels differ (at most {:.2}%)",
            report.share * 100.0,
            tolerance.max_share * 100.0
        ));
    }
    if report.ssim < tolerance.min_ssim {
        return Err(format!(
            "structural similarity {:.4} is under {:.4}",
            report.ssim, tolerance.min_ssim
        ));
    }
    Ok(report)
}

/// Paints `size` logical pixels at `scale` on the CPU canvas. `paint` receives the canvas with
/// the scale transform already pushed, so it draws in logical units (the way the app does).
pub fn render(size: Size, scale: f64, paint: impl FnOnce(&mut dyn Canvas)) -> Image {
    let (w, h) = (
        (size.width * scale).round() as u16,
        (size.height * scale).round() as u16,
    );
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    canvas.push_transform(Affine::scale(scale));
    paint(&mut canvas);
    canvas.pop_transform();
    let pixmap = canvas.finish();
    let mut rgba = pixmap.data_as_u8_slice().to_vec();
    // Straight alpha for the PNG (the panes are opaque, so this only matters at the rim).
    unpremultiply(&mut rgba);
    Image {
        width: u32::from(w),
        height: u32::from(h),
        rgba,
    }
}

fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Encodes `image` as a PNG.
pub fn encode(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, image.width, image.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut w = enc.write_header().expect("png header");
    w.write_image_data(&image.rgba).expect("png data");
    w.finish().expect("png end");
    out
}

/// Decodes an RGBA8 PNG.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{:?}/{:?} is not RGBA8",
            info.color_type, info.bit_depth
        ));
    }
    buf.truncate(info.buffer_size());
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba: buf,
    })
}

/// The red-on-dim picture of where `actual` leaves `golden` (same size).
pub fn diff_image(actual: &Image, golden: &Image, tolerance: Tolerance) -> Image {
    let mut out = golden.clone();
    let pairs = actual
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(golden.rgba.as_chunks::<4>().0);
    for (o, (a, g)) in out.rgba.as_chunks_mut::<4>().0.iter_mut().zip(pairs) {
        if differs(a, g, tolerance) {
            *o = [255, 0, 0, 255];
        } else {
            for c in &mut o[..3] {
                *c /= 3;
            }
        }
    }
    out
}

fn pixel_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pixel")
}

fn artefact_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/pixel-diff")
}

/// Compares `actual` with `tests/pixel/<name>.png` (or the OS-pinned `<name>.<os>.png`), or
/// writes it when `ATOMIK_UPDATE_GOLDEN=1`. Panics with the measurements on a difference.
pub fn check(name: &str, actual: &Image) {
    let os = std::env::consts::OS;
    let pinned = pixel_dir().join(format!("{name}.{os}.png"));
    let reference = pixel_dir().join(format!("{name}.png"));
    let update = std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1");
    // The OS-pinned golden wins on its own OS and is compared strictly.
    let (path, tolerance) = if pinned.exists() {
        (pinned, Tolerance::SAME_OS)
    } else {
        (reference, Tolerance::for_this_machine(REFERENCE_OS))
    };
    if update {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("create tests/pixel");
        std::fs::write(&path, encode(actual)).expect("write golden");
        return;
    }
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()));
    let golden = decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if let Err(why) = compare(actual, &golden, tolerance) {
        let dir = artefact_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{name}.actual.png")), encode(actual));
        let _ = std::fs::write(dir.join(format!("{name}.golden.png")), &bytes);
        if (actual.width, actual.height) == (golden.width, golden.height) {
            let _ = std::fs::write(
                dir.join(format!("{name}.diff.png")),
                encode(&diff_image(actual, &golden, tolerance)),
            );
        }
        panic!(
            "{name} differs from {}: {why}\n(images in target/pixel-diff/; regenerate with \
             {UPDATE_ENV}=1)",
            path.display()
        );
    }
}

/// The OS the reference PNGs were made on.
pub const REFERENCE_OS: &str = "windows";
