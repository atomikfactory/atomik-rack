//! The light theme: display-list goldens of the rack, the console, the arrangement and the piano
//! roll; a live switch from one theme to the other leaves no stale colour behind (a view that
//! was switched paints exactly what a view built with the theme paints); a pixel check on the
//! CPU canvas that each theme is really light or dark. Regenerate the goldens with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test theme_light`.

mod support;

use atomik_ui::kurbo::Size;
use atomik_ui::{
    ArrangementView, Canvas, ChannelMeters, ConsoleView, DisplayList, Flip, PianoRollView,
    RackView, TextEngine, Theme, ThemeChoice, TransportBar, UiEvent,
};
use std::path::PathBuf;
use support::console::*;
use support::cpu::CpuCanvas;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn rack_list(v: &mut RackView) -> String {
    let (doc, reg) = (reference_doc(), registry());
    let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 700.0)), &doc, &reg);
    v.set_selected(Some(key("delay")));
    let mut dl = DisplayList::new();
    v.set_flip(Flip::Front);
    v.paint(&mut dl, &doc, &reg);
    v.set_flip(Flip::Back);
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn arrangement_list(v: &mut ArrangementView) -> String {
    let (doc, reg) = (song_doc(), registry());
    v.set_playhead(2 * BAR + PPQ + PPQ / 2);
    v.set_selected_clips(&[atomik_model::ClipId(2)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn roll_list(v: &mut PianoRollView) -> String {
    let (doc, reg) = (song_doc(), registry());
    v.set_playhead(2 * BAR + PPQ);
    v.set_selected_notes(&[4, 5, 6]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn console_list(v: &mut ConsoleView) -> String {
    let (doc, reg) = (console_doc(), console_registry());
    let m = ChannelMeters {
        out_peak_l: 0.5,
        out_peak_r: 1.0,
        out_rms_l: 0.25,
        out_rms_r: 0.4,
        gr: 0.7,
    };
    v.set_meters(&[(key("ch-bass"), m), (key("master"), m)]);
    v.set_frame_time(1.0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn transport_list(v: &mut TransportBar) -> String {
    let (doc, reg) = (song_doc(), registry());
    v.sync(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

fn rack(theme: Theme) -> RackView {
    RackView::new(theme)
}

fn arr(theme: Theme) -> ArrangementView {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_theme(theme);
    v
}

fn roll(theme: Theme) -> PianoRollView {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_theme(theme);
    v
}

fn con(theme: Theme) -> ConsoleView {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    v.set_theme(theme);
    v
}

#[test]
fn the_light_rack_matches_its_golden() {
    check("light_rack_1x.json", &rack_list(&mut rack(Theme::light())));
}

#[test]
fn the_light_arrangement_matches_its_golden() {
    check(
        "light_arrangement_1x.json",
        &arrangement_list(&mut arr(Theme::light())),
    );
}

#[test]
fn the_light_piano_roll_matches_its_golden() {
    check(
        "light_piano_roll_1x.json",
        &roll_list(&mut roll(Theme::light())),
    );
}

#[test]
fn the_light_console_matches_its_golden() {
    check(
        "light_console_1x.json",
        &console_list(&mut con(Theme::light())),
    );
}

/// A view painted with one theme, switched, paints what a view of the other theme paints: no
/// cached list or derived colour of the old theme survives (both ways).
#[test]
fn a_live_switch_leaves_no_stale_colours() {
    for (from, to) in [
        (Theme::dark(), Theme::light()),
        (Theme::light(), Theme::dark()),
    ] {
        let mut r = rack(from.clone());
        let _ = rack_list(&mut r);
        r.set_theme(to.clone());
        assert_eq!(rack_list(&mut r), rack_list(&mut rack(to.clone())), "rack");

        let mut a = arr(from.clone());
        let _ = arrangement_list(&mut a);
        a.set_theme(to.clone());
        assert_eq!(
            arrangement_list(&mut a),
            arrangement_list(&mut arr(to.clone())),
            "arrangement"
        );

        let mut p = roll(from.clone());
        let _ = roll_list(&mut p);
        p.set_theme(to.clone());
        assert_eq!(roll_list(&mut p), roll_list(&mut roll(to.clone())), "roll");

        let mut c = con(from.clone());
        let _ = console_list(&mut c);
        c.set_theme(to.clone());
        assert_eq!(
            console_list(&mut c),
            console_list(&mut con(to.clone())),
            "console"
        );

        let (doc, reg) = (song_doc(), registry());
        let mut t = transport_bar(&doc, &reg);
        t.set_theme(from.clone());
        let _ = transport_list(&mut t);
        t.set_theme(to.clone());
        let mut fresh = transport_bar(&doc, &reg);
        fresh.set_theme(to.clone());
        assert_eq!(
            transport_list(&mut t),
            transport_list(&mut fresh),
            "transport"
        );
    }
}

/// The dark theme is the default one and the light theme paints something else.
#[test]
fn the_two_themes_paint_differently() {
    assert_eq!(Theme::default(), Theme::dark());
    assert_ne!(
        rack_list(&mut rack(Theme::dark())),
        rack_list(&mut rack(Theme::light()))
    );
    assert_ne!(
        arrangement_list(&mut arr(Theme::dark())),
        arrangement_list(&mut arr(Theme::light()))
    );
}

fn mean_luminance(theme: Theme) -> f32 {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_theme(theme);
    let mut text = TextEngine::new();
    let (w, h) = (1200u16, 420u16);
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, &doc, &reg);
    let pixmap = canvas.finish();
    let data = pixmap.data_as_u8_slice();
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for px in data.as_chunks::<4>().0 {
        sum += 0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2]);
        n += 1;
    }
    (sum / n as f64 / 255.0) as f32
}

/// On the CPU canvas the dark arrangement is dark and the light one is light.
#[test]
fn the_pixels_of_each_theme_are_dark_or_light() {
    let dark = mean_luminance(Theme::dark());
    let light = mean_luminance(Theme::light());
    assert!(dark < 0.2, "dark arrangement mean luminance {dark:.2}");
    assert!(light > 0.6, "light arrangement mean luminance {light:.2}");
}

#[test]
fn a_theme_choice_round_trips_and_resolves() {
    for c in ThemeChoice::ALL {
        assert_eq!(ThemeChoice::from_id(c.id()), Some(c));
    }
    assert_eq!(ThemeChoice::from_id("sepia"), None);
    assert_eq!(ThemeChoice::Dark.theme(false), Theme::dark());
    assert_eq!(ThemeChoice::Light.theme(true), Theme::light());
    assert_eq!(ThemeChoice::System.theme(true), Theme::dark());
    assert_eq!(ThemeChoice::System.theme(false), Theme::light());
    assert_eq!(ThemeChoice::Dark.next().next().next(), ThemeChoice::Dark);
}

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_light_png() {
    let (doc, reg) = (song_doc(), registry());
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(1200, 1480, &mut text);
    let theme = Theme::light();
    let mut a = arrangement(&doc, &reg);
    a.set_theme(theme.clone());
    a.paint(&mut canvas, &doc, &reg);
    canvas.push_transform(atomik_ui::kurbo::Affine::translate((0.0, 420.0)));
    let mut p = piano_roll(&doc, &reg);
    p.set_theme(theme.clone());
    p.paint(&mut canvas, &doc, &reg);
    canvas.pop_transform();
    let png = canvas.finish().into_png().expect("png");
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/preview/light_seq.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();

    let (rdoc, rreg) = (reference_doc(), registry());
    let scale = 1.0;
    let size = Size::new(820.0, 700.0);
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(1640, 700, &mut text);
    for (i, flip) in [Flip::Front, Flip::Back].into_iter().enumerate() {
        let mut v = RackView::new(theme.clone());
        let _ = v.handle(&UiEvent::Resize(size), &rdoc, &rreg);
        v.set_flip(flip);
        v.set_selected(Some(key("delay")));
        canvas.push_transform(atomik_ui::kurbo::Affine::translate((
            size.width * scale * i as f64,
            0.0,
        )));
        v.paint(&mut canvas, &rdoc, &rreg);
        canvas.pop_transform();
    }
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/light_rack.png");
    std::fs::write(&path, png).unwrap();

    let (cdoc, creg) = (console_doc(), console_registry());
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(1000, 600, &mut text);
    let mut c = console_view(&cdoc, &creg);
    c.set_theme(theme);
    c.paint(&mut canvas, &cdoc, &creg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/light_console.png");
    std::fs::write(&path, png).unwrap();
}
