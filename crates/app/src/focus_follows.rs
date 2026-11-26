//! What the two foci of the open songs do when the song you work in changes (Settings,
//! Workflow, **Focus follows the active song**).
//!
//! The **Play Focus** is the song that owns the audio stream and plays; the **Input Focus** is
//! the song that takes the live MIDI keyboard and the controllers. Clicking in another song's
//! window, choosing it in the Window menu or opening a new song makes it the active song; this
//! choice says which of the foci go with it.

/// Which foci follow the active song.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusFollows {
    /// Both: the audio and the MIDI input go to the active song (the default; with one song at
    /// a time having both, nothing else is ever needed).
    #[default]
    Both,
    /// Only the Play Focus: the audio goes to the active song, the MIDI input stays where it
    /// was put with the Input button of its song.
    PlayOnly,
    /// Only the Input Focus: the MIDI input goes to the active song, the audio stays where it
    /// was put with the Play button of its song.
    InputOnly,
    /// Neither: both stay until a Play or Input button moves them.
    Neither,
}

impl FocusFollows {
    /// Every choice, in button order.
    pub const ALL: [Self; 4] = [Self::Both, Self::PlayOnly, Self::InputOnly, Self::Neither];

    /// The name saved in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::PlayOnly => "play",
            Self::InputOnly => "input",
            Self::Neither => "none",
        }
    }

    /// The choice saved as `id`, if it is one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Both => "Both",
            Self::PlayOnly => "Play only",
            Self::InputOnly => "Input only",
            Self::Neither => "None",
        }
    }

    /// The next choice, wrapping at the end.
    pub fn next(self) -> Self {
        Self::ALL[(Self::ALL.iter().position(|m| *m == self).unwrap_or(0) + 1) % Self::ALL.len()]
    }

    /// Whether the Play Focus goes to the song that becomes active.
    pub fn moves_play(self) -> bool {
        matches!(self, Self::Both | Self::PlayOnly)
    }

    /// Whether the Input Focus goes to the song that becomes active.
    pub fn moves_input(self) -> bool {
        matches!(self, Self::Both | Self::InputOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_round_trip_through_their_names_and_cycle() {
        let mut seen = FocusFollows::default();
        for mode in FocusFollows::ALL {
            assert_eq!(FocusFollows::from_id(mode.id()), Some(mode));
            assert_eq!(seen, mode);
            seen = seen.next();
        }
        assert_eq!(seen, FocusFollows::Both, "the cycle closes");
        assert_eq!(FocusFollows::from_id("other"), None);
        assert_eq!(FocusFollows::default(), FocusFollows::Both);
    }

    #[test]
    fn each_choice_moves_the_foci_it_names() {
        let moves = |m: FocusFollows| (m.moves_play(), m.moves_input());
        assert_eq!(moves(FocusFollows::Both), (true, true));
        assert_eq!(moves(FocusFollows::PlayOnly), (true, false));
        assert_eq!(moves(FocusFollows::InputOnly), (false, true));
        assert_eq!(moves(FocusFollows::Neither), (false, false));
    }
}
