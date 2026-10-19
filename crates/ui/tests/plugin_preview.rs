//! Renders PNG previews of the plugin panel, the parameter pane, the browser group, the settings
//! section and the banner with vello_cpu.
//! Run: `cargo test -p atomik-ui --test plugin_preview -- --ignored`. Writes
//! `target/preview/*.png` (not committed).

mod support;

use atomik_ui::kurbo::Size;
use atomik_ui::plugin_data::PluginDeviceStatus;
use atomik_ui::{Flip, RackView, TextEngine, Theme, UiEvent};
use support::cpu::CpuCanvas;
use support::plugin::*;
use support::*;

const SCALE: f64 = 1.5;

fn write(name: &str, canvas: CpuCanvas<'_>) {
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

fn rack_view(doc: &atomik_model::RackDoc, size: Size) -> RackView {
    let reg = registry();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(size), doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(SCALE), doc, &reg);
    v
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    let mut text = TextEngine::new();
    let d = plugin_rack();
    let size = Size::new(920.0, 520.0);
    let (pw, ph) = ((size.width * SCALE) as u16, (size.height * SCALE) as u16);
    let mut v = rack_view(d.doc(), size);
    v.set_plugin_catalog(catalog());
    v.set_plugin_status(
        &key("synth"),
        PluginDeviceStatus {
            has_editor: true,
            ..PluginDeviceStatus::default()
        },
    );
    v.set_plugin_status(
        &key("fx"),
        PluginDeviceStatus {
            missing: Some("not installed".into()),
            ..PluginDeviceStatus::default()
        },
    );
    v.set_device_meter_values(&key("synth"), &[0.0, 0.0, 0.0, 128.0, 2.0]);
    v.set_browser_visible(true);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("plugin_front.png", canvas);
    v.set_flip(Flip::Back);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &registry());
    write("plugin_back.png", canvas);
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_params_pane() {
    use atomik_ui::PluginParamsView;
    use atomik_ui::kurbo::Rect;
    let mut text = TextEngine::new();
    let reg = registry();
    let bounds = Rect::new(0.0, 0.0, 1000.0, 520.0);
    let (pw, ph) = (
        (bounds.width() * SCALE) as u16,
        (bounds.height() * SCALE) as u16,
    );
    let d = plugin_in_container();
    let mut v = PluginParamsView::new(Theme::dark());
    v.set_bounds(bounds);
    v.set_scale(SCALE);
    v.open(key("layer/s2"));
    v.sync(d.doc(), &reg);
    v.set_texts(&[(10, 0.0, "0.0 %".into()), (17, 0.0, "Low pass".into())]);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    write("params_scope.png", canvas);
    let d = big_rack();
    let mut v = PluginParamsView::new(Theme::dark());
    v.set_bounds(bounds);
    v.set_scale(SCALE);
    v.open(key("big"));
    v.sync(d.doc(), &reg);
    v.set_query("group 3");
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    write("params_big.png", canvas);
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_settings_and_banner() {
    use atomik_device_sdk::{PluginFormat, PluginRef};
    use atomik_ui::kurbo::Rect;
    use atomik_ui::plugin_banner::BannerKind;
    use atomik_ui::plugin_data::{
        FailedFileInfo, PluginEntryStatus, PluginSettingsData, ScanProgressInfo, SettingsPluginRow,
    };
    use atomik_ui::{PluginBannerView, PluginSettingsView};
    let mut text = TextEngine::new();
    let row = |id: &str, n: &str, s| SettingsPluginRow {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.into(),
        },
        name: n.into(),
        vendor: "Atomik Test".into(),
        version: "1.2.0".into(),
        status: s,
    };
    let data = PluginSettingsData {
        enabled: true,
        extra_paths: vec!["D:/Plugins/CLAP".into()],
        timeout_secs: 30,
        plugins: vec![
            row("a", "Test Synth", PluginEntryStatus::Ok),
            row("b", "Switched Off", PluginEntryStatus::Disabled),
            row(
                "c",
                "Surround",
                PluginEntryStatus::Unsupported("surround only".into()),
            ),
        ],
        failed: vec![FailedFileInfo {
            path: "C:/CLAP/bad.clap".into(),
            reason: "init failed".into(),
        }],
        scan: Some(ScanProgressInfo {
            done: 12,
            total: 40,
        }),
        scanner_available: true,
        last_scan: None,
        log: vec!["[Test Synth] loaded".into()],
    };
    let bounds = Rect::new(0.0, 0.0, 760.0, 520.0);
    let (pw, ph) = (
        (bounds.width() * SCALE) as u16,
        (bounds.height() * SCALE) as u16,
    );
    let mut v = PluginSettingsView::new(Theme::dark());
    v.set_bounds(bounds);
    v.set_scale(SCALE);
    v.set_data(data);
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas);
    write("settings_on.png", canvas);
    let mut v = PluginSettingsView::new(Theme::dark());
    v.set_bounds(bounds);
    v.set_scale(SCALE);
    let _ = v.handle(&atomik_ui::UiEvent::PointerDown {
        pos: v.enable_rect().center(),
        button: atomik_ui::PointerButton::Primary,
        modifiers: atomik_ui::Modifiers::NONE,
        clicks: 1,
    });
    let mut canvas = CpuCanvas::new(pw, ph, &mut text);
    v.paint(&mut canvas);
    write("settings_warning.png", canvas);
    let mut b = PluginBannerView::new(Theme::dark());
    b.set_bounds(Rect::new(0.0, 0.0, 900.0, 40.0));
    b.set_scale(SCALE);
    b.show(BannerKind::ProjectUsesPlugins {
        count: 3,
        needs_consent: true,
    });
    let _ = b.handle(&atomik_ui::UiEvent::PointerDown {
        pos: b.load_rect().unwrap().center(),
        button: atomik_ui::PointerButton::Primary,
        modifiers: atomik_ui::Modifiers::NONE,
        clicks: 1,
    });
    let mut canvas = CpuCanvas::new((900.0 * SCALE) as u16, (140.0 * SCALE) as u16, &mut text);
    b.paint(&mut canvas);
    write("banner_warning.png", canvas);
}
