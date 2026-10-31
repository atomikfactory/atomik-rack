//! Typed entry in the position, time and locator readouts of the transport bar.
//!
//! A double-click on POS, TIME, L or R starts typing. POS and the locators take
//! `bar.beat.tick` (`5.1.0`, `5`, `5.3`) or the four-part form of the segmented fields
//! (`5.1.1.0`); TIME takes `h:m:s.ms`, `m:s.ms` or plain seconds (`00:12.500`, `12.5`). Enter
//! moves the playhead (POS, TIME) or sets the locator, in one undo step; Escape drops the text.

use super::cluster::{Readout, ReadoutEdit};
use super::*;
use crate::segment_field::{SegmentField, SegmentMode};
use crate::timeline::seconds_to_tick;

/// The longest text typed.
const TEXT_MAX: usize = 14;

/// The tick a typed position means: `bar[.beat[.tick]]` (the tick counted inside the beat, as
/// the readout shows it) or four dotted parts (bar, beat, sixteenth, tick). `None` when it does
/// not parse or names a bar or beat before the first.
pub(super) fn parse_position(text: &str, bars: &Bars) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() || t.starts_with(['+', '-']) {
        return None;
    }
    let parts: Vec<&str> = t.split('.').collect();
    match parts.len() {
        1..=3 => {
            let num = |i: usize, default: i64| -> Option<i64> {
                match parts.get(i) {
                    Some(s) if !s.is_empty() => s.parse::<i64>().ok(),
                    _ => Some(default),
                }
            };
            let (bar, beat, tick) = (num(0, 1)?, num(1, 1)?, num(2, 0)?);
            if bar < 1 || beat < 1 || tick < 0 {
                return None;
            }
            let start = bars.bar_start(bar);
            Some(start + (beat - 1) * bars.beat_ticks_at(start) + tick)
        }
        4 => {
            let first = parts[0].parse::<i64>().ok()?;
            if first < 1 {
                return None;
            }
            SegmentField::new(bars, SegmentMode::Position).parse(0, 0, t)
        }
        _ => None,
    }
}

/// The tick a typed time means (`h:m:s.ms`, `m:s.ms`, `s.ms`) through the tempo map.
pub(super) fn parse_time(text: &str, tempo: &[TempoEvent]) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let mut seconds = 0.0_f64;
    for part in t.split(':') {
        if part.is_empty() {
            return None;
        }
        let v: f64 = part.parse().ok()?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + v;
    }
    Some(seconds_to_tick(seconds, tempo))
}

impl TransportBar {
    /// True while a position, time or locator is being typed: route every key event here.
    pub fn is_editing_readout(&self) -> bool {
        self.cl.edit.is_some()
    }

    /// The text being typed in a position, time or locator readout.
    pub fn readout_text(&self) -> Option<&str> {
        self.cl.edit.as_ref().map(|e| e.text.as_str())
    }

    fn readout_value_text(&self, which: Readout) -> String {
        match which {
            Readout::Pos => format_bbt(self.state.position, &self.bars),
            Readout::Time => format_time(tick_to_seconds(self.state.position, &self.tempo)),
            Readout::Left => format_bbt(self.loop_range.0, &self.bars),
            Readout::Right => format_bbt(self.loop_range.1, &self.bars),
        }
    }

    pub(super) fn begin_readout_edit(&mut self, which: Readout) {
        self.cl.edit = Some(ReadoutEdit {
            which,
            text: self.readout_value_text(which),
            fresh: true,
        });
        self.repaint = true;
    }

    /// Starts typing a position (POS), for tests and the app.
    pub fn begin_position_edit(&mut self) {
        self.begin_readout_edit(Readout::Pos);
    }

    /// Starts typing a time (TIME).
    pub fn begin_time_edit(&mut self) {
        self.begin_readout_edit(Readout::Time);
    }

    /// Starts typing a locator (the left one, or the right one when `right`).
    pub fn begin_locator_edit(&mut self, right: bool) {
        self.begin_readout_edit(if right { Readout::Right } else { Readout::Left });
    }

    pub(super) fn commit_readout(&mut self, out: &mut Vec<UiAction>) {
        let Some(edit) = self.cl.edit.take() else {
            return;
        };
        self.repaint = true;
        match edit.which {
            Readout::Pos => {
                if let Some(tick) = parse_position(&edit.text, &self.bars) {
                    out.push(UiAction::Transport(TransportAction::Locate(tick)));
                }
            }
            Readout::Time => {
                if let Some(tick) = parse_time(&edit.text, &self.tempo) {
                    out.push(UiAction::Transport(TransportAction::Locate(tick)));
                }
            }
            Readout::Left | Readout::Right => {
                let Some(tick) = parse_position(&edit.text, &self.bars) else {
                    return;
                };
                let (start, end) = self.loop_range;
                let range = if edit.which == Readout::Left {
                    (tick, end)
                } else {
                    (start, tick)
                };
                // The loop keeps at least a tick, and a locator never moves past the other.
                if range.0 < range.1 && range != self.loop_range {
                    out.push(UiAction::Command(Command::Song(SongCommand::SetLoop {
                        range,
                        enabled: self.loop_enabled,
                    })));
                }
            }
        }
    }

    pub(super) fn readout_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some(edit) = &mut self.cl.edit else {
            return;
        };
        match key {
            Key::Char(c) if c.is_ascii_digit() || matches!(c, '.' | ':') => {
                if edit.fresh {
                    edit.text.clear();
                    edit.fresh = false;
                }
                if edit.text.len() < TEXT_MAX {
                    edit.text.push(*c);
                }
            }
            Key::Backspace => {
                edit.fresh = false;
                edit.text.pop();
            }
            Key::Enter => self.commit_readout(out),
            Key::Escape => self.cl.edit = None,
            _ => {}
        }
        self.repaint = true;
    }

    /// The typed text and the readout's rectangle, for painting POS and TIME.
    pub(super) fn readout_typing(&self, which: Readout) -> Option<&str> {
        self.cl
            .edit
            .as_ref()
            .filter(|e| e.which == which)
            .map(|e| e.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::MeterEvent;

    fn bars_44() -> Bars {
        Bars::from_events(&[MeterEvent {
            bar: 1,
            num: 4,
            den: 4,
        }])
    }

    #[test]
    fn positions_parse_as_bar_beat_tick() {
        let b = bars_44();
        let bar = b.bar_start(5);
        assert_eq!(parse_position("5.1.0", &b), Some(bar));
        assert_eq!(parse_position("5", &b), Some(bar));
        assert_eq!(
            parse_position("5.3", &b),
            Some(bar + 2 * b.beat_ticks_at(bar))
        );
        assert_eq!(parse_position("5.1.7", &b), Some(bar + 7));
        assert_eq!(parse_position("0.1.0", &b), None);
        assert_eq!(parse_position("x", &b), None);
        assert_eq!(parse_position("", &b), None);
        assert_eq!(parse_position("1.2.3.4.5", &b), None);
    }

    #[test]
    fn times_parse_with_minutes_and_hours() {
        let tempo = [TempoEvent::step(0, 120.0)];
        // 120 BPM: 2 beats a second, 960 ticks a beat.
        let per_s = 2 * crate::timeline::PPQ;
        assert_eq!(parse_time("00:12.500", &tempo), Some(per_s * 25 / 2));
        assert_eq!(parse_time("12.5", &tempo), Some(per_s * 25 / 2));
        assert_eq!(parse_time("1:00", &tempo), Some(per_s * 60));
        assert_eq!(parse_time("1:00:00", &tempo), Some(per_s * 3600));
        assert_eq!(parse_time("1::2", &tempo), None);
        assert_eq!(parse_time("", &tempo), None);
    }
}
