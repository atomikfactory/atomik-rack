//! How the master keyboard (the MIDI keyboard and the computer keys) picks its track (Settings,
//! Workflow, **Master keyboard**).

/// Which track receives the master keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MasterKeyboard {
    /// The keyboard follows the selected track: selecting a track arms it and sends the keyboard
    /// to it.
    #[default]
    Standard,
    /// The keyboard plays the track chosen with the arrow of its header, whatever is selected;
    /// selecting other tracks to edit them leaves it where it is.
    Separated,
}

impl MasterKeyboard {
    /// Every choice, in menu order.
    pub const ALL: [Self; 2] = [Self::Standard, Self::Separated];

    /// The name saved in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Separated => "separated",
        }
    }

    /// The choice saved as `id`, if it is one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Separated => "Separated",
        }
    }

    /// The other choice.
    pub fn next(self) -> Self {
        match self {
            Self::Standard => Self::Separated,
            Self::Separated => Self::Standard,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_round_trip_through_their_names() {
        for mode in MasterKeyboard::ALL {
            assert_eq!(MasterKeyboard::from_id(mode.id()), Some(mode));
            assert_ne!(mode.next(), mode);
            assert_eq!(mode.next().next(), mode);
        }
        assert_eq!(MasterKeyboard::from_id("other"), None);
        assert_eq!(MasterKeyboard::default(), MasterKeyboard::Standard);
    }
}
