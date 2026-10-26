//! Every button kind of the views has a tooltip. Each view answers `help_at(pos)`; the tests
//! walk the button enums of the views through their `*_center` helpers and fail when a button
//! has no text (a new button added without help is caught here), and check that every shortcut
//! a tooltip names is a command of the registry.

mod support;

use atomik_ui::arrangement::LaneButton;
use atomik_ui::keymap::{Keymap, Os, Profile, Registry as Commands};
use atomik_ui::kurbo::Point;
use atomik_ui::piano_roll::ScaleBarField;
use atomik_ui::{ConsoleButton, Help, SeqButton, Tool, TrackButton, TransportButton};
use support::console::*;
use support::song::*;
use support::*;

/// What a test collects: the help of every button it walked.
#[derive(Default)]
struct Seen(Vec<(String, Help)>);

impl Seen {
    fn expect(&mut self, what: &str, help: Option<Help>) {
        let help = help.unwrap_or_else(|| panic!("{what} has no tooltip"));
        assert!(!help.text.trim().is_empty(), "{what} has an empty tooltip");
        assert!(
            help.text.len() <= 90,
            "{what}: a tooltip is one short line ({} characters)",
            help.text.len()
        );
        self.0.push((what.to_string(), help));
    }
}

const ALL_TRANSPORT: [TransportButton; 6] = [
    TransportButton::ReturnToZero,
    TransportButton::Stop,
    TransportButton::Play,
    TransportButton::Record,
    TransportButton::Loop,
    TransportButton::Metronome,
];

#[test]
fn every_transport_button_and_readout_has_a_tooltip() {
    let (doc, reg) = (song_doc(), registry());
    let bar = transport_bar(&doc, &reg);
    let mut seen = Seen::default();
    for b in ALL_TRANSPORT {
        seen.expect(&format!("{b:?}"), bar.help_at(bar.button_center(b)));
    }
    // The shortcuts of the transport are in the text of the keymap, not in the help.
    let play = bar
        .help_at(bar.button_center(TransportButton::Play))
        .unwrap();
    assert_eq!(play.command, Some("transport.play_stop"));
    // Away from the bar there is nothing.
    assert!(bar.help_at(Point::new(-5.0, -5.0)).is_none());
}

#[test]
fn every_arrangement_tool_snap_and_track_button_has_a_tooltip() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    let mut seen = Seen::default();
    for t in Tool::ALL {
        seen.expect(&format!("tool {t:?}"), v.help_at(v.tool_button_center(t)));
        assert_eq!(
            v.help_at(v.tool_button_center(t)).unwrap().command,
            Some(t.command_id())
        );
    }
    seen.expect("snap", v.help_at(v.snap_button_center()));
    for b in [
        TrackButton::Mute,
        TrackButton::Solo,
        TrackButton::Arm,
        TrackButton::Expand,
        TrackButton::AddLane,
    ] {
        let p = v
            .track_button_center(&key("bass"), b)
            .unwrap_or_else(|| panic!("no {b:?} on the bass track"));
        seen.expect(&format!("track {b:?}"), v.help_at(p));
    }
    let p = v.add_track_center(&key("pad")).unwrap();
    seen.expect("add track", v.help_at(p));
    for b in [LaneButton::Fold, LaneButton::Arm, LaneButton::Mode] {
        let p = v
            .lane_button_center(&key("bass"), "filter.cutoff", b)
            .unwrap_or_else(|| panic!("no {b:?} on the cutoff lane"));
        seen.expect(&format!("lane {b:?}"), v.help_at(p));
    }
}

#[test]
fn every_piano_roll_button_has_a_tooltip() {
    let (doc, reg) = (song_doc(), registry());
    let v = piano_roll(&doc, &reg);
    let mut seen = Seen::default();
    for t in Tool::ALL {
        seen.expect(&format!("tool {t:?}"), v.help_at(v.tool_button_center(t)));
    }
    seen.expect("snap", v.help_at(v.snap_button_center()));
    seen.expect("quantize", v.help_at(v.quantize_button_center()));
    seen.expect(
        "quantize options",
        v.help_at(v.quantize_options_button_center()),
    );
    seen.expect("close", v.help_at(v.close_button_center()));
    // The scale bar: every field has help.
    for f in [
        ScaleBarField::Root,
        ScaleBarField::Kind,
        ScaleBarField::Detect,
        ScaleBarField::SnapScale,
        ScaleBarField::Fold,
        ScaleBarField::Chord,
        ScaleBarField::Inversion,
    ] {
        seen.expect(
            &format!("scale bar {f:?}"),
            v.help_at(v.scale_bar_center(f)),
        );
    }
}

#[test]
fn every_console_button_has_a_tooltip() {
    let (doc, reg) = (console_doc(), console_registry());
    let v = console_view(&doc, &reg);
    let mut seen = Seen::default();
    for b in [
        ConsoleButton::FoldAll,
        ConsoleButton::AddChannel,
        ConsoleButton::Compensation,
        ConsoleButton::Arrange,
        ConsoleButton::Maximise,
        ConsoleButton::ClearSolos,
        ConsoleButton::ClearMutes,
        ConsoleButton::StoreSnapshot,
        ConsoleButton::Compare,
    ] {
        if let Some(p) = v.button_center(b) {
            seen.expect(&format!("console {b:?}"), v.help_at(p));
        }
    }
    assert!(
        !seen.0.is_empty(),
        "the console fixture has toolbar buttons"
    );
    // A channel's mute and solo buttons say which parameter they flip.
    let p = v.control_center(&key("ch-bass"), "mute").unwrap();
    let h = v.help_at(p).expect("the mute button has a tooltip");
    assert!(h.text.to_lowercase().contains("mute"), "{}", h.text);
}

#[test]
fn rack_buttons_of_players_containers_and_plugins_have_tooltips() {
    let reg = registry();
    let mut seen = Seen::default();

    let doc = support::seq::players_doc();
    let v = plain_view(&doc, &reg);
    for b in [
        SeqButton::Slot(0),
        SeqButton::Add,
        SeqButton::Duplicate,
        SeqButton::Remove,
        SeqButton::Copy,
        SeqButton::Paste,
        SeqButton::Clear,
        SeqButton::Edit,
        SeqButton::Preview,
    ] {
        let p = v.seq_button_center(&key("seq"), b).unwrap();
        seen.expect(&format!("seq {b:?}"), v.help_at(p));
    }
    for m in 0..6 {
        let p = v.arp_mode_center(&key("arp"), m).unwrap();
        seen.expect(&format!("arp mode {m}"), v.help_at(p));
    }
    for r in 0..7 {
        let p = v.arp_rate_center(&key("arp"), r).unwrap();
        seen.expect(&format!("arp rate {r}"), v.help_at(p));
    }
    for up in [false, true] {
        let p = v.arp_octave_center(&key("arp"), up).unwrap();
        seen.expect(&format!("arp octave {up}"), v.help_at(p));
    }
    let p = v.fold_button_center(&key("seq")).unwrap();
    seen.expect("fold", v.help_at(p));
}

#[test]
fn every_shortcut_a_tooltip_names_is_a_known_command() {
    let commands = Commands::bundled();
    let (doc, reg) = (song_doc(), registry());
    let bar = transport_bar(&doc, &reg);
    let arr = arrangement(&doc, &reg);
    let roll = piano_roll(&doc, &reg);
    let mut helps: Vec<Help> = ALL_TRANSPORT
        .iter()
        .filter_map(|b| bar.help_at(bar.button_center(*b)))
        .collect();
    helps.extend(
        Tool::ALL
            .iter()
            .filter_map(|t| arr.help_at(arr.tool_button_center(*t))),
    );
    helps.extend(
        Tool::ALL
            .iter()
            .filter_map(|t| roll.help_at(roll.tool_button_center(*t))),
    );
    helps.extend(roll.help_at(roll.quantize_button_center()));
    let keymap = Keymap::bundled(Profile::Standard, Os::Windows);
    let mut shown = 0;
    for h in helps {
        if let Some(c) = h.command {
            assert!(
                commands.validate(c, h.arg),
                "{}: `{c}` is not a command",
                h.text
            );
            if h.text_with(&keymap) != h.text {
                shown += 1;
            }
        }
    }
    assert!(
        shown >= 4,
        "the bundled keymap gives shortcuts to the tooltips ({shown})"
    );
}

#[test]
fn every_editor_button_has_a_tooltip() {
    use atomik_device_sdk::{LoopMode, TriggerMode};
    use support::{drum, sampler, seq, slicer};
    let reg = registry();
    let mut seen = Seen::default();

    let doc = sampler::sampler_doc(sampler::layered_zones());
    let v = sampler::editor(&doc, &reg);
    seen.expect("sample close", v.help_at(v.close_button_center()));
    seen.expect("sample load", v.help_at(v.load_button_center()));
    for m in [LoopMode::Off, LoopMode::Forward, LoopMode::PingPong] {
        seen.expect(&format!("loop {m:?}"), v.help_at(v.loop_button_center(m)));
    }
    for m in [TriggerMode::Gate, TriggerMode::OneShot] {
        seen.expect(
            &format!("trigger {m:?}"),
            v.help_at(v.trigger_button_center(m)),
        );
    }

    let doc = slicer::slicer_doc(slicer::sliced_zones(&[0, 24_000, 48_000]));
    let v = slicer::editor(&doc, &reg);
    seen.expect("slice close", v.help_at(v.close_button_center()));
    seen.expect("slice load", v.help_at(v.load_button_center()));
    seen.expect("slice transients", v.help_at(v.mode_button_center(false)));
    seen.expect("slice grid", v.help_at(v.mode_button_center(true)));
    seen.expect("slice sensitivity", v.help_at(v.sensitivity_center()));
    seen.expect("slice beats", v.help_at(v.beats_center()));
    seen.expect("slice button", v.help_at(v.slice_button_center()));
    seen.expect("slice to track", v.help_at(v.to_track_button_center()));
    seen.expect("slice reverse", v.help_at(v.reverse_button_center()));

    let doc = drum::drum_doc();
    let v = drum::editor(&doc, &reg, None);
    seen.expect("step close", v.help_at(v.close_button_center()));
    let v = drum::editor(&doc, &reg, Some(atomik_model::ClipId(7)));
    seen.expect("step swing", v.help_at(v.swing_field_center()));
    for lv in atomik_ui::VelocityLevel::ALL {
        seen.expect(
            &format!("level {lv:?}"),
            v.help_at(v.level_button_center(lv)),
        );
    }

    let doc = seq::seq_only();
    let v = seq::editor(&doc, &reg);
    seen.expect("seq close", v.help_at(v.close_button_center()));
    seen.expect("seq slot", v.help_at(v.slot_center(0)));
    for b in [
        atomik_ui::seq_editor::PatternButton::Add,
        atomik_ui::seq_editor::PatternButton::Duplicate,
        atomik_ui::seq_editor::PatternButton::Remove,
        atomik_ui::seq_editor::PatternButton::Copy,
        atomik_ui::seq_editor::PatternButton::Paste,
        atomik_ui::seq_editor::PatternButton::Clear,
    ] {
        seen.expect(&format!("seq {b:?}"), v.help_at(v.pattern_button_center(b)));
    }
    seen.expect("seq length -", v.help_at(v.length_button_center(false)));
    seen.expect("seq length +", v.help_at(v.length_button_center(true)));
    seen.expect("seq swing", v.help_at(v.swing_field_center()));
}

#[test]
fn the_rack_button_tooltips_follow_the_switch_but_the_drag_readout_stays() {
    use atomik_ui::kurbo::Vec2;
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let bypass = ["delay", "echo", "reverb", "comp"]
        .iter()
        .find_map(|k| v.bypass_button_rect(&key(k)))
        .expect("an effect with a BYPASS switch")
        .center();
    let _ = v.handle(&mv(bypass), &doc, &reg);
    assert!(
        v.tooltip().is_some_and(|t| t.contains("bypass")),
        "BYPASS explains itself by default"
    );
    v.set_button_tips(false);
    assert_eq!(v.tooltip(), None, "no button tooltip with the setting off");
    // The value readout of a drag is not a help tooltip: it stays.
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let _ = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    assert_eq!(v.tooltip().as_deref(), Some("Mix  40 %"));
    let _ = v.handle(&up(c), &doc, &reg);
}

#[test]
fn container_buttons_have_tooltips() {
    use atomik_ui::container_panel::ContainerButton;
    let d = support::container::layered();
    let v = view(d.doc(), &registry());
    let mut seen = Seen::default();
    for b in [
        ContainerButton::Open,
        ContainerButton::Edit,
        ContainerButton::ToChannel,
    ] {
        let p = v.container_button_center(&key("layer"), b).unwrap();
        seen.expect(&format!("container {b:?}"), v.help_at(p));
    }
}
