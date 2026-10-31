//! Categories and per-category filtering.

use std::collections::HashMap;
use std::fmt;

use crate::Level;

/// Whether `category` is a valid category: a non-empty string of ASCII letters, digits, `_` and `.`.
#[must_use]
pub fn is_valid_category(category: &str) -> bool {
    !category.is_empty()
        && category
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.')
}

/// The text is not a valid category name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCategory {
    category: String,
}

impl InvalidCategory {
    /// The rejected text.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }
}

impl fmt::Display for InvalidCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid log category {:?}: use a non-empty name of ASCII letters, digits, '_' and '.'",
            self.category
        )
    }
}

impl std::error::Error for InvalidCategory {}

/// A default level and per-category levels.
///
/// For a record in category `a.b.c`, the most specific of `a.b.c`, `a.b` and `a` that has a level
/// wins; otherwise the default applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    default: Level,
    categories: HashMap<String, Level>,
}

impl Filter {
    /// A filter that applies `default` to every category.
    #[must_use]
    pub fn new(default: Level) -> Self {
        Self {
            default,
            categories: HashMap::new(),
        }
    }

    /// The same filter with `category` set to `level`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCategory`] when `category` is not a valid category name.
    pub fn with_category(mut self, category: &str, level: Level) -> Result<Self, InvalidCategory> {
        self.set_category(category, level)?;
        Ok(self)
    }

    /// Sets the level of `category`, replacing an earlier level for it.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCategory`] when `category` is not a valid category name.
    pub fn set_category(&mut self, category: &str, level: Level) -> Result<(), InvalidCategory> {
        if !is_valid_category(category) {
            return Err(InvalidCategory {
                category: category.to_owned(),
            });
        }
        self.categories.insert(category.to_owned(), level);
        Ok(())
    }

    /// The default level.
    #[must_use]
    pub const fn default_level(&self) -> Level {
        self.default
    }

    /// The most verbose level that is emitted for `category`.
    #[must_use]
    pub fn level_for(&self, category: &str) -> Level {
        let mut name = category;
        loop {
            if let Some(level) = self.categories.get(name) {
                return *level;
            }
            match name.rsplit_once('.') {
                Some((parent, _)) => name = parent,
                None => return self.default,
            }
        }
    }

    /// Whether a record of `level` in `category` is emitted.
    #[must_use]
    pub fn allows(&self, level: Level, category: &str) -> bool {
        level <= self.level_for(category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_category_names() {
        for good in ["runtime", "config", "lua.ui", "a_b.C9", "9"] {
            assert!(is_valid_category(good), "{good}");
        }
        for bad in ["", "a b", "a-b", "a:b", "caf\u{e9}", "a\n", "a/b"] {
            assert!(!is_valid_category(bad), "{bad:?}");
        }
    }

    #[test]
    fn default_only() {
        let filter = Filter::new(Level::Info);
        assert!(filter.allows(Level::Error, "runtime"));
        assert!(filter.allows(Level::Info, "runtime"));
        assert!(!filter.allows(Level::Debug, "runtime"));
        assert_eq!(filter.default_level(), Level::Info);
    }

    #[test]
    fn exact_category_overrides_the_default_in_both_directions() {
        let filter = Filter::new(Level::Info)
            .with_category("config", Level::Trace)
            .expect("must be Ok or Some")
            .with_category("noisy", Level::Error)
            .expect("must be Ok or Some");
        assert!(filter.allows(Level::Trace, "config"));
        assert!(!filter.allows(Level::Warn, "noisy"));
        assert!(filter.allows(Level::Error, "noisy"));
        assert!(!filter.allows(Level::Debug, "other"));
    }

    #[test]
    fn dotted_category_falls_back_to_its_parents_then_the_default() {
        let filter = Filter::new(Level::Warn)
            .with_category("lua", Level::Debug)
            .expect("must be Ok or Some");
        assert_eq!(filter.level_for("lua.ui"), Level::Debug);
        assert_eq!(filter.level_for("lua.ui.deep"), Level::Debug);
        assert_eq!(filter.level_for("lua"), Level::Debug);
        assert_eq!(
            filter.level_for("luau"),
            Level::Warn,
            "a prefix is not a parent"
        );
        assert_eq!(filter.level_for("runtime.lua"), Level::Warn);
    }

    #[test]
    fn the_most_specific_category_wins() {
        let filter = Filter::new(Level::Warn)
            .with_category("a", Level::Trace)
            .expect("must be Ok or Some")
            .with_category("a.b", Level::Error)
            .expect("must be Ok or Some")
            .with_category("a.b.c.d", Level::Info)
            .expect("must be Ok or Some");
        assert_eq!(filter.level_for("a.b.c"), Level::Error);
        assert_eq!(filter.level_for("a.b.c.d"), Level::Info);
        assert_eq!(filter.level_for("a.x"), Level::Trace);
        assert_eq!(filter.level_for("a.b.c.d.e"), Level::Info);
    }

    #[test]
    fn rejects_an_invalid_category_name() {
        let error = Filter::new(Level::Info)
            .with_category("bad name", Level::Debug)
            .expect_err("must be rejected");
        assert_eq!(error.category(), "bad name");
        assert!(error.to_string().contains("bad name"));
    }
}
