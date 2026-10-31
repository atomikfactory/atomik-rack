//! What the plugin tells the host from its own threads ([`PluginMessage`]) and how it reaches the
//! parent: one notification frame per message, in the order the plugin posted them.

use atomik_plugin_host::PluginMessage;
use atomik_plugin_ipc::{FromChild, MAX_LOG_BYTES, WireLevel};

/// `text` cut to what the parent accepts in a log line, at a character boundary.
fn truncated(mut text: String) -> String {
    if text.len() > MAX_LOG_BYTES {
        let mut end = MAX_LOG_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// The notification for a message, `None` for a message the parent has no use for.
pub fn to_wire(message: PluginMessage) -> Option<FromChild> {
    Some(match message {
        // JSON cannot carry NaN or infinity, and the peer rejects the whole frame.
        PluginMessage::ParamEdit { value, .. } if !value.is_finite() => return None,
        PluginMessage::ParamEdit { id, value, phase } => FromChild::ParamEdit {
            param: id,
            value,
            phase: phase.into(),
        },
        PluginMessage::StateDirty => FromChild::StateDirty,
        PluginMessage::LatencyChanged => FromChild::LatencyChanged,
        PluginMessage::RescanParams => FromChild::RescanParams,
        PluginMessage::RequestRestart => FromChild::RequestRestart,
        PluginMessage::GuiResize { w, h } => FromChild::GuiResize { w, h },
        PluginMessage::GuiClosed => FromChild::GuiClosed,
        PluginMessage::GuiRequestShow => FromChild::GuiRequestShow,
        PluginMessage::GuiRequestHide => FromChild::GuiRequestHide,
        PluginMessage::Log { level, text } => FromChild::Log {
            level: level.into(),
            text: truncated(text),
        },
        PluginMessage::Failed { reason } => FromChild::Log {
            level: WireLevel::Error,
            text: truncated(reason.to_string()),
        },
        // The tick polls the plugin anyway; the process exit is the parent's own business.
        PluginMessage::RequestCallback | PluginMessage::ProcessExited { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_plugin_host::{LogLevel, ParamPhase};
    use atomik_plugin_ipc::WirePhase;

    #[test]
    fn a_long_log_line_is_cut_to_what_the_parent_accepts() {
        let long = "é".repeat(MAX_LOG_BYTES);
        let Some(FromChild::Log { text, .. }) = to_wire(PluginMessage::Log {
            level: LogLevel::Info,
            text: long,
        }) else {
            panic!("a log line is forwarded");
        };
        assert!(text.len() <= MAX_LOG_BYTES && !text.is_empty());
        assert!(text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_param_edit_keeps_its_id_value_and_phase() {
        let m = PluginMessage::ParamEdit {
            id: 9,
            value: -3.5,
            phase: ParamPhase::Begin,
        };
        assert_eq!(
            to_wire(m),
            Some(FromChild::ParamEdit {
                param: 9,
                value: -3.5,
                phase: WirePhase::Begin
            })
        );
    }

    #[test]
    fn a_param_edit_that_is_not_a_number_is_dropped_because_json_cannot_carry_it() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let m = PluginMessage::ParamEdit {
                id: 1,
                value,
                phase: ParamPhase::Change,
            };
            assert_eq!(to_wire(m), None);
        }
    }

    #[test]
    fn the_plain_notifications_map_one_to_one() {
        assert_eq!(
            to_wire(PluginMessage::StateDirty),
            Some(FromChild::StateDirty)
        );
        assert_eq!(
            to_wire(PluginMessage::LatencyChanged),
            Some(FromChild::LatencyChanged)
        );
        assert_eq!(
            to_wire(PluginMessage::RescanParams),
            Some(FromChild::RescanParams)
        );
        assert_eq!(
            to_wire(PluginMessage::RequestRestart),
            Some(FromChild::RequestRestart)
        );
        assert_eq!(
            to_wire(PluginMessage::GuiResize { w: 300, h: 200 }),
            Some(FromChild::GuiResize { w: 300, h: 200 })
        );
        assert_eq!(
            to_wire(PluginMessage::GuiClosed),
            Some(FromChild::GuiClosed)
        );
        assert_eq!(
            to_wire(PluginMessage::GuiRequestShow),
            Some(FromChild::GuiRequestShow)
        );
        assert_eq!(
            to_wire(PluginMessage::GuiRequestHide),
            Some(FromChild::GuiRequestHide)
        );
    }

    #[test]
    fn a_log_line_keeps_its_level_and_text() {
        let m = PluginMessage::Log {
            level: LogLevel::Warning,
            text: "careful".to_string(),
        };
        assert_eq!(
            to_wire(m),
            Some(FromChild::Log {
                level: WireLevel::Warning,
                text: "careful".to_string()
            })
        );
    }

    #[test]
    fn a_failure_of_the_device_is_logged_as_an_error() {
        let m = PluginMessage::Failed { reason: "boom" };
        assert_eq!(
            to_wire(m),
            Some(FromChild::Log {
                level: WireLevel::Error,
                text: "boom".to_string()
            })
        );
    }

    #[test]
    fn a_callback_request_and_a_process_exit_have_no_notification() {
        assert_eq!(to_wire(PluginMessage::RequestCallback), None);
        assert_eq!(
            to_wire(PluginMessage::ProcessExited {
                reason: "x".to_string()
            }),
            None
        );
    }
}
