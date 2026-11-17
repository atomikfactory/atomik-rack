//! The techniques the tab editor toggles: what each command name does to a note's
//! [`Techniques`], and the buttons of the technique toolbar.
//!
//! Pure: the view picks the notes, asks [`Tech::parse`] what a name means and runs
//! [`Tech::toggle_on`] over the notes' current techniques to learn whether the command sets or
//! clears (a toggle is on only when every note has it, so a mixed selection gets it).

use atomik_tab::score::{
    Bend, BendKind, Harmonic, HarmonicKind, SlideKind, TechFlags, Techniques, VibratoKind,
};

/// The bend amounts of the toolbar chip, in cents (tones: ¼, ½, 1, 1½, 2).
pub const BEND_CENTS: [i32; 5] = [50, 100, 200, 300, 400];
/// The amount a bend has until the user picks another: a whole tone.
pub const DEFAULT_BEND: i32 = 200;

/// The command argument for a bend amount (`editor.tab_bend_amount`).
pub fn bend_name(cents: i32) -> Option<&'static str> {
    Some(match cents {
        50 => "quarter_tone",
        100 => "half_tone",
        200 => "one_tone",
        300 => "tone_and_half",
        400 => "two_tones",
        _ => return None,
    })
}

/// The bend amount a command argument names.
pub fn bend_from_name(name: &str) -> Option<i32> {
    BEND_CENTS
        .iter()
        .copied()
        .find(|c| bend_name(*c) == Some(name))
}

/// The amount a technique argument with its own amount names (`bend_half` is 100 cents); the
/// bend button then takes that amount too.
pub fn explicit_bend(name: &str) -> Option<i32> {
    Some(match name {
        "bend_quarter" => 50,
        "bend_half" => 100,
        "bend_full" => 200,
        "bend_one_half" => 300,
        "bend_two" => 400,
        _ => return None,
    })
}

/// The label of a bend amount: `1/4`, `1/2`, `1`, `1 1/2`, `2`.
pub fn bend_chip(cents: i32) -> &'static str {
    match cents {
        50 => "1/4",
        100 => "1/2",
        200 => "1",
        300 => "1 1/2",
        400 => "2",
        _ => "?",
    }
}

/// The shape of a bend a technique command makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BendShape {
    /// Up and hold.
    Up,
    /// Up, then back.
    Release,
    /// Start bent.
    Pre,
    /// Start bent, then back.
    PreRelease,
}

/// What a technique command does to a note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tech {
    /// An on/off flag.
    Flag(TechFlags),
    /// A slide.
    Slide(SlideKind),
    /// A bend of `cents` in a shape.
    Bend(BendShape, i32),
    /// A vibrato.
    Vibrato(VibratoKind),
    /// A harmonic.
    Harmonic(HarmonicKind),
    /// Removes every technique.
    Clear,
}

impl Tech {
    /// The technique a command argument names; `bend` is the amount the bend chip holds.
    pub fn parse(name: &str, chip: i32) -> Option<Tech> {
        Some(match name {
            "hammer_on" => Tech::Flag(TechFlags::HAMMER_ON),
            "pull_off" => Tech::Flag(TechFlags::PULL_OFF),
            "slide_legato" => Tech::Slide(SlideKind::Legato),
            "slide_shift" => Tech::Slide(SlideKind::Shift),
            "slide_in_below" => Tech::Slide(SlideKind::InFromBelow),
            "slide_in_above" => Tech::Slide(SlideKind::InFromAbove),
            "slide_out_down" => Tech::Slide(SlideKind::OutDown),
            "slide_out_up" => Tech::Slide(SlideKind::OutUp),
            "bend" => Tech::Bend(BendShape::Up, chip),
            "bend_quarter" => Tech::Bend(BendShape::Up, 50),
            "bend_half" => Tech::Bend(BendShape::Up, 100),
            "bend_full" => Tech::Bend(BendShape::Up, 200),
            "bend_one_half" => Tech::Bend(BendShape::Up, 300),
            "bend_two" => Tech::Bend(BendShape::Up, 400),
            "bend_release" => Tech::Bend(BendShape::Release, chip),
            "pre_bend" => Tech::Bend(BendShape::Pre, chip),
            "pre_bend_release" => Tech::Bend(BendShape::PreRelease, chip),
            "vibrato" => Tech::Vibrato(VibratoKind::Normal),
            "vibrato_wide" => Tech::Vibrato(VibratoKind::Wide),
            "harmonic" => Tech::Harmonic(HarmonicKind::Natural),
            "harmonic_artificial" => Tech::Harmonic(HarmonicKind::Artificial),
            "palm_mute" => Tech::Flag(TechFlags::PALM_MUTE),
            "let_ring" => Tech::Flag(TechFlags::LET_RING),
            "dead" => Tech::Flag(TechFlags::DEAD),
            "ghost" => Tech::Flag(TechFlags::GHOST),
            "accent" => Tech::Flag(TechFlags::ACCENT),
            "tap" => Tech::Flag(TechFlags::TAP),
            "clear" => Tech::Clear,
            _ => return None,
        })
    }

    /// The bend curve of a shape and amount.
    fn curve(shape: BendShape, cents: i32) -> Bend {
        match shape {
            BendShape::Up => Bend::up(cents),
            BendShape::Release => Bend::bend_release(cents),
            BendShape::Pre => Bend::pre_bend(cents),
            BendShape::PreRelease => Bend::pre_bend_release(cents),
        }
    }

    /// Whether `t` already has this technique.
    pub fn is_on(&self, t: &Techniques) -> bool {
        match *self {
            Tech::Flag(f) => t.has(f),
            Tech::Slide(k) => t.slide == Some(k),
            Tech::Bend(shape, cents) => t.bend.as_ref().is_some_and(|b| {
                b.peak() == cents
                    && b.kind()
                        == match shape {
                            BendShape::Up => BendKind::Bend,
                            BendShape::Release => BendKind::BendRelease,
                            BendShape::Pre => BendKind::PreBend,
                            BendShape::PreRelease => BendKind::PreBendRelease,
                        }
            }),
            Tech::Vibrato(k) => t.vibrato == Some(k),
            Tech::Harmonic(k) => t.harmonic.is_some_and(|h| h.kind == k),
            Tech::Clear => false,
        }
    }

    /// Whether the command switches the technique on for notes with these techniques: when not
    /// every one has it. Clear always acts.
    pub fn toggle_on<'a>(&self, all: impl IntoIterator<Item = &'a Techniques>) -> bool {
        match self {
            Tech::Clear => true,
            _ => !all.into_iter().all(|t| self.is_on(t)),
        }
    }

    /// Switches the technique on (`on`) or off in `t`. A hammer-on and a pull-off exclude each
    /// other, a dead note and a ghost note too, and a note has one bend curve (a new shape or
    /// amount replaces the old).
    pub fn apply(&self, t: &mut Techniques, on: bool) {
        match *self {
            Tech::Flag(f) => {
                if on {
                    t.flags.insert(f);
                    if f == TechFlags::HAMMER_ON {
                        t.flags.remove(TechFlags::PULL_OFF);
                    } else if f == TechFlags::PULL_OFF {
                        t.flags.remove(TechFlags::HAMMER_ON);
                    } else if f == TechFlags::DEAD {
                        t.flags.remove(TechFlags::GHOST);
                    } else if f == TechFlags::GHOST {
                        t.flags.remove(TechFlags::DEAD);
                    }
                } else {
                    t.flags.remove(f);
                }
            }
            Tech::Slide(k) => t.slide = on.then_some(k),
            Tech::Bend(shape, cents) => t.bend = on.then(|| Self::curve(shape, cents)),
            Tech::Vibrato(k) => t.vibrato = on.then_some(k),
            Tech::Harmonic(k) => {
                t.harmonic = on.then_some(Harmonic {
                    kind: k,
                    fret: None,
                });
            }
            Tech::Clear => *t = Techniques::default(),
        }
    }
}

/// A button of the technique toolbar.
#[derive(Debug, Clone, Copy)]
pub struct ToolItem {
    /// The `editor.tab_technique` argument it runs.
    pub arg: &'static str,
    /// The text on the button.
    pub label: &'static str,
    /// The tooltip.
    pub tip: &'static str,
}

/// The toolbar, in order. The bend amount chip sits after the bend button (see the view).
pub const TOOLBAR: &[ToolItem] = &[
    ToolItem {
        arg: "hammer_on",
        label: "H",
        tip: "Hammer-on into this note",
    },
    ToolItem {
        arg: "pull_off",
        label: "P",
        tip: "Pull-off into this note",
    },
    ToolItem {
        arg: "slide_legato",
        label: "Sl",
        tip: "Legato slide to the next note",
    },
    ToolItem {
        arg: "slide_shift",
        label: "Ss",
        tip: "Shift slide to the next note",
    },
    ToolItem {
        arg: "bend",
        label: "Bd",
        tip: "Bend by the amount shown",
    },
    ToolItem {
        arg: "bend_release",
        label: "Br",
        tip: "Bend and release",
    },
    ToolItem {
        arg: "pre_bend",
        label: "Pb",
        tip: "Pre-bend",
    },
    ToolItem {
        arg: "vibrato",
        label: "Vb",
        tip: "Vibrato",
    },
    ToolItem {
        arg: "harmonic",
        label: "Hm",
        tip: "Natural harmonic",
    },
    ToolItem {
        arg: "palm_mute",
        label: "PM",
        tip: "Palm mute",
    },
    ToolItem {
        arg: "let_ring",
        label: "LR",
        tip: "Let ring",
    },
    ToolItem {
        arg: "dead",
        label: "x",
        tip: "Dead note",
    },
    ToolItem {
        arg: "ghost",
        label: "( )",
        tip: "Ghost note",
    },
    ToolItem {
        arg: "accent",
        label: ">",
        tip: "Accent",
    },
    ToolItem {
        arg: "tap",
        label: "Tp",
        tip: "Tapped note",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Techniques {
        Techniques::default()
    }

    #[test]
    fn every_command_argument_is_understood_and_every_unknown_one_is_not() {
        for name in crate::keymap::TAB_TECHNIQUE_ARGS {
            assert!(Tech::parse(name, DEFAULT_BEND).is_some(), "{name}");
        }
        assert!(Tech::parse("whammy", DEFAULT_BEND).is_none());
        for item in TOOLBAR {
            assert!(
                Tech::parse(item.arg, DEFAULT_BEND).is_some(),
                "{}",
                item.arg
            );
        }
    }

    #[test]
    fn a_toggle_sets_when_any_note_lacks_it_and_clears_when_all_have_it() {
        let t = Tech::parse("palm_mute", DEFAULT_BEND).unwrap();
        let mut a = plain();
        let mut b = plain();
        assert!(t.toggle_on([&a, &b]));
        t.apply(&mut a, true);
        assert!(t.toggle_on([&a, &b]), "a mixed selection gets it");
        t.apply(&mut b, true);
        assert!(!t.toggle_on([&a, &b]));
        t.apply(&mut a, false);
        assert!(!a.has(TechFlags::PALM_MUTE));
    }

    #[test]
    fn exclusive_flags_replace_each_other() {
        let mut t = plain();
        Tech::Flag(TechFlags::HAMMER_ON).apply(&mut t, true);
        Tech::Flag(TechFlags::PULL_OFF).apply(&mut t, true);
        assert!(t.has(TechFlags::PULL_OFF) && !t.has(TechFlags::HAMMER_ON));
        Tech::Flag(TechFlags::DEAD).apply(&mut t, true);
        Tech::Flag(TechFlags::GHOST).apply(&mut t, true);
        assert!(t.has(TechFlags::GHOST) && !t.has(TechFlags::DEAD));
    }

    #[test]
    fn bends_carry_their_amount_and_shape() {
        let bend = Tech::parse("bend", 300).unwrap();
        let mut t = plain();
        bend.apply(&mut t, true);
        assert_eq!(t.bend.as_ref().map(Bend::peak), Some(300));
        assert!(bend.is_on(&t));
        assert!(
            !Tech::parse("bend", 200).unwrap().is_on(&t),
            "another amount"
        );
        assert!(
            !Tech::parse("bend_release", 300).unwrap().is_on(&t),
            "another shape"
        );
        let pre = Tech::parse("pre_bend", 100).unwrap();
        pre.apply(&mut t, true);
        assert_eq!(t.bend.as_ref().map(Bend::kind), Some(BendKind::PreBend));
        pre.apply(&mut t, false);
        assert!(t.bend.is_none());
    }

    #[test]
    fn clear_removes_everything() {
        let mut t = plain();
        Tech::parse("slide_shift", 0).unwrap().apply(&mut t, true);
        Tech::parse("harmonic", 0).unwrap().apply(&mut t, true);
        Tech::Clear.apply(&mut t, true);
        assert!(t.is_empty());
    }

    #[test]
    fn bend_amounts_have_names() {
        for c in BEND_CENTS {
            assert_eq!(bend_from_name(bend_name(c).unwrap()), Some(c));
        }
        assert_eq!(bend_chip(100), "1/2");
    }
}
