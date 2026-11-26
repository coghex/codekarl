//! Supplies the exact game derivation to the colour-sheet drawing tool.
#[path = "../theme/colours.rs"]
mod colours;

use std::{
    collections::BTreeMap,
    io::{self, BufRead},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("expected theme path")?;
    let document: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
    let entries = document["colour"].as_table().ok_or("missing palette")?;
    let mut palette = BTreeMap::new();
    for name in ["text", "body", "number", "over_limit", "over_limit_dark"] {
        let value = entries
            .get(name)
            .and_then(toml::Value::as_str)
            .ok_or("missing palette colour")?;
        if !value.is_ascii()
            || !(value.len() == 7 || (value.len() == 9 && value[7..].eq_ignore_ascii_case("ff")))
            || !value.starts_with('#')
            || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            return Err(format!("[colour] {name}: expected an opaque colour").into());
        }
        palette.insert(name.to_owned(), value[..7].to_ascii_lowercase());
    }
    for line in io::stdin().lock().lines() {
        let base = line?;
        if base.len() != 7
            || !base.starts_with('#')
            || !base.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            return Err("invalid base".into());
        }
        let result = colours::derive(&base, &palette);
        let fields = [
            "outline",
            "hot",
            "title_text",
            "pressed_text",
            "highlight_number",
            "highlight_over_limit",
        ];
        println!(
            "{}",
            fields
                .iter()
                .map(|key| result[*key].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    Ok(())
}
