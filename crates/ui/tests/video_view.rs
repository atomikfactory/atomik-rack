//! The Video window: where the picture sits for each size, what it paints (the pixels of the
//! frame it was given, black outside the film, a progress bar, a message), the overlay texts and
//! the bar's buttons.

mod support;

use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::video_view::{
    VideoFrame, VideoImage, VideoScreen, VideoSize, VideoView, picture_rect,
};
use atomik_ui::{Modifiers, PointerButton, Theme, UiEvent};
use std::sync::Arc;
use support::pixel::{Image, render};

const SIZE: Size = Size::new(400.0, 250.0);

fn view() -> VideoView {
    let mut v = VideoView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, SIZE.width, SIZE.height));
    v
}

/// A picture of `w` by `h` that is red on its left half and blue on its right half.
fn split(serial: u64, w: u32, h: u32) -> VideoImage {
    let mut rgba = Vec::new();
    for _ in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&if x < w / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            });
        }
    }
    VideoImage {
        serial,
        width: w,
        height: h,
        rgba: Arc::new(rgba),
    }
}

fn frame(screen: VideoScreen) -> VideoFrame {
    VideoFrame {
        screen,
        film: (1920, 1080),
        timecode: "01:00:02:05".to_owned(),
        frame_number: Some(1_805),
    }
}

fn paint(v: &mut VideoView) -> Image {
    render(SIZE, 1.0, |c| v.paint(c))
}

fn px(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.rgba[at..at + 4].try_into().unwrap()
}

fn near(p: [u8; 4], rgb: [u8; 3]) -> bool {
    p[..3]
        .iter()
        .zip(rgb)
        .all(|(a, b)| (i16::from(*a) - i16::from(b)).abs() <= 24)
}

fn click(v: &mut VideoView, at: Point) {
    v.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::default(),
        clicks: 1,
    });
}

#[test]
fn fit_keeps_the_aspect_ratio_and_the_percentages_follow_the_film() {
    let area = Rect::new(0.0, 0.0, 400.0, 220.0);
    // 16:9 in 400x220: the height limits it (220 / 9 * 16 = 391).
    let fit = picture_rect(area, (1920, 1080), VideoSize::Fit);
    assert!((fit.height() - 220.0).abs() < 1e-9);
    assert!((fit.width() / fit.height() - 16.0 / 9.0).abs() < 1e-9);
    assert!((fit.center().x - 200.0).abs() < 1e-9 && (fit.center().y - 110.0).abs() < 1e-9);
    // A tall film is limited by the height too; a wide window leaves bars either side.
    let tall = picture_rect(area, (1080, 1920), VideoSize::Fit);
    assert!((tall.height() - 220.0).abs() < 1e-9 && tall.width() < 140.0);
    let half = picture_rect(area, (1920, 1080), VideoSize::Half);
    assert_eq!((half.width(), half.height()), (960.0, 540.0));
    let full = picture_rect(area, (1920, 1080), VideoSize::Full);
    assert_eq!((full.width(), full.height()), (1920.0, 1080.0));
    let double = picture_rect(area, (640, 360), VideoSize::Double);
    assert_eq!((double.width(), double.height()), (1280.0, 720.0));
    // A window with no height does not divide by zero.
    let none = picture_rect(Rect::new(0.0, 0.0, 0.0, 0.0), (1920, 1080), VideoSize::Fit);
    assert_eq!(none.width(), 0.0);
}

#[test]
fn the_window_paints_the_pixels_of_the_frame_it_was_given() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Picture(split(1, 64, 36))));
    let rect = v.film_rect();
    let image = paint(&mut v);
    let (y, left, right) = (
        rect.center().y as u32,
        (rect.x0 + rect.width() * 0.25) as u32,
        (rect.x0 + rect.width() * 0.75) as u32,
    );
    assert!(
        near(px(&image, left, y), [255, 0, 0]),
        "{:?}",
        px(&image, left, y)
    );
    assert!(
        near(px(&image, right, y), [0, 0, 255]),
        "{:?}",
        px(&image, right, y)
    );
    // Outside the film's rectangle (the window is wider than 16:9 above the bar) it is the
    // dark surround, not the picture.
    assert!(!near(px(&image, 2, y), [255, 0, 0]));
    assert!(!near(px(&image, 2, y), [0, 0, 255]));
}

#[test]
fn another_picture_replaces_the_first_and_the_same_serial_is_not_a_change() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Picture(split(1, 64, 36))));
    paint(&mut v);
    assert!(!v.needs_repaint());
    v.set_frame(frame(VideoScreen::Picture(split(1, 64, 36))));
    assert!(!v.needs_repaint(), "the same picture needs no repaint");
    let mut other = split(2, 64, 36);
    other.rgba = Arc::new([0, 255, 0, 255].repeat(64 * 36));
    v.set_frame(frame(VideoScreen::Picture(other)));
    assert!(v.needs_repaint());
    let rect = v.film_rect();
    let image = paint(&mut v);
    let at = (rect.center().x as u32, (rect.y0 + 20.0) as u32);
    assert!(near(px(&image, at.0, at.1), [0, 255, 0]));
}

#[test]
fn before_or_after_the_film_it_is_black_with_the_timecode() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Black));
    let rect = v.film_rect();
    let image = paint(&mut v);
    let mid = px(&image, rect.center().x as u32, rect.center().y as u32);
    assert!(near(mid, [0, 0, 0]), "{mid:?}");
    assert_eq!(v.overlay_texts(), (Some("01:00:02:05".to_owned()), None));
    // The plate of the timecode is painted: a dark box in the bottom left corner of the film.
    let plate = px(&image, (rect.x0 + 14.0) as u32, (rect.y1 - 8.0) as u32);
    assert!(plate[0] < 40, "{plate:?}");
}

#[test]
fn the_overlay_shows_the_timecode_and_or_the_frame_number() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Black));
    assert_eq!(v.overlay_texts(), (Some("01:00:02:05".to_owned()), None));
    let mut prefs = v.prefs();
    prefs.overlay.frame_number = true;
    v.set_prefs(prefs);
    assert_eq!(
        v.overlay_texts(),
        (Some("01:00:02:05".to_owned()), Some("F 1805".to_owned()))
    );
    prefs.overlay.timecode = false;
    v.set_prefs(prefs);
    assert_eq!(v.overlay_texts(), (None, Some("F 1805".to_owned())));
    // The command's toggle: anything showing goes off, then the timecode comes back.
    v.toggle_overlay();
    assert_eq!(v.overlay_texts(), (None, None));
    v.toggle_overlay();
    assert_eq!(v.overlay_texts(), (Some("01:00:02:05".to_owned()), None));
}

#[test]
fn a_message_and_a_progress_bar_paint_on_black() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Message(
        "The video is missing".to_owned(),
    )));
    let image = paint(&mut v);
    assert!(near(px(&image, 3, 3), [0, 0, 0]));
    // Some text pixels are lit.
    let lit = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 100)
        .count();
    assert!(lit > 30, "the message is drawn");
    v.set_frame(frame(VideoScreen::Progress(
        0.5,
        "Preparing video: 50%".to_owned(),
    )));
    assert!(v.needs_repaint());
    let image = paint(&mut v);
    // The bar's left half is filled in the highlight colour, the right half is the track.
    let y = (v.area().center().y + 9.0) as u32;
    let left = px(&image, (v.area().center().x - 60.0) as u32, y);
    let right = px(&image, (v.area().center().x + 60.0) as u32, y);
    assert_ne!(left, right, "the filled part differs from the track");
}

#[test]
fn the_bar_buttons_set_the_size_and_the_overlay() {
    let mut v = view();
    assert_eq!(v.prefs().size, VideoSize::Fit);
    let bar_y = SIZE.height - 15.0;
    // The first button is FIT at the left, then 50%, 100% and 200%.
    click(&mut v, Point::new(60.0, bar_y));
    assert_eq!(v.prefs().size, VideoSize::Half);
    click(&mut v, Point::new(105.0, bar_y));
    assert_eq!(v.prefs().size, VideoSize::Full);
    click(&mut v, Point::new(155.0, bar_y));
    assert_eq!(v.prefs().size, VideoSize::Double);
    click(&mut v, Point::new(15.0, bar_y));
    assert_eq!(v.prefs().size, VideoSize::Fit);
    // The right end of the bar: # (frame number) and TC (timecode).
    assert!(!v.prefs().overlay.frame_number);
    click(&mut v, Point::new(SIZE.width - 20.0, bar_y));
    assert!(v.prefs().overlay.frame_number);
    click(&mut v, Point::new(SIZE.width - 55.0, bar_y));
    assert!(!v.prefs().overlay.timecode);
    // A click on the picture does nothing.
    let before = v.prefs();
    click(&mut v, Point::new(100.0, 100.0));
    assert_eq!(v.prefs(), before);
    // A hovered button has a tooltip.
    assert!(v.help_at(Point::new(60.0, bar_y)).is_some());
    assert!(v.help_at(Point::new(100.0, 100.0)).is_none());
}

#[test]
fn a_larger_size_is_cut_off_at_the_window_and_the_bar_stays() {
    let mut v = view();
    v.set_frame(frame(VideoScreen::Picture(split(1, 64, 36))));
    let mut prefs = v.prefs();
    prefs.size = VideoSize::Double;
    v.set_prefs(prefs);
    let image = paint(&mut v);
    // The bar is not covered by the magnified picture: its first button is where it was.
    let bar = px(&image, 3, (SIZE.height - 3.0) as u32);
    assert!(
        !near(bar, [255, 0, 0]) && !near(bar, [0, 0, 255]),
        "{bar:?}"
    );
}
