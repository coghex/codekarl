//! The checked, startup-loaded UI glossary and its Lua extension.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use ck_lua::{ConfigValue, Extension, HostFunction};

use crate::font::FontSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Glossary {
    entries: BTreeMap<String, Entry>,
}

impl Glossary {
    pub(crate) fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(key)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// The files the glossary is made of, in the order they load (CK-375): the game's terms, then the character terms. Each is its own byte copy of
/// the designer's file; a key in both is a load error, and a link in either may name a term of the other.
const FILES: [&str; 2] = ["ui/glossary.toml", "ui/glossary_characters.toml"];

/// Parses and validates one glossary file against the shipped small font: the tests' fixtures are single files; the game loads [`parse_files`].
#[cfg(test)]
pub(crate) fn parse(text: &str, fonts: &FontSet) -> Result<Glossary, String> {
    let entries = parse_terms(text, fonts)?;
    for (key, entry) in &entries {
        validate_markup(&entry.text, &format!("[term.{key}]"), &entries)?;
    }
    Ok(Glossary { entries })
}

/// Parses and validates several glossary files as one glossary: each file's terms, a key that two of them define refused, and every link
/// checked against all the terms. `files` are `(name, text)`; an error names its file.
pub(crate) fn parse_files(files: &[(&str, &str)], fonts: &FontSet) -> Result<Glossary, String> {
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut origin: BTreeMap<String, &str> = BTreeMap::new();
    for (name, text) in files {
        for (key, entry) in parse_terms(text, fonts).map_err(|error| format!("{name}: {error}"))? {
            if let Some(first) = origin.get(&key) {
                return Err(format!("{name}: [term.{key}]: defined in {first} as well"));
            }
            origin.insert(key.clone(), name);
            entries.insert(key, entry);
        }
    }
    for (name, text) in files {
        for key in parse_terms(text, fonts)?.keys() {
            validate_markup(
                &entries[key].text,
                &format!("{name}: [term.{key}]"),
                &entries,
            )?;
        }
    }
    Ok(Glossary { entries })
}

/// The terms of one file, each checked on its own (the keys, the fields, the glyphs); the links are checked by the caller against every term.
fn parse_terms(text: &str, fonts: &FontSet) -> Result<BTreeMap<String, Entry>, String> {
    let root: toml::Value = toml::from_str(text).map_err(|error| error.message().to_owned())?;
    let root = root
        .as_table()
        .ok_or_else(|| "the file is not a table".to_owned())?;
    for key in root.keys() {
        if key != "term" {
            return Err(format!("`{key}`: only [term.<key>] entries are accepted"));
        }
    }
    let mut entries = BTreeMap::new();
    if let Some(terms) = root.get("term") {
        let terms = terms
            .as_table()
            .ok_or_else(|| "[term] is not a table".to_owned())?;
        for (key, value) in terms {
            if !valid_key(key) {
                return Err(format!("[term.{key}]: key must match [a-z][a-z0-9_]*"));
            }
            let place = format!("[term.{key}]");
            let fields = value
                .as_table()
                .ok_or_else(|| format!("{place}: not a table"))?;
            for field in fields.keys() {
                if !matches!(field.as_str(), "title" | "text") {
                    return Err(format!("{place}: unknown key `{field}`"));
                }
            }
            let title = fields
                .get("title")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("{place}: missing string `title`"))?
                .to_owned();
            let text = fields
                .get("text")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("{place}: missing string `text`"))?
                .to_owned();
            validate_glyphs(fonts, &place, &title)?;
            validate_glyphs(fonts, &place, &text)?;
            entries.insert(
                key.clone(),
                Entry {
                    key: key.clone(),
                    title,
                    text,
                },
            );
        }
    }
    Ok(entries)
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.bytes();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn validate_glyphs(fonts: &FontSet, place: &str, text: &str) -> Result<(), String> {
    if let Some(character) = text
        .chars()
        .find(|character| !fonts.has_small_glyph(*character))
    {
        return Err(format!(
            "{place}: character {character:?} has no glyph in font small"
        ));
    }
    Ok(())
}

fn validate_markup(
    text: &str,
    place: &str,
    entries: &BTreeMap<String, Entry>,
) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' if bytes.get(index + 1) == Some(&b'{') => index += 2,
            b'}' if bytes.get(index + 1) == Some(&b'}') => index += 2,
            b'{' => {
                let start = index + 1;
                let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == b'}')
                else {
                    return Err(format!("{place}: unclosed '{{' in text"));
                };
                let end = start + relative_end;
                let body = &text[start..end];
                let (key, words) = body
                    .split_once('|')
                    .map_or((body, None), |(key, words)| (key, Some(words)));
                if words.is_some_and(str::is_empty) {
                    return Err(format!("{place}: term `{{{body}}}` has empty words"));
                }
                if !entries.contains_key(key) {
                    return Err(format!(
                        "{place}: term `{{{body}}}` names no glossary entry `{key}`"
                    ));
                }
                index = end + 1;
            }
            b'}' => return Err(format!("{place}: unmatched '}}' in text")),
            _ => index += 1,
        }
    }
    Ok(())
}

pub(crate) fn load(assets: &Path, fonts: &FontSet) -> Result<Glossary, String> {
    let mut texts = Vec::new();
    for file in FILES {
        let path = assets.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        texts.push((path.display().to_string(), text));
    }
    let files: Vec<(&str, &str)> = texts
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    parse_files(&files, fonts)
}

fn entry_function(glossary: Arc<Glossary>) -> HostFunction {
    Arc::new(move |arguments| match arguments {
        [Some(ConfigValue::String(key))] => {
            Ok(glossary.entry(key).map_or_else(Vec::new, |entry| {
                vec![ConfigValue::Table(BTreeMap::from([
                    ("key".to_owned(), ConfigValue::String(entry.key.clone())),
                    ("title".to_owned(), ConfigValue::String(entry.title.clone())),
                    ("text".to_owned(), ConfigValue::String(entry.text.clone())),
                ]))]
            }))
        }
        _ => Err("glossary.entry: expected one string key".to_owned()),
    })
}

fn keys_function(glossary: Arc<Glossary>) -> HostFunction {
    Arc::new(move |arguments| match arguments {
        [] => Ok(vec![ConfigValue::Array(
            glossary
                .keys()
                .map(|key| ConfigValue::String(key.to_owned()))
                .collect(),
        )]),
        _ => Err("glossary.keys: expected no arguments".to_owned()),
    })
}

pub(crate) fn extension(glossary: Arc<Glossary>) -> Extension {
    Extension::new("glossary")
        .function("entry", entry_function(Arc::clone(&glossary)))
        .function("keys", keys_function(glossary))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn fonts() -> FontSet {
        crate::font::load_all(&assets()).expect("shipped fonts")
    }

    #[test]
    fn shipped_glossary_has_172_sorted_entries() {
        let fonts = fonts();
        let shipped = load(&assets(), &fonts).expect("the shipped glossary");
        let keys = shipped.keys().collect::<Vec<_>>();
        // The game's 109 terms and the 63 of the character glossary (CK-375).
        assert_eq!(keys.len(), 109 + 63);
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!assets().join("ui/glossary-r1.toml").exists());
    }

    #[test]
    fn approved_glossary_literals_are_exact() {
        let shipped = load(&assets(), &fonts()).expect("the shipped glossary");
        let location = shipped.entry("location").expect("location entry");
        assert_eq!(location.title, "location");
        assert_eq!(
            location.text,
            "A single place on the map, consisting of {pop|pops} and {building|buildings}. It draws on the land around it in its {resource_radius|resource radius}."
        );
        assert_eq!(
            shipped.entry("peas_beans").expect("peas_beans entry").title,
            "peas and beans"
        );
        assert_eq!(
            shipped
                .entry("northern_ui_neill")
                .expect("northern_ui_neill entry")
                .title,
            "Northern Uí Néill"
        );
    }

    #[test]
    fn all_economy_ids_have_glossary_entries() {
        let shipped = load(&assets(), &fonts()).expect("the shipped glossary");
        let economy: toml::Value = toml::from_str(
            &std::fs::read_to_string(assets().join("game/economy.toml"))
                .expect("shipped economy data"),
        )
        .expect("valid shipped economy data");

        let mut ids = Vec::new();
        for good in economy["goods"].as_array().expect("goods array") {
            ids.push(good["id"].as_str().expect("good id"));
        }
        for building in economy["buildings"].as_array().expect("buildings array") {
            if let Some(methods) = building.get("methods").and_then(toml::Value::as_array) {
                for method in methods {
                    ids.push(method["id"].as_str().expect("method id"));
                }
            }
        }
        let peoples: toml::Value =
            toml::from_str(&std::fs::read_to_string(assets().join("game/peoples.toml")).unwrap())
                .unwrap();
        ids.extend(
            peoples["cultures"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value["id"].as_str().unwrap()),
        );
        ids.extend(
            economy["religions"]
                .as_array()
                .expect("religion array")
                .iter()
                .map(|value| value.as_str().expect("religion id")),
        );
        for id in ids {
            assert!(shipped.entry(id).is_some(), "missing glossary entry {id}");
        }
    }

    #[test]
    fn fixture_glossary_is_sorted_and_expands() {
        let fonts = fonts();
        let fixture = parse(
            "[term.pop]\ntitle = \"population\"\ntext = \"A group of people.\"\n\
             [term.population]\ntitle = \"Population\"\ntext = \"The {pop|pops} count.\"\n",
            &fonts,
        )
        .expect("fixture glossary");
        assert_eq!(fixture.keys().collect::<Vec<_>>(), ["pop", "population"]);
        assert_eq!(
            fixture.entry("population").unwrap().text,
            "The {pop|pops} count."
        );
    }

    #[test]
    fn glossary_refusals_name_the_file_entry_and_problem() {
        let fonts = fonts();
        for (text, expected) in [
            (
                "[term.pop]\ntitle=\"Pop\"\ntext=\"{county}\"\n",
                "no glossary entry",
            ),
            ("[term.pop]\ntitle=\"Pop\"\ntext=\"{\"\n", "unclosed"),
            (
                "[term.pop]\ntitle=\"Pop\"\ntext=\"{pop|}\"\n",
                "empty words",
            ),
            ("[term.pop]\ntext=\"body\"\n", "title"),
            (
                "[term.pop]\ntitle=\"Pop\"\ntext=\"body\"\ncolour=\"red\"\n",
                "colour",
            ),
            ("[term.pop]\ntitle=\"Pop\"\ntext=\"—\"\n", "font small"),
        ] {
            let error = parse(text, &fonts).expect_err(text);
            assert!(
                error.contains("[term.pop]") && error.contains(expected),
                "{text:?}: {error}"
            );
        }
    }

    #[test]
    fn two_files_load_as_one_glossary_with_links_across_and_a_shared_key_is_refused() {
        let fonts = fonts();
        let game = "[term.pop]\ntitle = \"pop\"\ntext = \"A group of {strength}.\"\n";
        let characters =
            "[term.strength]\ntitle = \"strength\"\ntext = \"How strong a {pop} is.\"\n";
        let both = parse_files(
            &[("game.toml", game), ("characters.toml", characters)],
            &fonts,
        )
        .expect("links cross between the files");
        assert_eq!(both.keys().collect::<Vec<_>>(), ["pop", "strength"]);
        // Each alone has a link to a term it does not have.
        assert!(
            parse(game, &fonts)
                .unwrap_err()
                .contains("no glossary entry")
        );
        assert!(
            parse(characters, &fonts)
                .unwrap_err()
                .contains("no glossary entry")
        );
        // A key in both is a load error naming the second file and the term.
        let twice = "[term.pop]\ntitle = \"pop\"\ntext = \"Again.\"\n";
        let error = parse_files(&[("game.toml", game), ("characters.toml", twice)], &fonts)
            .expect_err("a duplicated key");
        assert!(
            error.contains("characters.toml")
                && error.contains("[term.pop]")
                && error.contains("game.toml"),
            "{error}"
        );
        // A link to nothing in either file names the file and the term.
        let error = parse_files(
            &[
                ("game.toml", game),
                (
                    "characters.toml",
                    "[term.strength]\ntitle = \"s\"\ntext = \"{nowhere}\"\n",
                ),
            ],
            &fonts,
        )
        .expect_err("a dead link");
        assert!(
            error.contains("characters.toml") && error.contains("[term.strength]"),
            "{error}"
        );
    }

    #[test]
    fn a_character_term_links_to_a_game_term_and_every_term_of_the_first_tab_is_there() {
        let shipped = load(&assets(), &fonts()).expect("the shipped glossary");
        // A link from a character term to a game term resolves (the load checked it); this one is known to cross.
        let crossing = shipped
            .keys()
            .filter(|key| {
                let text = &shipped.entry(key).expect("an entry").text;
                std::fs::read_to_string(assets().join("ui/glossary_characters.toml"))
                    .expect("the character glossary")
                    .contains(&format!("[term.{key}]"))
                    && [
                        "{pop",
                        "{culture",
                        "{religion",
                        "{satisfaction",
                        "{food_days",
                    ]
                    .iter()
                    .any(|link| text.contains(link))
            })
            .count();
        assert!(crossing >= 1, "some character term links to a game term");
        // Every live quantity the first tab names in a tooltip has a term: the nine body stats, the eleven skills, the seven virtue pairs, the
        // four humours, the stress and its threshold, the nine manner pairs, the three standings and the two chip words.
        for key in [
            "height",
            "weight",
            "strength",
            "wit",
            "intellect",
            "cunning",
            "longevity",
            "constitution",
            "fecundity",
            "leadership",
            "craftsmanship",
            "seamanship",
            "scholarship",
            "horsemanship",
            "martial_prowess",
            "oratory",
            "administration",
            "theology",
            "medicine",
            "husbandry",
            "pride_humility",
            "greed_charity",
            "lust_chastity",
            "envy_kindness",
            "gluttony_temperance",
            "wrath_patience",
            "sloth_diligence",
            "blood",
            "yellow_bile",
            "black_bile",
            "phlegm",
            "stress",
            "stress_threshold",
            "guarded_open",
            "deceitful_honest",
            "suspicious_trusting",
            "wary_daring",
            "traditional_curious",
            "yielding_stubborn",
            "solitary_gregarious",
            "content_ambitious",
            "harsh_merciful",
            "popularity",
            "fame",
            "honour",
            "trait",
            "attribute",
        ] {
            assert!(shipped.entry(key).is_some(), "no term for {key}");
        }
    }
}
