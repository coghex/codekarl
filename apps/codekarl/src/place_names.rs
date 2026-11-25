//! Which of a place's two names the player sees (CK-423).
//!
//! A scenario location has a local (native) `name` and may have an English `name_en`. One rule gives the name shown: under
//! `"english"`, the English name when the place has one and otherwise the local name; under `"local"`, the local name. The rule
//! is one shared handle, read at the moment a view names a place from its id, so a change of the setting reaches every view
//! that asks afterwards, without a restart or a simulation tick. The setting is the player's (`[ui] place_names` in the settings
//! file), never part of a scenario or a save.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The settings value that shows English names, falling back to the local name. The default.
pub(crate) const ENGLISH: &str = "english";
/// The settings value that shows local names.
pub(crate) const LOCAL: &str = "local";

/// The player's choice of place names, shared by every reader (cloning shares it).
#[derive(Clone, Debug)]
pub(crate) struct PlaceNameSetting(Arc<AtomicBool>);

impl Default for PlaceNameSetting {
    fn default() -> Self {
        Self::new(ENGLISH)
    }
}

impl PlaceNameSetting {
    /// A handle holding `choice`: [`LOCAL`] shows local names, anything else English ones.
    pub(crate) fn new(choice: &str) -> Self {
        Self(Arc::new(AtomicBool::new(choice == LOCAL)))
    }

    /// Replaces the choice for every holder of this handle.
    pub(crate) fn set(&self, choice: &str) {
        self.0.store(choice == LOCAL, Ordering::Release);
    }

    /// The current choice, [`ENGLISH`] or [`LOCAL`].
    pub(crate) fn choice(&self) -> &'static str {
        if self.0.load(Ordering::Acquire) {
            LOCAL
        } else {
            ENGLISH
        }
    }

    /// The name shown for a place with this local name and, perhaps, an English one.
    pub(crate) fn shown<'a>(&self, local: &'a str, english: Option<&'a str>) -> &'a str {
        if self.choice() == LOCAL {
            local
        } else {
            english.unwrap_or(local)
        }
    }

    /// The place's other name: the one not shown, when it exists and differs from the shown one.
    pub(crate) fn other<'a>(&self, local: &'a str, english: Option<&'a str>) -> Option<&'a str> {
        let shown = self.shown(local, english);
        let other = if self.choice() == LOCAL {
            english?
        } else {
            local
        };
        (other != shown).then_some(other)
    }
}

#[cfg(test)]
mod tests {
    use super::{ENGLISH, LOCAL, PlaceNameSetting};

    #[test]
    fn the_default_shows_english_names_and_falls_back_to_the_local_name() {
        let setting = PlaceNameSetting::default();
        assert_eq!(setting.choice(), ENGLISH);
        assert_eq!(setting.shown("Biorra", Some("Birr")), "Birr");
        assert_eq!(setting.shown("Emain Macha", None), "Emain Macha");
    }

    #[test]
    fn the_local_choice_shows_the_local_name_with_or_without_an_english_one() {
        let setting = PlaceNameSetting::new(LOCAL);
        assert_eq!(setting.choice(), LOCAL);
        assert_eq!(setting.shown("Biorra", Some("Birr")), "Biorra");
        assert_eq!(setting.shown("Emain Macha", None), "Emain Macha");
    }

    #[test]
    fn a_change_reaches_every_clone_of_the_handle() {
        let setting = PlaceNameSetting::default();
        let reader = setting.clone();
        setting.set(LOCAL);
        assert_eq!(reader.shown("Corcaigh", Some("Cork")), "Corcaigh");
        setting.set(ENGLISH);
        assert_eq!(reader.shown("Corcaigh", Some("Cork")), "Cork");
    }

    #[test]
    fn the_other_name_is_the_one_not_shown_and_only_when_it_differs() {
        let setting = PlaceNameSetting::default();
        assert_eq!(setting.other("Biorra", Some("Birr")), Some("Biorra"));
        assert_eq!(setting.other("Emain Macha", None), None);
        assert_eq!(setting.other("Tara", Some("Tara")), None);
        setting.set(LOCAL);
        assert_eq!(setting.other("Biorra", Some("Birr")), Some("Birr"));
        assert_eq!(setting.other("Emain Macha", None), None);
        assert_eq!(setting.other("Tara", Some("Tara")), None);
    }
}
