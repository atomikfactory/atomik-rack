//! The glow a theme can put on the level meters and the playhead line.
//!
//! A [`Glow`] is a theme property (the optional `glow` section of a theme file). It is drawn as a
//! few stacked, widening, translucent shapes behind the thing that glows: no blur pass, no
//! gradient and no allocation, so a frame that draws it costs a handful of extra fills. The
//! Light and Dark themes leave it off; Synthwave turns it on. The app turns it off at Low power
//! (see `Shell::current_theme`), so a Low power frame never draws one of these shapes.

use crate::canvas::{Canvas, CanvasExt};
use vello::kurbo::{Rect, RoundedRect};
use vello::peniko::Color;

/// How many widening shapes make one glow.
const LAYERS: usize = 4;

/// The least intensity and radius that draws anything.
const MIN_INTENSITY: f32 = 0.01;
const MIN_RADIUS: f32 = 0.5;

/// The most the radius of a glow can be, in logical pixels.
pub const MAX_RADIUS: f32 = 16.0;

/// The glow of a theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    /// Whether the theme glows.
    pub enabled: bool,
    /// The colour of the glow; `None` is the accent: the colour of the thing that glows (the
    /// zone a meter is in, the playhead's own colour).
    pub colour: Option<Color>,
    /// How strong the glow is at the edge of the thing, 0 to 1.
    pub intensity: f32,
    /// How far it reaches, in logical pixels, 0 to [`MAX_RADIUS`].
    pub radius: f32,
}

impl Glow {
    /// No glow: what Light and Dark have.
    pub const OFF: Glow = Glow {
        enabled: false,
        colour: None,
        intensity: 0.5,
        radius: 6.0,
    };

    /// The same glow, switched off (the Low power look of a theme).
    pub fn disabled(self) -> Glow {
        Glow {
            enabled: false,
            ..self
        }
    }

    /// Whether drawing it changes a pixel.
    pub fn active(&self) -> bool {
        self.enabled && self.intensity >= MIN_INTENSITY && self.radius >= MIN_RADIUS
    }

    /// The glow with its numbers brought into range.
    pub fn clamped(self) -> Glow {
        Glow {
            intensity: self.intensity.clamp(0.0, 1.0),
            radius: self.radius.clamp(0.0, MAX_RADIUS),
            ..self
        }
    }

    /// A halo around `r`, behind it. `own` is the colour of the thing that glows, used when the
    /// theme's glow is the accent.
    pub fn halo<C: Canvas + ?Sized>(&self, c: &mut C, r: Rect, own: Color) {
        if !self.active() {
            return;
        }
        let base = self.colour.unwrap_or(own);
        let each = (f64::from(self.intensity) * 0.2).min(0.5) as f32;
        let radius = f64::from(self.radius);
        for k in 0..LAYERS {
            let spread = radius * (LAYERS - k) as f64 / LAYERS as f64;
            let shape = RoundedRect::from_rect(r.inflate(spread, spread), spread);
            c.fill_with(shape, base.multiply_alpha(each));
        }
    }

    /// A halo along a vertical line at `x` from `y0` to `y1` (the playhead).
    pub fn vline<C: Canvas + ?Sized>(&self, c: &mut C, x: f64, y0: f64, y1: f64, own: Color) {
        self.halo(c, Rect::new(x, y0, x, y1), own);
    }
}

impl Default for Glow {
    fn default() -> Self {
        Glow::OFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::DisplayList;

    fn count(g: &Glow) -> usize {
        let mut list = DisplayList::default();
        g.halo(&mut list, Rect::new(0.0, 0.0, 10.0, 40.0), Color::WHITE);
        list.len()
    }

    #[test]
    fn an_off_glow_draws_nothing_and_an_on_one_draws_its_layers() {
        assert_eq!(count(&Glow::OFF), 0);
        let on = Glow {
            enabled: true,
            ..Glow::OFF
        };
        assert_eq!(count(&on), LAYERS);
        assert_eq!(count(&on.disabled()), 0);
        let none = Glow {
            intensity: 0.0,
            ..on
        };
        assert_eq!(count(&none), 0);
    }
}
