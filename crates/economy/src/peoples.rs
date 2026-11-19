//! Validated culture and language registry.
use crate::{DataError, unique_ids};
use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// The complete authored registry.
pub struct Peoples {
    /// Declared culture subgroups.
    pub subgroups: Vec<String>,
    /// Declared supercultures.
    pub supercultures: Vec<String>,
    /// Declared language families.
    pub families: Vec<String>,
    /// Language supergroups.
    pub supergroups: Vec<Parent>,
    /// Language groups.
    pub groups: Vec<Parent>,
    /// Declared languages.
    pub languages: Vec<Language>,
    /// Declared cultures.
    pub cultures: Vec<Culture>,
    /// Declared religions.
    pub religions: Vec<Religion>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// A keyed hierarchy entry and its parent.
pub struct Parent {
    /// Stable key.
    pub id: String,
    /// Key in the parent tier.
    pub parent: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// A language and its dialects.
pub struct Language {
    /// Stable key.
    pub id: String,
    /// Language group key.
    pub group: String,
    #[serde(default)]
    /// Dialect keys belonging to this language.
    pub dialects: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// A culture and its language.
pub struct Culture {
    /// Stable key.
    pub id: String,
    /// Culture subgroup key.
    pub subgroup: String,
    /// Superculture key.
    pub superculture: String,
    /// Language key.
    pub language: String,
    /// Dialect key, when the language has dialects.
    pub dialect: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// A religion and its optional liturgical language.
pub struct Religion {
    /// Stable key.
    pub id: String,
    /// Optional liturgical language key.
    pub liturgical_language: Option<String>,
}
impl Peoples {
    /// Load and validate a registry.
    ///
    /// # Errors
    /// Returns a source-qualified error for invalid or unreadable data.
    pub fn load(path: &Path) -> Result<Self, DataError> {
        let file = path.display().to_string();
        let text = fs::read_to_string(path).map_err(|e| DataError {
            file: file.clone(),
            entry: "<file>".into(),
            problem: e.to_string(),
        })?;
        let data: Self = toml::from_str(&text).map_err(|e: toml::de::Error| DataError {
            file: file.clone(),
            entry: "<document>".into(),
            problem: e.message().into(),
        })?;
        data.validate(&file)?;
        Ok(data)
    }
    /// Check uniqueness and references.
    ///
    /// # Errors
    /// Returns a source-qualified error for invalid data.
    pub fn validate(&self, file: &str) -> Result<(), DataError> {
        let bad = |entry: String, value: &str| DataError {
            file: file.into(),
            entry,
            problem: format!("unknown or invalid key \"{value}\""),
        };
        for (name, ids) in [
            (
                "subgroups",
                self.subgroups
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            ),
            (
                "supercultures",
                self.supercultures.iter().map(String::as_str).collect(),
            ),
            (
                "families",
                self.families.iter().map(String::as_str).collect(),
            ),
            (
                "supergroups",
                self.supergroups.iter().map(|x| x.id.as_str()).collect(),
            ),
            (
                "groups",
                self.groups.iter().map(|x| x.id.as_str()).collect(),
            ),
            (
                "languages",
                self.languages.iter().map(|x| x.id.as_str()).collect(),
            ),
            (
                "cultures",
                self.cultures.iter().map(|x| x.id.as_str()).collect(),
            ),
            (
                "religions",
                self.religions.iter().map(|x| x.id.as_str()).collect(),
            ),
        ] {
            unique_ids(file, name, ids.into_iter())?;
        }
        for x in &self.supergroups {
            if !self.families.contains(&x.parent) {
                return Err(bad(format!("supergroups.{}.parent", x.id), &x.parent));
            }
        }
        for x in &self.groups {
            if !self.supergroups.iter().any(|p| p.id == x.parent) {
                return Err(bad(format!("groups.{}.parent", x.id), &x.parent));
            }
        }
        let mut dialects = BTreeSet::new();
        for x in &self.languages {
            if !self.groups.iter().any(|p| p.id == x.group) {
                return Err(bad(format!("languages.{}.group", x.id), &x.group));
            }
            for d in &x.dialects {
                if !dialects.insert(d) {
                    return Err(bad(format!("languages.{}.dialects", x.id), d));
                }
            }
        }
        for x in &self.cultures {
            if !self.subgroups.contains(&x.subgroup) {
                return Err(bad(format!("cultures.{}.subgroup", x.id), &x.subgroup));
            }
            if !self.supercultures.contains(&x.superculture) {
                return Err(bad(
                    format!("cultures.{}.superculture", x.id),
                    &x.superculture,
                ));
            }
            let l = self
                .language(&x.language)
                .ok_or_else(|| bad(format!("cultures.{}.language", x.id), &x.language))?;
            if match &x.dialect {
                Some(d) => !l.dialects.contains(d),
                None => !l.dialects.is_empty(),
            } {
                return Err(bad(
                    format!("cultures.{}.dialect", x.id),
                    x.dialect.as_deref().unwrap_or("<missing>"),
                ));
            }
        }
        for x in &self.religions {
            if let Some(l) = &x.liturgical_language
                && self.language(l).is_none()
            {
                return Err(bad(format!("religions.{}.liturgical_language", x.id), l));
            }
        }
        Ok(())
    }
    /// Find a culture by key.
    #[must_use]
    pub fn culture(&self, id: &str) -> Option<&Culture> {
        self.cultures.iter().find(|x| x.id == id)
    }
    /// Find a language by key.
    #[must_use]
    pub fn language(&self, id: &str) -> Option<&Language> {
        self.languages.iter().find(|x| x.id == id)
    }
    /// Return the group, supergroup and family for a language.
    #[must_use]
    pub fn language_ancestry(&self, id: &str) -> Option<(&str, &str, &str)> {
        let l = self.language(id)?;
        let g = self.groups.iter().find(|x| x.id == l.group)?;
        let s = self.supergroups.iter().find(|x| x.id == g.parent)?;
        Some((&g.id, &s.id, &s.parent))
    }
    /// Find a religion and its optional liturgical language.
    #[must_use]
    pub fn liturgical_language(&self, id: &str) -> Option<Option<&str>> {
        self.religions
            .iter()
            .find(|x| x.id == id)
            .map(|x| x.liturgical_language.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shipped() -> Peoples {
        Peoples::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/peoples.toml"))
            .unwrap()
    }
    #[test]
    fn shipped_hierarchy_and_liturgy_resolve() {
        let p = shipped();
        let c = p.culture("mercian").unwrap();
        assert_eq!(
            (
                &*c.subgroup,
                &*c.superculture,
                &*c.language,
                c.dialect.as_deref()
            ),
            ("english", "isles", "old_english", Some("mercian"))
        );
        assert_eq!(
            p.language_ancestry("old_irish"),
            Some(("goidelic", "celtic", "indo_european"))
        );
        assert_eq!(p.liturgical_language("christianity"), Some(Some("latin")));
        assert_eq!(p.liturgical_language("irish_paganism"), Some(None));
        assert_eq!(p.liturgical_language("missing"), None);
    }
    #[test]
    fn rejects_bad_references_duplicates_and_dialects() {
        let original = shipped();
        let mut cases: Vec<(Peoples, &str)> = Vec::new();
        let mut p = original.clone();
        p.groups[0].parent = "missing".into();
        cases.push((p, "groups.goidelic.parent"));
        let mut p = original.clone();
        p.groups.push(p.groups[0].clone());
        cases.push((p, "groups"));
        let mut p = original.clone();
        p.languages[1].dialects.push("irish".into());
        cases.push((p, "languages.old_welsh.dialects"));
        let mut p = original.clone();
        p.cultures[0].subgroup = "missing".into();
        cases.push((p, "cultures.ulaid.subgroup"));
        let mut p = original.clone();
        p.cultures[0].superculture = "missing".into();
        cases.push((p, "cultures.ulaid.superculture"));
        let mut p = original.clone();
        p.cultures[0].language = "missing".into();
        cases.push((p, "cultures.ulaid.language"));
        let mut p = original.clone();
        p.cultures[0].dialect = Some("missing".into());
        cases.push((p, "cultures.ulaid.dialect"));
        let mut p = original.clone();
        p.cultures[0].dialect = Some("mercian".into());
        cases.push((p, "cultures.ulaid.dialect"));
        let mut p = original.clone();
        p.cultures[0].dialect = None;
        cases.push((p, "cultures.ulaid.dialect"));
        let mut p = original.clone();
        p.religions[0].liturgical_language = Some("missing".into());
        cases.push((p, "religions.christianity.liturgical_language"));
        for (p, entry) in cases {
            let e = p.validate("fixture/peoples.toml").unwrap_err();
            assert_eq!(e.file, "fixture/peoples.toml");
            assert!(e.entry.starts_with(entry), "{e}");
        }
    }
}
