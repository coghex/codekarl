//! The checked theme data and the `theme` extension of the UI VM.
//!
//! Palette colours and the four opaque kind bases are read from `assets/ui/theme.toml`.
//! Managed kinds derive their shades from a base; custom kinds provide their own outline and hot.

mod colours;

use std::collections::BTreeMap;
use std::sync::Arc;

use ck_lua::{ConfigValue, Extension, HostFunction};

/// The keys every kind must have.
const REQUIRED: [&str; 2] = ["outline", "hot"];

/// The checked theme: colour strings by name, and kinds as tables of colour strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Theme {
    colours: BTreeMap<String, String>,
    kinds: BTreeMap<String, BTreeMap<String, String>>,
    bases: BTreeMap<String, String>,
}

impl Theme {
    /// A `[colour]` value, as written.
    pub(crate) fn colour(&self, name: &str) -> Option<&str> {
        self.colours.get(name).map(String::as_str)
    }

    /// A kind's checked data and derived shades.
    pub(crate) fn kind(&self, name: &str) -> Option<&BTreeMap<String, String>> {
        self.kinds.get(name)
    }
}

pub(crate) fn valid_base(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

fn managed(name: &str) -> Option<&'static str> {
    match name {
        "window" | "time_bar" | "places_bar" | "tooltip" => Some("general"),
        "place" => Some("location"),
        "social" => Some("social"),
        "expedition" => Some("expedition"),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn partial_for_test(text: &str) -> Theme {
    let document: toml::Value = toml::from_str(text).expect("test theme TOML");
    let colours = document["colour"]
        .as_table()
        .expect("test colours")
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.as_str().expect("test colour string").to_owned(),
            )
        })
        .collect();
    Theme {
        colours,
        kinds: BTreeMap::new(),
        bases: BTreeMap::new(),
    }
}

/// Whether `name` matches `[a-z][a-z0-9_]*`.
fn is_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A colour value of `place` (a table and key, as the message names it).
fn colour_value(place: &str, value: &toml::Value) -> Result<String, String> {
    let toml::Value::String(text) = value else {
        return Err(format!(
            "{place}: not a colour string (#rrggbb or #rrggbbaa)"
        ));
    };
    crate::render_api::parse_tint("theme", "colour", text)
        .map_err(|_| format!("{place}: `{text}` is not a colour (#rrggbb or #rrggbbaa)"))?;
    Ok(text.clone())
}

/// A table of the file named `place`.
fn table<'a>(
    place: &str,
    value: &'a toml::Value,
) -> Result<&'a toml::map::Map<String, toml::Value>, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{place}: not a table"))
}

/// Parses `theme.toml` and checks it.
///
/// # Errors
/// A message naming a malformed entry, a missing required base or palette colour, or a forbidden managed-kind key.
#[expect(
    clippy::too_many_lines,
    reason = "the theme is validated as one file before derived kinds are built"
)]
pub(crate) fn parse(text: &str) -> Result<Theme, String> {
    let root: toml::Value = toml::from_str(text).map_err(|error| error.message().to_owned())?;
    let root = table("the file", &root)?;
    let mut colours = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    let mut bases = BTreeMap::new();
    for (name, value) in root {
        match name.as_str() {
            "colour" => {
                for (key, value) in table("[colour]", value)? {
                    if !is_name(key) {
                        return Err(format!("[colour] {key}: not a name ([a-z][a-z0-9_]*)"));
                    }
                    colours.insert(
                        key.clone(),
                        colour_value(&format!("[colour] {key}"), value)?,
                    );
                }
            }
            "kind_colours" => {
                for (key, value) in table("[kind_colours]", value)? {
                    let place = format!("[kind_colours] {key}");
                    if !["general", "location", "social", "expedition"].contains(&key.as_str()) {
                        return Err(format!("{place}: unknown base colour"));
                    }
                    let colour = colour_value(&place, value)?;
                    if colour.len() != 7 {
                        return Err(format!("{place}: expected #rrggbb without alpha"));
                    }
                    bases.insert(key.clone(), colour.to_ascii_lowercase());
                }
            }
            "kind" => {
                for (kind, value) in table("[kind]", value)? {
                    if !is_name(kind) {
                        return Err(format!("[kind.{kind}]: not a name ([a-z][a-z0-9_]*)"));
                    }
                    let mut keys = BTreeMap::new();
                    for (key, value) in table(&format!("[kind.{kind}]"), value)? {
                        if !is_name(key) {
                            return Err(format!(
                                "[kind.{kind}] {key}: not a name ([a-z][a-z0-9_]*)"
                            ));
                        }
                        keys.insert(
                            key.clone(),
                            colour_value(&format!("[kind.{kind}] {key}"), value)?,
                        );
                    }
                    if managed(kind).is_none() {
                        for required in REQUIRED {
                            if !keys.contains_key(required) {
                                return Err(format!("[kind.{kind}]: missing `{required}`"));
                            }
                        }
                    } else {
                        for key in keys.keys() {
                            let allowed = (kind == "tooltip" && key == "title_text")
                                || (kind == "time_bar" && key == "speed_box")
                                || ((kind == "place" || kind == "expedition") && key == "divider");
                            if !allowed {
                                return Err(format!(
                                    "[kind.{kind}] {key}: managed kind key is derived or not permitted"
                                ));
                            }
                        }
                    }
                    kinds.insert(kind.clone(), keys);
                }
            }
            other => {
                return Err(format!(
                    "`{other}`: not a table of the theme (only `colour` and `kind`)"
                ));
            }
        }
    }
    for name in ["general", "location", "social", "expedition"] {
        if !bases.contains_key(name) {
            return Err(format!("[kind_colours] {name}: missing colour"));
        }
    }
    for name in ["text", "body", "number", "over_limit", "over_limit_dark"] {
        let value = colours
            .get_mut(name)
            .ok_or_else(|| format!("[colour] {name}: missing colour"))?;
        if value.len() == 9 {
            if !value[7..].eq_ignore_ascii_case("ff") {
                return Err(format!("[colour] {name}: must be opaque"));
            }
            value.truncate(7);
        }
        *value = value.to_ascii_lowercase();
    }
    for name in [
        "window",
        "time_bar",
        "places_bar",
        "tooltip",
        "place",
        "social",
        "expedition",
    ] {
        let base = &bases[managed(name).expect("managed kind")];
        let mut derived = colours::derive(base, &colours);
        if let Some(data) = kinds.remove(name) {
            derived.extend(data);
        }
        if name == "time_bar" {
            derived.insert("speed_fill".into(), derived["hot"].clone());
        }
        kinds.insert(name.to_owned(), derived);
    }
    Ok(Theme {
        colours,
        kinds,
        bases,
    })
}

/// Reads and checks `<assets>/ui/theme.toml`.
///
/// # Errors
/// A message naming the file: it cannot be read, or it is not a valid theme (see [`parse`]).
pub(crate) fn load(assets: &std::path::Path) -> Result<Theme, String> {
    let path = assets.join("ui/theme.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// `theme.colour(name)`: the `[colour]` value, the string as written.
fn colour_function(theme: Arc<Theme>) -> HostFunction {
    Arc::new(move |arguments: &[Option<ConfigValue>]| match arguments {
        [Some(ConfigValue::String(name))] => theme
            .colour(name)
            .map(|value| vec![ConfigValue::String(value.to_owned())])
            .ok_or_else(|| format!("theme.colour: no colour named `{name}`")),
        _ => Err("theme.colour: expected one string".to_owned()),
    })
}

/// `theme.kind(name)`: a new table of the kind's keys and strings as written.
fn kind_function(theme: Arc<Theme>) -> HostFunction {
    Arc::new(move |arguments: &[Option<ConfigValue>]| match arguments {
        [Some(ConfigValue::String(name))] | [Some(ConfigValue::String(name)), None] => theme
            .kind(name)
            .map(|keys| {
                vec![ConfigValue::Table(
                    keys.iter()
                        .map(|(key, value)| (key.clone(), ConfigValue::String(value.clone())))
                        .collect(),
                )]
            })
            .ok_or_else(|| format!("theme.kind: no kind named `{name}`")),
        [
            Some(ConfigValue::String(name)),
            Some(ConfigValue::String(base)),
        ] if managed(name).is_some() => {
            if !valid_base(base) {
                return Err("theme.kind: invalid base colour".into());
            }
            let mut keys = colours::derive(&base.to_ascii_lowercase(), &theme.colours);
            let data = theme
                .kind(name)
                .ok_or_else(|| format!("theme.kind: no kind named `{name}`"))?;
            for key in ["divider", "speed_box"] {
                if let Some(value) = data.get(key) {
                    keys.insert(key.into(), value.clone());
                }
            }
            if name == "tooltip" {
                keys.insert("title_text".into(), data["title_text"].clone());
            }
            if name == "time_bar" {
                keys.insert("speed_fill".into(), keys["hot"].clone());
            }
            Ok(vec![ConfigValue::Table(
                keys.into_iter()
                    .map(|(k, v)| (k, ConfigValue::String(v)))
                    .collect(),
            )])
        }
        _ => Err("theme.kind: expected one string".to_owned()),
    })
}

fn palette_function(theme: Arc<Theme>) -> HostFunction {
    Arc::new(move |_| {
        Ok(vec![ConfigValue::Table(
            theme
                .colours
                .iter()
                .filter_map(|(name, value)| {
                    if value.len() == 7 || value[7..].eq_ignore_ascii_case("ff") {
                        Some((
                            name.clone(),
                            ConfigValue::String(value[..7].to_ascii_lowercase()),
                        ))
                    } else {
                        None
                    }
                })
                .collect(),
        )])
    })
}

/// The `theme` extension of the UI VM: `theme.colour(name)` and `theme.kind(name)`.
///
/// `colour` returns the `[colour]` value, the string as written. `kind` returns a new table of the kind's keys and strings as
/// written, with no defaults applied, so a script that changes it changes nothing else. An unknown name, or an argument that is not
/// one string, is a Lua error naming it.
pub(crate) fn extension(theme: Arc<Theme>) -> Extension {
    let bases = Arc::clone(&theme);
    Extension::new("theme")
        .function("colour", colour_function(Arc::clone(&theme)))
        .function(
            "bases",
            Arc::new(move |_| {
                Ok(vec![ConfigValue::Table(
                    bases
                        .bases
                        .iter()
                        .map(|(k, v)| (k.clone(), ConfigValue::String(v.clone())))
                        .collect(),
                )])
            }),
        )
        .function("palette", palette_function(Arc::clone(&theme)))
        .function(
            "row_ink",
            Arc::new(move |args| match args {
                [
                    Some(ConfigValue::String(ink)),
                    Some(ConfigValue::String(fill)),
                    Some(ConfigValue::String(fallback)),
                ] => {
                    let opaque = ink.is_ascii()
                        && (ink.len() == 7
                            || (ink.len() == 9 && ink[7..].eq_ignore_ascii_case("ff")));
                    let keep = opaque
                        && valid_base(&ink[..7])
                        && valid_base(fill)
                        && colours::contrast(ink, fill) >= 3.0;
                    Ok(vec![ConfigValue::String(if keep {
                        ink.clone()
                    } else {
                        fallback.clone()
                    })])
                }
                _ => Err("theme.row_ink: expected three colours".into()),
            }),
        )
        .function("kind", kind_function(theme))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
        time::SystemTime,
    };

    #[test]
    fn derived_kind_colours_follow_the_specified_contrast_rules() {
        let theme = shipped();
        for (kind, outline, hot, title, pressed, number, limit) in [
            (
                "window", "#6e130f", "#a71d17", "#ece3cb", "#ece3cb", "#d4a8d7", "#f66155",
            ),
            (
                "social", "#6e130f", "#a71d17", "#ece3cb", "#ece3cb", "#d4a8d7", "#f66155",
            ),
            (
                "place", "#4fb3c8", "#b0dde6", "#15110a", "#15110a", "#663069", "#4b0b06",
            ),
            (
                "expedition",
                "#c9932e",
                "#e3c386",
                "#15110a",
                "#15110a",
                "#663069",
                "#4b0b06",
            ),
        ] {
            let actual = theme.kind(kind).unwrap();
            for (key, expected) in [
                ("outline", outline),
                ("hot", hot),
                ("title_text", title),
                ("pressed_text", pressed),
                ("highlight_number", number),
                ("highlight_over_limit", limit),
            ] {
                assert_eq!(
                    actual.get(key).map(String::as_str),
                    Some(expected),
                    "{kind}.{key}"
                );
            }
        }
    }

    use ck_resources::Image;

    use super::*;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("create fixture directory");
        for entry in fs::read_dir(source).expect("read source directory") {
            let entry = entry.expect("read directory entry");
            let target = destination.join(entry.file_name());
            if entry.file_type().expect("read entry type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).expect("copy fixture file");
            }
        }
    }

    fn goods_check(script: &Path) -> std::process::Output {
        std::process::Command::new("python3")
            .arg("-I")
            .arg(script)
            .arg("--check")
            .output()
            .expect("python3 runs the goods art check")
    }

    fn shipped_text() -> String {
        fs::read_to_string(assets().join("ui/theme.toml")).expect("the shipped theme.toml")
    }

    fn shipped() -> Theme {
        parse(&shipped_text()).expect("the shipped theme is valid")
    }

    #[test]
    fn palette_offers_new_opaque_colours_and_excludes_translucent_entries() {
        let data = shipped_text().replace(
            "[colour]",
            "[colour]\nnew_opaque = \"#aabbccff\"\nnew_translucent = \"#11223380\"",
        );
        let theme = Arc::new(parse(&data).unwrap());
        let result = palette_function(theme)(&[]).unwrap();
        let ConfigValue::Table(entries) = &result[0] else {
            panic!("palette table")
        };
        assert_eq!(
            entries.get("new_opaque"),
            Some(&ConfigValue::String("#aabbcc".into()))
        );
        assert!(!entries.contains_key("new_translucent"));
    }

    #[test]
    fn derived_grey_uses_separate_contrast_choices_and_caps_hot() {
        let theme = shipped();
        let grey = colours::derive("#666666", &theme.colours);
        assert_eq!(grey["hot"], "#969696");
        assert_eq!(grey["title_text"], "#ece3cb");
        assert_eq!(grey["pressed_text"], "#15110a");
        let pale = colours::derive("#e6e6e6", &theme.colours);
        assert_eq!(pale["hot"], "#ebebeb");
    }

    #[test]
    fn managed_data_rejects_invalid_bases_alpha_and_derived_keys() {
        let text = shipped_text();
        for (old, new, expected) in [
            (
                "general = \"#6e130f\"",
                "general = \"#6e130fff\"",
                "[kind_colours] general",
            ),
            (
                "general = \"#6e130f\"",
                "general = \"bad\"",
                "[kind_colours] general",
            ),
            ("general = \"#6e130f\"", "", "[kind_colours] general"),
            (
                "text = \"#ece3cb\"",
                "text = \"#ece3cb00\"",
                "[colour] text",
            ),
            (
                "[kind.window]",
                "[kind.window]\nhot = \"#000000\"",
                "[kind.window] hot",
            ),
        ] {
            assert!(
                parse(&text.replacen(old, new, 1))
                    .unwrap_err()
                    .contains(expected),
                "{expected}"
            );
        }
        let opaque = text.replacen("text = \"#ece3cb\"", "text = \"#ece3cbff\"", 1);
        assert_eq!(parse(&opaque).unwrap().colour("text"), Some("#ece3cb"));
    }

    #[test]
    fn the_shipped_theme_loads_with_the_owners_colours() {
        let theme = shipped();
        assert_eq!(theme.colour("body"), Some("#15110a"));
        assert_eq!(theme.colour("text"), Some("#ece3cb"));
        assert_eq!(theme.colour("lock"), Some("#f0a830"));
        assert_eq!(theme.colour("term"), Some("#92ddea"));
        assert_eq!(theme.colour("number"), Some("#5fb35a"));
        assert_eq!(theme.colour("over_limit"), Some("#f66155"));
        assert_eq!(theme.colour("good"), None);
        let place = theme.kind("place").expect("the place kind");
        assert_eq!(place.get("divider").map(String::as_str), Some("#c9932e"));
        assert_eq!(place.get("outline").map(String::as_str), Some("#4fb3c8"));
        assert_eq!(place.get("hot").map(String::as_str), Some("#b0dde6"));
        assert_eq!(place.get("title_text").map(String::as_str), Some("#15110a"));
        assert_eq!(
            place.get("pressed_text").map(String::as_str),
            Some("#15110a")
        );
        assert_eq!(
            place.get("highlight_number").map(String::as_str),
            Some("#663069")
        );
        assert_eq!(
            place.get("highlight_over_limit").map(String::as_str),
            Some("#4b0b06")
        );
        assert_eq!(place.len(), 7);
        let window = theme.kind("window").expect("the window kind");
        assert_eq!(window.get("outline").map(String::as_str), Some("#6e130f"));
        assert_eq!(window.get("hot").map(String::as_str), Some("#a71d17"));
        assert_eq!(
            window.get("highlight_number").map(String::as_str),
            Some("#d4a8d7")
        );
        for name in ["time_bar", "places_bar"] {
            assert_eq!(
                theme
                    .kind(name)
                    .and_then(|kind| kind.get("highlight_number"))
                    .map(String::as_str),
                Some("#d4a8d7"),
                "{name} highlight number"
            );
        }
        let tooltip = theme.kind("tooltip").expect("the tooltip kind");
        assert_eq!(tooltip.get("outline").map(String::as_str), Some("#6e130f"));
        assert_eq!(
            tooltip.get("title_text").map(String::as_str),
            Some("#c9932e")
        );
        assert!(theme.kind("time_bar").is_some());
    }

    #[test]
    fn a_bad_theme_is_refused_naming_the_table_and_key() {
        let refused = |text: &str| parse(text).expect_err("refused");
        assert_eq!(
            refused("[colour]\nbody = \"red\"\n"),
            "[colour] body: `red` is not a colour (#rrggbb or #rrggbbaa)"
        );
        assert_eq!(
            refused("[colour]\nbody = 5\n"),
            "[colour] body: not a colour string (#rrggbb or #rrggbbaa)"
        );
        assert_eq!(
            refused("[colour]\nBody = \"#000000\"\n"),
            "[colour] Body: not a name ([a-z][a-z0-9_]*)"
        );
        assert_eq!(
            refused("[kind.Big]\noutline = \"#000000\"\nhot = \"#000000\"\n"),
            "[kind.Big]: not a name ([a-z][a-z0-9_]*)"
        );
        assert_eq!(
            refused("[kind.a]\nhot = \"#000000\"\n"),
            "[kind.a]: missing `outline`"
        );
        assert_eq!(
            refused("[kind.a]\noutline = \"#000000\"\n"),
            "[kind.a]: missing `hot`"
        );
        assert_eq!(
            refused("[kind.a]\noutline = \"#000000\"\nhot = \"#00000\"\n"),
            "[kind.a] hot: `#00000` is not a colour (#rrggbb or #rrggbbaa)"
        );
        assert_eq!(
            refused("[kind.a]\noutline = \"#000000\"\nhot = \"#000000\"\nBad = \"#000000\"\n"),
            "[kind.a] Bad: not a name ([a-z][a-z0-9_]*)"
        );
        assert_eq!(
            refused("[extra]\nx = 1\n"),
            "`extra`: not a table of the theme (only `colour` and `kind`)"
        );
        assert_eq!(
            refused("[colour]\nbody = 5\n\n[kind]\nx = 1\n"),
            refused("[colour]\nbody = 5\n")
        );
        assert!(refused("not toml at all =").len() > 3);
        // Alpha is accepted, and so are further kinds and keys.
        let fine =
            "[kind.mod_kind]\noutline = \"#000000\"\nhot = \"#ffffffff\"\nnew_key = \"#010203\"\n";
        let custom = parse(&format!("{}\n{}", shipped_text(), fine)).unwrap();
        let kind = custom.kind("mod_kind").unwrap();
        assert_eq!(kind["outline"], "#000000");
        assert_eq!(kind["hot"], "#ffffffff");
        assert_eq!(kind["new_key"], "#010203");
    }

    fn call(function: &str, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let theme = Arc::new(shipped());
        let host = match function {
            "colour" => colour_function(theme),
            _ => kind_function(theme),
        };
        host(arguments)
    }

    #[test]
    fn the_extension_returns_the_strings_as_written_and_a_fresh_table_each_time() {
        let text = |value: &str| vec![Some(ConfigValue::String(value.to_owned()))];
        assert_eq!(
            call("colour", &text("body")).unwrap(),
            vec![ConfigValue::String("#15110a".into())]
        );
        assert!(call("colour", &text("good")).is_err());
        let first = call("kind", &text("place")).unwrap();
        let ConfigValue::Table(keys) = &first[0] else {
            panic!("a table")
        };
        assert_eq!(
            keys.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "divider",
                "highlight_number",
                "highlight_over_limit",
                "hot",
                "outline",
                "pressed_text",
                "title_text"
            ]
        );
        assert_eq!(keys["divider"], ConfigValue::String("#c9932e".into()));
        assert_eq!(
            keys["highlight_number"],
            ConfigValue::String("#663069".into())
        );
        // Managed keys are derived for the window.
        let ConfigValue::Table(window) = &call("kind", &text("window")).unwrap()[0] else {
            panic!("a table")
        };
        assert_eq!(window.len(), 6);
        assert_eq!(
            window["highlight_number"],
            ConfigValue::String("#d4a8d7".into())
        );
        // Each call builds the result anew.
        assert_eq!(call("kind", &text("place")).unwrap(), first);
        for (function, bad) in [
            ("colour", text("nope")),
            ("kind", text("nope")),
            ("colour", vec![]),
            ("colour", vec![Some(ConfigValue::Integer(1))]),
            ("kind", vec![None]),
            (
                "kind",
                vec![
                    Some(ConfigValue::String("a".into())),
                    Some(ConfigValue::String("b".into())),
                ],
            ),
        ] {
            assert!(call(function, &bad).is_err(), "{function}");
        }
        assert_eq!(
            call("colour", &text("nope")).unwrap_err(),
            "theme.colour: no colour named `nope`"
        );
        assert_eq!(
            call("kind", &text("nope")).unwrap_err(),
            "theme.kind: no kind named `nope`"
        );
    }

    /// The pixels of a tile of `cut` by the rule of the spec (§4.2): `part` is the fill or the line of the corner.
    fn expected_tile(cut: usize, corner: &str, part: &str) -> Vec<u8> {
        let mut pixels = Vec::new();
        for j in 0..=cut {
            for i in 0..=cut {
                let line = i + j == cut;
                let fill = if corner == "tl" {
                    i + j > cut
                } else {
                    i + j < cut
                };
                let set = if part == "line" { line } else { fill };
                pixels.extend(if set {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 0]
                });
            }
        }
        pixels
    }

    /// Pixel rows of an icon: full 11-row shapes are drawn directly; seven-row icons start at row 2 and column 1.
    fn expected_icon(width: usize, shape: &[&str]) -> Vec<u8> {
        let mut pixels = vec![0; width * 11 * 4];
        let full_shape = shape.len() == 11;
        for (row, pattern) in shape.iter().enumerate() {
            for (column, byte) in pattern.bytes().enumerate() {
                if byte == b'#' {
                    let y = if full_shape { row } else { row + 2 };
                    let x = if full_shape { column } else { column + 1 };
                    let offset = (y * width + x) * 4;
                    pixels[offset..offset + 4].copy_from_slice(&[255; 4]);
                }
            }
        }
        pixels
    }

    #[test]
    fn the_art_scripts_report_their_images_up_to_date() {
        // `make_theme.py --check` and `make_font.py --check` fail when a committed image differs from what the script writes, or is
        // missing or extra.
        for script in [
            "ui/theme/make_theme.py",
            "ui/font/make_font.py",
            "ui/goods/make_goods.py",
            "ui/characters/make_characters.py",
        ] {
            let output = std::process::Command::new("python3")
                .arg("-I")
                .arg(assets().join(script))
                .arg("--check")
                .output()
                .expect("python3 runs the art script");
            assert!(
                output.status.success(),
                "{script} --check: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn goods_icons_match_the_generator_and_the_fixed_ui_list() {
        let generator = assets().join("ui/goods/make_goods.py");
        let output = std::process::Command::new("python3")
            .arg("-I")
            .arg("-c")
            .arg("import runpy,sys; print('\\n'.join(sorted(runpy.run_path(sys.argv[1])['ART'])))")
            .arg(&generator)
            .output()
            .expect("python3 reads the goods generator ids");
        assert!(
            output.status.success(),
            "reading goods ids: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let art_ids: BTreeSet<String> = String::from_utf8(output.stdout)
            .expect("generator ids are UTF-8")
            .lines()
            .map(str::to_owned)
            .collect();
        let economy_text = fs::read_to_string(assets().join("game/economy.toml"))
            .expect("the shipped economy data");
        let economy: toml::Value = toml::from_str(&economy_text).expect("valid economy data");
        let good_ids: BTreeSet<String> = economy["goods"]
            .as_array()
            .expect("goods array")
            .iter()
            .map(|good| good["id"].as_str().expect("good id").to_owned())
            .collect();
        assert_eq!(
            art_ids, good_ids,
            "frozen art covers each shipped good once"
        );

        let scripts = assets()
            .parent()
            .expect("the project root")
            .join("scripts/ui/lib/location.lua");
        let location = fs::read_to_string(scripts).expect("the location UI script");
        let fixed_list = location
            .split("local GOODS_WITH_ICONS = {")
            .nth(1)
            .expect("the fixed goods icon list")
            .split('}')
            .next()
            .expect("the end of the fixed goods icon list");
        let listed_ids: BTreeSet<String> = fixed_list
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        assert_eq!(
            listed_ids, art_ids,
            "the UI fallback list matches the generator ids"
        );

        let oats = Image::decode(
            fs::File::open(assets().join("ui/goods/oats.png")).expect("the oats icon"),
        )
        .expect("valid oats PNG");
        assert_eq!((oats.width(), oats.height()), (9, 9));
        assert_eq!(&oats.pixels()[2 * 4..3 * 4], &[0xee, 0xd8, 0x8a, 0xff]);
        assert_eq!(&oats.pixels()[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn unknown_good_art_check_requires_the_exact_single_fallback_image() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("codekarl-goods-art-{}-{stamp}", std::process::id()));
        let goods = root.join("assets/ui/goods");
        copy_tree(&assets().join("ui/goods"), &goods);
        fs::create_dir_all(root.join("assets/ui/font")).expect("create font sibling");
        fs::copy(
            assets().join("ui/font/make_font.py"),
            root.join("assets/ui/font/make_font.py"),
        )
        .expect("copy the generator's font dependency");
        let script = goods.join("make_goods.py");
        let fallback = goods.join("fallback/unknown.png");
        assert!(
            goods_check(&script).status.success(),
            "the copied baseline checks"
        );

        fs::remove_file(&fallback).expect("remove the unknown art");
        assert!(
            !goods_check(&script).status.success(),
            "a missing fallback is rejected"
        );
        let generated = std::process::Command::new("python3")
            .arg("-I")
            .arg(&script)
            .status()
            .expect("python3 regenerates the fixture art");
        assert!(generated.success(), "regenerate the exact fallback art");

        let mut bytes = fs::read(&fallback).expect("the generated fallback");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&fallback, bytes).expect("corrupt the fallback");
        assert!(
            !goods_check(&script).status.success(),
            "changed fallback pixels are rejected"
        );
        let generated = std::process::Command::new("python3")
            .arg("-I")
            .arg(&script)
            .status()
            .expect("python3 restores the fixture art");
        assert!(generated.success(), "restore the exact fallback art");

        fs::write(goods.join("fallback/extra.png"), b"extra").expect("add unlisted fallback art");
        assert!(
            !goods_check(&script).status.success(),
            "an extra fallback PNG is rejected"
        );
        fs::remove_dir_all(root).expect("remove temporary art fixture");
    }

    /// The application's icons: name, width in pixels and the seven shape rows after the two blank ones.
    const ICONS: [(&str, usize, &[&str]); 26] = [
        (
            "graph_f1",
            9,
            &[
                ".........",
                ".........",
                "...##....",
                "...##....",
                "##.##....",
                "##.##.##.",
                "##.##.##.",
                ".........",
                "##.##.##.",
                "##.##.##.",
                ".........",
            ],
        ),
        (
            "graph_f2",
            9,
            &[
                ".........",
                ".........",
                "........#",
                ".#.....#.",
                "#.#...#..",
                "...#.#...",
                "....#....",
                ".........",
                "##...###.",
                "..###...#",
                ".........",
            ],
        ),
        (
            "graph_f3",
            9,
            &[
                ".........",
                ".........",
                ".........",
                "......###",
                "...######",
                "#########",
                "#########",
                ".........",
                "#########",
                "#########",
                ".........",
            ],
        ),
        (
            "graph_export",
            9,
            &[
                ".........",
                "....#....",
                "....#....",
                "....#....",
                "..#.#.#..",
                "...###...",
                "....#....",
                "#.......#",
                "#.......#",
                "#########",
                ".........",
            ],
        ),
        (
            "graph",
            9,
            &[
                ".........",
                "#........",
                "#.......#",
                "#......#.",
                "#.....#..",
                "#..#.#...",
                "#.#.#....",
                "##.......",
                "#........",
                "#########",
                ".........",
            ],
        ),
        (
            "event_log",
            9,
            &[
                "#######.", "#.....#.", "#.###.#.", "#.....#.", "#.###.#.", "#.....#.", "#######.",
            ],
        ),
        (
            "rewind",
            9,
            &[
                "...#...#", "..##..##", ".###.###", "########", ".###.###", "..##..##", "...#...#",
            ],
        ),
        (
            "fast_forward",
            9,
            &[
                "#...#...", "##..##..", "###.###.", "########", "###.###.", "##..##..", "#...#...",
            ],
        ),
        (
            "pause",
            7,
            &[
                "##.##.", "##.##.", "##.##.", "##.##.", "##.##.", "##.##.", "##.##.",
            ],
        ),
        (
            "play",
            7,
            &[
                "#.....", "##....", "###...", "####..", "###...", "##....", "#.....",
            ],
        ),
        (
            "stockpile",
            9,
            &[
                "..####..", ".######.", "########", "########", "########", ".######.", "..####..",
            ],
        ),
        (
            "yield",
            9,
            &[
                "......##", "......##", "...##.##", "...##.##", "##.##.##", "##.##.##", "##.##.##",
            ],
        ),
        (
            "elevation",
            9,
            &[
                "...##...", "..####..", "..####..", ".######.", ".######.", "########", "########",
            ],
        ),
        // Original map layers icon, from bars and an upward chevron.
        (
            "layers",
            9,
            &[
                "...#....", "..#.#...", ".#...#..", "........", "#######.", "........", "#######.",
            ],
        ),
        // Original sprout icon, drawn from plain geometric shapes for CK-382.
        (
            "fertility",
            9,
            &[
                "#......#", "##....##", ".##..##.", "..####..", "...##...", "...##...", "...##...",
            ],
        ),
        // Original river icon: three wavy strokes.
        (
            "river",
            9,
            &[
                ".........",
                ".##...##.",
                "...###...",
                ".........",
                ".##...##.",
                "...###...",
                ".........",
                ".##...##.",
                "...###...",
                ".........",
                ".........",
            ],
        ),
        (
            "location",
            9,
            &[
                "....#....",
                "...###...",
                "..#####..",
                "..##.##..",
                "..#####..",
                "..#####..",
                "..##.##..",
                "..#####..",
                "..#####..",
                "..##.##..",
                ".#######.",
            ],
        ),
        (
            "pops",
            9,
            &[
                "...###...",
                "...###...",
                "...###...",
                "....#....",
                ".#######.",
                "#.#####.#",
                "#.#####.#",
                "..#####..",
                "..##.##..",
                "..##.##..",
                "..##.##..",
            ],
        ),
        (
            "characters",
            9,
            &[
                ".........",
                "...###...",
                "..#####..",
                "..#####..",
                "..#####..",
                "...###...",
                ".........",
                "..#####..",
                ".#######.",
                "#########",
                "#########",
            ],
        ),
        (
            "cards",
            9,
            &[
                ".........",
                ".........",
                ".#######.",
                ".#.....#.",
                ".#######.",
                ".........",
                ".#######.",
                ".#.....#.",
                ".#######.",
                ".........",
                ".........",
            ],
        ),
        (
            "rows",
            9,
            &[
                ".........",
                ".........",
                "##.######",
                ".........",
                ".........",
                "##.######",
                ".........",
                ".........",
                "##.######",
                ".........",
                ".........",
            ],
        ),
        (
            "farm",
            9,
            &[
                ".........",
                "....#....",
                "...###...",
                "..#####..",
                ".#######.",
                "#########",
                ".#.....#.",
                ".#.###.#.",
                ".#.#.#.#.",
                ".#.#.#.#.",
                ".#######.",
            ],
        ),
        (
            "holdings",
            9,
            &[
                "....#....",
                "...###...",
                "....#....",
                "...###...",
                "..#####..",
                ".#######.",
                ".#.....#.",
                ".#..#..#.",
                ".#.###.#.",
                ".#.###.#.",
                ".#######.",
            ],
        ),
        (
            "storehouse",
            9,
            &[
                ".........",
                ".........",
                ".#######.",
                "#########",
                "#.......#",
                "#.#####.#",
                "#.......#",
                "#.#####.#",
                "#.......#",
                "#########",
                ".........",
            ],
        ),
        // CK-295, rule 1: the list bar's two buttons, the rows exactly as the spec writes them.
        (
            "filter",
            9,
            &[
                ".........",
                "#########",
                ".#######.",
                "..#####..",
                "...###...",
                "...###...",
                "...###...",
                "...###...",
                "...##....",
                ".........",
                ".........",
            ],
        ),
        (
            "sort",
            9,
            &[
                ".........",
                ".#..#####",
                ".#.......",
                ".#..####.",
                ".#.......",
                ".#..###..",
                ".#.......",
                "###.##...",
                ".#.......",
                ".........",
                ".........",
            ],
        ),
    ];

    /// CK-295, rule 2: the sort picker's direction marks, 5 by 5, white on clear.
    const MARKS: [(&str, [&str; 5]); 2] = [
        ("sort_down", ["..#..", "..#..", "#.#.#", ".###.", "..#.."]),
        ("sort_up", ["..#..", ".###.", "#.#.#", "..#..", "..#.."]),
    ];

    #[test]
    fn each_tile_png_equals_its_rule_pixel_for_pixel_and_no_other_png_is_there() {
        let directory = assets().join("ui/theme");
        let mut names = BTreeSet::new();
        let white = Image::decode(fs::File::open(directory.join("white.png")).unwrap()).unwrap();
        assert_eq!((white.width(), white.height()), (1, 1));
        assert_eq!(white.pixels(), [255, 255, 255, 255]);
        names.insert("white.png".to_owned());
        let speed_box =
            Image::decode(fs::File::open(directory.join("speed_box.png")).unwrap()).unwrap();
        assert_eq!((speed_box.width(), speed_box.height()), (6, 11));
        let mut expected_box = vec![0; 6 * 11 * 4];
        for y in 0..11 {
            for x in 0..6 {
                if (y == 0 || y == 10) && (1..=4).contains(&x)
                    || (x == 0 || x == 5) && (1..=9).contains(&y)
                {
                    let offset = (y * 6 + x) * 4;
                    expected_box[offset..offset + 4].copy_from_slice(&[255; 4]);
                }
            }
        }
        assert_eq!(speed_box.pixels(), expected_box);
        names.insert("speed_box.png".to_owned());
        let icons = ICONS;
        for (name, width, shape) in icons {
            let file = format!("icon_{name}.png");
            let image = Image::decode(fs::File::open(directory.join(&file)).unwrap()).unwrap();
            assert_eq!(
                (image.width(), image.height()),
                (u32::try_from(width).expect("icon widths fit in u32"), 11),
                "{file}"
            );
            assert_eq!(image.pixels(), expected_icon(width, shape), "{file}");
            names.insert(file);
        }
        for (name, shape) in MARKS {
            let file = format!("{name}.png");
            let image = Image::decode(fs::File::open(directory.join(&file)).unwrap()).unwrap();
            assert_eq!((image.width(), image.height()), (5, 5), "{file}");
            let expected: Vec<u8> = shape
                .iter()
                .flat_map(|row| row.bytes())
                .flat_map(|byte| if byte == b'#' { [255; 4] } else { [0; 4] })
                .collect();
            assert_eq!(image.pixels(), expected, "{file}");
            names.insert(file);
        }
        for cut in [3_usize, 6, 7] {
            for corner in ["tl", "br"] {
                for part in ["fill", "line"] {
                    let name = format!("cut{cut}_{corner}_{part}.png");
                    let image =
                        Image::decode(fs::File::open(directory.join(&name)).unwrap()).unwrap();
                    assert_eq!(
                        (image.width() as usize, image.height() as usize),
                        (cut + 1, cut + 1),
                        "{name}"
                    );
                    assert_eq!(image.pixels(), expected_tile(cut, corner, part), "{name}");
                    names.insert(name);
                }
            }
        }
        let on_disk: BTreeSet<String> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            })
            .collect();
        assert_eq!(on_disk, names);
    }
}
