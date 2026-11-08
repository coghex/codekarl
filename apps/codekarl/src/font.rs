//! The pixel font's data: `assets/ui/font/font.toml` parsed and checked.
//!
//! Each glyph is a table keyed by its character, holding `rows` (strings of `#` and `.`) and an `advance` in font pixels, plus one
//! `[replacement]` glyph. Every glyph is [`GLYPH_HEIGHT`] rows: the 5 by 7 glyph box with two rows above it for the fada accents
//! and two below for descenders, so the baseline is at row 9 for all of them. The checks here are the ones the application makes
//! when it loads the font and the ones the data tests make of the shipped file.

use std::collections::BTreeMap;
use std::sync::Arc;

use ck_lua::{ConfigValue, Extension};
use serde::Deserialize;

/// Rows in every glyph: two above the cap line, seven from the cap line to the baseline, two below.
pub(crate) const GLYPH_HEIGHT: usize = 11;

/// The widest a glyph may be, in font pixels.
pub(crate) const GLYPH_WIDTH_MAX: usize = 5;

/// The characters beyond printable ASCII the font draws: the fada vowels and the middle dot.
const EXTRA: [char; 11] = ['á', 'é', 'í', 'ó', 'ú', 'Á', 'É', 'Í', 'Ó', 'Ú', '·'];

/// Every character the font draws a glyph for: printable ASCII (U+0020 to U+007E), the fada vowels and the middle dot.
pub(crate) fn font_set() -> Vec<char> {
    (0x20_u8..=0x7E).map(char::from).chain(EXTRA).collect()
}

/// The texture name of `character`'s glyph, relative to the assets directory, as `texture.load` takes it.
pub(crate) fn glyph_texture(character: char) -> String {
    format!("ui/font/glyphs/U+{:04X}.png", u32::from(character))
}

/// The texture name of the replacement glyph.
pub(crate) const REPLACEMENT_TEXTURE: &str = "ui/font/glyphs/replacement.png";

/// One glyph.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Glyph {
    /// The rows, top first: `#` is a set pixel and `.` a clear one.
    pub(crate) rows: Vec<String>,
    /// How far the pen moves after the glyph, in font pixels (at least 1).
    pub(crate) advance: u32,
}

impl Glyph {
    /// The glyph's width: the width of every row.
    pub(crate) fn width(&self) -> usize {
        self.rows.first().map_or(0, String::len)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    replacement: Glyph,
    glyph: BTreeMap<String, Glyph>,
}

/// The font: a glyph for each character of [`font_set`] and the replacement for any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Font {
    glyphs: BTreeMap<char, Glyph>,
    replacement: Glyph,
}

impl Font {
    /// The glyph of `character`, or the replacement when the font has none.
    pub(crate) fn glyph(&self, character: char) -> &Glyph {
        self.glyphs.get(&character).unwrap_or(&self.replacement)
    }

    /// The replacement glyph.
    pub(crate) const fn replacement(&self) -> &Glyph {
        &self.replacement
    }

    /// Every character with its own glyph, in code-point order.
    pub(crate) fn characters(&self) -> impl Iterator<Item = char> + '_ {
        self.glyphs.keys().copied()
    }
}

/// Reads and checks `<assets>/ui/font/font.toml`.
///
/// # Errors
/// A message naming the file: it cannot be read, or it is not a valid font (see [`parse`]).
pub(crate) fn load(assets: &std::path::Path) -> Result<Font, String> {
    let path = assets.join("ui/font/font.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// The `font` extension of the UI VM: `font.layout(string)`.
///
/// It decodes the string and returns `{width = W, height = 11, glyphs = {...}}` in font pixels. `W` is the sum of the advances and
/// `height` the glyph box's height. Each non-space character contributes one entry, in order: `{texture = <name>, x = <the running
/// advance before it>, width = <the glyph's width>}`; a character without a glyph uses the replacement. It never fails for a
/// string, and an argument that is not a string is a Lua error.
pub(crate) fn extension(font: Arc<Font>) -> Extension {
    Extension::new("font").function(
        "layout",
        Arc::new(move |arguments: &[Option<ConfigValue>]| match arguments {
            [Some(ConfigValue::String(text))] => Ok(vec![layout(&font, text)]),
            _ => Err("font.layout: expected one string".to_owned()),
        }),
    )
}

fn integer(value: usize) -> ConfigValue {
    ConfigValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

/// The layout of `text`: the table `font.layout` returns.
pub(crate) fn layout(font: &Font, text: &str) -> ConfigValue {
    let mut pen = 0_usize;
    let mut glyphs = Vec::new();
    for character in text.chars() {
        let known = font.glyphs.contains_key(&character);
        let glyph = font.glyph(character);
        if character != ' ' {
            let texture = if known {
                glyph_texture(character)
            } else {
                REPLACEMENT_TEXTURE.to_owned()
            };
            glyphs.push(ConfigValue::Table(BTreeMap::from([
                ("texture".to_owned(), ConfigValue::String(texture)),
                ("x".to_owned(), integer(pen)),
                ("width".to_owned(), integer(glyph.width())),
            ])));
        }
        pen += usize::try_from(glyph.advance).unwrap_or(usize::MAX);
    }
    ConfigValue::Table(BTreeMap::from([
        ("width".to_owned(), integer(pen)),
        ("height".to_owned(), integer(GLYPH_HEIGHT)),
        ("glyphs".to_owned(), ConfigValue::Array(glyphs)),
    ]))
}

fn check_glyph(name: &str, glyph: &Glyph) -> Result<(), String> {
    if glyph.rows.len() != GLYPH_HEIGHT {
        return Err(format!(
            "glyph {name}: {} rows, not {GLYPH_HEIGHT}",
            glyph.rows.len()
        ));
    }
    let width = glyph.width();
    if width == 0 || width > GLYPH_WIDTH_MAX {
        return Err(format!(
            "glyph {name}: width {width} is outside 1 to {GLYPH_WIDTH_MAX}"
        ));
    }
    for (index, row) in glyph.rows.iter().enumerate() {
        if row.len() != width {
            return Err(format!(
                "glyph {name}: row {index} is {} wide, not {width}",
                row.len()
            ));
        }
        if let Some(bad) = row.chars().find(|cell| !matches!(cell, '#' | '.')) {
            return Err(format!(
                "glyph {name}: row {index} holds {bad:?}, not '#' or '.'"
            ));
        }
    }
    if glyph.advance == 0 {
        return Err(format!("glyph {name}: advance 0, not at least 1"));
    }
    Ok(())
}

/// Parses `font.toml` and checks it.
///
/// # Errors
/// A message naming the problem: not valid TOML of the expected shape, a glyph that is not one character, a character outside
/// [`font_set`], a missing glyph, a glyph that is not 11 rows of equal width of at most 5 cells of `#` and `.`, an advance below
/// 1, or digits whose advances differ.
pub(crate) fn parse(text: &str) -> Result<Font, String> {
    let file: File =
        toml::from_str(text).map_err(|error| format!("font.toml: {}", error.message()))?;
    let set = font_set();
    let mut glyphs = BTreeMap::new();
    for (key, glyph) in file.glyph {
        let mut characters = key.chars();
        let (Some(character), None) = (characters.next(), characters.next()) else {
            return Err(format!("glyph {key:?} is not one character"));
        };
        if !set.contains(&character) {
            return Err(format!("glyph {key:?} is outside the font's set"));
        }
        check_glyph(&key, &glyph)?;
        glyphs.insert(character, glyph);
    }
    if let Some(missing) = set.iter().find(|character| !glyphs.contains_key(character)) {
        return Err(format!("glyph {missing:?} is missing"));
    }
    check_glyph("[replacement]", &file.replacement)?;
    let digit_advance = glyphs[&'0'].advance;
    if let Some(digit) = ('1'..='9').find(|digit| glyphs[digit].advance != digit_advance) {
        return Err(format!(
            "digit {digit} advances {}, not {digit_advance} like 0: digits must be tabular",
            glyphs[&digit].advance
        ));
    }
    Ok(Font {
        glyphs,
        replacement: file.replacement,
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::PathBuf};

    use ck_resources::Image;

    use super::*;

    fn directory() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/font")
    }

    fn shipped_text() -> String {
        fs::read_to_string(directory().join("font.toml")).expect("the shipped font.toml")
    }

    fn shipped() -> Font {
        parse(&shipped_text()).expect("the shipped font is valid")
    }

    /// The text with the block that starts at the line `header` removed (up to the next blank line).
    fn without(text: &str, header: &str) -> String {
        let start = text.find(&format!("\n{header}\n")).expect("the block") + 1;
        let end = text[start..]
            .find("\n\n")
            .map_or(text.len(), |end| start + end + 2);
        format!("{}{}", &text[..start], &text[end..])
    }

    #[test]
    fn every_character_of_the_set_has_a_glyph_of_the_right_shape_and_nothing_else_has() {
        let font = shipped();
        let set = font_set();
        assert_eq!(set.len(), 95 + 10 + 1);
        assert_eq!(
            font.characters().collect::<BTreeSet<_>>(),
            set.iter().copied().collect()
        );
        for character in set {
            let glyph = font.glyph(character);
            assert_eq!(glyph.rows.len(), GLYPH_HEIGHT, "{character:?}");
            assert!(
                (1..=GLYPH_WIDTH_MAX).contains(&glyph.width()),
                "{character:?}"
            );
            assert!(
                glyph.rows.iter().all(|row| row.len() == glyph.width()),
                "{character:?}"
            );
            assert!(glyph.advance >= 1, "{character:?}");
        }
        let replacement = font.replacement();
        assert_eq!(replacement.rows.len(), GLYPH_HEIGHT);
        assert_eq!(replacement.width(), 5);
    }

    fn entry(texture: &str, x: i64, width: i64) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            (
                "texture".to_owned(),
                ConfigValue::String(texture.to_owned()),
            ),
            ("x".to_owned(), ConfigValue::Integer(x)),
            ("width".to_owned(), ConfigValue::Integer(width)),
        ]))
    }

    fn table(width: i64, glyphs: Vec<ConfigValue>) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("width".to_owned(), ConfigValue::Integer(width)),
            ("height".to_owned(), ConfigValue::Integer(11)),
            ("glyphs".to_owned(), ConfigValue::Array(glyphs)),
        ]))
    }

    #[test]
    fn layout_of_a_date_gives_the_entries_and_width_of_the_committed_font() {
        // The advances: the digits 6, space 4, O 6, c 6, t 5; the glyph widths are the row widths.
        assert_eq!(
            layout(&shipped(), "9 Oct 768"),
            table(
                49,
                vec![
                    entry("ui/font/glyphs/U+0039.png", 0, 5),
                    entry("ui/font/glyphs/U+004F.png", 10, 5),
                    entry("ui/font/glyphs/U+0063.png", 16, 5),
                    entry("ui/font/glyphs/U+0074.png", 22, 4),
                    entry("ui/font/glyphs/U+0037.png", 31, 5),
                    entry("ui/font/glyphs/U+0036.png", 37, 5),
                    entry("ui/font/glyphs/U+0038.png", 43, 5),
                ]
            )
        );
        // The empty string and a string of spaces draw nothing and still take their room.
        assert_eq!(layout(&shipped(), ""), table(0, Vec::new()));
        assert_eq!(layout(&shipped(), "  "), table(8, Vec::new()));
    }

    #[test]
    fn a_fada_vowel_has_its_own_glyph_and_an_unknown_character_the_replacement() {
        assert_eq!(
            layout(&shipped(), "á ŵ"),
            table(
                16,
                vec![
                    entry("ui/font/glyphs/U+00E1.png", 0, 5),
                    entry("ui/font/glyphs/replacement.png", 10, 5),
                ]
            )
        );
        // The replacement takes the replacement's advance.
        assert_eq!(
            layout(&shipped(), "ŵŵ"),
            table(
                12,
                vec![
                    entry("ui/font/glyphs/replacement.png", 0, 5),
                    entry("ui/font/glyphs/replacement.png", 6, 5),
                ]
            )
        );
    }

    #[test]
    fn the_lua_function_lays_out_a_string_and_refuses_anything_else() {
        use std::sync::{Mutex, PoisonError};

        use ck_lua::HostFunction;

        use crate::testing::{Extras, Harness};

        let seen: Arc<Mutex<Vec<ConfigValue>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                seen.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(arguments.iter().flatten().cloned());
                Ok(Vec::new())
            })
        };
        let script = "local r = font.layout('9 Oct 768')\n\
            window.record(r.width, r.height, #r.glyphs, r.glyphs[2].texture, r.glyphs[2].x, r.glyphs[2].width)\n\
            local ok, message = pcall(font.layout, 7)\n\
            window.record(ok, tostring(message))\n\
            local none, text = pcall(font.layout)\n\
            window.record(none, tostring(text))";
        let _harness = Harness::new(
            script,
            Extras {
                functions: vec![("record", record)],
                extensions: vec![extension(Arc::new(shipped()))],
                ..Extras::default()
            },
        );
        let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let text = |value: &ConfigValue| match value {
            ConfigValue::String(text) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(seen[0], ConfigValue::Integer(49));
        assert_eq!(seen[1], ConfigValue::Integer(11));
        assert_eq!(seen[2], ConfigValue::Integer(7));
        assert_eq!(text(&seen[3]), "ui/font/glyphs/U+004F.png");
        assert_eq!(seen[4], ConfigValue::Integer(10));
        assert_eq!(seen[5], ConfigValue::Integer(5));
        assert_eq!(seen[6], ConfigValue::Bool(false));
        assert!(
            text(&seen[7]).contains("font.layout: expected one string"),
            "{seen:?}"
        );
        assert_eq!(seen[8], ConfigValue::Bool(false));
        assert!(text(&seen[9]).contains("font.layout"), "{seen:?}");
    }

    #[test]
    fn the_digits_are_tabular() {
        let font = shipped();
        let advances: BTreeSet<u32> = ('0'..='9').map(|digit| font.glyph(digit).advance).collect();
        assert_eq!(advances.len(), 1, "{advances:?}");
    }

    #[test]
    fn only_the_fadas_use_the_rows_above_and_only_the_descenders_the_rows_below() {
        let font = shipped();
        let blank = |glyph: &Glyph, rows: std::ops::Range<usize>| {
            glyph.rows[rows].iter().all(|row| !row.contains('#'))
        };
        let accented: BTreeSet<char> = "áéíóúÁÉÍÓÚ".chars().collect();
        let descending: BTreeSet<char> = "gjpqy,;".chars().collect();
        for character in font_set() {
            let glyph = font.glyph(character);
            assert_eq!(
                !blank(glyph, 0..2),
                accented.contains(&character),
                "{character:?} above"
            );
            assert_eq!(
                !blank(glyph, 9..11),
                descending.contains(&character),
                "{character:?} below"
            );
            // Every glyph has something on its baseline side of the cap line, except the space.
            if character != ' ' {
                assert!(
                    !blank(glyph, 2..9),
                    "{character:?} has no pixel in the glyph box"
                );
            }
        }
        // A fada vowel is its plain vowel with an accent above and nothing else changed (the dot of the i aside).
        for (fada, plain) in "áéóúÁÉÓÚ".chars().zip("aeouAEOU".chars()) {
            assert_eq!(
                font.glyph(fada).rows[2..],
                font.glyph(plain).rows[2..],
                "{fada:?}"
            );
            assert_eq!(
                font.glyph(fada).advance,
                font.glyph(plain).advance,
                "{fada:?}"
            );
        }
    }

    #[test]
    fn each_glyph_png_equals_its_rows_pixel_for_pixel_and_no_other_png_is_there() {
        let font = shipped();
        let check = |path: PathBuf, glyph: &Glyph| {
            let image = Image::decode(fs::File::open(&path).expect("the glyph PNG"))
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(image.width() as usize, glyph.width(), "{}", path.display());
            assert_eq!(image.height() as usize, GLYPH_HEIGHT, "{}", path.display());
            let expected: Vec<u8> = glyph
                .rows
                .iter()
                .flat_map(|row| row.chars())
                .flat_map(|cell| match cell {
                    '#' => [0xFF, 0xFF, 0xFF, 0xFF],
                    _ => [0, 0, 0, 0],
                })
                .collect();
            assert_eq!(image.pixels(), expected, "{}", path.display());
        };
        let assets = directory().join("../..");
        let mut names = BTreeSet::new();
        for character in font_set() {
            let name = glyph_texture(character);
            check(assets.join(&name), font.glyph(character));
            names.insert(name);
        }
        check(assets.join(REPLACEMENT_TEXTURE), font.replacement());
        names.insert(REPLACEMENT_TEXTURE.to_owned());
        let on_disk: BTreeSet<String> = fs::read_dir(directory().join("glyphs"))
            .expect("the glyphs directory")
            .map(|entry| {
                format!(
                    "ui/font/glyphs/{}",
                    entry.unwrap().file_name().to_string_lossy()
                )
            })
            .collect();
        assert_eq!(on_disk, names);
    }

    #[test]
    fn the_panel_is_one_opaque_pixel() {
        let image = Image::decode(fs::File::open(directory().join("panel.png")).unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (1, 1));
        assert_eq!(image.pixels(), [0x2A, 0x21, 0x19, 0xFF]);
    }

    #[test]
    fn texture_names_are_the_code_point_in_four_upper_case_hex_digits() {
        assert_eq!(glyph_texture('A'), "ui/font/glyphs/U+0041.png");
        assert_eq!(glyph_texture('á'), "ui/font/glyphs/U+00E1.png");
        assert_eq!(glyph_texture('·'), "ui/font/glyphs/U+00B7.png");
        assert_eq!(glyph_texture(' '), "ui/font/glyphs/U+0020.png");
    }

    #[test]
    fn a_character_outside_the_set_draws_the_replacement() {
        let font = shipped();
        assert_eq!(font.glyph('ŵ'), font.replacement());
        assert_ne!(font.glyph('w'), font.replacement());
    }

    #[test]
    fn the_readout_format_uses_only_characters_the_font_has() {
        let font = shipped();
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let text = format!("0123456789 {} · : speed paused", months.join(" "));
        for character in text.chars() {
            assert!(
                font.characters().any(|have| have == character),
                "{character:?}"
            );
        }
    }

    #[test]
    fn a_bad_font_is_refused_naming_the_problem() {
        let text = shipped_text();
        let refused = |text: &str| parse(text).expect_err("refused");

        assert!(refused(&without(&text, "[glyph.\"A\"]")).contains("'A'"));
        assert!(refused(&without(&text, "[replacement]")).contains("replacement"));
        assert!(refused("").contains("font.toml"));
        // A glyph outside the set, and one that is not a single character.
        let extra = format!(
            "{text}\n[glyph.\"ŵ\"]\nadvance = 6\nrows = {}\n",
            rows_literal(".....")
        );
        assert!(refused(&extra).contains("outside the font's set"));
        let long = format!(
            "{text}\n[glyph.\"ab\"]\nadvance = 6\nrows = {}\n",
            rows_literal(".....")
        );
        assert!(refused(&long).contains("not one character"));
        // Shape: rows, width, cells, advance.
        let one_row_short = text.replacen("    \".###.\",\n", "", 1);
        assert!(refused(&one_row_short).contains("rows, not 11"));
        assert!(refused(&text.replacen("\".###.\"", "\".###..\"", 1)).contains("wide"));
        assert!(refused(&text.replacen("\".###.\"", "\".#x#.\"", 1)).contains("not '#' or '.'"));
        assert!(refused(&text.replacen("advance = 6", "advance = 0", 1)).contains("advance 0"));
        // Digits must have one advance.
        let digit = text.find("[glyph.\"7\"]\nadvance = 6").expect("the 7");
        let skewed = format!(
            "{}{}",
            &text[..digit],
            text[digit..].replacen("advance = 6", "advance = 5", 1)
        );
        assert!(refused(&skewed).contains("tabular"));
        // An unknown key in a glyph.
        assert!(
            refused(&text.replacen("advance = 4\n", "advance = 4\nkerning = 1\n", 1))
                .contains("kerning")
        );
    }

    fn rows_literal(row: &str) -> String {
        format!("[{}]", vec![format!("\"{row}\""); GLYPH_HEIGHT].join(", "))
    }

    /// The characters of `name` the font has no glyph for.
    fn unsupported(font: &Font, name: &str) -> Vec<char> {
        let have: BTreeSet<char> = font.characters().collect();
        name.chars()
            .filter(|character| !have.contains(character))
            .collect()
    }

    /// Every unsupported character of every location name of every scenario under `scenarios`, as `scenario: name: chars`.
    fn unsupported_names(font: &Font, scenarios: &std::path::Path) -> Vec<String> {
        #[derive(Deserialize)]
        struct Scenario {
            locations: String,
        }
        #[derive(Deserialize, Default)]
        struct Locations {
            #[serde(default)]
            location: Vec<Named>,
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        let mut problems = Vec::new();
        let Ok(entries) = fs::read_dir(scenarios) else {
            return problems;
        };
        let mut directories: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        directories.sort();
        for directory in directories.into_iter().filter(|path| path.is_dir()) {
            let scenario: Scenario = toml::from_str(
                &fs::read_to_string(directory.join("scenario.toml")).expect("scenario.toml"),
            )
            .expect("a valid scenario.toml");
            let locations: Locations = toml::from_str(
                &fs::read_to_string(directory.join(&scenario.locations))
                    .expect("the locations file"),
            )
            .expect("a valid locations file");
            for named in locations.location {
                let missing = unsupported(font, &named.name);
                if !missing.is_empty() {
                    problems.push(format!(
                        "{}: {}: {missing:?}",
                        directory.file_name().unwrap().to_string_lossy(),
                        named.name
                    ));
                }
            }
        }
        problems
    }

    #[test]
    fn every_location_name_of_every_shipped_scenario_is_fully_supported() {
        let scenarios = directory().join("../../scenarios");
        assert_eq!(
            unsupported_names(&shipped(), &scenarios),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_names_check_accepts_a_fada_and_names_a_character_the_font_lacks() {
        let root = crate::testing::Dir::new();
        let scenario = root.0.join("scenarios/fixture");
        fs::create_dir_all(&scenario).unwrap();
        fs::write(
            scenario.join("scenario.toml"),
            "locations = \"locations.toml\"\n",
        )
        .unwrap();
        let location = |name: &str| format!("[[location]]\nname = \"{name}\"\n");
        fs::write(
            scenario.join("locations.toml"),
            format!(
                "{}{}",
                location("Gaillimh \u{b7} Cill \u{c1}ine"),
                location("Ward\u{175}")
            ),
        )
        .unwrap();
        // The fada and the middle dot are drawn; the w with a circumflex is not.
        assert_eq!(
            unsupported_names(&shipped(), &root.0.join("scenarios")),
            ["fixture: Ward\u{175}: ['\u{175}']"]
        );
        // No scenarios directory is nothing to check.
        assert!(unsupported_names(&shipped(), &root.0.join("none")).is_empty());
    }

    /// A configuration beside an assets directory holding a scenario and (when `font` is some) the font file, headless.
    fn run_with_font(font: Option<&str>) -> crate::lifecycle::Status {
        use std::sync::atomic::AtomicBool;

        let root = crate::testing::Dir::new();
        crate::game::tests::write_scenario(
            &root.0.join("assets/scenarios/fixture"),
            ck_sim::Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 0,
            },
        );
        // The scenario helper provides the shipped font and theme; this test decides what the run finds of the font.
        fs::remove_dir_all(root.0.join("assets/ui/font")).unwrap();
        if let Some(text) = font {
            fs::create_dir_all(root.0.join("assets/ui/font")).unwrap();
            fs::write(root.0.join("assets/ui/font/font.toml"), text).unwrap();
        }
        let config = root.0.join("config.toml");
        fs::write(&config, "[game]\nscenario = \"assets/scenarios/fixture\"\n").unwrap();
        let options = crate::cli::RunOptions {
            config: Some(config),
            headless: true,
            backend: None,
            load: None,
            seed: Some(1),
        };
        crate::execute_with(
            &options,
            crate::Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: ck_window::Notifier::new(),
            },
            &[],
            |_| panic!("a headless run has no window plan"),
        )
    }

    #[test]
    fn a_scenario_run_reads_the_font_at_start_up_and_refuses_a_missing_or_invalid_one() {
        assert_eq!(run_with_font(Some(&shipped_text())).code, 0);
        assert_eq!(run_with_font(None).code, 1, "a missing font.toml");
        assert_eq!(run_with_font(Some("")).code, 1, "an empty font.toml");
        assert_eq!(
            run_with_font(Some(&without(&shipped_text(), "[glyph.\"A\"]"))).code,
            1,
            "a font without a glyph"
        );
    }

    #[test]
    fn without_a_scenario_no_font_is_read() {
        use std::sync::atomic::AtomicBool;

        let root = crate::testing::Dir::new();
        let config = root.0.join("config.toml");
        // The assets directory does not exist and nothing minds.
        fs::write(&config, "[render]\nassets = \"nowhere\"\n").unwrap();
        let options = crate::cli::RunOptions {
            config: Some(config),
            headless: true,
            backend: None,
            load: None,
            seed: None,
        };
        let status = crate::execute_with(
            &options,
            crate::Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: ck_window::Notifier::new(),
            },
            &[],
            |_| panic!("a headless run has no window plan"),
        );
        assert_eq!(status.code, 0);
    }
}
