//! What Add Transition makes: the choices of the menus and of `sequencer.add_transition`.

/// What Add Transition makes at a section boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionChoice {
    /// A reverse reverb of the next section's first hit, this many bars long, ending where the
    /// hit starts (printed).
    Riser(u8),
    /// The next section's first hit saturated and through a long reverb (printed).
    Impact,
    /// A low-pass on the master that closes over this many bars before the boundary.
    FilterDown(u8),
    /// The same on the group bus of the selected track.
    GroupFilterDown(u8),
    /// The master fader at zero for the first beat of the next section.
    Silence,
    /// The last beat of the drums played twice at double rate.
    Fill,
}

impl TransitionChoice {
    /// Every choice, in menu order.
    pub const ALL: [TransitionChoice; 12] = [
        TransitionChoice::Riser(1),
        TransitionChoice::Riser(2),
        TransitionChoice::Riser(4),
        TransitionChoice::Impact,
        TransitionChoice::FilterDown(1),
        TransitionChoice::FilterDown(2),
        TransitionChoice::FilterDown(4),
        TransitionChoice::GroupFilterDown(1),
        TransitionChoice::GroupFilterDown(2),
        TransitionChoice::GroupFilterDown(4),
        TransitionChoice::Silence,
        TransitionChoice::Fill,
    ];

    /// The registry argument of `sequencer.add_transition`.
    pub fn arg(self) -> &'static str {
        match self {
            TransitionChoice::Riser(1) => "riser-1",
            TransitionChoice::Riser(2) => "riser-2",
            TransitionChoice::Riser(_) => "riser-4",
            TransitionChoice::Impact => "impact",
            TransitionChoice::FilterDown(1) => "filter-1",
            TransitionChoice::FilterDown(2) => "filter-2",
            TransitionChoice::FilterDown(_) => "filter-4",
            TransitionChoice::GroupFilterDown(1) => "group-1",
            TransitionChoice::GroupFilterDown(2) => "group-2",
            TransitionChoice::GroupFilterDown(_) => "group-4",
            TransitionChoice::Silence => "silence",
            TransitionChoice::Fill => "fill",
        }
    }

    /// The choice of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.arg() == arg)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            TransitionChoice::Riser(1) => "Riser, 1 Bar",
            TransitionChoice::Riser(2) => "Riser, 2 Bars",
            TransitionChoice::Riser(_) => "Riser, 4 Bars",
            TransitionChoice::Impact => "Impact",
            TransitionChoice::FilterDown(1) => "Filter Down, 1 Bar",
            TransitionChoice::FilterDown(2) => "Filter Down, 2 Bars",
            TransitionChoice::FilterDown(_) => "Filter Down, 4 Bars",
            TransitionChoice::GroupFilterDown(1) => "Group Filter Down, 1 Bar",
            TransitionChoice::GroupFilterDown(2) => "Group Filter Down, 2 Bars",
            TransitionChoice::GroupFilterDown(_) => "Group Filter Down, 4 Bars",
            TransitionChoice::Silence => "Silence on Beat One",
            TransitionChoice::Fill => "Fill",
        }
    }

    /// The kind of transition it makes.
    pub fn kind(self) -> atomik_model::transition::TransitionKind {
        use atomik_model::transition::TransitionKind as K;
        match self {
            TransitionChoice::Riser(_) => K::Riser,
            TransitionChoice::Impact => K::Impact,
            TransitionChoice::FilterDown(_) | TransitionChoice::GroupFilterDown(_) => K::FilterDown,
            TransitionChoice::Silence => K::Silence,
            TransitionChoice::Fill => K::Fill,
        }
    }

    /// Whether a separator goes before it in a menu: before the first of each group.
    pub fn starts_group(self) -> bool {
        matches!(
            self,
            TransitionChoice::Impact | TransitionChoice::FilterDown(1) | TransitionChoice::Silence
        )
    }
}
