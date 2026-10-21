//! Renders PNG previews of the file browser (tree and list, narrow, menu open) with vello_cpu.
//! Run: `cargo test -p atomik-ui --test file_browser_preview -- --ignored`. Writes
//! `target/preview/file_browser_*.png`, not committed.

mod support;

use atomik_library::{AudioFormat, AudioInfo, FileKind, FileRow};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    FileBrowserView, Modifiers, Place, PlacesData, PointerButton, TextEngine, Theme, TreeChild,
    UiEvent,
};
use std::path::{Path, PathBuf};
use support::cpu::CpuCanvas;

fn row(name: &str, kind: FileKind, tags: &[&str], favorite: bool, info: bool) -> FileRow {
    FileRow {
        path: PathBuf::from(format!("/lib/{name}")),
        name: name.to_string(),
        kind,
        size: 183_000,
        info: info.then_some(AudioInfo {
            sample_rate: 44100,
            channels: 2,
            frames: Some(44100 * 5 / 2),
        }),
        favorite,
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
    }
}

fn view(width: f64) -> FileBrowserView {
    let wav = FileKind::Audio(AudioFormat::Wav);
    let mut v = FileBrowserView::new(Theme::default());
    v.set_bounds(Rect::new(0.0, 0.0, width, 600.0));
    v.set_scale(1.5);
    v.set_focused(true);
    v.set_places(PlacesData {
        favorites: 3,
        tags: vec![("808".into(), 4), ("snare".into(), 2), ("loop".into(), 9)],
        bookmarks: vec![PathBuf::from("/music/drums")],
        roots: vec![PathBuf::from("C:\\"), PathBuf::from("/home/me")],
    });
    v.set_children(
        Path::new("/music/drums"),
        vec![TreeChild {
            name: "kicks".into(),
            path: PathBuf::from("/music/drums/kicks"),
        }],
    );
    v.set_location(Place::Folder(PathBuf::from("/music/drums/kicks")));
    v.set_rows(vec![
        row("808 pack", FileKind::Folder, &[], false, false),
        row("Kick 01.wav", wav, &["808", "punchy"], true, true),
        row("Kick 02.wav", wav, &["808"], false, true),
        row(
            "Kick 10 with a very long descriptive name.wav",
            wav,
            &[],
            false,
            true,
        ),
        row("Snare.flac", wav, &["snare"], false, false),
        row("Hat.wav", wav, &[], true, true),
    ]);
    let click = |v: &mut FileBrowserView, p: Point| {
        v.handle(&UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        });
        v.handle(&UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        });
    };
    click(&mut v, Point::new(15.0, 34.0 + 20.0 * 5.0 + 10.0));
    click(&mut v, Point::new(80.0, 308.4 + 22.0 * 2.0 + 11.0));
    v.set_preview(Some(PathBuf::from("/lib/Kick 01.wav")), 0.4);
    v
}

fn render(v: &mut FileBrowserView, width: f64, file: &str) {
    let scale = 1.5;
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new((width * scale) as u16, (600.0 * scale) as u16, &mut text);
    v.paint(&mut canvas);
    let png = canvas.finish().into_png().expect("png");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn file_browser_preview_png() {
    let mut v = view(300.0);
    render(&mut v, 300.0, "file_browser_default.png");
    let mut wide = view(520.0);
    render(&mut wide, 520.0, "file_browser_wide.png");
    let mut narrow = view(FileBrowserView::MIN_WIDTH);
    render(
        &mut narrow,
        FileBrowserView::MIN_WIDTH,
        "file_browser_narrow.png",
    );
    let mut menu = view(300.0);
    menu.handle(&UiEvent::PointerDown {
        pos: Point::new(80.0, 308.4 + 22.0 * 3.0 + 11.0),
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    render(&mut menu, 300.0, "file_browser_menu.png");
}
