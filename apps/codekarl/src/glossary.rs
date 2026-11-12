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

/// Parses and validates the glossary against the shipped small font.
pub(crate) fn parse(text: &str, fonts: &FontSet) -> Result<Glossary, String> {
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
    for (key, entry) in &entries {
        validate_markup(&entry.text, &format!("[term.{key}]"), &entries)?;
    }
    Ok(Glossary { entries })
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
    let path = assets.join("ui/glossary.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&text, fonts).map_err(|error| format!("{}: {error}", path.display()))
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
    fn shipped_glossary_is_empty_and_fixture_glossary_is_sorted_and_expands() {
        let fonts = fonts();
        let shipped = load(&assets(), &fonts).expect("the shipped empty glossary");
        assert_eq!(shipped.keys().count(), 0);
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
}
