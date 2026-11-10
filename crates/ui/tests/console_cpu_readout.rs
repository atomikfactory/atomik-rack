//! The CPU figure on the name plates of the console strips: the track's total, shown from the
//! loads the app sets, nothing drawn when it is off, and a repaint only when a shown figure
//! changes. Goldens regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_cpu_readout`.

mod support;

use atomik_model::DeviceKey;
use atomik_ui::paint::{CPU_WARN_PERCENT, cpu_color, cpu_text};
use atomik_ui::{DisplayList, Theme};
use std::collections::BTreeMap;
use support::console::*;
use support::*;

fn loads(pairs: &[(&str, f32)]) -> BTreeMap<DeviceKey, f32> {
    pairs.iter().map(|(k, v)| (key(k), *v)).collect()
}

#[test]
fn plates_show_the_totals_and_nothing_when_off() {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    let paint = |v: &mut atomik_ui::ConsoleView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    let off = paint(&mut v);
    let shown = loads(&[("ch-bass", 3.24), ("ch-lead", 41.0)]);
    v.set_cpu_loads(Some(&shown));
    assert_eq!(v.cpu_load(&key("ch-bass")), Some(3.24));
    assert_eq!(v.cpu_load(&key("ch-keys")), None, "no reading, no figure");
    let on = paint(&mut v);
    assert_ne!(on, off);
    starter::check_golden("console_cpu_readout_1x.json", &on);
    v.set_cpu_loads(None);
    assert_eq!(paint(&mut v), off, "off draws exactly as before");
}

#[test]
fn a_figure_repaints_only_when_what_is_shown_changes() {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    v.set_cpu_loads(Some(&loads(&[("ch-bass", 12.0)])));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint(), "painted");
    v.set_cpu_loads(Some(&loads(&[("ch-bass", 12.2)])));
    assert!(!v.needs_repaint(), "12.2 shows as 12 too");
    v.set_cpu_loads(Some(&loads(&[("ch-bass", 13.0)])));
    assert!(v.needs_repaint());
}

#[test]
fn the_text_has_a_decimal_under_ten_and_the_colour_warns_above_a_quarter() {
    assert_eq!(cpu_text(3.24), "3.2%");
    assert_eq!(cpu_text(0.0), "0.0%");
    assert_eq!(cpu_text(9.96), "10%");
    assert_eq!(cpu_text(25.4), "25%");
    assert_eq!(cpu_text(120.0), "120%");
    let theme = Theme::dark();
    assert_eq!(cpu_color(&theme, CPU_WARN_PERCENT), theme.text);
    assert_eq!(cpu_color(&theme, CPU_WARN_PERCENT + 0.1), theme.warning);
    assert_eq!(cpu_color(&theme, 2.0), theme.text_muted);
}
