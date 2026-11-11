//! The theme: `assets/ui/theme.toml` parsed and checked, and the `theme` extension of the UI VM.
//!
//! The file has two tables, `[colour]` (named colours) and `[kind.<name>]` (the colours of one kind of panel). Every value is a
//! colour string in the tint grammar, `#rrggbb` or `#rrggbbaa`; every name matches `[a-z][a-z0-9_]*`; every kind has an `outline` and
//! a `hot`. Other tables and keys are allowed under `[kind.*]` so that further kinds and mods need no change here. The extension
//! returns the strings as written; the UI library applies the defaults.

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
}

impl Theme {
    /// A `[colour]` value, as written.
    pub(crate) fn colour(&self, name: &str) -> Option<&str> {
        self.colours.get(name).map(String::as_str)
    }

    /// The keys of a kind, as written.
    pub(crate) fn kind(&self, name: &str) -> Option<&BTreeMap<String, String>> {
        self.kinds.get(name)
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
/// A message naming the table and key: not valid TOML, a top-level table other than `colour` and `kind`, a value that is not a colour
/// string, a name outside `[a-z][a-z0-9_]*`, or a kind without `outline` or `hot`.
pub(crate) fn parse(text: &str) -> Result<Theme, String> {
    let root: toml::Value = toml::from_str(text).map_err(|error| error.message().to_owned())?;
    let root = table("the file", &root)?;
    let mut colours = BTreeMap::new();
    let mut kinds = BTreeMap::new();
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
                    for required in REQUIRED {
                        if !keys.contains_key(required) {
                            return Err(format!("[kind.{kind}]: missing `{required}`"));
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
    Ok(Theme { colours, kinds })
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
        [Some(ConfigValue::String(name))] => theme
            .kind(name)
            .map(|keys| {
                vec![ConfigValue::Table(
                    keys.iter()
                        .map(|(key, value)| (key.clone(), ConfigValue::String(value.clone())))
                        .collect(),
                )]
            })
            .ok_or_else(|| format!("theme.kind: no kind named `{name}`")),
        _ => Err("theme.kind: expected one string".to_owned()),
    })
}

/// The `theme` extension of the UI VM: `theme.colour(name)` and `theme.kind(name)`.
///
/// `colour` returns the `[colour]` value, the string as written. `kind` returns a new table of the kind's keys and strings as
/// written, with no defaults applied, so a script that changes it changes nothing else. An unknown name, or an argument that is not
/// one string, is a Lua error naming it.
pub(crate) fn extension(theme: Arc<Theme>) -> Extension {
    Extension::new("theme")
        .function("colour", colour_function(Arc::clone(&theme)))
        .function("kind", kind_function(theme))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use ck_resources::Image;

    use super::*;

    fn assets() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn shipped_text() -> String {
        fs::read_to_string(assets().join("ui/theme.toml")).expect("the shipped theme.toml")
    }

    fn shipped() -> Theme {
        parse(&shipped_text()).expect("the shipped theme is valid")
    }

    #[test]
    fn the_shipped_theme_loads_with_the_owners_colours() {
        let theme = shipped();
        assert_eq!(theme.colour("body"), Some("#15110a"));
        assert_eq!(theme.colour("text"), Some("#ece3cb"));
        let place = theme.kind("place").expect("the place kind");
        assert_eq!(place.get("divider").map(String::as_str), Some("#c9932e"));
        assert_eq!(place.get("outline").map(String::as_str), Some("#4fb3c8"));
        assert_eq!(place.get("hot").map(String::as_str), Some("#92ddea"));
        assert_eq!(place.get("title_text").map(String::as_str), Some("#15110a"));
        assert_eq!(
            place.get("pressed_text").map(String::as_str),
            Some("#15110a")
        );
        assert_eq!(place.len(), 5);
        let window = theme.kind("window").expect("the window kind");
        assert_eq!(window.get("outline").map(String::as_str), Some("#6e130f"));
        assert_eq!(window.get("hot").map(String::as_str), Some("#a8261c"));
        let tooltip = theme.kind("tooltip").expect("the tooltip kind");
        assert_eq!(tooltip.get("outline").map(String::as_str), Some("#ece3cb"));
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
        let fine = "[colour]\nx = \"#12345678\"\n[kind.mod_kind]\noutline = \"#000000\"\nhot = \"#ffffffff\"\nnew_key = \"#010203\"\n";
        assert!(parse(fine).is_ok());
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
        let first = call("kind", &text("place")).unwrap();
        let ConfigValue::Table(keys) = &first[0] else {
            panic!("a table")
        };
        assert_eq!(
            keys.keys().map(String::as_str).collect::<Vec<_>>(),
            ["divider", "hot", "outline", "pressed_text", "title_text"]
        );
        assert_eq!(keys["divider"], ConfigValue::String("#c9932e".into()));
        // No defaults are applied: `window` has just its two keys.
        let ConfigValue::Table(window) = &call("kind", &text("window")).unwrap()[0] else {
            panic!("a table")
        };
        assert_eq!(window.len(), 2);
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
        for script in ["ui/theme/make_theme.py", "ui/font/make_font.py"] {
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

    /// The application's icons: name, width in pixels and the seven shape rows after the two blank ones.
    const ICONS: [(&str, usize, &[&str]); 9] = [
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
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            })
            .collect();
        assert_eq!(on_disk, names);
    }
}
