//! The CPU readout in the shell: the tooltip of the status line (the five heaviest devices with
//! their tracks) and the card that suggests freezing the heaviest track when the engine load
//! stays high ([`crate::cpu_hint`], decided by [`crate::cpu_gauge::HintWatch`]).

use super::*;
use crate::app_core::DeviceCpu;
use crate::cpu_hint::{Choice, CpuHintCard, Outcome};
use atomik_ui::Help;

/// How many devices the status line tooltip lists.
pub(super) const TOP_DEVICES: usize = 5;

/// The tooltip text of the status line: the engine load with its peak, then one line per device
/// (name, the track it belongs to when that is another name, its share and its peak).
pub fn load_tooltip(load: f32, peak: f32, devices: &[DeviceCpu]) -> String {
    let mut text = format!(
        "Engine load {} (peak {})",
        atomik_ui::paint::cpu_text(load),
        atomik_ui::paint::cpu_text(peak)
    );
    if devices.is_empty() {
        return text;
    }
    text.push_str("\nHeaviest devices:");
    for d in devices {
        text.push('\n');
        text.push_str(&d.name);
        if let Some(track) = &d.track {
            text.push_str(&format!(" (track {track})"));
        }
        text.push_str(&format!(
            "  {}, peak {}",
            atomik_ui::paint::cpu_text(d.percent),
            atomik_ui::paint::cpu_text(d.peak)
        ));
    }
    text
}

impl<B: AudioBackend> Shell<B> {
    /// The freeze hint card, while it shows.
    pub fn cpu_hint(&self) -> Option<&CpuHintCard> {
        self.cpu_hint.as_ref()
    }

    /// The tooltip of the status line: where it is, and what it says now.
    pub(super) fn status_help(&self, pos: Point) -> Option<Help> {
        let size = self.size();
        let bar_top = size.height - STATUS_BAR_HEIGHT;
        // The target is the text of the status line, not the empty rest of the bar.
        let width = (atomik_ui::paint::estimate_width(&self.status, 12.0) + 20.0).min(size.width);
        let rect = Rect::new(0.0, bar_top, width, size.height);
        if !rect.contains(pos) {
            return None;
        }
        let text = load_tooltip(
            self.audio_status.dsp_load * 100.0,
            self.audio_status.dsp_peak * 100.0,
            &self.core.heaviest_devices(TOP_DEVICES),
        );
        Some(Help::new(rect, text).id("status.load"))
    }

    /// Once per status poll: looks at the engine load and puts the card up when it is time. The
    /// card waits while a modal, a dialog or the update notice is on screen.
    pub(super) fn sync_cpu_hint(&mut self) {
        if let Some(card) = &self.cpu_hint
            && self.core.doc().song.track(card.track()).is_none()
        {
            self.cpu_hint = None;
            self.repaint = true;
        }
        let allowed = self.cpu_hint.is_none()
            && self.update_notice.is_none()
            && self.dialog.is_none()
            && !self.modal_open();
        let load = self.audio_status.dsp_load * 100.0;
        if let Some(hint) = self.core.cpu_hint_poll(load, allowed) {
            let name = self.core.device_label(&hint.track);
            self.cpu_hint = Some(CpuHintCard::new(hint.track, &name, hint.percent));
            self.repaint = true;
        }
    }

    /// An event for the card: `Some` when it took the event.
    pub(super) fn cpu_hint_event(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Option<Vec<ShellRequest>> {
        if window != WindowKind::Main || self.capture.is_some() {
            return None;
        }
        let size = self.size();
        let outcome = self.cpu_hint.as_mut()?.handle(event, size);
        match outcome {
            Outcome::Pass => None,
            Outcome::Taken => {
                self.repaint = true;
                Some(Vec::new())
            }
            Outcome::Chose(choice) => {
                self.repaint = true;
                if let Some(card) = self.cpu_hint.take()
                    && choice == Choice::Freeze
                {
                    self.core.freeze_track_of(card.track());
                }
                Some(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, track: Option<&str>, percent: f32, peak: f32) -> DeviceCpu {
        DeviceCpu {
            name: name.into(),
            track: track.map(Into::into),
            percent,
            peak,
        }
    }

    #[test]
    fn the_tooltip_lists_the_devices_with_their_tracks() {
        let text = load_tooltip(
            42.0,
            55.0,
            &[
                row("Poly Synth", None, 12.34, 20.0),
                row("Reverb", Some("Lead"), 3.2, 4.0),
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Engine load 42% (peak 55%)");
        assert_eq!(lines[1], "Heaviest devices:");
        assert_eq!(lines[2], "Poly Synth  12%, peak 20%");
        assert_eq!(lines[3], "Reverb (track Lead)  3.2%, peak 4.0%");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn without_readings_the_tooltip_is_the_load_alone() {
        assert_eq!(load_tooltip(3.1, 5.0, &[]), "Engine load 3.1% (peak 5.0%)");
    }
}
