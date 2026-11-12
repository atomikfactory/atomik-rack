//! The little image work the models need: a rotated, scaled crop of an RGB frame into the
//! float tensor a model takes. Everything outside the frame reads as black, as the models were
//! trained.

/// A borrowed RGB8 image.
#[derive(Debug, Clone, Copy)]
pub struct Rgb<'a> {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height * 3` bytes, row by row, red first.
    pub data: &'a [u8],
}

impl Rgb<'_> {
    fn at(&self, x: isize, y: isize) -> [f32; 3] {
        if x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize {
            return [0.0; 3];
        }
        let i = (y as usize * self.width + x as usize) * 3;
        [
            f32::from(self.data[i]),
            f32::from(self.data[i + 1]),
            f32::from(self.data[i + 2]),
        ]
    }

    /// The colour at `(x, y)`, where a pixel covers `i..i + 1` (so its centre is `i + 0.5`),
    /// bilinear.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let (a, b, c, d) = (
            self.at(x0, y0),
            self.at(x0 + 1, y0),
            self.at(x0, y0 + 1),
            self.at(x0 + 1, y0 + 1),
        );
        let mut out = [0.0; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bottom - top) * ty;
        }
        out
    }
}

/// A square window on the frame, in pixels: its centre, its side and how far it is turned.
/// Positive `rotation` turns the window clockwise in the image (y down), so a hand whose fingers
/// point to the right needs `+90 degrees` to be upright in the crop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    /// Centre, in pixels.
    pub cx: f32,
    /// Centre, in pixels.
    pub cy: f32,
    /// Side of the square, in pixels.
    pub size: f32,
    /// Turn in radians.
    pub rotation: f32,
}

impl Roi {
    /// Where a point of the window lands in the frame: `(u, v)` are the coordinates in the window,
    /// 0 to 1 from its top-left corner (before the turn).
    pub fn to_frame(&self, u: f32, v: f32) -> (f32, f32) {
        let (ox, oy) = ((u - 0.5) * self.size, (v - 0.5) * self.size);
        let (s, c) = self.rotation.sin_cos();
        (self.cx + ox * c - oy * s, self.cy + ox * s + oy * c)
    }
}

/// Fills `out` (length `n * n * 3`, rows of RGB, values 0 to 1) with the window `roi` of `img`
/// scaled to `n` by `n`.
pub fn crop(img: &Rgb<'_>, roi: &Roi, n: usize, out: &mut Vec<f32>) {
    out.clear();
    out.reserve(n * n * 3);
    let (s, c) = roi.rotation.sin_cos();
    let step = roi.size / n as f32;
    for j in 0..n {
        let oy = (j as f32 + 0.5) * step - roi.size * 0.5;
        for i in 0..n {
            let ox = (i as f32 + 0.5) * step - roi.size * 0.5;
            let px = img.sample(roi.cx + ox * c - oy * s, roi.cy + ox * s + oy * c);
            out.extend_from_slice(&[px[0] / 255.0, px[1] / 255.0, px[2] / 255.0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image whose red channel is the column and whose green channel is the row.
    fn ramp(w: usize, h: usize) -> Vec<u8> {
        let mut d = Vec::new();
        for y in 0..h {
            for x in 0..w {
                d.extend_from_slice(&[x as u8, y as u8, 0]);
            }
        }
        d
    }

    #[test]
    fn an_upright_crop_is_the_window_scaled() {
        let data = ramp(100, 100);
        let img = Rgb {
            width: 100,
            height: 100,
            data: &data,
        };
        let roi = Roi {
            cx: 50.0,
            cy: 50.0,
            size: 40.0,
            rotation: 0.0,
        };
        let mut out = Vec::new();
        crop(&img, &roi, 4, &mut out);
        // The first output pixel is the middle of the top-left quarter-cell: x = 30 + 5.
        assert!((out[0] * 255.0 - 34.5).abs() < 0.6, "{}", out[0] * 255.0);
        assert!((out[1] * 255.0 - 34.5).abs() < 0.6);
    }

    #[test]
    fn a_quarter_turn_makes_up_in_the_crop_point_right_in_the_frame() {
        let roi = Roi {
            cx: 50.0,
            cy: 50.0,
            size: 40.0,
            rotation: std::f32::consts::FRAC_PI_2,
        };
        // The middle of the top edge of the window (u = 0.5, v = 0).
        let (x, y) = roi.to_frame(0.5, 0.0);
        assert!(
            (x - 70.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3,
            "{x} {y}"
        );
        let (x, y) = roi.to_frame(0.5, 0.5);
        assert!((x - 50.0).abs() < 1e-4 && (y - 50.0).abs() < 1e-4);
    }

    #[test]
    fn outside_the_frame_is_black() {
        let data = ramp(10, 10);
        let img = Rgb {
            width: 10,
            height: 10,
            data: &data,
        };
        let roi = Roi {
            cx: 0.0,
            cy: 0.0,
            size: 8.0,
            rotation: 0.0,
        };
        let mut out = Vec::new();
        crop(&img, &roi, 8, &mut out);
        assert_eq!(&out[..3], &[0.0, 0.0, 0.0]);
        assert!(out[out.len() - 3] > 0.0 || out[out.len() - 2] > 0.0);
    }
}
