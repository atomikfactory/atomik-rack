//! Which graphics adapter the renderer uses, and what the user is told when there is none.
//!
//! The renderer draws with compute shaders and has no other drawing path, so it needs a real
//! graphics adapter (DirectX 12, Vulkan or Metal). The rule here is pure, over a list of
//! described adapters, so the table of cases is unit-tested without a GPU:
//!
//! - `WGPU_ADAPTER_NAME` names the adapter (the first whose name contains the text, whatever its
//!   kind: a software adapter chosen by name is the user's call, and is only warned about);
//! - otherwise the adapters that can present to the window are candidates, a software (CPU)
//!   adapter never among them, and `WGPU_POWER_PREF` (`low`, `high`, `none`) orders them the way
//!   the renderer's own default does: without it, the first one the system lists is used. That
//!   keeps the choice the system's graphics settings make for a machine with two GPUs (a
//!   window on a monitor driven by the other one included): only the software adapter is
//!   excluded, which a machine without a vendor driver would otherwise fall back to and crash on.
//!
//! The same rule is used for the first start and for every rebuild after a lost device.

use atomik_ui::i18n;

/// The kind of an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A separate graphics card.
    Discrete,
    /// Graphics built into the processor.
    Integrated,
    /// A virtual machine's adapter.
    Virtual,
    /// A software rasteriser.
    Cpu,
    /// A kind the system does not say.
    Other,
}

impl Kind {
    /// The word the log and the message use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Discrete => "discrete",
            Kind::Integrated => "integrated",
            Kind::Virtual => "virtual",
            Kind::Cpu => "CPU",
            Kind::Other => "other",
        }
    }
}

/// One adapter as the rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterDesc {
    /// The adapter's name.
    pub name: String,
    /// The graphics API, as the log names it (`DX12`, `Vulkan`, `Metal`).
    pub backend: String,
    /// What kind of adapter it is.
    pub kind: Kind,
    /// Whether it can present to the window being opened (always true without a window).
    pub can_present: bool,
}

/// The order `WGPU_POWER_PREF` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Power {
    /// The order the system lists the adapters in.
    #[default]
    None,
    /// Integrated before discrete.
    Low,
    /// Discrete before integrated.
    High,
}

/// The environment overrides of the selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    /// `WGPU_ADAPTER_NAME`, lower-cased.
    pub name: Option<String>,
    /// `WGPU_POWER_PREF`.
    pub power: Power,
}

impl Overrides {
    /// From the texts of `WGPU_ADAPTER_NAME` and `WGPU_POWER_PREF` (an empty name is no name; an
    /// unknown preference is none, as the renderer reads it).
    #[must_use]
    pub fn from_values(name: Option<&str>, power: Option<&str>) -> Self {
        Self {
            name: name
                .map(|n| n.trim().to_lowercase())
                .filter(|n| !n.is_empty()),
            power: match power.map(|p| p.trim().to_lowercase()).as_deref() {
                Some("low") => Power::Low,
                Some("high") => Power::High,
                _ => Power::None,
            },
        }
    }

    /// From the environment of the process.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("WGPU_ADAPTER_NAME").ok().as_deref(),
            std::env::var("WGPU_POWER_PREF").ok().as_deref(),
        )
    }
}

/// The adapter chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// The position in the list given to [`choose`].
    pub index: usize,
    /// A software adapter chosen by name: not supported, and the caller warns.
    pub forced_software: bool,
}

/// The rank of a kind for the power preference (lower first); the renderer's own order.
fn rank(kind: Kind, power: Power) -> u8 {
    match (kind, power) {
        (Kind::Discrete, Power::Low) | (Kind::Integrated, Power::High | Power::None) => 2,
        (Kind::Integrated, Power::Low) | (Kind::Discrete, Power::High | Power::None) => 1,
        (Kind::Other, _) => 3,
        (Kind::Virtual, _) => 4,
        (Kind::Cpu, _) => 5,
    }
}

/// The adapter to use among `adapters` (in the order the system lists them), or `None` when
/// none is acceptable.
#[must_use]
pub fn choose(adapters: &[AdapterDesc], overrides: &Overrides) -> Option<Pick> {
    if let Some(wanted) = &overrides.name {
        let index = adapters
            .iter()
            .position(|a| a.can_present && a.name.to_lowercase().contains(wanted.as_str()))?;
        return Some(Pick {
            index,
            forced_software: adapters[index].kind == Kind::Cpu,
        });
    }
    let candidates = adapters
        .iter()
        .enumerate()
        .filter(|(_, a)| a.can_present && a.kind != Kind::Cpu);
    let index = match overrides.power {
        // The system's order, as the renderer's default has it.
        Power::None => candidates.map(|(i, _)| i).next()?,
        power => candidates.min_by_key(|(i, a)| (rank(a.kind, power), *i))?.0,
    };
    Some(Pick {
        index,
        forced_software: false,
    })
}

/// `Name (DX12, integrated); Name 2 (Vulkan, CPU)`, or `none`.
#[must_use]
pub fn found_text(adapters: &[AdapterDesc]) -> String {
    if adapters.is_empty() {
        return i18n::t("none").to_string();
    }
    adapters
        .iter()
        .map(|a| format!("{} ({}, {})", a.name, a.backend, a.kind.name()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// What the start-up message says is needed.
const FAILURE_SENTENCE: &str = "This version of Atomik Rack needs a graphics driver that supports DirectX 12 or Vulkan (Metal on macOS), and none that works was found.";

/// The title of the start-up message.
#[must_use]
pub fn failure_title() -> String {
    i18n::t("Graphics driver needed").to_string()
}

/// The message of a start that found no usable graphics: what is needed, what was found, and
/// where the log is, as paragraphs.
#[must_use]
pub fn failure_text(found: &str, log_path: &str) -> String {
    format!(
        "{}\n\n{}\n\n{}",
        i18n::t(FAILURE_SENTENCE),
        i18n::tf("Graphics found: {found}", &[("found", found)]),
        i18n::tf("Details are in the log file: {path}", &[("path", log_path)]),
    )
}

/// What a start without usable graphics reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReport {
    /// The error that ends the run: one line, in English, for the log (the exit is non-zero).
    pub log: String,
    /// The message box (title, text) in the user's language; `None` for an unattended run,
    /// which must never block on a dialog.
    pub dialog: Option<(String, String)>,
}

/// The report of a start that found no usable graphics: `found` is what the selection saw,
/// `reason` the technical error, `log_path` where the log is.
#[must_use]
pub fn start_report(
    found: &[AdapterDesc],
    reason: &str,
    log_path: &str,
    unattended: bool,
) -> StartReport {
    let english = i18n::with_language(atomik_ui::Lang::English, || {
        failure_text(&found_text(found), log_path)
    });
    let flat = english.split("\n\n").collect::<Vec<_>>().join(" ");
    StartReport {
        log: format!("{flat} ({reason})"),
        dialog: (!unattended)
            .then(|| (failure_title(), failure_text(&found_text(found), log_path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::Lang;

    fn a(name: &str, backend: &str, kind: Kind) -> AdapterDesc {
        AdapterDesc {
            name: name.into(),
            backend: backend.into(),
            kind,
            can_present: true,
        }
    }

    fn cant(mut d: AdapterDesc) -> AdapterDesc {
        d.can_present = false;
        d
    }

    fn pick(adapters: &[AdapterDesc], ov: &Overrides) -> Option<usize> {
        choose(adapters, ov).map(|p| p.index)
    }

    #[test]
    fn the_rule_over_a_table_of_machines() {
        let none = Overrides::default();
        let igpu = a("Integrated 1", "DX12", Kind::Integrated);
        let dgpu = a("Discrete 1", "DX12", Kind::Discrete);
        let soft = a("Software 1", "DX12", Kind::Cpu);
        let virt = a("Virtual 1", "Vulkan", Kind::Virtual);
        // (adapters in the system's order, overrides, expected index)
        let table: Vec<(Vec<AdapterDesc>, Overrides, Option<usize>)> = vec![
            // Integrated and discrete: the system's first one, whichever it is.
            (vec![igpu.clone(), dgpu.clone()], none.clone(), Some(0)),
            (vec![dgpu.clone(), igpu.clone()], none.clone(), Some(0)),
            // Discrete only.
            (vec![dgpu.clone(), soft.clone()], none.clone(), Some(0)),
            // Software only: nothing.
            (vec![soft.clone()], none.clone(), None),
            // Software listed first: skipped.
            (vec![soft.clone(), igpu.clone()], none.clone(), Some(1)),
            // Nothing at all.
            (vec![], none.clone(), None),
            // The adapter that cannot present to the window is not a candidate.
            (
                vec![cant(igpu.clone()), dgpu.clone()],
                none.clone(),
                Some(1),
            ),
            (vec![cant(igpu.clone())], none.clone(), None),
            // A virtual adapter is a last resort before nothing, after the real ones.
            (
                vec![virt.clone(), dgpu.clone()],
                Overrides::from_values(None, Some("low")),
                Some(1),
            ),
            (vec![virt.clone()], none.clone(), Some(0)),
            // The power preference.
            (
                vec![dgpu.clone(), igpu.clone()],
                Overrides::from_values(None, Some("low")),
                Some(1),
            ),
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(None, Some("HIGH")),
                Some(1),
            ),
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(None, Some("none")),
                Some(0),
            ),
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(None, Some("whatever")),
                Some(0),
            ),
            // A preference never brings the software adapter back.
            (
                vec![soft.clone()],
                Overrides::from_values(None, Some("low")),
                None,
            ),
            // A name picks the first adapter that contains it, case-insensitively.
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(Some("DISCRETE"), None),
                Some(1),
            ),
            // A name beats the preference and the kind.
            (
                vec![igpu.clone(), soft.clone()],
                Overrides::from_values(Some("software"), Some("low")),
                Some(1),
            ),
            // A name nothing matches: none (the start-up message says so).
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(Some("no such adapter"), None),
                None,
            ),
            // An empty name is no name.
            (
                vec![igpu.clone(), dgpu.clone()],
                Overrides::from_values(Some("  "), None),
                Some(0),
            ),
        ];
        for (n, (adapters, ov, want)) in table.iter().enumerate() {
            assert_eq!(pick(adapters, ov), *want, "case {n}: {ov:?}");
        }
    }

    #[test]
    fn a_software_adapter_is_only_picked_by_name_and_flagged() {
        let list = [
            a("Integrated 1", "DX12", Kind::Integrated),
            a("Software 1", "DX12", Kind::Cpu),
        ];
        let by_name = choose(&list, &Overrides::from_values(Some("software"), None)).unwrap();
        assert_eq!(by_name.index, 1);
        assert!(by_name.forced_software);
        let by_default = choose(&list, &Overrides::default()).unwrap();
        assert_eq!(by_default.index, 0);
        assert!(!by_default.forced_software);
        let hardware_by_name = choose(&list, &Overrides::from_values(Some("integ"), None)).unwrap();
        assert!(!hardware_by_name.forced_software);
    }

    #[test]
    fn a_name_that_cannot_present_is_not_picked() {
        let list = [cant(a("Integrated 1", "DX12", Kind::Integrated))];
        assert_eq!(
            pick(&list, &Overrides::from_values(Some("integrated"), None)),
            None
        );
    }

    #[test]
    fn what_was_found_is_listed_or_none() {
        assert_eq!(found_text(&[]), "none");
        let list = [
            a("Integrated 1", "DX12", Kind::Integrated),
            a("Software 1", "Vulkan", Kind::Cpu),
        ];
        assert_eq!(
            found_text(&list),
            "Integrated 1 (DX12, integrated); Software 1 (Vulkan, CPU)"
        );
    }

    #[test]
    fn the_message_says_what_is_needed_what_was_found_and_where_the_log_is() {
        let text = i18n::with_language(Lang::English, || failure_text("none", "/logs/atomik.log"));
        assert!(
            text.contains("DirectX 12 or Vulkan (Metal on macOS)"),
            "{text}"
        );
        assert!(text.contains("Graphics found: none"), "{text}");
        assert!(
            text.contains("Details are in the log file: /logs/atomik.log"),
            "{text}"
        );
        assert_eq!(
            i18n::with_language(Lang::English, failure_title),
            "Graphics driver needed"
        );
    }

    #[test]
    fn an_attended_start_shows_the_box_and_an_unattended_one_only_logs() {
        let attended = start_report(&[], "no adapter", "/logs/atomik.log", false);
        let (title, text) = attended.dialog.as_ref().expect("a dialog");
        assert!(!title.is_empty());
        assert!(text.contains("Graphics found: none"), "{text}");
        let unattended = start_report(&[], "no adapter", "/logs/atomik.log", true);
        assert_eq!(unattended.dialog, None);
        // The log line is the same either way: one line, English, with the reason.
        assert_eq!(attended.log, unattended.log);
        assert!(!attended.log.contains('\n'), "{}", attended.log);
        assert!(attended.log.contains("Graphics found: none"));
        assert!(attended.log.contains("/logs/atomik.log"));
        assert!(attended.log.ends_with("(no adapter)"));
    }

    #[test]
    fn the_log_line_stays_english_in_a_french_session() {
        let report = i18n::with_language(Lang::French, || {
            start_report(&[], "r", "/logs/atomik.log", false)
        });
        assert!(
            report.log.contains("Graphics found: none"),
            "{}",
            report.log
        );
        let (_, text) = report.dialog.unwrap();
        assert!(!text.contains("Graphics found"), "{text}");
    }

    #[test]
    fn the_message_has_french_entries() {
        let english = i18n::with_language(Lang::English, || failure_text("x", "p"));
        let french = i18n::with_language(Lang::French, || failure_text("x", "p"));
        assert_ne!(english, french);
        assert!(french.contains("DirectX 12"), "{french}");
        assert!(french.contains('x') && french.contains('p'), "{french}");
        for key in [
            "Graphics driver needed",
            FAILURE_SENTENCE,
            "Graphics found: {found}",
            "Details are in the log file: {path}",
        ] {
            assert!(i18n::has(Lang::French, key), "{key}");
        }
        assert_ne!(
            i18n::with_language(Lang::French, failure_title),
            "Graphics driver needed"
        );
    }
}
