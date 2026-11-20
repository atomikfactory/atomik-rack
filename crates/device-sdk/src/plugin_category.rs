//! The category a plugin declares for itself, in Atomik's own terms.
//!
//! A CLAP plugin lists features (`audio-effect`, `analyzer`, `reverb`, ...) and a VST3 class has
//! a `|`-separated sub-category string (`Fx|Reverb`, `Instrument|Synth`, `Fx|Analyzer`). The
//! scanner maps both onto a [`PluginCategory`]: a [`PluginClass`] (instrument, effect, note
//! effect, analyser) and, for an effect, an [`EffectKind`]. How a plugin is hosted stays
//! [`PluginKindHint`] (an analyser has audio ports like any effect); the category is only what
//! the browser and the plugin manager show and filter on.

use crate::PluginKindHint;

/// What a plugin is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PluginClass {
    /// Not known: a scan entry written before categories were read.
    Unknown,
    /// A synth or sampler.
    Instrument,
    /// An audio effect.
    Effect,
    /// A note effect (arpeggiator, transposer).
    NoteEffect,
    /// A plugin that only measures or shows the audio (a meter, a spectrum).
    Analyser,
}

/// The kind of an audio effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EffectKind {
    /// Reverb.
    Reverb,
    /// Delay and echo.
    Delay,
    /// Equaliser.
    Eq,
    /// Compressor, limiter, gate, expander, de-esser, transient shaper.
    Dynamics,
    /// Distortion and saturation.
    Distortion,
    /// Filter.
    Filter,
    /// Chorus, flanger, phaser, tremolo and other modulation.
    Modulation,
    /// Pitch shifting and correction.
    Pitch,
    /// Stereo width and spatial processing.
    Spatial,
    /// Any other effect.
    Other,
}

/// A plugin's category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginCategory {
    /// What the plugin is for.
    pub class: PluginClass,
    /// The kind of an effect (`None` for the other classes, or an effect of no stated kind).
    pub effect: Option<EffectKind>,
}

impl PluginClass {
    /// The name in saved data.
    fn code(self) -> &'static str {
        match self {
            PluginClass::Unknown => "unknown",
            PluginClass::Instrument => "instrument",
            PluginClass::Effect => "effect",
            PluginClass::NoteEffect => "note-effect",
            PluginClass::Analyser => "analyser",
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            PluginClass::Unknown => "Unknown",
            PluginClass::Instrument => "Instrument",
            PluginClass::Effect => "Effect",
            PluginClass::NoteEffect => "Note effect",
            PluginClass::Analyser => "Analyser",
        }
    }
}

impl EffectKind {
    /// Every kind, in menu order.
    pub const ALL: [EffectKind; 10] = [
        EffectKind::Reverb,
        EffectKind::Delay,
        EffectKind::Eq,
        EffectKind::Dynamics,
        EffectKind::Distortion,
        EffectKind::Filter,
        EffectKind::Modulation,
        EffectKind::Pitch,
        EffectKind::Spatial,
        EffectKind::Other,
    ];

    fn code(self) -> &'static str {
        match self {
            EffectKind::Reverb => "reverb",
            EffectKind::Delay => "delay",
            EffectKind::Eq => "eq",
            EffectKind::Dynamics => "dynamics",
            EffectKind::Distortion => "distortion",
            EffectKind::Filter => "filter",
            EffectKind::Modulation => "modulation",
            EffectKind::Pitch => "pitch",
            EffectKind::Spatial => "spatial",
            EffectKind::Other => "other",
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            EffectKind::Reverb => "Reverb",
            EffectKind::Delay => "Delay",
            EffectKind::Eq => "EQ",
            EffectKind::Dynamics => "Dynamics",
            EffectKind::Distortion => "Distortion",
            EffectKind::Filter => "Filter",
            EffectKind::Modulation => "Modulation",
            EffectKind::Pitch => "Pitch",
            EffectKind::Spatial => "Spatial",
            EffectKind::Other => "Other",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// The effect kind a feature word or sub-category names (either format), if any.
    fn of_tag(tag: &str) -> Option<Self> {
        let tag = tag.trim().to_ascii_lowercase();
        Some(match tag.as_str() {
            "reverb" => EffectKind::Reverb,
            "delay" | "echo" => EffectKind::Delay,
            "equalizer" | "equaliser" | "eq" => EffectKind::Eq,
            "compressor" | "limiter" | "gate" | "expander" | "de-esser" | "transient-shaper"
            | "dynamics" => EffectKind::Dynamics,
            "distortion" | "saturation" => EffectKind::Distortion,
            "filter" => EffectKind::Filter,
            "chorus" | "flanger" | "phaser" | "tremolo" | "modulation" | "frequency-shifter" => {
                EffectKind::Modulation
            }
            "pitch-shifter" | "pitch-correction" | "pitch shift" | "pitch" => EffectKind::Pitch,
            "spatial" | "surround" | "ambisonic" => EffectKind::Spatial,
            "glitch" | "granular" | "phase-vocoder" | "restoration" | "mastering"
            | "multi-effects" => EffectKind::Other,
            _ => return None,
        })
    }
}

impl PluginCategory {
    /// A category that is not known.
    pub const UNKNOWN: Self = Self {
        class: PluginClass::Unknown,
        effect: None,
    };

    /// The category of what a plugin declares: `kind` is how the host found it (its ports), and
    /// `tags` are its feature words or sub-categories. The ports decide instrument and note
    /// effect; among effects, an `analyzer` tag with no other effect kind makes an analyser,
    /// and the first tag that names an effect kind gives the kind.
    pub fn from_tags<'a>(kind: PluginKindHint, tags: impl IntoIterator<Item = &'a str>) -> Self {
        let tags: Vec<&str> = tags.into_iter().collect();
        match kind {
            PluginKindHint::Instrument => Self {
                class: PluginClass::Instrument,
                effect: None,
            },
            PluginKindHint::NoteEffect => Self {
                class: PluginClass::NoteEffect,
                effect: None,
            },
            PluginKindHint::Effect => {
                let effect = tags.iter().find_map(|t| EffectKind::of_tag(t));
                let analyser = tags.iter().any(|t| {
                    matches!(
                        t.trim().to_ascii_lowercase().as_str(),
                        "analyzer" | "analyser"
                    )
                });
                if analyser && effect.is_none() {
                    Self {
                        class: PluginClass::Analyser,
                        effect: None,
                    }
                } else {
                    Self {
                        class: PluginClass::Effect,
                        effect,
                    }
                }
            }
        }
    }

    /// The category of a CLAP plugin's feature list.
    pub fn from_clap_features(kind: PluginKindHint, features: &[String]) -> Self {
        Self::from_tags(kind, features.iter().map(String::as_str))
    }

    /// The category of a VST3 class's `|`-separated sub-categories (`Fx|Reverb`).
    pub fn from_vst3_sub_categories(kind: PluginKindHint, sub_categories: &str) -> Self {
        Self::from_tags(kind, sub_categories.split('|'))
    }

    /// What an entry that carries no category reads as: the class the ports give, no kind.
    pub fn from_kind(kind: PluginKindHint) -> Self {
        Self::from_tags(kind, std::iter::empty())
    }

    /// This category, or what the ports say when it is not known.
    pub fn or_kind(self, kind: PluginKindHint) -> Self {
        if self.class == PluginClass::Unknown {
            Self::from_kind(kind)
        } else {
            self
        }
    }

    /// The text form kept in the scan cache (`unknown`, `instrument`, `effect:reverb`, ...).
    pub fn to_text(self) -> String {
        match self.effect {
            Some(k) => format!("{}:{}", self.class.code(), k.code()),
            None => self.class.code().to_string(),
        }
    }

    /// Reads the text form; anything unreadable is [`PluginCategory::UNKNOWN`], so a newer or
    /// damaged entry never fails a whole scan cache.
    pub fn from_text(text: &str) -> Self {
        let (class, kind) = text.split_once(':').unwrap_or((text, ""));
        let class = match class {
            "instrument" => PluginClass::Instrument,
            "effect" => PluginClass::Effect,
            "note-effect" => PluginClass::NoteEffect,
            "analyser" => PluginClass::Analyser,
            _ => return Self::UNKNOWN,
        };
        Self {
            class,
            effect: if class == PluginClass::Effect {
                EffectKind::from_code(kind)
            } else {
                None
            },
        }
    }

    /// The name shown in the plugin manager (`Effect: Reverb`, `Analyser`).
    pub fn label(self) -> String {
        match self.effect {
            Some(k) => format!("{}: {}", self.class.label(), k.label()),
            None => self.class.label().to_string(),
        }
    }
}

impl Default for PluginCategory {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clap(kind: PluginKindHint, f: &[&str]) -> PluginCategory {
        let f: Vec<String> = f.iter().map(|s| (*s).to_string()).collect();
        PluginCategory::from_clap_features(kind, &f)
    }

    #[test]
    fn clap_features_give_the_class_and_the_effect_kind() {
        let c = clap(
            PluginKindHint::Effect,
            &["audio-effect", "reverb", "stereo"],
        );
        assert_eq!(
            (c.class, c.effect),
            (PluginClass::Effect, Some(EffectKind::Reverb))
        );
        let c = clap(PluginKindHint::Effect, &["audio-effect", "equalizer"]);
        assert_eq!(c.effect, Some(EffectKind::Eq));
        let c = clap(PluginKindHint::Effect, &["audio-effect", "limiter"]);
        assert_eq!(c.effect, Some(EffectKind::Dynamics));
        let c = clap(PluginKindHint::Instrument, &["instrument", "synthesizer"]);
        assert_eq!((c.class, c.effect), (PluginClass::Instrument, None));
        let c = clap(PluginKindHint::NoteEffect, &["note-effect"]);
        assert_eq!(c.class, PluginClass::NoteEffect);
    }

    #[test]
    fn a_plugin_that_only_analyses_is_an_analyser() {
        let c = clap(PluginKindHint::Effect, &["audio-effect", "analyzer"]);
        assert_eq!(c.class, PluginClass::Analyser);
        let c = PluginCategory::from_vst3_sub_categories(PluginKindHint::Effect, "Fx|Analyzer");
        assert_eq!(c.class, PluginClass::Analyser);
        // An analyser that also names an effect kind is an effect of that kind.
        let c = clap(PluginKindHint::Effect, &["analyzer", "compressor"]);
        assert_eq!(
            (c.class, c.effect),
            (PluginClass::Effect, Some(EffectKind::Dynamics))
        );
    }

    #[test]
    fn vst3_sub_categories_are_split_on_the_bar() {
        let c = PluginCategory::from_vst3_sub_categories(PluginKindHint::Effect, "Fx|Reverb");
        assert_eq!(
            (c.class, c.effect),
            (PluginClass::Effect, Some(EffectKind::Reverb))
        );
        let c = PluginCategory::from_vst3_sub_categories(PluginKindHint::Effect, "Fx|Pitch Shift");
        assert_eq!(c.effect, Some(EffectKind::Pitch));
        let c = PluginCategory::from_vst3_sub_categories(
            PluginKindHint::Instrument,
            "Instrument|Synth",
        );
        assert_eq!(c.class, PluginClass::Instrument);
        let c = PluginCategory::from_vst3_sub_categories(PluginKindHint::Effect, "Fx");
        assert_eq!((c.class, c.effect), (PluginClass::Effect, None));
    }

    #[test]
    fn the_text_form_round_trips_and_unreadable_text_is_unknown() {
        for class in [
            PluginClass::Instrument,
            PluginClass::NoteEffect,
            PluginClass::Analyser,
        ] {
            let c = PluginCategory {
                class,
                effect: None,
            };
            assert_eq!(PluginCategory::from_text(&c.to_text()), c);
        }
        for k in EffectKind::ALL {
            let c = PluginCategory {
                class: PluginClass::Effect,
                effect: Some(k),
            };
            assert_eq!(PluginCategory::from_text(&c.to_text()), c);
        }
        assert_eq!(PluginCategory::from_text("effect:nonsense").effect, None);
        assert_eq!(
            PluginCategory::from_text("effect").class,
            PluginClass::Effect
        );
        assert_eq!(
            PluginCategory::from_text("flux-capacitor"),
            PluginCategory::UNKNOWN
        );
        assert_eq!(PluginCategory::from_text(""), PluginCategory::UNKNOWN);
    }

    #[test]
    fn an_unknown_category_falls_back_to_the_ports() {
        let c = PluginCategory::UNKNOWN.or_kind(PluginKindHint::Instrument);
        assert_eq!(c.class, PluginClass::Instrument);
        let known = PluginCategory::from_kind(PluginKindHint::Effect);
        assert_eq!(known.or_kind(PluginKindHint::Instrument), known);
    }
}
