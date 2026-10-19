//! Display-list snapshots of the console (3 channels and the master), unfolded and
//! with every section folded, compared with checked-in goldens. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_snapshot`.

mod support;

use atomik_ui::console_layout::{CHANNEL_TYPE, Folds, MASTER_TYPE, SectionId, strip_layout};
use atomik_ui::{ChannelMeters, DisplayList};
use std::path::PathBuf;
use support::console::*;
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

fn record(folded: bool) -> DisplayList {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    v.set_all_folded(folded);
    v.set_latency_readouts(&[(key("ch-lead"), 480)]);
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
    dl
}

#[test]
fn console_unfolded_matches_golden() {
    check("console_1x.json", &record(false).to_json());
}

#[test]
fn console_folded_matches_golden() {
    check("console_folded_1x.json", &record(true).to_json());
}

#[test]
fn recordings_are_deterministic_and_folding_hides_controls() {
    let open = record(false);
    assert_eq!(open.to_json(), record(false).to_json());
    let shut = record(true);
    assert!(shut.len() < open.len());
    let (o, s) = (open.texts(), shut.texts());
    for t in ["GAIN", "HPF", "THR", "PAN", "M", "S", "-5.0 dB"] {
        assert!(o.contains(&t), "unfolded is missing {t:?}");
    }
    for t in ["Ø", "HPF", "BELL", "-5.0 dB"] {
        assert!(!s.contains(&t), "folded still shows {t:?}");
    }
    for t in [
        "INPUT",
        "EQ",
        "SENDS",
        "FADER",
        "Bass",
        "MASTER",
        "FX RETURNS",
        "CLR S",
    ] {
        assert!(s.contains(&t), "folded is missing {t:?}");
    }
}

/// The real devices use the §8 keys: everything lands in a named section, nothing in
/// "MORE", and the hidden params stay hidden. Skipped while they are not registered.
#[test]
fn real_console_devices_fit_the_strip_map() {
    let reg = registry();
    let (Some(ch), Some(ms)) = (reg.get(CHANNEL_TYPE), reg.get(MASTER_TYPE)) else {
        return;
    };
    let l = strip_layout(ch.descriptor, Folds::default(), 700.0);
    let more: Vec<&str> = l
        .controls
        .iter()
        .filter(|c| c.section == SectionId::More)
        .map(|c| c.param)
        .collect();
    assert!(more.is_empty(), "unmapped channel params: {more:?}");
    let ml = atomik_ui::console_layout::master_layout(ms.descriptor, 700.0);
    let more: Vec<&str> = ml
        .controls
        .iter()
        .filter(|c| c.section == SectionId::More)
        .map(|c| c.param)
        .collect();
    assert!(more.is_empty(), "unmapped master params: {more:?}");
    assert!(l.controls.iter().any(|c| c.param == "fader"));
    assert!(ml.controls.iter().any(|c| c.param == "return8.pan"));
}
