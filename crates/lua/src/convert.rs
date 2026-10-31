//! Conversion between Lua values and [`ConfigValue`] for host function arguments and results, and for
//! the arguments and return values of posted callback calls.
//!
//! Nothing is converted lossily. A Lua string is a byte string, so it converts only if its bytes are
//! valid UTF-8 (embedded NUL bytes are valid); a table key is held to the same rule, which also keeps
//! two distinct Lua keys from becoming one Rust key.

use std::collections::BTreeMap;
use std::fmt;

use mlua::{Lua, Value};

use crate::config::ConfigValue;

/// The deepest table nesting accepted (the argument's own table is level 1). A cycle exceeds it.
pub(crate) const MAX_DEPTH: usize = 32;

/// One step from a value to the part of it that could not be converted.
enum Step {
    Key(String),
    Index(usize),
    InvalidKey,
}

/// Why a Lua value could not become a [`ConfigValue`], and where in the value.
pub(crate) struct ConvertError {
    reason: String,
    /// Innermost step first.
    steps: Vec<Step>,
}

impl ConvertError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            steps: Vec::new(),
        }
    }

    fn within(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)?;
        if !self.steps.is_empty() {
            formatter.write_str(" (at ")?;
            for step in self.steps.iter().rev() {
                match step {
                    Step::Key(key) => write!(formatter, ".{key}")?,
                    Step::Index(index) => write!(formatter, "[{index}]")?,
                    Step::InvalidKey => formatter.write_str(".<key>")?,
                }
            }
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

/// The text of a string value, if its bytes are valid UTF-8.
fn text(string: &mlua::LuaString) -> Option<String> {
    String::from_utf8(string.as_bytes().to_vec()).ok()
}

/// Converts a Lua value (`nil` is `None`). `depth` is the nesting level of the value's container.
pub(crate) fn from_lua(value: &Value, depth: usize) -> Result<Option<ConfigValue>, ConvertError> {
    match value {
        Value::Nil => Ok(None),
        other => present(other, depth).map(Some),
    }
}

/// Converts a Lua value that is not `nil`.
fn present(value: &Value, depth: usize) -> Result<ConfigValue, ConvertError> {
    Ok(match value {
        Value::Boolean(flag) => ConfigValue::Bool(*flag),
        Value::Integer(number) => ConfigValue::Integer(*number),
        Value::Number(number) => ConfigValue::Float(*number),
        Value::String(string) => {
            ConfigValue::String(text(string).ok_or_else(|| ConvertError::new("not valid UTF-8"))?)
        }
        Value::Table(table) => table_from_lua(table, depth + 1)?,
        Value::Function(_) => return Err(ConvertError::new("a function cannot be converted")),
        Value::Thread(_) => return Err(ConvertError::new("a thread cannot be converted")),
        Value::UserData(_) | Value::LightUserData(_) => {
            return Err(ConvertError::new("a userdata cannot be converted"));
        }
        other => {
            return Err(ConvertError::new(format!(
                "a {} cannot be converted",
                other.type_name()
            )));
        }
    })
}

/// A table whose keys are all strings is a table; one whose keys are exactly `1..n` is an array; an
/// empty one is an empty table.
fn table_from_lua(table: &mlua::Table, depth: usize) -> Result<ConfigValue, ConvertError> {
    if depth > MAX_DEPTH {
        return Err(ConvertError::new(format!(
            "nested deeper than {MAX_DEPTH} levels (or cyclic)"
        )));
    }
    let mixed = || ConvertError::new("a table with mixed or other keys");
    let mut named: BTreeMap<String, ConfigValue> = BTreeMap::new();
    let mut numbered: BTreeMap<i64, ConfigValue> = BTreeMap::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|error| ConvertError::new(error.to_string()))?;
        match key {
            Value::String(name) => {
                let name = text(&name).ok_or_else(|| {
                    ConvertError::new("a table key is not valid UTF-8").within(Step::InvalidKey)
                })?;
                let converted = present(&value, depth)
                    .map_err(|error| error.within(Step::Key(name.clone())))?;
                named.insert(name, converted);
            }
            Value::Integer(index) => {
                let converted = present(&value, depth).map_err(|error| {
                    error.within(Step::Index(usize::try_from(index).unwrap_or(0)))
                })?;
                numbered.insert(index, converted);
            }
            _ => return Err(mixed()),
        }
    }
    match (named.is_empty(), numbered.is_empty()) {
        (_, true) => Ok(ConfigValue::Table(named)),
        (true, false) => {
            // `BTreeMap` order is key order, so the keys are exactly 1..n when they count up from 1.
            if !(1_i64..)
                .zip(numbered.keys())
                .all(|(want, key)| want == *key)
            {
                return Err(mixed());
            }
            Ok(ConfigValue::Array(numbered.into_values().collect()))
        }
        (false, false) => Err(mixed()),
    }
}

/// The plain Lua value for `value`: a table or an array as a new table, not a proxy.
pub(crate) fn to_lua(lua: &Lua, value: &ConfigValue) -> mlua::Result<Value> {
    Ok(match value {
        ConfigValue::String(text) => Value::String(lua.create_string(text)?),
        ConfigValue::Integer(number) => Value::Integer(*number),
        ConfigValue::Float(number) => Value::Number(*number),
        ConfigValue::Bool(flag) => Value::Boolean(*flag),
        ConfigValue::Table(entries) => {
            let table = lua.create_table()?;
            for (key, entry) in entries {
                table.raw_set(lua.create_string(key)?, to_lua(lua, entry)?)?;
            }
            Value::Table(table)
        }
        ConfigValue::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in (1_i64..).zip(items) {
                table.raw_set(index, to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
    })
}
